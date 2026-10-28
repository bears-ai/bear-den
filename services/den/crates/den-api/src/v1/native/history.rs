use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::HeaderMap,
    Json,
};
use den_oauth::oauth::OAuthScope;
use den_service::{
    conversation::persistence::{
        self, ConversationHistoryProjection, PersistedConversationMessage,
    },
    DenState,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{access, conversations::timestamp, error::Error};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HistoryQuery {
    pub bear_id: Uuid,
    pub conversation_id: String,
    pub before: Option<i64>,
    pub limit: Option<i64>,
}
#[derive(Debug, Clone, Serialize)]
pub(super) struct Message {
    pub id: String,
    pub sequence_no: i64,
    pub role: String,
    pub kind: String,
    pub text: String,
    pub created_at: String,
}
#[derive(Serialize)]
pub(super) struct Page {
    pub messages: Vec<Message>,
    pub has_more: bool,
    pub next_before: Option<String>,
}

pub(super) fn bounds(before: Option<i64>, limit: Option<i64>) -> Result<i64, Error> {
    if before.is_some_and(|n| n <= 0) || limit.is_some_and(|n| !(1..=100).contains(&n)) {
        return Err(Error::invalid(
            "before must be positive and limit must be between 1 and 100.",
        ));
    }
    Ok(limit.unwrap_or(50))
}

pub(super) async fn get(
    State(state): State<DenState>,
    headers: HeaderMap,
    query: Result<Query<HistoryQuery>, QueryRejection>,
) -> Result<Json<Page>, Error> {
    let principal = access::authenticate(&state, &headers, OAuthScope::DataRead).await?;
    let Query(query) = query.map_err(|_| Error::invalid("Invalid history query."))?;
    let limit = bounds(query.before, query.limit)?;
    let conversation = access::selected(
        &state,
        principal.user_id,
        query.bear_id,
        &query.conversation_id,
    )
    .await?;
    let rows = persistence::list_projected_messages_page(
        &state.sqlx_pool,
        conversation.id,
        query.before,
        limit,
        ConversationHistoryProjection::UserHistory,
    )
    .await?;
    let next_before = rows.last().map(|r| r.sequence_no.to_string());
    let messages = rows
        .iter()
        .rev()
        .filter_map(|row| project(conversation.id, row))
        .collect();
    Ok(Json(Page {
        has_more: rows.len() == limit as usize,
        next_before,
        messages,
    }))
}

pub(super) fn project(conversation: Uuid, row: &PersistedConversationMessage) -> Option<Message> {
    let visible = row.to_user_history_record()?;
    Some(Message {
        id: format!("{conversation}:{}", row.sequence_no),
        sequence_no: row.sequence_no,
        role: visible.role,
        kind: visible.kind,
        text: den_runtime::agent_assist::sanitize_visible_transcript_text(&visible.content),
        created_at: timestamp(row.created_at),
    })
}

// Durable transcript inspection makes retries fail closed after process/connection
// loss. A bounded scan rejects very large sources rather than executing an
// unproven retry. There is no independently writable idempotency database.
pub(super) async fn require_new_turn(
    state: &DenState,
    conversation: Uuid,
    turn: Uuid,
) -> Result<(), Error> {
    let turn = turn.to_string();
    let mut before = None;
    for _ in 0..100 {
        let rows = persistence::list_projected_messages_page(
            &state.sqlx_pool,
            conversation,
            before,
            100,
            ConversationHistoryProjection::UserHistory,
        )
        .await?;
        if rows
            .iter()
            .any(|row| row.provider_message_id.as_deref() == Some(turn.as_str()))
        {
            return Err(Error::conflict(
                "Turn already exists. Refresh canonical history; do not replay the send.",
            ));
        }
        if rows.len() < 100 {
            return Ok(());
        }
        before = rows.last().map(|row| row.sequence_no);
    }
    Err(Error::conflict(
        "This source exceeds the MVP retry-verification bound. Start a new conversation.",
    ))
}
