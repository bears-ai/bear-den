// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
//! End-user JSON + SSE under `/v1/*` (session cookie, same origin as Deep Chat).

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use axum_extra::extract::Query;
use axum_login::login_required;
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    auth_backend::{AuthSession, Backend},
    core::{
        acp_sessions, archived_conversations,
        bears::{
            db::{self as bears_db, role_is_bear_admin},
            BearAgentRole,
        },
        den_tools::{self, DenToolChannelContext, DenToolInvocationContext},
        letta::{load_agent_conversations, strip_letta_harness_for_user},
        work_plans::{self, WorkPlanListFilter, WorkPlanStatus},
    },
    errors::CustomError,
    observability::chat_proxy_stream::BearChannelSseProxyStream,
    web::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bears", get(list_my_bears))
        .route("/chat/conversations", get(chat_conversations))
        .route(
            "/chat/conversations/{conversation_id}",
            patch(chat_conversation_patch),
        )
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
    /// Letta conversation: `default` (agent main conversation) or `conv-…`.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Letta cursor: messages older than this id (see `GET /v1/agents/{id}/messages?before=`).
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub debug: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChatConversationsQuery {
    pub bear_id: Uuid,
}

#[derive(Serialize)]
pub struct ChatConversationRow {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<String>,
}

#[derive(Serialize)]
pub struct ChatConversationsResponse {
    pub conversations: Vec<ChatConversationRow>,
}

#[derive(Debug, Deserialize)]
pub struct ChatConversationPatchBody {
    pub bear_id: Uuid,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub deleted: Option<bool>,
}

#[derive(Serialize)]
pub struct ChatConversationPatchResponse {
    pub ok: bool,
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

/// `None` / empty / `default` → agent main conversation. Existing Letta conversations are `conv-...`.
/// The web UI may also send a temporary `new-...` placeholder before Letta allocates the real
/// conversation id; Codepool turns that into an SDK `createSession(agent_id)` call.
fn normalize_client_conversation_id(raw: Option<&str>) -> Result<String, CustomError> {
    let s = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    if s == "default" {
        return Ok("default".to_string());
    }
    let ok = (s.starts_with("conv-") || s.starts_with("new-"))
        && s.len() > 8
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(s.to_string())
    } else {
        Err(CustomError::ValidationError(format!(
            "invalid conversation_id (expected 'default', a Letta conv- id, or a pending new- id): {s}"
        )))
    }
}

async fn chat_conversations(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(q): Query<ChatConversationsQuery>,
) -> Result<Json<ChatConversationsResponse>, CustomError> {
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

    let default_only = || {
        Json(ChatConversationsResponse {
            conversations: vec![ChatConversationRow {
                id: "default".to_string(),
                title: "Main chat".to_string(),
                last_message_at: None,
            }],
        })
    };

    if !state.letta.is_enabled() {
        return Ok(default_only());
    }

    let Some(agent_id) = bears_db::role_agent_id(state.sqlx_pool(), bear.id, BearAgentRole::Talk)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return Ok(default_only());
    };

    let archived_ids = archived_conversations::list_for_bear(state.sqlx_pool(), bear.id).await?;
    let snap = load_agent_conversations(state.letta.as_ref(), &agent_id).await;
    let conversations: Vec<ChatConversationRow> = snap
        .all
        .into_iter()
        .filter(|r| !r.archived && !archived_ids.contains(&r.id))
        .map(|r| ChatConversationRow {
            id: r.id,
            title: r.title,
            last_message_at: r.last_message_at,
        })
        .collect();

    Ok(Json(ChatConversationsResponse { conversations }))
}

