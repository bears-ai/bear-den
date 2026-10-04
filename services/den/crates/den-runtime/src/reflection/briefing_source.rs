//! Authority for the tool-free memory-curate summary, not generic internal turns.

use den_core::{
    ids::{BearId, ConversationId, SessionId},
    DenError,
};
#[cfg(test)]
use den_protocol::{RuntimeEventStream, RuntimeSemanticEvent, RuntimeStreamEvent};
#[cfg(test)]
use futures::StreamExt;
use sqlx::PgPool;
use uuid::Uuid;

/// Canonical identifier of a persisted Reflection run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReflectionRunId(Uuid);

impl ReflectionRunId {
    pub const fn new(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

/// A server-resolved source. It grants no human, hat, or tool authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurateBriefingSource {
    run_id: ReflectionRunId,
    bear_id: BearId,
    canonical_conversation_id: Uuid,
    conversation_id: ConversationId,
    session_id: SessionId,
}

impl CurateBriefingSource {
    pub fn run_id(&self) -> ReflectionRunId {
        self.run_id
    }

    pub fn bear_id(&self) -> BearId {
        self.bear_id
    }

    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub async fn require_live(&self, pool: &PgPool) -> Result<(), DenError> {
        let live = resolve_curate_briefing_source(pool, self.bear_id, self.run_id).await?;
        if live != *self {
            return Err(DenError::Authorization(
                "memory-curate briefing source was rebound".into(),
            ));
        }
        Ok(())
    }
}

/// Text and its checked projection destination travel together.
#[derive(Debug)]
pub struct CurateBriefingText {
    pub source: CurateBriefingSource,
    pub text: String,
}

pub async fn resolve_curate_briefing_source(
    pool: &PgPool,
    bear_id: BearId,
    run_id: ReflectionRunId,
) -> Result<CurateBriefingSource, DenError> {
    let row = sqlx::query!(
        r#"
        SELECT c.id AS canonical_conversation_id,
               c.external_conversation_id AS "conversation_id!"
        FROM bear_reflection_runs r
        JOIN reflection_conversations rc
          ON rc.bear_id = r.bear_id
         AND rc.lane = 'memory_curate'
         AND rc.conversation_id = r.conversation_id
        JOIN conversations c
          ON c.bear_id = rc.bear_id
         AND c.external_conversation_id = rc.conversation_id
        WHERE r.id = $1
          AND r.bear_id = $2
          AND r.lane = 'memory_curate'
          AND r.status = 'running'
          AND r.completed_at IS NULL
          AND btrim(r.conversation_id) <> ''
          AND c.status = 'active'
        "#,
        run_id.as_uuid(),
        bear_id.as_uuid(),
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        DenError::Authorization(
            "memory-curate briefing requires a running Bear-owned Reflection run with a live canonical conversation binding".into(),
        )
    })?;

    Ok(CurateBriefingSource {
        run_id,
        bear_id,
        canonical_conversation_id: row.canonical_conversation_id,
        conversation_id: ConversationId::new(row.conversation_id),
        session_id: SessionId::new(format!("memory-curate-{}", run_id.as_uuid())),
    })
}

#[cfg(test)]
pub(crate) async fn collect_curate_briefing_text(
    pool: &PgPool,
    source: CurateBriefingSource,
    mut stream: RuntimeEventStream,
) -> Result<CurateBriefingText, DenError> {
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        match item? {
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::AssistantTextDelta {
                text: delta,
            }) => text.push_str(&delta),
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested { .. }) => {
                return Err(DenError::Authorization(
                    "memory-curate briefing is tool-free; model requested a tool".into(),
                ));
            }
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::Error { message, .. })
            | RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnFailed { message, .. }) => {
                return Err(DenError::ValidationError(message));
            }
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCancelled { .. }) => {
                return Err(DenError::ValidationError(
                    "memory-curate briefing was cancelled".into(),
                ));
            }
            _ => {}
        }
    }
    // No partial text may escape if the run ended or changed destination mid-call.
    source.require_live(pool).await?;
    Ok(CurateBriefingText { source, text })
}

#[cfg(test)]
mod tests;
