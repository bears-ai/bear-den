//! Minimal Agent Client Protocol (ACP) gateway for adapter clients.
//!
//! This is the Phase 7 basic-chat slice: Den authenticates, authorizes the selected bear,
//! injects trusted context, and maps text prompts to the Bear's API-direct `pair` Letta agent.
//! Client-tool relay and full ACP stdio transport live in later slices / an external adapter.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use bytes::Bytes;
use futures::{ready, Stream};
use serde::{Deserialize, Serialize};
use std::{future::Future, path::Path as FsPath, pin::Pin, task::Poll};
use time::format_description::well_known::Rfc3339;
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    api::{
        auth::{self, ApiError},
        oauth::OAuthScope,
        service::ApiState,
    },
    core::{
        acp_sessions::{self, UpsertAcpSession},
        acp_tokens, archived_conversations,
        bears::{db as bears_db, Bear, BearAgentRole},
        letta::load_agent_conversations,
    },
    errors::CustomError,
};

const ACP_SESSIONS_PAGE_SIZE: i64 = 50;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/bears/{slug}/sessions", get(list_acp_sessions))
        .route("/bears/{slug}/sessions/{session_id}", get(get_acp_session))
        .route("/bears/{slug}/sessions/{session_id}/prompt", post(prompt))
        .route(
            "/bears/{slug}/sessions/{session_id}/close",
            post(close_session),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/cancel",
            post(cancel_session),
        )
        .route("/bears/{slug}/conversations", get(conversations))
        .route(
            "/bears/{slug}/conversations/{conversation_id}/history",
            get(conversation_history),
        )
        .route("/bears/{slug}/auth-check", get(auth_check))
}