async fn chat_conversation_patch(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(conversation_id): Path<String>,
    Json(body): Json<ChatConversationPatchBody>,
) -> Result<Json<ChatConversationPatchResponse>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, body.bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }

    let conv_id = normalize_client_conversation_id(Some(&conversation_id))?;
    if conv_id == "default" || conv_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "only saved conversations can be renamed or archived".to_string(),
        ));
    }

    let bear = bears_db::get_bear(state.sqlx_pool(), body.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    if !state.letta.is_enabled() {
        return Err(CustomError::System(
            "Letta is not configured (set LETTA_BASE_URL)".to_string(),
        ));
    }

    let Some(agent_id) = bears_db::role_agent_id(state.sqlx_pool(), bear.id, BearAgentRole::Talk)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return Err(CustomError::ValidationError(
            "this bear is not linked to a talk Letta agent".to_string(),
        ));
    };

    let snap = load_agent_conversations(state.letta.as_ref(), &agent_id).await;
    let found = snap.all.iter().any(|r| r.id == conv_id);
    if !found {
        return Err(CustomError::NotFound("conversation not found".to_string()));
    }

    let title = body
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if body.title.is_some() && title.is_none() {
        return Err(CustomError::ValidationError(
            "conversation title cannot be empty".to_string(),
        ));
    }
    if body.title.is_none() && body.archived.is_none() && body.deleted != Some(true) {
        return Err(CustomError::ValidationError(
            "no conversation update requested".to_string(),
        ));
    }

    if body.deleted == Some(true) {
        state.letta.delete_conversation(&conv_id).await?;
        archived_conversations::set_archived(
            state.sqlx_pool(),
            bear.id,
            &conv_id,
            Some(user_id),
            "delete",
            false,
        )
        .await?;
        return Ok(Json(ChatConversationPatchResponse { ok: true }));
    }

    if let Some(title) = title {
        let title = title.chars().take(120).collect::<String>();
        state
            .letta
            .patch_conversation_summary(&conv_id, &title)
            .await?;
        let _ = acp_sessions::set_title_for_bear_conversation(
            state.sqlx_pool(),
            bear.id,
            &conv_id,
            &title,
        )
        .await?;
    }

    if let Some(archived) = body.archived {
        state
            .letta
            .patch_conversation_archived(&conv_id, archived)
            .await?;
        archived_conversations::set_archived(
            state.sqlx_pool(),
            bear.id,
            &conv_id,
            Some(user_id),
            "web",
            archived,
        )
        .await?;
    }

    Ok(Json(ChatConversationPatchResponse { ok: true }))
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

    let Some(agent_id) = bears_db::role_agent_id(state.sqlx_pool(), bear.id, BearAgentRole::Talk)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return Ok(empty());
    };

    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let before = q.before.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let conv_id = normalize_client_conversation_id(q.conversation_id.as_deref())?;
    let agent_for_conv = if conv_id == "default" {
        Some(agent_id.as_str())
    } else {
        None
    };

    let body = state
        .letta
        .list_conversation_messages(&conv_id, agent_for_conv, limit, before, false)
        .await?;

    let (messages, has_more, next_before) = map_letta_history_page(&body, limit, q.debug);
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
        return if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        };
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

/// Skip structured rows that are not end-user input (when Letta exposes `role` on the message).
fn letta_user_message_role_is_human(inner: &serde_json::Value, msg: &serde_json::Value) -> bool {
    for v in [inner, msg] {
        let Some(role) = v.get("role").and_then(|x| x.as_str()) else {
            continue;
        };
        let r = role.trim();
        if r.eq_ignore_ascii_case("system") || r.eq_ignore_ascii_case("developer") {
            return false;
        }
    }
    true
}

