//! Canonical interaction lifecycle: diagnostic-only typed records, not prose or
//! model transcript messages. Protocol edges may project these explicitly.
use den_core::DenError;
use den_service::conversation::{
    message_types::{
        ConversationMessageType, ConversationMessageVisibility, ConversationMessageWrite,
    },
    persistence::{self, PersistedConversationMessage},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelTurnOutcome {
    Completed,
    Cancelled,
    Failed,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelTurnTerminal {
    pub turn_id: Uuid,
    pub user_message_sequence: i64,
    pub assistant_message_id: Option<Uuid>,
    pub outcome: ChannelTurnOutcome,
    pub duration_ms: u64,
    pub error_message: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "event", content = "turn", rename_all = "snake_case")]
enum LifecycleRecord {
    ChannelTurnTerminal(ChannelTurnTerminal),
}

pub async fn persist_terminal(
    pool: &PgPool,
    conversation: Uuid,
    record: &ChannelTurnTerminal,
) -> Result<ChannelTurnTerminal, DenError> {
    let payload = serde_json::to_value(LifecycleRecord::ChannelTurnTerminal(record.clone()))
        .map_err(|error| DenError::Parsing(error.to_string()))?;
    let write = ConversationMessageWrite::workflow_diagnostic(String::new(), payload)
        .with_source_event_id(Some(format!("channel-turn-terminal:{}", record.turn_id)));
    let appended = persistence::append_message(pool, conversation, &write).await?;
    let rows =
        persistence::list_messages_page(pool, conversation, Some(appended.sequence_no + 1), 1)
            .await?;
    rows.first()
        .and_then(terminal_record)
        .ok_or_else(|| DenError::Parsing("canonical turn terminal record is invalid".into()))
}

/// Canonical user-visible rows plus this module's typed terminal diagnostics.
/// Other diagnostic and model-only rows remain excluded.
pub async fn interaction_history_page(
    pool: &PgPool,
    conversation: Uuid,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<PersistedConversationMessage>, DenError> {
    let limit = limit.clamp(1, 100) as usize;
    let mut cursor = before;
    let mut projected = Vec::with_capacity(limit);
    loop {
        let rows = persistence::list_messages_page(pool, conversation, cursor, 100).await?;
        let raw_count = rows.len();
        cursor = rows.last().map(|row| row.sequence_no);
        projected.extend(
            rows.into_iter()
                .filter(|row| row.is_user_history_visible() || terminal_record(row).is_some())
                .take(limit - projected.len()),
        );
        if projected.len() == limit || raw_count < 100 || cursor.is_none() {
            return Ok(projected);
        }
    }
}

pub fn terminal_record(row: &PersistedConversationMessage) -> Option<ChannelTurnTerminal> {
    if row.storage_message_type().ok() != Some(ConversationMessageType::WorkflowEvent)
        || row.storage_visibility().ok() != Some(ConversationMessageVisibility::DiagnosticOnly)
    {
        return None;
    }
    match serde_json::from_value::<LifecycleRecord>(row.content_json.clone()).ok()? {
        LifecycleRecord::ChannelTurnTerminal(record) => Some(record),
    }
}