#[derive(Debug, Deserialize)]
pub struct AcpPromptRequest {
    pub message: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub client_capabilities: serde_json::Value,
    #[serde(default)]
    pub client_context: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct AcpCloseSessionResponse {
    ok: bool,
    archived: bool,
    conversation_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcpErrorResponse {
    error: String,
    error_code: &'static str,
    request_id: String,
}

#[derive(Debug, Deserialize)]
struct AcpConversationsQuery {
    #[serde(default)]
    include_archived: bool,
}

#[derive(Debug, Serialize)]
struct AcpConversationRow {
    id: String,
    title: String,
    last_message_at: Option<String>,
    archived: bool,
}

#[derive(Debug, Serialize)]
struct AcpConversationsResponse {
    conversations: Vec<AcpConversationRow>,
}

#[derive(Debug, Deserialize)]
struct AcpConversationHistoryQuery {
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct AcpConversationHistoryMessage {
    role: String,
    text: String,
    created_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcpConversationHistoryResponse {
    messages: Vec<AcpConversationHistoryMessage>,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_before: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AcpSessionsListQuery {
    #[serde(default)]
    include_closed: bool,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcpSessionsListHttpResponse {
    sessions: Vec<AcpSessionHttp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcpSessionHttp {
    acp_session_id: String,
    runtime_session_id: String,
    conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_conversation_id: Option<String>,
    client: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    closed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
}

fn format_acp_session_timestamp(t: time::OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

fn acp_session_row_to_http(row: acp_sessions::AcpSessionRow) -> AcpSessionHttp {
    AcpSessionHttp {
        acp_session_id: row.acp_session_id,
        runtime_session_id: row.runtime_session_id,
        conversation_id: row.conversation_id,
        resolved_conversation_id: row.resolved_conversation_id,
        client: row.client,
        cwd: row.cwd,
        closed_at: row.closed_at.map(format_acp_session_timestamp),
        archived_at: row.archived_at.map(format_acp_session_timestamp),
        created_at: format_acp_session_timestamp(row.created_at),
        updated_at: format_acp_session_timestamp(row.updated_at),
    }
}

#[derive(Debug, Clone)]
struct AcpSessionsCursor {
    updated_at: time::OffsetDateTime,
    id: Uuid,
}

fn encode_acp_sessions_cursor(row: &acp_sessions::AcpSessionRow) -> String {
    let payload = serde_json::json!({
        "updated_at": format_acp_session_timestamp(row.updated_at),
        "id": row.id,
    });
    URL_SAFE_NO_PAD.encode(
        serde_json::to_string(&payload)
            .unwrap_or_else(|_| r#"{}"#.to_string())
            .as_bytes(),
    )
}

fn decode_acp_sessions_cursor(raw: Option<&str>) -> Result<Option<AcpSessionsCursor>, CustomError> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .map_err(|_| CustomError::ValidationError("invalid sessions cursor".to_string()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| CustomError::ValidationError("invalid sessions cursor payload".to_string()))?;
    if value.get("offset").is_some() {
        return Err(CustomError::ValidationError(
            "stale offset-based sessions cursor; restart pagination".to_string(),
        ));
    }
    let updated_at_raw = value
        .get("updated_at")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            CustomError::ValidationError("invalid sessions cursor updated_at".to_string())
        })?;
    let updated_at = time::OffsetDateTime::parse(updated_at_raw, &Rfc3339).map_err(|_| {
        CustomError::ValidationError("invalid sessions cursor updated_at".to_string())
    })?;
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CustomError::ValidationError("invalid sessions cursor id".to_string()))?
        .parse::<Uuid>()
        .map_err(|_| CustomError::ValidationError("invalid sessions cursor id".to_string()))?;
    Ok(Some(AcpSessionsCursor { updated_at, id }))
}

fn is_absolute_local_path(path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    FsPath::new(path).is_absolute()
        || path.starts_with("\\\\")
        || (path.len() >= 3
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':'
            && matches!(path.as_bytes()[2], b'/' | b'\\'))
}

fn optional_absolute_cwd_filter(raw: Option<&str>) -> Result<Option<&str>, CustomError> {
    let Some(cwd) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if is_absolute_local_path(cwd) {
        Ok(Some(cwd))
    } else {
        tracing::warn!(cwd = %cwd, "rejecting non-absolute ACP sessions cwd filter");
        Err(CustomError::ValidationError(
            "cwd filter must be an absolute local path".to_string(),
        ))
    }
}

fn require_absolute_cwd(raw: Option<&str>) -> Result<String, CustomError> {
    let Some(cwd) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        tracing::warn!("rejecting ACP prompt with missing cwd");
        return Err(CustomError::ValidationError(
            "ACP client_context.cwd must be an absolute local path".to_string(),
        ));
    };
    if is_absolute_local_path(cwd) {
        Ok(cwd.to_string())
    } else {
        tracing::warn!(cwd = %cwd, "rejecting ACP prompt with non-absolute cwd");
        Err(CustomError::ValidationError(
            "ACP client_context.cwd must be an absolute local path".to_string(),
        ))
    }
}

fn acp_error_status_message(err: &CustomError) -> (StatusCode, &'static str, String) {
    match err {
        CustomError::Authentication(s) => (StatusCode::UNAUTHORIZED, "authentication", s.clone()),
        CustomError::Authorization(s) => (StatusCode::FORBIDDEN, "authorization", s.clone()),
        CustomError::NotFound(s) => (StatusCode::NOT_FOUND, "not_found", s.clone()),
        CustomError::ValidationError(s) => (StatusCode::BAD_REQUEST, "validation", s.clone()),
        CustomError::DatabaseUnavailable(s) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "database_unavailable",
            s.clone(),
        ),
        CustomError::System(s)
        | CustomError::Database(s)
        | CustomError::Session(s)
        | CustomError::Parsing(s) => (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable", s.clone()),
        CustomError::Render(s) => (StatusCode::INTERNAL_SERVER_ERROR, "render", s.clone()),
        CustomError::Email(s) => (StatusCode::FAILED_DEPENDENCY, "email", s.clone()),
        CustomError::Anyhow(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("{e:#}"),
        ),
    }
}

fn acp_error_response(err: CustomError, request_id: Uuid) -> Response {
    tracing::error!(%request_id, error = %err, "ACP prompt rejected");
    let (status, error_code, message) = acp_error_status_message(&err);
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    let body = serde_json::to_string(&AcpErrorResponse {
        error: message,
        error_code,
        request_id: request_id.to_string(),
    })
    .unwrap_or_else(|_| "{\"error\":\"response serialization failed\"}".to_string());

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn api_auth_error_response(err: ApiError, request_id: Uuid) -> Response {
    tracing::error!(
        %request_id,
        error_code = err.error_code,
        error = %err.message,
        "ACP prompt authentication rejected"
    );
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    let body = serde_json::to_string(&AcpErrorResponse {
        error: err.message,
        error_code: err.error_code,
        request_id: request_id.to_string(),
    })
    .unwrap_or_else(|_| "{\"error\":\"response serialization failed\"}".to_string());

    Response::builder()
        .status(err.status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn normalize_acp_client(raw: Option<&str>) -> String {
    let value = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("acp_adapter");
    match value.to_ascii_lowercase().as_str() {
        "zed" => "zed".to_string(),
        "opencode" => "opencode".to_string(),
        _ => "acp_adapter".to_string(),
    }
}

fn new_acp_conversation_id(client: &str) -> String {
    let uuid = Uuid::new_v4();
    format!(
        "new-acp-{client}-{}",
        URL_SAFE_NO_PAD.encode(uuid.as_bytes())
    )
}

fn is_valid_pending_acp_conversation_id(conversation_id: &str) -> bool {
    conversation_id.starts_with("new-")
        && conversation_id.len() <= 42
        && normalize_acp_conversation_id(Some(conversation_id)).is_ok()
}

fn normalize_acp_conversation_id(raw: Option<&str>) -> Result<String, CustomError> {
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

fn acp_missing_pair_agent_message(bear_slug: &str) -> String {
    format!(
        "ACP requires this Bear to have a provisioned `pair` role Letta agent, but none is recorded for bear `{bear_slug}`. Ask an operator to open Admin → Bears → this Bear and click `Provision missing role agents`, then retry."
    )
}

async fn require_pair_agent_id(state: &ApiState, bear: &Bear) -> Result<String, CustomError> {
    if !state.letta.is_enabled() {
        return Err(CustomError::System(
            "Letta is not configured (set LETTA_BASE_URL); ACP pair role cannot run.".to_string(),
        ));
    }
    bears_db::role_agent_id(&state.sqlx_pool, bear.id, BearAgentRole::Pair)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CustomError::ValidationError(acp_missing_pair_agent_message(&bear.slug)))
}

async fn verify_acp_conversation_belongs_to_bear(
    state: &ApiState,
    agent_id: &str,
    conversation_id: &str,
) -> Result<(), CustomError> {
    if conversation_id == "default" || conversation_id.starts_with("new-") {
        return Ok(());
    }
    if !conversation_id.starts_with("conv-") {
        return Err(CustomError::ValidationError(format!(
            "invalid conversation_id: {conversation_id}"
        )));
    }
    if !state.letta.is_enabled() {
        return Err(CustomError::System(
            "Letta is not configured (set LETTA_BASE_URL)".to_string(),
        ));
    }
    let agent_id = agent_id.trim();
    if agent_id.is_empty() {
        return Err(CustomError::ValidationError(
            "this bear is not linked to a Letta agent".to_string(),
        ));
    }
    let snap = load_agent_conversations(state.letta.as_ref(), agent_id).await;
    let found = snap.all.iter().any(|row| row.id == conversation_id);
    if found {
        Ok(())
    } else {
        Err(CustomError::Authorization(
            "conversation not found for this bear".to_string(),
        ))
    }
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

fn letta_inner_for_acp_history(msg: &serde_json::Value) -> &serde_json::Value {
    match msg.get("contents") {
        Some(c) if c.get("message_type").is_some() => c,
        _ => msg,
    }
}

fn letta_message_text(inner: &serde_json::Value) -> Option<String> {
    let content = inner.get("content")?;
    if let Some(s) = content.as_str() {
        let s = s.trim();
        return if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        };
    }
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
    for part in parts {
        if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
            out.push_str(t);
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn letta_message_id_string(msg: &serde_json::Value) -> Option<String> {
    match msg.get("id")? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn letta_message_created_at(msg: &serde_json::Value) -> Option<String> {
    msg.get("date")
        .or_else(|| msg.get("created_at"))
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

fn map_acp_history_page(
    body: &serde_json::Value,
    page_limit: u32,
) -> (Vec<AcpConversationHistoryMessage>, bool, Option<String>) {
    let raw = letta_messages_top_array(body);
    let has_more = raw.len() >= page_limit as usize;
    let next_before = raw.iter().filter_map(letta_message_id_string).next_back();
    let mut rows = Vec::new();
    for msg in raw.iter().rev() {
        let inner = letta_inner_for_acp_history(msg);
        let message_type = inner
            .get("message_type")
            .and_then(|x| x.as_str())
            .or_else(|| msg.get("message_type").and_then(|x| x.as_str()))
            .unwrap_or("");
        let role = match message_type {
            "user_message" => "user",
            "assistant_message" => "assistant",
            _ => continue,
        };
        let Some(text) = letta_message_text(inner).or_else(|| letta_message_text(msg)) else {
            continue;
        };
        rows.push(AcpConversationHistoryMessage {
            role: role.to_string(),
            text,
            created_at: letta_message_created_at(msg),
        });
    }
    (rows, has_more, next_before)
}

async fn prompt(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpPromptRequest>,
) -> impl IntoResponse {
    let request_id = Uuid::new_v4();
    let result = async { prompt_inner(state, slug, session_id, headers, body, request_id).await }
        .instrument(tracing::info_span!("acp_prompt", request_id = %request_id))
        .await;
    match result {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => acp_error_response(err, request_id),
        Err(err) => api_auth_error_response(err, request_id),
    }
}

async fn auth_check(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match authenticate_acp_code_token(&state, &headers, &slug).await {
        Ok(user_id) => Json(serde_json::json!({
            "ok": true,
            "user_id": user_id,
            "scopes": {
                "acp:chat": true
            }
        }))
        .into_response(),
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn authenticate_acp_code_token(
    state: &ApiState,
    headers: &HeaderMap,
    slug: &str,
) -> Result<i32, CustomError> {
    let token = auth::extract_bearer_token(headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    if !acp_tokens::is_acp_token(&token) {
        return Err(CustomError::Authentication(
            "expected a bear-scoped BEARS ACP token".to_string(),
        ));
    }
    let auth = acp_tokens::authenticate_for_bear_slug_with_scopes(&state.sqlx_pool, &token, slug)
        .await?
        .ok_or_else(|| {
            CustomError::Authentication(
                "invalid, expired, revoked, or unauthorized ACP token".to_string(),
            )
        })?;
    if !acp_tokens::scopes_contains(&auth.scopes, OAuthScope::AcpChat.as_str()) {
        return Err(CustomError::Authentication(
            "ACP token is missing required acp:chat scope".to_string(),
        ));
    }
    Ok(auth.user_id)
}

async fn list_acp_sessions(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(query): Query<AcpSessionsListQuery>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match list_acp_sessions_inner(state, slug, query, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn list_acp_sessions_inner(
    state: ApiState,
    slug: String,
    query: AcpSessionsListQuery,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;
    let cursor = decode_acp_sessions_cursor(query.cursor.as_deref())?;
    let cwd_filter = optional_absolute_cwd_filter(query.cwd.as_deref())?;
    let fetch_limit = ACP_SESSIONS_PAGE_SIZE + 1;
    let mut rows = acp_sessions::list_for_user_bear(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        query.include_closed,
        cwd_filter,
        fetch_limit,
        cursor.as_ref().map(|c| c.updated_at),
        cursor.as_ref().map(|c| c.id),
    )
    .await?;
    let has_more = rows.len() > ACP_SESSIONS_PAGE_SIZE as usize;
    rows.truncate(ACP_SESSIONS_PAGE_SIZE as usize);
    let next_cursor = if has_more {
        rows.last().map(encode_acp_sessions_cursor)
    } else {
        None
    };
    let mut sessions = Vec::new();
    for row in rows {
        if row
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| is_absolute_local_path(s))
            .is_none()
        {
            tracing::warn!(
                acp_session_id = %row.acp_session_id,
                bear_slug = %row.bear_slug,
                "omitting ACP session list row with missing or non-absolute cwd"
            );
            continue;
        }
        sessions.push(acp_session_row_to_http(row));
    }
    Ok(Json(AcpSessionsListHttpResponse {
        sessions,
        next_cursor,
    })
    .into_response())
}

async fn get_acp_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match get_acp_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn get_acp_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err(CustomError::ValidationError(
            "session_id must not be empty".to_string(),
        ));
    }
    let row =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?;
    Ok(Json(acp_session_row_to_http(row)).into_response())
}

async fn conversations(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(query): Query<AcpConversationsQuery>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match conversations_inner(state, slug, query, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn conversations_inner(
    state: ApiState,
    slug: String,
    query: AcpConversationsQuery,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;

    let default_only = || {
        Json(AcpConversationsResponse {
            conversations: vec![AcpConversationRow {
                id: "default".to_string(),
                title: "Main chat".to_string(),
                last_message_at: None,
                archived: false,
            }],
        })
        .into_response()
    };

    if !state.letta.is_enabled() {
        return Ok(default_only());
    }
    let agent_id = require_pair_agent_id(&state, &bear).await?;

    let archived_ids = archived_conversations::list_for_bear(&state.sqlx_pool, bear.id).await?;
    let snap = load_agent_conversations(state.letta.as_ref(), &agent_id).await;
    let source: Vec<_> = if query.include_archived {
        snap.all
            .into_iter()
            .map(|mut row| {
                if archived_ids.contains(&row.id) {
                    row.archived = true;
                }
                row
            })
            .collect()
    } else {
        snap.all
            .into_iter()
            .filter(|row| !row.archived && !archived_ids.contains(&row.id))
            .collect()
    };
    let conversations = source
        .into_iter()
        .map(|row| AcpConversationRow {
            id: row.id,
            title: row.title,
            last_message_at: row.last_message_at,
            archived: row.archived,
        })
        .collect();
    Ok(Json(AcpConversationsResponse { conversations }).into_response())
}

async fn conversation_history(
    State(state): State<ApiState>,
    Path((slug, conversation_id)): Path<(String, String)>,
    Query(query): Query<AcpConversationHistoryQuery>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match conversation_history_inner(state, slug, conversation_id, query, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn conversation_history_inner(
    state: ApiState,
    slug: String,
    conversation_id: String,
    query: AcpConversationHistoryQuery,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;
    if !state.letta.is_enabled() {
        return Ok(Json(AcpConversationHistoryResponse {
            messages: vec![],
            has_more: false,
            next_before: None,
        })
        .into_response());
    }
    let agent_id = require_pair_agent_id(&state, &bear).await?;
    let conv_id = normalize_acp_conversation_id(Some(&conversation_id))?;
    if conv_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "history is only available for default or saved conv- conversations".to_string(),
        ));
    }
    if conv_id.starts_with("conv-") {
        verify_acp_conversation_belongs_to_bear(&state, &agent_id, &conv_id).await?;
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let before = query
        .before
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let agent_for_conv = if conv_id == "default" {
        Some(agent_id.as_str())
    } else {
        None
    };
    let body = state
        .letta
        .list_conversation_messages(&conv_id, agent_for_conv, limit, before, false)
        .await?;
    let (messages, has_more, next_before) = map_acp_history_page(&body, limit);
    Ok(Json(AcpConversationHistoryResponse {
        messages,
        has_more,
        next_before,
    })
    .into_response())
}

async fn close_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let response_request_id = Uuid::new_v4();
    match close_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

async fn cancel_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let response_request_id = Uuid::new_v4();
    match cancel_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

async fn cancel_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let Some(session) =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &slug, &session_id)
            .await?
    else {
        return Ok(Json(serde_json::json!({ "ok": true, "cancelled": false })).into_response());
    };
    tracing::info!(
        bear_id = %session.bear_id,
        acp_session_id = %session.acp_session_id,
        conversation_id = %session.conversation_id,
        "ACP cancel requested; pair role uses API-direct Letta streaming"
    );
    Ok(Json(serde_json::json!({
        "ok": true,
        "cancelled": false,
        "message": "ACP is API-direct for the pair role; this endpoint marked no active runtime stream as cancelled."
    }))
    .into_response())
}

async fn close_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;

    let Some(session) =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &slug, &session_id)
            .await?
    else {
        return Ok(Json(AcpCloseSessionResponse {
            ok: true,
            archived: false,
            conversation_id: None,
        })
        .into_response());
    };

    tracing::info!(
        bear_id = %session.bear_id,
        acp_session_id = %session.acp_session_id,
        conversation_id = %session.conversation_id,
        "ACP close requested; marking API-direct pair session closed"
    );
    acp_sessions::mark_closed(&state.sqlx_pool, session.id).await?;
    let archive_target = session
        .resolved_conversation_id
        .as_deref()
        .unwrap_or(&session.conversation_id);
    let mut archived = false;
    if archive_target.starts_with("conv-") && state.letta.is_enabled() {
        state
            .letta
            .patch_conversation_archived(archive_target, true)
            .await?;
        archived_conversations::set_archived(
            &state.sqlx_pool,
            session.bear_id,
            archive_target,
            Some(user_id),
            "acp",
            true,
        )
        .await?;
        acp_sessions::mark_archived(&state.sqlx_pool, session.id).await?;
        archived = true;
    }

    Ok(Json(AcpCloseSessionResponse {
        ok: true,
        archived,
        conversation_id: Some(archive_target.to_string()),
    })
    .into_response())
}

async fn prompt_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
    body: AcpPromptRequest,
    request_id: Uuid,
) -> Result<Result<Response, CustomError>, ApiError> {
    let slug = slug.trim().to_string();
    let user_id = authenticate_acp_code_token(&state, &headers, &slug)
        .await
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    let prompt = body.message.trim();
    if prompt.is_empty() {
        return Ok(Err(CustomError::ValidationError(
            "message must not be empty".to_string(),
        )));
    }

    let slug = slug.trim();
    if slug.is_empty() {
        return Ok(Err(CustomError::NotFound("bear not found".to_string())));
    }

    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug)
        .await
        .map_err(|err| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "database",
                err.to_string(),
            )
        })?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "bear not found or you do not have access",
            )
        })?;

    let pair_agent_id = match require_pair_agent_id(&state, &bear).await {
        Ok(agent_id) => agent_id,
        Err(err) => return Ok(Err(err)),
    };

    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Ok(Err(CustomError::ValidationError(
            "session_id must not be empty".to_string(),
        )));
    }

    let client = normalize_acp_client(body.client.as_deref());
    let cwd = require_absolute_cwd(body.client_context.get("cwd").and_then(|v| v.as_str()))
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    let existing_session =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await
            .map_err(|err| {
                ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "database",
                    err.to_string(),
                )
            })?;
    let requested_conversation_id = body
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| normalize_acp_conversation_id(Some(s)))
        .transpose()
        .map_err(|err| {
            let (_, _, message) = acp_error_status_message(&err);
            ApiError::new(StatusCode::BAD_REQUEST, "validation", message)
        })?
        // Older adapters hardcoded `conversation_id: "default"` for every prompt.
        // Treat that as omission so ACP session/new still creates a pending thread and
        // later prompts can use the stored/resolved conversation for this ACP session.
        .filter(|id| id != "default");
    let generated_conversation_id = new_acp_conversation_id(&client);
    let (conversation_id, conversation_selection_source) =
        if let Some(id) = requested_conversation_id.clone() {
            (id, "explicit")
        } else if let Some(id) = existing_session
            .as_ref()
            .and_then(|s| s.resolved_conversation_id.as_deref())
            .map(str::to_string)
        {
            (id, "resolved")
        } else if let Some(id) = existing_session
            .as_ref()
            .map(|s| s.conversation_id.trim())
            .filter(|s| !s.is_empty())
            .filter(|s| s.starts_with("conv-") || is_valid_pending_acp_conversation_id(s))
            .map(str::to_string)
        {
            (id, "stored")
        } else {
            (generated_conversation_id.clone(), "generated")
        };
    if conversation_id.starts_with("conv-") && conversation_selection_source == "explicit" {
        verify_acp_conversation_belongs_to_bear(&state, &pair_agent_id, &conversation_id)
            .await
            .map_err(|err| {
                let (status, code, message) = acp_error_status_message(&err);
                ApiError::new(status, code, message)
            })?;
    }
    let resolved_conversation_id = if conversation_id.starts_with("conv-") {
        Some(conversation_id.clone())
    } else if existing_session
        .as_ref()
        .is_some_and(|s| s.conversation_id == conversation_id)
    {
        existing_session
            .as_ref()
            .and_then(|s| s.resolved_conversation_id.clone())
    } else if conversation_id == "default" || conversation_id.starts_with("new-") {
        // API-direct Letta streaming does not currently emit a separate `conversation_resolved`
        // event. Keep the session usable by treating the selected conversation as resolved.
        Some(conversation_id.clone())
    } else {
        None
    };
    let runtime_session_id = format!("acp-api-direct:{client}:{}:{session_id}", bear.id);
    acp_sessions::upsert_session(
        &state.sqlx_pool,
        UpsertAcpSession {
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            runtime_session_id: runtime_session_id.clone(),
            conversation_id: conversation_id.clone(),
            resolved_conversation_id: resolved_conversation_id.clone(),
            client: client.clone(),
            cwd: Some(cwd.clone()),
        },
    )
    .await
    .map_err(|err| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "database",
            err.to_string(),
        )
    })?;
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        bear_slug = %bear.slug,
        bear_id = %bear.id,
        role = "pair",
        letta_agent_id = %pair_agent_id,
        client = %client,
        cwd = %cwd,
        requested_conversation_id = requested_conversation_id.as_deref(),
        conversation_id = %conversation_id,
        conversation_selection_source = %conversation_selection_source,
        resolved_conversation_id = resolved_conversation_id.as_deref(),
        "ACP gateway routing prompt to pair role via Letta API"
    );
    let upstream = match state
        .letta
        .post_conversation_messages_streaming(&conversation_id, Some(&pair_agent_id), prompt)
        .await
    {
        Ok(upstream) => upstream,
        Err(err) => return Ok(Err(err)),
    };

    let stream = AcpLettaSseStream::new(
        upstream.bytes_stream(),
        AcpStreamContext {
            pool: state.sqlx_pool.clone(),
            user_id,
            bear_id: bear.id,
            acp_session_id: session_id.to_string(),
            request_id,
        },
        Vec::new(),
    );
    let request_id_header = HeaderValue::from_str(&request_id.to_string()).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid_request_id",
            "invalid request id for response header",
        )
    })?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from_stream(stream))
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "response_build",
                format!("response build: {e}"),
            )
        })
        .map(Ok)
}