fn text_contains_letta_harness(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("<system-reminder")
        || t.contains("<system_reminder")
        || t.contains(
            "you have been forked from the primary conversational thread to run as an independent subagent",
        )
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

fn map_letta_history_page(
    body: &serde_json::Value,
    page_limit: u32,
    debug: bool,
) -> (Vec<ChatHistoryMessage>, bool, Option<String>) {
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
        let Some(mut text) = letta_message_text(inner).or_else(|| letta_message_text(msg)) else {
            continue;
        };
        let role = if debug {
            match mt {
                "system_message" => "system",
                "user_message" => {
                    if letta_user_message_role_is_human(inner, msg)
                        && !text_contains_letta_harness(&text)
                    {
                        "user"
                    } else {
                        "system"
                    }
                }
                "assistant_message" => {
                    if text_contains_letta_harness(&text)
                        && strip_letta_harness_for_user(&text).trim().is_empty()
                    {
                        "system"
                    } else {
                        "ai"
                    }
                }
                _ => continue,
            }
        } else {
            let role = match mt {
                "user_message" => "user",
                "assistant_message" => "ai",
                _ => continue,
            };
            if mt == "user_message" && !letta_user_message_role_is_human(inner, msg) {
                continue;
            }
            text = strip_letta_harness_for_user(&text);
            if text.trim().is_empty() {
                continue;
            }
            role
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
    /// Reserved for Letta conversation / OTID pass-through (optional).
    #[serde(default)]
    pub conversation_id: Option<String>,
}

fn chat_send_api_status_message(err: &CustomError) -> (StatusCode, String) {
    match err {
        CustomError::Anyhow(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        CustomError::System(s) => (StatusCode::UNPROCESSABLE_ENTITY, s.clone()),
        CustomError::Database(s) => (StatusCode::UNPROCESSABLE_ENTITY, s.clone()),
        CustomError::DatabaseUnavailable(s) => (StatusCode::SERVICE_UNAVAILABLE, s.clone()),
        CustomError::Session(s) => (StatusCode::INTERNAL_SERVER_ERROR, s.clone()),
        CustomError::Authentication(s) => (StatusCode::UNAUTHORIZED, s.clone()),
        CustomError::Authorization(s) => (StatusCode::FORBIDDEN, s.clone()),
        CustomError::Render(s) => (StatusCode::INTERNAL_SERVER_ERROR, s.clone()),
        CustomError::Parsing(s) => (StatusCode::UNPROCESSABLE_ENTITY, s.clone()),
        CustomError::Email(s) => (StatusCode::FAILED_DEPENDENCY, s.clone()),
        CustomError::NotFound(s) => (StatusCode::NOT_FOUND, s.clone()),
        CustomError::ValidationError(s) => (StatusCode::BAD_REQUEST, s.clone()),
    }
}

fn chat_send_error_response(err: CustomError, request_id: Uuid) -> Response {
    tracing::error!(%request_id, error = %err, "chat_send rejected");
    let (status, message) = chat_send_api_status_message(&err);
    let body = serde_json::json!({
        "error": message,
        "request_id": request_id,
    });
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    match Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body.to_string()))
    {
        Ok(r) => r,
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("response build: {e}"),
        )
            .into_response(),
    }
}

async fn web_chat_workboard_prompt_context(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    user_id: i32,
) -> Result<String, CustomError> {
    let plans = work_plans::list_visible_work_plans(
        pool,
        bear_id,
        BearAgentRole::Talk,
        user_id,
        WorkPlanListFilter {
            statuses: Some(vec![WorkPlanStatus::Active, WorkPlanStatus::Blocked]),
            owner_role: None,
            include_archived: false,
        },
    )
    .await?;
    Ok(work_plans::render_workboard_prompt_context(&plans))
}

async fn chat_send(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(body): Json<ChatSendRequest>,
) -> impl IntoResponse {
    let request_id = Uuid::new_v4();
    let result = async { chat_send_inner(state, auth_session, body, request_id).await }
        .instrument(tracing::info_span!("chat_send", request_id = %request_id))
        .await;
    match result {
        Ok(r) => r.into_response(),
        Err(e) => chat_send_error_response(e, request_id),
    }
}

fn parse_set_conversation_title_request(message: &str) -> Option<String> {
    let trimmed = message.trim();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in [
        "set conversation title to ",
        "rename conversation to ",
        "rename this conversation to ",
        "set this conversation title to ",
    ] {
        if lower.starts_with(prefix) {
            return Some(
                trimmed[prefix.len()..]
                    .trim()
                    .trim_matches(['\"', '\''])
                    .to_string(),
            )
            .filter(|title| !title.is_empty());
        }
    }
    None
}

