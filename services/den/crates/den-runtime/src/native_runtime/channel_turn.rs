//! Controlled native channel turns: one persistence owner, explicit input
//! provenance, and awaited finalization on success, failure and cancellation.
use super::{web_chat_loop::NativeWebChatLoopStream, NativeWebChatTurnParams};
use crate::channel_lifecycle::{self as turn_lifecycle, ChannelTurnOutcome, ChannelTurnTerminal};
pub(super) mod tool_transcript;
use self::tool_transcript::ToolTranscript;
use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
use den_service::{
    conversation::{
        events::{
            canonical_persistence_context, CanonicalConversationRecord, ConversationEventProvenance,
        },
        message_types::ConversationMessageWrite,
        persistence::{self, ConversationHistoryProjection},
        viewer::require_ordinary_tool_source,
    },
    tool_turns::ConversationTurnLease,
};
use futures::{Stream, StreamExt};
use serde_json::json;
use sqlx::PgPool;
use std::{sync::Arc, time::Instant};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PersistedChannelInput {
    conversation: Uuid,
    user: i32,
    request: Uuid,
    sequence: i64,
    text: String,
}
impl PersistedChannelInput {
    pub fn sequence(&self) -> i64 {
        self.sequence
    }
    pub fn turn_id(&self) -> Uuid {
        self.request
    }
    pub async fn finish_without_runtime(
        &self,
        pool: &PgPool,
        outcome: ChannelTurnOutcome,
        duration_ms: u64,
        error_message: Option<String>,
    ) -> Result<ChannelTurnTerminal, DenError> {
        let terminal = ChannelTurnTerminal {
            turn_id: self.request,
            user_message_sequence: self.sequence,
            assistant_message_id: None,
            outcome,
            duration_ms,
            error_message,
        };
        turn_lifecycle::persist_terminal(pool, self.conversation, &terminal).await
    }
}

pub async fn persist_channel_input(
    pool: &PgPool,
    bear: Uuid,
    user: i32,
    lease: &ConversationTurnLease,
    request: Uuid,
    text: &str,
) -> Result<PersistedChannelInput, DenError> {
    let conversation = persistence::get_conversation_by_id(pool, lease.conversation_id())
        .await?
        .ok_or_else(|| DenError::Authorization("channel source missing".into()))?;
    if conversation.bear_id != bear {
        return Err(DenError::Authorization(
            "channel source belongs to another Bear".into(),
        ));
    }
    let external = conversation
        .external_conversation_id
        .as_deref()
        .ok_or_else(|| DenError::Authorization("channel source has no external identity".into()))?;
    require_ordinary_tool_source(pool, BearId::new(bear), UserId::new(user), external).await?;
    let mut message = ConversationMessageWrite::user_turn(
        text,
        json!({"type":"user_input","request_id":request}),
        Some(format!("ahp-user:{request}")),
    );
    message.provider_message_id = Some(request.to_string());
    let appended = persistence::append_message(pool, conversation.id, &message).await?;
    let canonical = persistence::list_projected_messages_page(
        pool,
        conversation.id,
        Some(appended.sequence_no + 1),
        1,
        ConversationHistoryProjection::UserHistory,
    )
    .await?;
    if !canonical.first().is_some_and(|row| {
        row.sequence_no == appended.sequence_no
            && row.content_text == text
            && row.provider_message_id.as_deref() == Some(request.to_string().as_str())
    }) {
        return Err(DenError::ValidationError(
            "current channel input differs from its canonical message".into(),
        ));
    }
    Ok(PersistedChannelInput {
        conversation: conversation.id,
        user,
        request,
        sequence: appended.sequence_no,
        text: text.into(),
    })
}

pub struct NativeChannelTurn {
    stream: NativeWebChatLoopStream,
    pool: PgPool,
    bear: Uuid,
    external: String,
    session: String,
    input: PersistedChannelInput,
    _lease: Arc<ConversationTurnLease>,
    started: Instant,
    text: String,
    terminal: Option<ChannelTurnTerminal>,
}

pub async fn start_native_channel_turn(
    params: NativeWebChatTurnParams<'_>,
    input: PersistedChannelInput,
    lease: Arc<ConversationTurnLease>,
) -> Result<NativeChannelTurn, DenError> {
    if input.conversation != lease.conversation_id()
        || input.user != params.user_id
        || input.request != params.request_id
        || input.text != params.prompt
    {
        return Err(DenError::Authorization(
            "persisted current input does not match this turn".into(),
        ));
    }
    let canonical = persistence::get_conversation_by_id(params.deps.pool, input.conversation)
        .await?
        .ok_or_else(|| DenError::Authorization("channel source missing".into()))?;
    if canonical.bear_id != params.bear_id
        || canonical.external_conversation_id.as_deref() != Some(params.conversation_id)
    {
        return Err(DenError::Authorization(
            "persisted input source changed".into(),
        ));
    }
    let pool = params.deps.pool.clone();
    let bear = params.bear_id;
    let external = params.conversation_id.to_owned();
    let session = params.session_id.to_owned();
    let stream = super::turn::start_web_chat_turn(
        params,
        crate::agent_loop::CurrentInputPersistence::AlreadyPersisted {
            sequence: input.sequence,
        },
        true,
    )
    .await?;
    Ok(NativeChannelTurn {
        stream,
        pool,
        bear,
        external,
        session,
        input,
        _lease: lease,
        started: Instant::now(),
        text: String::new(),
        terminal: None,
    })
}