#[derive(Clone)]
struct AcpStreamContext {
    pool: sqlx::PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: String,
    request_id: Uuid,
}

async fn persist_stream_event_side_effects(
    context: &AcpStreamContext,
    event: &serde_json::Value,
) -> Result<(), CustomError> {
    if event.get("type").and_then(|v| v.as_str()) == Some("conversation_resolved") {
        if let Some(conversation_id) = event.get("conversation_id").and_then(|v| v.as_str()) {
            acp_sessions::mark_resolved(
                &context.pool,
                context.user_id,
                context.bear_id,
                &context.acp_session_id,
                conversation_id,
            )
            .await?;
        }
    }
    Ok(())
}

fn map_letta_stream_event_to_acp_adapter_event(event: &serde_json::Value) -> Option<Bytes> {
    let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let mapped = match ty {
        "assistant_delta" => serde_json::json!({
            "type": "agent_message_chunk",
            "content": { "type": "text", "text": event.get("text").and_then(|v| v.as_str()).unwrap_or("") },
        }),
        "reasoning_delta" => serde_json::json!({
            "type": "status",
            "content": { "type": "text", "text": event.get("text").and_then(|v| v.as_str()).unwrap_or("") },
        }),
        "error" => {
            let mut mapped = serde_json::json!({
                "type": "error",
                "message": event.get("message").and_then(|v| v.as_str()).unwrap_or("Upstream error"),
                "detail": event.get("detail").and_then(|v| v.as_str()),
            });
            if let Some(context) = event.get("context") {
                mapped["context"] = context.clone();
            }
            if let Some(request_id) = event.get("request_id") {
                mapped["request_id"] = request_id.clone();
            }
            mapped
        }
        "done" => serde_json::json!({
            "type": "done",
            "outcome": event.get("outcome").and_then(|v| v.as_str()).unwrap_or("ok"),
        }),
        "conversation_resolved" => serde_json::json!({
            "type": "conversation_resolved",
            "conversation_id": event.get("conversation_id").and_then(|v| v.as_str()),
        }),

        _ => return None,
    };
    Some(Bytes::from(format!("data: {}\n\n", mapped)))
}

