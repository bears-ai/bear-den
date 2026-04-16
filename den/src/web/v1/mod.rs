// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
//! End-user JSON + SSE under `/v1/*` (session cookie, same origin as Deep Chat).

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::{get, post},
};
use axum_extra::extract::Query;
use axum_login::login_required;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth_backend::{AuthSession, Backend},
    core::bears::db::{self as bears_db, role_is_bear_admin},
    errors::CustomError,
    web::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bears", get(list_my_bears))
        .route("/chat/history", get(chat_history))
        .route("/chat/send", post(chat_send))
        .route_layer(login_required!(Backend, login_url = "/login"))
}

/// Membership-filtered bears for the chat UI (no Letta agent id exposed).
#[derive(Serialize)]
pub struct BearPublic {
    pub bear_id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    /// `user_bear.role == "admin"` for this user (bear admin, not site operator).
    pub is_bear_admin: bool,
}

async fn list_my_bears(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Json<Vec<BearPublic>>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    let rows = bears_db::list_bears_for_user(state.sqlx_pool(), user_id).await?;
    let out: Vec<BearPublic> = rows
        .into_iter()
        .map(|row| BearPublic {
            bear_id: row.bear.id,
            slug: row.bear.slug,
            name: row.bear.name,
            description: row.bear.description,
            is_bear_admin: role_is_bear_admin(row.membership_role.as_deref()),
        })
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct ChatHistoryQuery {
    pub bear_id: Uuid,
    /// Letta cursor: messages older than this id (see `GET /v1/agents/{id}/messages?before=`).
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Serialize)]
pub struct ChatHistoryMessage {
    pub role: String,
    pub text: String,
}

#[derive(Serialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<ChatHistoryMessage>,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before: Option<String>,
}

async fn chat_history(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(q): Query<ChatHistoryQuery>,
) -> Result<Json<ChatHistoryResponse>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, q.bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }

    let bear = bears_db::get_bear(state.sqlx_pool(), q.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let empty = || {
        Json(ChatHistoryResponse {
            messages: vec![],
            has_more: false,
            next_before: None,
        })
    };

    if !state.letta.is_enabled() {
        return Ok(empty());
    }

    let Some(agent_id) = bear
        .letta_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(empty());
    };

    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let before = q.before.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let body = state
        .letta
        .list_agent_messages(agent_id, limit, before)
        .await?;

    let (messages, has_more, next_before) = map_letta_history_page(&body, limit);
    Ok(Json(ChatHistoryResponse {
        messages,
        has_more,
        next_before,
    }))
}

fn letta_messages_top_array<'a>(v: &'a serde_json::Value) -> &'a [serde_json::Value] {
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

/// Same unwrap as `lettaInnerForStream` in `bear_chat.html` (stream payloads may nest under `contents`).
fn letta_inner_for_history(msg: &serde_json::Value) -> &serde_json::Value {
    match msg.get("contents") {
        Some(c) if c.get("message_type").is_some() => c,
        _ => msg,
    }
}

fn letta_message_type<'a>(msg: &'a serde_json::Value, inner: &'a serde_json::Value) -> &'a str {
    inner
        .get("message_type")
        .and_then(|x| x.as_str())
        .or_else(|| msg.get("message_type").and_then(|x| x.as_str()))
        .unwrap_or("")
}