async fn maybe_handle_direct_set_conversation_title(
    state: &AppState,
    bear: &crate::core::bears::Bear,
    talk_agent_id: &str,
    user_id: i32,
    username: Option<&str>,
    membership_role: Option<&str>,
    conv_id: &str,
    session_id: &str,
    message: &str,
    request_id: Uuid,
) -> Result<Option<Response>, CustomError> {
    let Some(title) = parse_set_conversation_title_request(message) else {
        return Ok(None);
    };
    let context = DenToolInvocationContext {
        bear_id: bear.id,
        bear_slug: bear.slug.clone(),
        role_agent_id: talk_agent_id.to_string(),
        agent_role: Some(BearAgentRole::Talk),
        user_id,
        username: username.map(str::to_string),
        membership_role: membership_role.map(str::to_string),
        conversation_id: conv_id.to_string(),
        session_id: session_id.to_string(),
        acp_session_id: None,
        conversation_selection: Some(conv_id.to_string()),
        runtime_target: Some(conv_id.to_string()),
        request_id: Some(request_id.to_string()),
        channel: DenToolChannelContext {
            family: Some("browser_chat".to_string()),
            client: Some("den_web".to_string()),
            protocol: Some("den_chat".to_string()),
        },
    };
    let value = den_tools::invoke_den_tool(
        state.sqlx_pool(),
        state.config.as_ref(),
        den_tools::DEN_CONVERSATION_SET_TITLE,
        serde_json::json!({ "title": title }),
        context,
    )
    .await?;
    let text = value
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("Conversation title updated.");
    let body = format!(
        "data: {}\n\ndata: {}\n\n",
        serde_json::json!({ "type": "assistant_delta", "text": text }),
        serde_json::json!({ "type": "done", "stop_reason": "end_turn" })
    );
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .map_err(|_| CustomError::System("invalid request id for response header".to_string()))?;
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body))
        .map_err(|err| CustomError::System(format!("response build: {err}")))?;
    Ok(Some(response))
}