/// Byte offset **after** the first complete SSE frame delimiter (`\n\n` or `\r\n\r\n`).
fn find_sse_frame_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn strip_trailing_sse_delimiter_owned(mut frame: Vec<u8>) -> Vec<u8> {
    if frame.ends_with(b"\r\n\r\n") {
        frame.truncate(frame.len().saturating_sub(4));
    } else if frame.ends_with(b"\n\n") {
        frame.truncate(frame.len().saturating_sub(2));
    }
    frame
}

#[cfg(test)]
fn strip_trailing_sse_delimiter(frame: &[u8]) -> &[u8] {
    if frame.ends_with(b"\r\n\r\n") {
        &frame[..frame.len().saturating_sub(4)]
    } else if frame.ends_with(b"\n\n") {
        &frame[..frame.len().saturating_sub(2)]
    } else {
        frame
    }
}

const SSE_JSON_PREVIEW_MAX: usize = 192;

fn preview_bytes_utf8_lossy(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    preview_str_truncated(&s, SSE_JSON_PREVIEW_MAX)
}

fn preview_str_truncated(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// One SSE *event* (between delimiters): join all `data:` field lines with `\n`, then parse JSON once.
fn parse_sse_event_body_to_json(body: &[u8]) -> Result<Option<serde_json::Value>, String> {
    let text = std::str::from_utf8(body).map_err(|_| {
        format!(
            "invalid UTF-8 in SSE event body (preview: {})",
            preview_bytes_utf8_lossy(body)
        )
    })?;
    let mut chunks: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        chunks.push(rest);
    }
    let joined = chunks.join("\n");
    let joined = joined.trim();
    if joined.is_empty() || joined == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str::<serde_json::Value>(joined)
        .map(Some)
        .map_err(|e| {
            format!(
                "{e} (preview: {})",
                preview_str_truncated(joined, SSE_JSON_PREVIEW_MAX)
            )
        })
}