impl NativeChannelTurn {
    pub fn terminal(&self) -> Option<&ChannelTurnTerminal> {
        self.terminal.as_ref()
    }
    pub async fn next_event(&mut self) -> Option<Result<RuntimeStreamEvent, DenError>> {
        if self.terminal.is_some() {
            return None;
        }
        match self.stream.next().await {
            Some(Ok(RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::AssistantTextDelta {
                text,
            }))) => {
                self.text.push_str(&text);
                Some(Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::AssistantTextDelta { text },
                )))
            }
            Some(Ok(
                event @ RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted { .. }),
            )) => Some(
                self.finalize(ChannelTurnOutcome::Completed, None)
                    .await
                    .map(|_| event),
            ),
            Some(Ok(
                event @ RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::TurnFailed { .. } | RuntimeSemanticEvent::Error { .. },
                ),
            )) => Some(
                self.stop(
                    ChannelTurnOutcome::Failed,
                    Some("The model turn failed.".into()),
                )
                .await
                .map(|_| event),
            ),
            Some(Ok(
                event @ RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCancelled { .. }),
            )) => Some(
                self.stop(ChannelTurnOutcome::Cancelled, None)
                    .await
                    .map(|_| event),
            ),
            Some(Err(error)) => {
                if let Err(finalization) = self
                    .stop(
                        ChannelTurnOutcome::Failed,
                        Some("The model stream failed.".into()),
                    )
                    .await
                {
                    return Some(Err(finalization));
                }
                Some(Err(error))
            }
            None => Some(
                self.finalize(ChannelTurnOutcome::Completed, None)
                    .await
                    .map(|_| {
                        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted {
                            turn: None,
                        })
                    }),
            ),
            other => other,
        }
    }
    pub async fn stop(
        &mut self,
        outcome: ChannelTurnOutcome,
        error_message: Option<String>,
    ) -> Result<ChannelTurnTerminal, DenError> {
        self.stream.interrupt();
        self.finalize(outcome, error_message).await
    }
    async fn finalize(
        &mut self,
        outcome: ChannelTurnOutcome,
        error_message: Option<String>,
    ) -> Result<ChannelTurnTerminal, DenError> {
        if let Some(terminal) = &self.terminal {
            return Ok(terminal.clone());
        }
        let messages = self.stream.current_turn_messages();
        ToolTranscript {
            pool: &self.pool,
            bear: self.bear,
            user: self.input.user,
            external: &self.external,
            session: &self.session,
            request: &self.input.request.to_string(),
        }
        .persist(&messages)
        .await?;
        let mut assistant_message_id = None;
        if !self.text.trim().is_empty() {
            let provenance = ConversationEventProvenance::client_session(self.session.clone());
            let context = canonical_persistence_context(
                self.pool.clone(),
                self.bear,
                Some(self.input.user),
                self.external.clone(),
                Some(self.session.clone()),
                Some(self.input.request.to_string()),
                self.session.clone(),
                false,
            );
            assistant_message_id =
                den_service::conversation::events::persist_canonical_conversation_record_with_id(
                    &context,
                    &CanonicalConversationRecord::assistant_output(
                        self.text.clone(),
                        &provenance,
                        Some(format!("response-{}", self.input.request)),
                        Some(self.input.request.to_string()),
                    ),
                )
                .await?;
        }
        let terminal = ChannelTurnTerminal {
            turn_id: self.input.request,
            user_message_sequence: self.input.sequence,
            assistant_message_id,
            outcome,
            duration_ms: self.started.elapsed().as_millis() as u64,
            error_message,
        };
        let terminal =
            turn_lifecycle::persist_terminal(&self.pool, self.input.conversation, &terminal)
                .await?;
        self.terminal = Some(terminal.clone());
        Ok(terminal)
    }
}

pub fn hold_conversation_turn<S: Stream + Send + 'static>(
    stream: S,
    lease: Arc<ConversationTurnLease>,
) -> impl Stream<Item = S::Item> + Send
where
    S::Item: Send,
{
    futures::stream::unfold(
        (Box::pin(stream), lease),
        |(mut stream, lease)| async move { stream.next().await.map(|event| (event, (stream, lease))) },
    )
}

#[cfg(test)]
mod tests;