async fn chat_send_inner(
    state: AppState,
    auth_session: AuthSession,
    body: ChatSendRequest,
    request_id: Uuid,
) -> Result<Response, CustomError> {
    let session_user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = session_user.id;
    let username = session_user.username.clone();

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

    let talk_agent_id = bears_db::role_agent_id(state.sqlx_pool(), bear.id, BearAgentRole::Talk)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CustomError::System(
                "This bear is not provisioned in Letta yet (missing talk role agent).".to_string(),
            )
        })?;
    let conv_id = normalize_client_conversation_id(body.conversation_id.as_deref())?;

    if !state.codepool.is_enabled() {
        return Err(CustomError::System(
            "Chat is unavailable: CODEPOOL_BASE_URL is not set (Codepool is required for \
             streaming when RUN_WEB=true)."
                .to_string(),
        ));
    }
    let runtime_plan =
        crate::core::bears::effective_runtime_plan(bear.runtime_plan.as_ref().map(|j| j.as_ref()));
    let membership_role =
        bears_db::membership_role_for_user(state.sqlx_pool(), user_id, body.bear_id)
            .await?
            .flatten();
    let session_id = format!("den-web:{}:{}", body.bear_id, conv_id);
    if let Some(response) = maybe_handle_direct_set_conversation_title(
        &state,
        &bear,
        &talk_agent_id,
        user_id,
        Some(username.as_str()),
        membership_role.as_deref(),
        &conv_id,
        &session_id,
        body.message.trim(),
        request_id,
    )
    .await?
    {
        return Ok(response);
    }

    let workboard_context =
        web_chat_workboard_prompt_context(state.sqlx_pool(), bear.id, user_id).await?;
    let upstream_message = format!("{}{}", body.message.trim(), workboard_context);

    crate::observability::metrics::chat_send_runtime_bear_channel();

    let upstream = state
        .codepool
        .post_bear_channel_message_streaming(
            &session_id,
            &conv_id,
            &bear,
            &talk_agent_id,
            user_id,
            Some(username.as_str()),
            membership_role.as_deref(),
            &upstream_message,
            &runtime_plan,
            request_id,
        )
        .await?;

    crate::observability::metrics::chat_send_started();

    let stream = BearChannelSseProxyStream::new(
        upstream.bytes_stream(),
        request_id,
        user_id,
        body.bear_id,
        conv_id,
    );

    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .map_err(|_| CustomError::System("invalid request id for response header".to_string()))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
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
        let (msgs, has_more, next_before) = map_letta_history_page(&body, 3, false);
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
        let (_msgs, has_more, _) = map_letta_history_page(&body, 1, false);
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
        let (msgs, _, _) = map_letta_history_page(&body, 10, false);
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
        let (msgs, _, nb) = map_letta_history_page(&body, 10, false);
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
        let (msgs, _, _) = map_letta_history_page(&body, 10, false);
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
        let (msgs, _, _) = map_letta_history_page(&body, 10, false);
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
        let (msgs, _, nb) = map_letta_history_page(&body, 10, false);
        assert_eq!(nb.as_deref(), Some("older"));
        assert_eq!(msgs[0].text, "first");
        assert_eq!(msgs[1].text, "second");
    }

    #[test]
    fn assistant_strips_harness_subagent_and_reminder() {
        let body = serde_json::json!([{
            "id": "a1",
            "date": "2025-01-01T00:00:00Z",
            "message_type": "assistant_message",
            "content": concat!(
                "If you want, I can help.\n",
                "<system-reminder>\n",
                "You have been forked from the primary conversational thread to run as an independent subagent.\n",
                "You CANNOT ask questions mid-execution - all instructions are provided upfront.\n",
                "</system-reminder>"
            )
        }]);
        let (msgs, _, _) = map_letta_history_page(&body, 10, false);
        assert_eq!(msgs[0].text, "If you want, I can help.");
    }

    #[test]
    fn user_harness_only_reminder_is_not_shown() {
        let body = serde_json::json!([
            {
                "id": "u1",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "<system-reminder>The user has just initiated a new connection via the Letta Code CLI client.</system-reminder>"
            },
            {
                "id": "u2",
                "date": "2025-01-02T00:00:00Z",
                "message_type": "user_message",
                "content": "Real user question"
            }
        ]);

        let (msgs, _, _) = map_letta_history_page(&body, 10, false);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Real user question");
    }

    #[test]
    fn user_inline_reminder_is_stripped_from_history() {
        let body = serde_json::json!([{
            "id": "u1",
            "date": "2025-01-01T00:00:00Z",
            "message_type": "user_message",
            "content": "<system-reminder>context</system-reminder>\n\nSummarize this doc"
        }]);

        let (msgs, _, _) = map_letta_history_page(&body, 10, false);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "Summarize this doc");
    }

    #[test]
    fn user_role_system_is_not_shown() {
        let body = serde_json::json!([
            {
                "id": "u1",
                "role": "system",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "You are a helpful assistant."
            },
            {
                "id": "u2",
                "date": "2025-01-02T00:00:00Z",
                "message_type": "user_message",
                "content": "What is 2+2?"
            }
        ]);

        let (msgs, _, _) = map_letta_history_page(&body, 10, false);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "What is 2+2?");
    }

    #[test]
    fn debug_history_keeps_harness_user_rows_as_system() {
        let body = serde_json::json!([
            {
                "id": "u1",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "<system-reminder>debug context</system-reminder>"
            },
            {
                "id": "u2",
                "date": "2025-01-02T00:00:00Z",
                "message_type": "user_message",
                "content": "Real user question"
            }
        ]);

        let (msgs, _, _) = map_letta_history_page(&body, 10, true);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(
            msgs[0].text,
            "<system-reminder>debug context</system-reminder>"
        );
        assert_eq!(msgs[1].role, "user");
    }

    #[test]
    fn debug_history_keeps_role_system_user_rows_as_system() {
        let body = serde_json::json!([
            {
                "id": "u1",
                "role": "system",
                "date": "2025-01-01T00:00:00Z",
                "message_type": "user_message",
                "content": "You are a helpful assistant."
            }
        ]);

        let (msgs, _, _) = map_letta_history_page(&body, 10, true);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].text, "You are a helpful assistant.");
    }
}

#[cfg(test)]
mod conversation_id_tests {
    use super::normalize_client_conversation_id;

    #[test]
    fn normalizes_default_aliases() {
        assert_eq!(normalize_client_conversation_id(None).unwrap(), "default");
        assert_eq!(
            normalize_client_conversation_id(Some("")).unwrap(),
            "default"
        );
        assert_eq!(
            normalize_client_conversation_id(Some("default")).unwrap(),
            "default"
        );
    }

    #[test]
    fn accepts_conv_prefix_ids() {
        assert_eq!(
            normalize_client_conversation_id(Some("conv-abc12345")).unwrap(),
            "conv-abc12345"
        );
    }

    #[test]
    fn accepts_pending_new_prefix_ids() {
        assert_eq!(
            normalize_client_conversation_id(Some("new-abc12345")).unwrap(),
            "new-abc12345"
        );
    }

    #[test]
    fn rejects_garbage_ids() {
        assert!(normalize_client_conversation_id(Some("../../../etc/passwd")).is_err());
        assert!(normalize_client_conversation_id(Some("conv-x")).is_err());
    }
}