#[cfg(test)]
fn map_letta_stream_frame_to_acp_adapter_events(frame: &[u8]) -> Vec<Bytes> {
    let body = strip_trailing_sse_delimiter(frame);
    match parse_sse_event_body_to_json(body) {
        Ok(Some(value)) => map_letta_stream_event_to_acp_adapter_event(&value)
            .into_iter()
            .collect(),
        Ok(None) | Err(_) => Vec::new(),
    }
}

async fn map_letta_stream_frame_to_acp_adapter_events_with_persistence(
    frame: Vec<u8>,
    context: AcpStreamContext,
) -> Result<Vec<Bytes>, std::io::Error> {
    let body = strip_trailing_sse_delimiter_owned(frame);
    let value = match parse_sse_event_body_to_json(&body) {
        Err(msg) => {
            tracing::warn!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                error = %msg,
                "ACP upstream Letta SSE event JSON parse failed"
            );
            return Ok(Vec::new());
        }
        Ok(None) => return Ok(Vec::new()),
        Ok(Some(v)) => v,
    };

    persist_stream_event_side_effects(&context, &value)
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(map_letta_stream_event_to_acp_adapter_event(&value)
        .into_iter()
        .collect())
}

struct AcpLettaSseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    /// Complete upstream SSE event bodies (delimiter stripped), FIFO.
    pending_raw_frames: std::collections::VecDeque<Vec<u8>>,
    pending: std::collections::VecDeque<Bytes>,
    context: AcpStreamContext,
    persist_future:
        Option<Pin<Box<dyn Future<Output = Result<Vec<Bytes>, std::io::Error>> + Send>>>,
}