/// ISO `date` from the envelope (sorting must not use `inner` alone — `contents` may omit it).
fn letta_message_sort_key(msg: &serde_json::Value) -> (String, i64) {
    let date = msg
        .get("date")
        .or_else(|| msg.get("created_at"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let seq = msg.get("seq_id").and_then(|x| x.as_i64()).unwrap_or(0);
    (date, seq)
}

fn letta_message_text(inner: &serde_json::Value) -> Option<String> {
    let content = inner.get("content")?;
    if content.is_null() {
        return None;
    }
    if let Some(s) = content.as_str() {
        let s = s.trim();
        return if s.is_empty() { None } else { Some(s.to_string()) };
    }
    // Single structured part, e.g. `{ "type": "text", "text": "..." }`
    if let Some(obj) = content.as_object() {
        if let Some(t) = obj.get("text").and_then(|x| x.as_str()) {
            let t = t.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    let parts = content.as_array()?;
    let mut out = String::new();
    for p in parts {
        let ty = p.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if matches!(
            ty,
            "text" | "Text" | "text_delta" | "reasoning_text" | "output_text"
        ) {
            if let Some(t) = p.get("text").and_then(|x| x.as_str()) {
                out.push_str(t);
            }
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn letta_message_id_string(m: &serde_json::Value) -> Option<String> {
    match m.get("id")? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Cursor for `before=` on the next page: chronologically oldest id in this Letta batch (any type).
fn oldest_raw_message_id(raw: &[serde_json::Value]) -> Option<String> {
    let mut best: Option<(String, i64, String)> = None;
    for m in raw {
        let Some(id) = letta_message_id_string(m) else {
            continue;
        };
        let key = letta_message_sort_key(m);
        if best
            .as_ref()
            .is_none_or(|b| key.0 < b.0 || (key.0 == b.0 && key.1 < b.1))
        {
            best = Some((key.0, key.1, id));
        }
    }
    best.map(|(_, _, id)| id)
}

fn map_letta_history_page(body: &serde_json::Value, page_limit: u32) -> (Vec<ChatHistoryMessage>, bool, Option<String>) {
    let raw = letta_messages_top_array(body);
    let next_before = oldest_raw_message_id(raw);
    let has_more = raw.len() >= page_limit as usize;

    #[derive(Clone)]
    struct Row {
        /// `(date, seq_id, raw_index)` — `raw_index` stabilizes ordering when dates are absent.
        sort: (String, i64, usize),
        role: String,
        text: String,
    }

    let mut rows: Vec<Row> = Vec::new();
    for (raw_idx, msg) in raw.iter().enumerate() {
        let inner = letta_inner_for_history(msg);
        let mt = letta_message_type(msg, inner);
        let role = match mt {
            "user_message" => "user",
            "assistant_message" => "ai",
            _ => continue,
        };
        let Some(text) = letta_message_text(inner).or_else(|| letta_message_text(msg)) else {
            continue;
        };
        let (d, s) = letta_message_sort_key(msg);
        rows.push(Row {
            sort: (d, s, raw_idx),
            role: role.to_string(),
            text,
        });
    }

    let batch_has_dates = raw.iter().any(|m| !letta_message_sort_key(m).0.is_empty());
    if batch_has_dates {
        rows.sort_by(|a, b| a.sort.cmp(&b.sort));
    } else {
        // Letta is requested with `order=desc` (newest first). Without timestamps, preserve that
        // ordering by sorting mapped rows in descending raw index (older messages first).
        rows.sort_by(|a, b| b.sort.2.cmp(&a.sort.2));
    }

    let messages = rows
        .into_iter()
        .map(|r| ChatHistoryMessage {
            role: r.role,
            text: r.text,
        })
        .collect();

    (messages, has_more, next_before)
}

#[derive(Debug, Deserialize)]
pub struct ChatSendRequest {
    pub bear_id: Uuid,
    pub message: String,
    /// Reserved for Letta threading / OTID pass-through (optional).
    #[serde(default)]
    pub conversation_id: Option<String>,
}

async fn chat_send(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(body): Json<ChatSendRequest>,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    if body.message.trim().is_empty() {
        return Err(CustomError::ValidationError(
            "message must not be empty".to_string(),
        ));
    }

    if !state.letta.is_enabled() {
        return Err(CustomError::System(
            "Chat is unavailable: LETTA_BASE_URL is not set".to_string(),
        ));
    }

    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, body.bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }

    let bear = bears_db::get_bear(state.sqlx_pool(), body.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let agent_id = bear
        .letta_agent_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CustomError::System(
                "This bear is not provisioned in Letta yet (missing letta_agent_id).".to_string(),
            )
        })?;

    if body.conversation_id.is_some() {
        tracing::debug!("conversation_id accepted but not yet forwarded to Letta in this build");
    }

    if let Err(e) = bears_db::record_chat_activity(
        state.sqlx_pool(),
        body.bear_id,
        user_id,
        "den_web",
        body.message.trim(),
    )
    .await
    {
        tracing::warn!(%user_id, bear_id = %body.bear_id, "bear_chat_activity insert failed: {e}");
    }

    let upstream = state
        .letta
        .post_messages_streaming(agent_id, body.message.trim())
        .await?;

    let stream = upstream.bytes_stream().map(|res| {
        res.map_err(|e| std::io::Error::other(e.to_string()))
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .map_err(|e| CustomError::System(format!("response build: {e}")))
}

#[cfg(test)]
mod chat_history_map_tests {
    use super::*;

    #[test]
    fn map_page_desc_order_to_chronological_bubbles() {
        let body = serde_json::json!([
            {
                "id": "m-new",
                "date": "2025-01-02T00:00:00Z",
                "message_type": "assistant_message",
                "content": "Hi there"
            },
            {
                "id": "m-old",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "Hello"
            }
        ]);
        let (msgs, has_more, next_before) = map_letta_history_page(&body, 3);
        assert_eq!(next_before.as_deref(), Some("m-old"));
        assert!(!has_more);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].role, "ai");
        assert_eq!(msgs[1].text, "Hi there");
    }

    #[test]
    fn has_more_when_page_full() {
        let body = serde_json::json!([
            {
                "id": "a",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "x"
            }
        ]);
        let (_msgs, has_more, _) = map_letta_history_page(&body, 1);
        assert!(has_more);
    }

    #[test]
    fn unwraps_contents_wrapper() {
        let body = serde_json::json!([
            {
                "id": "w",
                "date": "2025-01-01T00:00:00Z",
                "contents": {
                    "message_type": "user_message",
                    "content": "wrapped"
                }
            }
        ]);
        let (msgs, _, _) = map_letta_history_page(&body, 10);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "wrapped");
    }

    #[test]
    fn skips_unknown_message_types() {
        let body = serde_json::json!([
            {
                "id": "t",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "tool_call_message",
                "content": "{}"
            },
            {
                "id": "u",
                "date": "2025-01-02T00:00:00Z",
                "message_type": "user_message",
                "content": "ok"
            }
        ]);
        let (msgs, _, nb) = map_letta_history_page(&body, 10);
        assert_eq!(nb.as_deref(), Some("t"));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "ok");
    }

    #[test]
    fn assistant_content_as_single_text_object() {
        let body = serde_json::json!([
            {
                "id": "a1",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "hey"
            },
            {
                "id": "a2",
                "date": "2025-01-02T00:00:00Z",
                "message_type": "assistant_message",
                "content": {"type": "text", "text": "hello back"}
            }
        ]);
        let (msgs, _, _) = map_letta_history_page(&body, 10);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].role, "ai");
        assert_eq!(msgs[1].text, "hello back");
    }

    #[test]
    fn user_text_on_envelope_when_contents_has_no_content() {
        let body = serde_json::json!([
            {
                "id": "u1",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "from envelope",
                "contents": {
                    "message_type": "user_message"
                }
            }
        ]);
        let (msgs, _, _) = map_letta_history_page(&body, 10);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "from envelope");
    }

    #[test]
    fn oldest_cursor_skips_rows_without_id() {
        let raw = vec![
            serde_json::json!({"date": "2025-01-02T00:00:00Z", "message_type": "tool_call_message"}),
            serde_json::json!({"id": "keep", "date": "2025-01-01T00:00:00Z", "message_type": "user_message", "content": "x"}),
        ];
        assert_eq!(oldest_raw_message_id(&raw).as_deref(), Some("keep"));
    }

    #[test]
    fn sorts_by_date_when_api_order_is_asc() {
        let body = serde_json::json!([
            {
                "id": "older",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "first"
            },
            {
                "id": "newer",
                "date": "2025-01-02T00:00:00Z",
                "message_type": "assistant_message",
                "content": "second"
            }
        ]);
        let (msgs, _, nb) = map_letta_history_page(&body, 10);
        assert_eq!(nb.as_deref(), Some("older"));
        assert_eq!(msgs[0].text, "first");
        assert_eq!(msgs[1].text, "second");
    }
}
