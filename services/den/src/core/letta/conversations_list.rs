//! Shared runtime conversation listing for the chat JSON API and bear details HTML.

use std::cmp::Ordering;

use serde::Serialize;
use serde_json::Value;

use super::conversation_title::{
    display_conversation_title, first_user_message_text_for_title, is_meaningful_conversation_title,
};
use super::LettaClient;

/// One thread (default main chat or `conv-…`), including archive state from Letta.
#[derive(Debug, Clone, Serialize)]
pub struct LettaConversationRow {
    pub id: String,
    pub title: String,
    pub last_message_at: Option<String>,
    pub archived: bool,
}

/// Result of [`load_agent_conversations`].
#[derive(Debug, Clone)]
pub struct AgentConversationsSnapshot {
    /// Non-archived rows, newest activity first.
    pub active: Vec<LettaConversationRow>,
    /// All rows including archived, newest activity first.
    pub all: Vec<LettaConversationRow>,
    /// Number of archived `conv-…` threads returned by Letta.
    pub archived_count: usize,
}

pub fn letta_conversations_top_array(v: &Value) -> &[Value] {
    if let Some(a) = v.as_array() {
        return a.as_slice();
    }
    if let Some(a) = v.get("conversations").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = v.get("items").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    &[]
}

fn letta_messages_top_array(v: &Value) -> &[Value] {
    if let Some(a) = v.as_array() {
        return a.as_slice();
    }
    if let Some(a) = v.get("messages").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = v.get("items").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    &[]
}

fn value_has_true_flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(|x| x.as_bool()) == Some(true)
        || v.get(key).and_then(|x| x.as_str()) == Some("true")
}

fn object_marks_archived(v: &Value) -> bool {
    value_has_true_flag(v, "archived")
        || value_has_true_flag(v, "is_archived")
        || value_has_true_flag(v, "deleted")
        || value_has_true_flag(v, "hidden")
        || v.get("archived_at").is_some_and(|x| !x.is_null())
        || v.get("status")
            .and_then(|x| x.as_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("archived"))
}

/// Hide rows that look archived (Letta may extend the schema; Den may add flags later).
pub fn conversation_is_archived(v: &Value) -> bool {
    object_marks_archived(v)
        || v.get("metadata").is_some_and(object_marks_archived)
        || v.get("attributes").is_some_and(object_marks_archived)
        || v.get("tags")
            .and_then(|x| x.as_array())
            .is_some_and(|tags| {
                tags.iter().any(|tag| {
                    tag.as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case("archived"))
                })
            })
}

pub fn cmp_conversation_row_newest_first(
    a: &LettaConversationRow,
    b: &LettaConversationRow,
) -> Ordering {
    match (&a.last_message_at, &b.last_message_at) {
        (Some(al), Some(bl)) => bl.cmp(al),
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
        (Some(_), None) => Ordering::Less,
    }
}

async fn resolve_conversation_row_title(
    letta: &LettaClient,
    item: &Value,
    conv_id: &str,
) -> String {
    let summary = item.get("summary").and_then(|x| x.as_str());
    if is_meaningful_conversation_title(summary, conv_id) {
        return summary.unwrap().trim().to_string();
    }

    let body = match letta
        .list_conversation_messages(conv_id, None, 100, None, true)
        .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(%e, %conv_id, "list messages for conversation title failed");
            return display_conversation_title(summary, conv_id, None);
        }
    };

    let first_user = first_user_message_text_for_title(&body);
    display_conversation_title(summary, conv_id, first_user.as_deref())
}

/// Loads and sorts conversations for a Letta agent: main thread plus `conv-…` rows from
/// `GET /v1/conversations/`. Used by `/v1/chat/conversations` and bear details pages.
pub async fn load_agent_conversations(
    letta: &LettaClient,
    agent_id: &str,
) -> AgentConversationsSnapshot {
    let mut rows: Vec<LettaConversationRow> = Vec::new();
    let mut archived_count = 0usize;

    let default_last = match letta
        .list_conversation_messages("default", Some(agent_id), 1, None, false)
        .await
    {
        Ok(peek) => letta_messages_top_array(&peek).first().and_then(|m| {
            m.get("date")
                .or_else(|| m.get("created_at"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        }),
        Err(e) => {
            tracing::warn!(%e, "default conversation activity peek failed");
            None
        }
    };

    rows.push(LettaConversationRow {
        id: "default".to_string(),
        title: "Main chat".to_string(),
        last_message_at: default_last,
        archived: false,
    });

    let list_body = match letta.list_conversations_for_agent(agent_id, 100).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(%e, "list conversations failed; returning main thread only");
            rows.sort_by(cmp_conversation_row_newest_first);
            let active = rows.clone();
            return AgentConversationsSnapshot {
                active,
                all: rows,
                archived_count: 0,
            };
        }
    };

    for item in letta_conversations_top_array(&list_body) {
        let Some(id) = item
            .get("id")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if id == "default" {
            continue;
        }
        if !id.starts_with("conv-") {
            continue;
        }

        let archived = conversation_is_archived(item);
        if archived {
            archived_count += 1;
        }

        let last_message_at = item
            .get("last_message_at")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                item.get("updated_at")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            });
        let title = resolve_conversation_row_title(letta, item, id).await;
        rows.push(LettaConversationRow {
            id: id.to_string(),
            title,
            last_message_at,
            archived,
        });
    }

    rows.sort_by(cmp_conversation_row_newest_first);
    let all = rows.clone();
    let active: Vec<LettaConversationRow> = rows.into_iter().filter(|r| !r.archived).collect();

    AgentConversationsSnapshot {
        active,
        all,
        archived_count,
    }
}