impl AcpLettaSseStream {
    fn new(
        inner: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        context: AcpStreamContext,
        initial_pending: Vec<Bytes>,
    ) -> Self {
        Self {
            inner: Box::pin(inner),
            buffer: Vec::new(),
            pending_raw_frames: std::collections::VecDeque::new(),
            pending: initial_pending.into(),
            context,
            persist_future: None,
        }
    }
}

impl Stream for AcpLettaSseStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        if let Some(bytes) = this.pending.pop_front() {
            return Poll::Ready(Some(Ok(bytes)));
        }

        if let Some(fut) = this.persist_future.as_mut() {
            match ready!(fut.as_mut().poll(cx)) {
                Ok(bytes) => {
                    this.persist_future = None;
                    for item in bytes {
                        this.pending.push_back(item);
                    }
                    return self.poll_next(cx);
                }
                Err(err) => {
                    this.persist_future = None;
                    return Poll::Ready(Some(Err(err)));
                }
            }
        }

        if let Some(frame_body) = this.pending_raw_frames.pop_front() {
            let context = this.context.clone();
            this.persist_future = Some(Box::pin(async move {
                map_letta_stream_frame_to_acp_adapter_events_with_persistence(frame_body, context)
                    .await
            }));
            return self.poll_next(cx);
        }

        match ready!(this.inner.as_mut().poll_next(cx)) {
            Some(Ok(chunk)) => {
                this.buffer.extend_from_slice(&chunk);
                while let Some(end) = find_sse_frame_end(&this.buffer) {
                    let raw: Vec<u8> = this.buffer.drain(..end).collect();
                    let frame_body = strip_trailing_sse_delimiter_owned(raw);
                    this.pending_raw_frames.push_back(frame_body);
                }
                self.poll_next(cx)
            }
            Some(Err(err)) => {
                let message = format!("Letta stream read failed: {err}");
                tracing::warn!(
                    request_id = %this.context.request_id,
                    acp_session_id = %this.context.acp_session_id,
                    error = %err,
                    "ACP upstream Letta SSE stream read error"
                );
                let event = serde_json::json!({
                    "type": "error",
                    "message": "Letta stream ended unexpectedly while BEARS was waiting for events.",
                    "detail": message,
                    "request_id": this.context.request_id.to_string(),
                    "diagnostic": {
                        "code": "letta_stream_read_error",
                        "component": "den.acp"
                    }
                });
                Poll::Ready(Some(Ok(Bytes::from(format!("data: {}\n\n", event)))))
            }
            None => {
                if !this.buffer.is_empty() {
                    let preview = preview_bytes_utf8_lossy(&this.buffer);
                    tracing::warn!(
                        request_id = %this.context.request_id,
                        acp_session_id = %this.context.acp_session_id,
                        incomplete_bytes = this.buffer.len(),
                        preview = %preview,
                        "ACP upstream Letta SSE stream ended with incomplete frame"
                    );
                    this.buffer.clear();
                }
                if !this.pending_raw_frames.is_empty() {
                    self.poll_next(cx)
                } else {
                    Poll::Ready(None)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_assistant_delta_to_acp_adapter_event() {
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"type\":\"assistant_delta\",\"text\":\"hello\"}\n\n",
        );
        let text = String::from_utf8(out[0].to_vec()).unwrap();
        assert!(text.contains("\"type\":\"agent_message_chunk\""));
        assert!(text.contains("\"text\":\"hello\""));
    }

    #[test]
    fn maps_done_to_acp_adapter_event() {
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"type\":\"done\",\"outcome\":\"ok\"}\n\n",
        );
        let text = String::from_utf8(out[0].to_vec()).unwrap();
        assert!(text.contains("\"type\":\"done\""));
    }

    #[test]
    fn sse_parser_joins_multiple_data_lines_into_one_json_value() {
        let body = br#"data: {"type":"assistant_delta","text":
data: "hello"}"#;
        let v = parse_sse_event_body_to_json(body).unwrap().unwrap();
        assert_eq!(v["type"], "assistant_delta");
        assert_eq!(v["text"], "hello");
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"type\":\"assistant_delta\",\"text\":\ndata: \"hello\"}\n\n",
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn sse_parser_rejects_invalid_json_with_parse_path_empty() {
        let body = br#"data: not-json"#;
        assert!(parse_sse_event_body_to_json(body).is_err());
        let out = map_letta_stream_frame_to_acp_adapter_events(b"data: not-json\n\n");
        assert!(out.is_empty());
    }

    #[test]
    fn sse_frame_end_prefers_earliest_lf_or_crlf_delimiter() {
        let buf = b"data: {}\r\n\r\n";
        assert_eq!(find_sse_frame_end(buf), Some(12));
        let buf2 = b"data: {}\n\n";
        assert_eq!(find_sse_frame_end(buf2), Some(10));
    }

    #[test]
    fn normalizes_acp_conversation_ids() {
        assert_eq!(normalize_acp_conversation_id(None).unwrap(), "default");
        assert_eq!(
            normalize_acp_conversation_id(Some("conv-abc12345")).unwrap(),
            "conv-abc12345"
        );
        assert_eq!(
            normalize_acp_conversation_id(Some("new-acp-zed-abc12345")).unwrap(),
            "new-acp-zed-abc12345"
        );
        assert!(normalize_acp_conversation_id(Some("conv-x")).is_err());
        assert!(normalize_acp_conversation_id(Some("../../etc/passwd")).is_err());
    }

    #[test]
    fn generated_acp_conversation_ids_are_compact_opaque_ids() {
        let id = new_acp_conversation_id("zed");
        assert!(id.starts_with("new-acp-zed-"));
        assert_eq!(id.len(), 34);
        assert!(is_valid_pending_acp_conversation_id(&id));

        let id = new_acp_conversation_id("acp_adapter");
        assert!(id.starts_with("new-acp-acp_adapter-"));
        assert_eq!(id.len(), 42);
        assert!(is_valid_pending_acp_conversation_id(&id));
    }

    #[test]
    fn rejects_legacy_pending_acp_conversation_ids_that_exceed_letta_limit() {
        let legacy = "new-acp-zed-acp-12345678-1234-1234-1234-123456789abc";
        assert!(normalize_acp_conversation_id(Some(legacy)).is_ok());
        assert!(!is_valid_pending_acp_conversation_id(legacy));
    }
}
