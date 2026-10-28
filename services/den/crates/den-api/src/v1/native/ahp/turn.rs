//! AHP owns transport only. The controlled channel runtime owns assistant/tool
//! persistence and typed canonical terminal records on every admitted exit.
use super::{
    super::{access, conversations::timestamp, error::Error, history},
    wire::{Action, ErrorInfo, Origin, Part, UserMessage},
};
use axum::http::HeaderMap;
use den_core::ids::BearId;
use den_oauth::oauth::OAuthScope;
use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
use den_runtime::channel_lifecycle::{ChannelTurnOutcome, ChannelTurnTerminal};
use den_runtime::native_runtime::{
    self, NativeRuntimeDeps, NativeWebChatTurnParams, PersistedChannelInput,
};
use den_service::{
    conversation::persistence::ConversationRecord, tool_turns::ConversationTurnLease, DenState,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

pub(super) enum Event {
    Action(Action, Option<Origin>),
    Rejected(String),
    Finished,
}
#[derive(Clone)]
pub(super) struct Input {
    pub conversation: Uuid,
    pub turn: Uuid,
    pub message: UserMessage,
    pub origin: Origin,
}
struct Prepared {
    conversation: ConversationRecord,
    user: i32,
    session: String,
    lease: Arc<ConversationTurnLease>,
    input: PersistedChannelInput,
}

pub(super) async fn run(
    state: DenState,
    headers: HeaderMap,
    input: Input,
    mut cancel: watch::Receiver<bool>,
    tx: mpsc::Sender<Event>,
) {
    if *cancel.borrow() {
        reject(
            &tx,
            "Turn cancelled before acceptance. Refresh history before retrying.",
        )
        .await;
        return;
    }
    // Do not drop a possibly committed input write on cancellation: admission
    // returns its typed token, then cancellation finalizes that exact input.
    let prepared = match prepare(&state, &headers, &input).await {
        Ok(prepared) => prepared,
        Err(error) => {
            reject(&tx, &error.message).await;
            return;
        }
    };
    let Prepared {
        conversation,
        user,
        session,
        lease,
        input: persisted,
    } = prepared;
    // A committed input is accepted even if cancellation arrived during its
    // write. Confirm it before emitting the durable cancelled outcome.
    let acknowledged = tx
        .send(Event::Action(
            Action::Started {
                turn_id: input.turn,
                started_at: timestamp(time::OffsetDateTime::now_utc()),
                message: input.message.clone(),
            },
            Some(input.origin.clone()),
        ))
        .await
        .is_ok();
    if !acknowledged || *cancel.borrow() {
        finish_before_start(&state, &persisted, ChannelTurnOutcome::Cancelled, None, &tx).await;
        return;
    }
    let bear = match access::membership(&state, user, conversation.bear_id).await {
        Ok(bear) => bear,
        Err(_) => {
            finish_before_start(
                &state,
                &persisted,
                ChannelTurnOutcome::Failed,
                Some("Source access changed.".into()),
                &tx,
            )
            .await;
            return;
        }
    };
    let membership =
        match den_service::bears::db::membership_role_for_user(&state.sqlx_pool, user, bear.id)
            .await
        {
            Ok(role) => role.flatten(),
            Err(_) => {
                finish_before_start(
                    &state,
                    &persisted,
                    ChannelTurnOutcome::Failed,
                    Some("Source access could not be verified.".into()),
                    &tx,
                )
                .await;
                return;
            }
        };
    let Some(invoker) = native_runtime::tool_invoker() else {
        finish_before_start(
            &state,
            &persisted,
            ChannelTurnOutcome::Failed,
            Some("The tool runtime is unavailable.".into()),
            &tx,
        )
        .await;
        return;
    };
    let deps = NativeRuntimeDeps {
        pool: &state.sqlx_pool,
        config: state.config.as_ref(),
        stores: &state.memory_stores,
    };
    let binding =
        den_service::bears::hats::turn_binding::NativeTurnSource::Conversation(conversation.id)
            .binding_id(BearId::new(conversation.bear_id));
    let external = conversation
        .external_conversation_id
        .as_deref()
        .expect("admitted external source");
    let turn = tokio::select! {
        biased;
        _=cancel.changed()=>{finish_before_start(&state,&persisted,ChannelTurnOutcome::Cancelled,None,&tx).await;return;},
        result=native_runtime::start_native_channel_turn(NativeWebChatTurnParams{deps:&deps,bear_id:bear.id,bear_slug:&bear.slug,turn_binding_id:&binding,user_id:user,username:None,membership_role:membership.as_deref(),conversation_id:external,session_id:&session,prompt:&input.message.text,request_id:input.turn,tool_invoker:invoker},persisted.clone(),lease.clone())=>result,
    };
    let mut turn = match turn {
        Ok(turn) => turn,
        Err(_) => {
            finish_before_start(
                &state,
                &persisted,
                ChannelTurnOutcome::Failed,
                Some(
                    "The model turn could not be started. Refresh history before retrying.".into(),
                ),
                &tx,
            )
            .await;
            return;
        }
    };
    let part_id = format!("response-{}", input.turn);
    let _ = tx
        .send(Event::Action(
            Action::Part {
                turn_id: input.turn,
                part: Part::Markdown {
                    id: part_id.clone(),
                    content: String::new(),
                },
            },
            None,
        ))
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut recheck = tokio::time::interval(Duration::from_secs(5));
    let mut bytes = 0usize;
    let outcome = loop {
        tokio::select! {
            biased;
            _=cancel.changed()=>break turn.stop(ChannelTurnOutcome::Cancelled,None).await,
            ()=tokio::time::sleep_until(deadline)=>break turn.stop(ChannelTurnOutcome::Failed,Some("The reply exceeded the time limit.".into())).await,
            _=recheck.tick()=>{
                if access::authenticate(&state,&headers,OAuthScope::DataWrite).await.is_err()||access::executable(&state,user,&conversation).await.is_err(){break turn.stop(ChannelTurnOutcome::Failed,Some("Authorization changed during the turn.".into())).await;}
            },
            event=turn.next_event()=>match event {
                Some(Ok(RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::AssistantTextDelta{text:delta})))=>{
                    bytes=bytes.saturating_add(delta.len());
                    if bytes>1024*1024 {break turn.stop(ChannelTurnOutcome::Failed,Some("The reply exceeded the output limit.".into())).await;}
                    if tx.send(Event::Action(Action::Delta{turn_id:input.turn,part_id:part_id.clone(),content:delta},None)).await.is_err(){break turn.stop(ChannelTurnOutcome::Cancelled,None).await;}
                }
                Some(Ok(RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted{..}|RuntimeSemanticEvent::TurnCancelled{..}|RuntimeSemanticEvent::TurnFailed{..}|RuntimeSemanticEvent::Error{..})))=>break turn.terminal().cloned().ok_or_else(||den_core::DenError::System("channel terminal was not finalized".into())),
                Some(Ok(RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested{approval_required:true,..})))=>break turn.stop(ChannelTurnOutcome::Failed,Some("Permissions are unavailable in this text-only client.".into())).await,
                Some(Ok(RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunPaused{..})))=>break turn.stop(ChannelTurnOutcome::Failed,Some("Interactive continuation is unsupported.".into())).await,
                Some(Err(error))=>break turn.terminal().cloned().ok_or(error),
                Some(Ok(_))=>{},
                None=>break turn.terminal().cloned().ok_or_else(||den_core::DenError::System("channel stream ended without finalization".into())),
            }
        }
    };
    emit_terminal(&tx, input.turn, outcome).await;
}

async fn reject(tx: &mpsc::Sender<Event>, message: &str) {
    let _ = tx.send(Event::Rejected(message.into())).await;
    let _ = tx.send(Event::Finished).await;
}
async fn finish_before_start(
    state: &DenState,
    input: &PersistedChannelInput,
    outcome: ChannelTurnOutcome,
    error: Option<String>,
    tx: &mpsc::Sender<Event>,
) {
    let terminal = input
        .finish_without_runtime(&state.sqlx_pool, outcome, 0, error)
        .await;
    // The terminal record carries the admitted turn even when it has no prose.
    let id = input.turn_id();
    emit_terminal(tx, id, terminal).await;
}
async fn emit_terminal(
    tx: &mpsc::Sender<Event>,
    id: Uuid,
    result: Result<ChannelTurnTerminal, den_core::DenError>,
) {
    let action = match result {
        Ok(terminal) => match terminal.outcome {
            ChannelTurnOutcome::Completed => Action::Complete {
                turn_id: terminal.turn_id,
                duration: terminal.duration_ms,
            },
            ChannelTurnOutcome::Cancelled => Action::Cancelled {
                turn_id: terminal.turn_id,
                duration: terminal.duration_ms,
            },
            ChannelTurnOutcome::Failed => Action::Error {
                turn_id: terminal.turn_id,
                duration: terminal.duration_ms,
                part: Part::Error {
                    error: ErrorInfo {
                        error_type: "turn_failed".into(),
                        message: terminal
                            .error_message
                            .unwrap_or_else(|| "The turn failed.".into()),
                    },
                    resumable: false,
                },
            },
        },
        Err(error) => {
            tracing::error!(%error,"channel finalization failed");
            Action::Error{turn_id:id,duration:0,part:Part::Error{error:ErrorInfo{error_type:"persistence_failed".into(),message:"Turn finalization failed. Refresh canonical history; do not replay the send.".into()},resumable:false}}
        }
    };
    let _ = tx.send(Event::Action(action, None)).await;
    let _ = tx.send(Event::Finished).await;
}

async fn prepare(state: &DenState, headers: &HeaderMap, input: &Input) -> Result<Prepared, Error> {
    let user = access::authenticate(state, headers, OAuthScope::DataWrite)
        .await?
        .user_id;
    let conversation = access::readable(state, user, input.conversation).await?;
    access::executable(state, user, &conversation).await?;
    let session = format!("den-native:{user}:{}", conversation.id);
    let lease = ConversationTurnLease::acquire(&state.sqlx_pool, conversation.id)
        .await
        .map_err(|_| {
            Error::conflict("A turn is already running for this canonical conversation.")
        })?;
    history::require_new_turn(state, conversation.id, input.turn).await?;
    let primary = den_service::model_selection::resolve_conversation_primary_model(
        &state.sqlx_pool,
        BearId::new(conversation.bear_id),
        conversation.id,
        &state.config.default_llm_model,
    )
    .await?;
    state
        .bifrost
        .validate_bear_model_execution(
            &state.sqlx_pool,
            conversation.bear_id,
            &primary.model_handle,
            &state.config.den_secret_encryption_key,
            primary.source,
        )
        .await?;
    if native_runtime::tool_invoker().is_none() {
        return Err(Error::conflict("The native runtime is unavailable."));
    }
    access::executable(state, user, &conversation).await?;
    let persisted = native_runtime::persist_channel_input(
        &state.sqlx_pool,
        conversation.bear_id,
        user,
        &lease,
        input.turn,
        &input.message.text,
    )
    .await?;
    Ok(Prepared {
        conversation,
        user,
        session,
        lease,
        input: persisted,
    })
}
