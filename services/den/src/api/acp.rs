//! Minimal Agent Client Protocol (ACP) gateway for adapter clients.
//!
//! This is the Phase 7 basic-chat slice: Den authenticates, authorizes the selected bear,
//! injects trusted context, and maps text prompts to Codepool `bear_channel`. Client-tool
//! relay and full ACP stdio transport live in later slices / an external adapter.

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
        bears::db as bears_db,
        codepool::CodepoolToolResultRequest,
        letta::load_agent_conversations,
        user,
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
            "/bears/{slug}/sessions/{session_id}/tool-results/{call_id}",
            post(tool_result),
        )
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

#[derive(Debug, Deserialize)]
pub struct AcpToolResultRequest {
    pub request_id: Uuid,
    #[serde(default = "default_conversation_id")]
    pub conversation_id: String,
    pub status: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub client_observation: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct AcpToolResultResponse {
    ok: bool,
    delivered: bool,
    call_id: String,
    reason: Option<String>,
    runtime_id: Option<String>,
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
    codepool_session_id: String,
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
        codepool_session_id: row.codepool_session_id,
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

fn default_conversation_id() -> String {
    "default".to_string()
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

fn client_supports_write_text_file(client_capabilities: &serde_json::Value) -> bool {
    client_capabilities
        .pointer("/fs/writeTextFile")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || client_capabilities
            .pointer("/fs/write_text_file")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || client_capabilities
            .pointer("/filesystem/writeTextFile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || client_capabilities
            .pointer("/filesystem/write_text_file")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || client_capabilities
            .pointer("/fs/write_text_file/supported")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || client_capabilities
            .pointer("/filesystem/write_text_file/supported")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

fn client_supports_read_text_file(client_capabilities: &serde_json::Value) -> bool {
    client_capabilities
        .pointer("/fs/readTextFile")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || client_capabilities
            .pointer("/fs/read_text_file")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || client_capabilities
            .pointer("/filesystem/readTextFile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || client_capabilities
            .pointer("/filesystem/read_text_file")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || client_capabilities
            .pointer("/fs/read_text_file/supported")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || client_capabilities
            .pointer("/filesystem/read_text_file/supported")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

fn acp_conversation_id(client: &str, session_id: &str) -> String {
    format!("new-acp-{client}-{}", stable_session_suffix(session_id))
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

fn stable_session_suffix(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(80)
        .collect()
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

fn authorized_client_tool_descriptors(
    has_acp_tools_scope: bool,
    client: &str,
    client_capabilities: &serde_json::Value,
    client_context: &serde_json::Value,
) -> Vec<serde_json::Value> {
    if !has_acp_tools_scope {
        return Vec::new();
    }
    let mut descriptors = Vec::new();
    if client_supports_read_text_file(client_capabilities) {
        descriptors.push(serde_json::json!({
        "id": "acp_fs_read_text_file",
        "name": "acp_fs_read_text_file",
        "title": "Read text file from editor workspace",
        "description": "Read a UTF-8 text file from the user's active editor workspace through the ACP client. Use this for inspecting project files that are available to the local editor session.",
        "provider": "acp_client",
        "execution_target": "acp_client",
        "scope": "client_connection",
        "client": client,
        "permissions": ["filesystem", "read"],
        "approval_policy": "never",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative or client-accepted path to read."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        },
        "output_schema": {
            "type": "object",
            "properties": {
                "content": { "type": "string" }
            },
            "required": ["content"],
            "additionalProperties": true
        },
        "acp": {
            "method": "fs/read_text_file",
            "requires_client_capability": "fs.readTextFile"
        },
        "client_context": client_context,
    }));
    }
    if acp_write_tools_enabled() && client_supports_write_text_file(client_capabilities) {
        descriptors.push(serde_json::json!({
            "id": "acp_fs_write_text_file",
            "name": "acp_fs_write_text_file",
            "title": "Write text file in editor workspace",
            "description": "Write UTF-8 text content to a file in the user's active editor workspace through the ACP client. Use this only when you need to create or replace file contents.",
            "provider": "acp_client",
            "execution_target": "acp_client",
            "scope": "client_connection",
            "client": client,
            "permissions": ["filesystem", "write"],
            "approval_policy": "always",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or absolute path to write."
                    },
                    "content": {
                        "type": "string",
                        "description": "Complete UTF-8 text content to write."
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            },
            "output_schema": {
                "type": "object",
                "properties": {
                    "_meta": {
                        "type": ["object", "null"],
                        "additionalProperties": true
                    }
                },
                "additionalProperties": false
            },
            "acp": {
                "method": "fs/write_text_file",
                "requires_client_capability": "fs.writeTextFile"
            },
            "client_context": client_context,
        }));
    }
    descriptors
}

fn acp_write_tools_enabled() -> bool {
    std::env::var("ACP_WRITE_TOOLS_ENABLED")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
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
    match authenticate_acp_code_token(&state, &headers, &slug, true).await {
        Ok((user_id, has_tools)) => Json(serde_json::json!({
            "ok": true,
            "user_id": user_id,
            "scopes": {
                "acp:chat": true,
                "acp:tools": has_tools
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
    require_tools: bool,
) -> Result<(i32, bool), CustomError> {
    let token = auth::extract_bearer_token(headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    if !acp_tokens::is_acp_token(&token) {
        return Err(CustomError::Authentication(
            "expected a bear-scoped BEARS ACP Code token".to_string(),
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
    let has_tools = acp_tokens::scopes_contains(&auth.scopes, OAuthScope::AcpTools.as_str());
    if require_tools && !has_tools {
        return Err(CustomError::Authorization(
            "ACP token is missing required acp:tools scope; generate a new Den Code token"
                .to_string(),
        ));
    }
    Ok((auth.user_id, has_tools))
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
    let (user_id, _) = authenticate_acp_code_token(&state, &headers, &slug, false).await?;
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
    let (user_id, _) = authenticate_acp_code_token(&state, &headers, &slug, false).await?;
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
    let (user_id, _) = authenticate_acp_code_token(&state, &headers, &slug, false).await?;
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
    let Some(agent_id) = bear
        .letta_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(default_only());
    };

    let archived_ids = archived_conversations::list_for_bear(&state.sqlx_pool, bear.id).await?;
    let snap = load_agent_conversations(state.letta.as_ref(), agent_id).await;
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
    let (user_id, _) = authenticate_acp_code_token(&state, &headers, &slug, false).await?;
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
    let agent_id = bear
        .letta_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CustomError::ValidationError("this bear is not linked to a Letta agent".to_string())
        })?;
    let conv_id = normalize_acp_conversation_id(Some(&conversation_id))?;
    if conv_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "history is only available for default or saved conv- conversations".to_string(),
        ));
    }
    if conv_id.starts_with("conv-") {
        verify_acp_conversation_belongs_to_bear(&state, agent_id, &conv_id).await?;
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let before = query
        .before
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let agent_for_conv = if conv_id == "default" {
        Some(agent_id)
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
    let (user_id, _) = authenticate_acp_code_token(&state, &headers, &slug, false).await?;
    let Some(session) =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &slug, &session_id)
            .await?
    else {
        return Ok(Json(serde_json::json!({ "ok": true, "cancelled": false })).into_response());
    };
    let cancelled = state
        .codepool
        .post_bear_channel_cancel(&session.codepool_session_id, Uuid::new_v4())
        .await
        .map(|response| response.cancelled)
        .unwrap_or(false);
    Ok(Json(serde_json::json!({
        "ok": true,
        "cancelled": cancelled,
    }))
    .into_response())
}

async fn close_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let (user_id, _) = authenticate_acp_code_token(&state, &headers, &slug, false).await?;

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

    let _ = state
        .codepool
        .post_bear_channel_cancel(&session.codepool_session_id, Uuid::new_v4())
        .await;
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

async fn tool_result(
    State(state): State<ApiState>,
    Path((slug, session_id, call_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpToolResultRequest>,
) -> Response {
    let response_request_id = Uuid::new_v4();
    match tool_result_inner(state, slug, session_id, call_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

async fn tool_result_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    call_id: String,
    headers: HeaderMap,
    body: AcpToolResultRequest,
) -> Result<Response, CustomError> {
    let (user_id, _) = authenticate_acp_code_token(&state, &headers, &slug, true).await?;
    let codepool_status = normalize_codepool_tool_result_status(&body.status)?;
    let session =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &slug, &session_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?;

    let codepool_payload = CodepoolToolResultRequest {
        conversation_id: body.conversation_id.trim().to_string(),
        request_id: body.request_id.to_string(),
        call_id: call_id.clone(),
        tool_name: body.tool_name.unwrap_or_default(),
        status: codepool_status.to_string(),
        result: body.result,
        error: body.error,
    };
    let (delivered, delivery_reason, runtime_id) = match state
        .codepool
        .post_bear_channel_tool_result(
            &session.codepool_session_id,
            &codepool_payload,
            body.request_id,
        )
        .await
    {
        Ok(response) => (response.delivered, response.reason, response.runtime_id),
        Err(err) => {
            tracing::warn!(
                request_id = %body.request_id,
                acp_session_id = %session_id,
                codepool_session_id = %session.codepool_session_id,
                call_id = %call_id,
                tool_name = %codepool_payload.tool_name,
                error = %err,
                "ACP tool result accepted by Den but could not be delivered to Codepool"
            );
            (false, Some("codepool_forward_error".to_string()), None)
        }
    };
    Ok(Json(AcpToolResultResponse {
        ok: true,
        delivered,
        call_id,
        reason: delivery_reason,
        runtime_id,
    })
    .into_response())
}

fn normalize_codepool_tool_result_status(status: &str) -> Result<&'static str, CustomError> {
    match status.trim() {
        "ok" => Ok("ok"),
        "error" => Ok("error"),
        "cancelled" => Ok("cancelled"),
        "timeout" => Ok("timeout"),
        other => Err(CustomError::ValidationError(format!(
            "unsupported tool result status: {other}"
        ))),
    }
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
    let (user_id, has_acp_tools_scope) =
        authenticate_acp_code_token(&state, &headers, &slug, false)
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

    if bear
        .letta_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return Ok(Err(CustomError::System(
            "This bear is not provisioned in Letta yet (missing letta_agent_id).".to_string(),
        )));
    }

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
    let generated_conversation_id = acp_conversation_id(&client, session_id);
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
            .map(str::to_string)
        {
            (id, "stored")
        } else {
            (generated_conversation_id.clone(), "generated")
        };
    if conversation_id.starts_with("conv-") && conversation_selection_source == "explicit" {
        verify_acp_conversation_belongs_to_bear(
            &state,
            bear.letta_agent_id.as_deref().unwrap_or(""),
            &conversation_id,
        )
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
    } else {
        None
    };
    let client_tools = authorized_client_tool_descriptors(
        has_acp_tools_scope,
        &client,
        &body.client_capabilities,
        &body.client_context,
    );
    let missing_tools_scope = !has_acp_tools_scope
        && (client_supports_read_text_file(&body.client_capabilities)
            || client_supports_write_text_file(&body.client_capabilities));
    if missing_tools_scope {
        tracing::info!(
            %request_id,
            acp_session_id = %session_id,
            bear_slug = %slug,
            client = %client,
            "acp_tools_scope_missing; omitting ACP client tool descriptors"
        );
    }
    let username = user::user_by_id(&state.sqlx_pool, user_id)
        .await
        .ok()
        .map(|u| u.username);
    let membership_role = bears_db::membership_role_for_user(&state.sqlx_pool, user_id, bear.id)
        .await
        .map_err(|err| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "database",
                err.to_string(),
            )
        })?
        .flatten();
    let runtime_plan =
        crate::core::bears::effective_runtime_plan(bear.runtime_plan.as_ref().map(|j| j.as_ref()));

    let channel_session_id = format!("acp:{client}:{}:{session_id}", bear.id);
    acp_sessions::upsert_session(
        &state.sqlx_pool,
        UpsertAcpSession {
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            codepool_session_id: channel_session_id.clone(),
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
    let letta_agent_id = bear.letta_agent_id.as_deref().unwrap_or("unknown");
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        bear_slug = %bear.slug,
        bear_id = %bear.id,
        letta_agent_id = %letta_agent_id,
        client = %client,
        cwd = %cwd,
        codepool_session_id = %channel_session_id,
        requested_conversation_id = requested_conversation_id.as_deref(),
        codepool_conversation_id = %conversation_id,
        conversation_selection_source = %conversation_selection_source,
        resolved_conversation_id = resolved_conversation_id.as_deref(),
        "ACP gateway routing prompt to Codepool"
    );
    let upstream = match state
        .codepool
        .post_bear_channel_message_for_channel_with_client_tools_streaming(
            &channel_session_id,
            &conversation_id,
            &bear,
            user_id,
            username.as_deref(),
            membership_role.as_deref(),
            prompt,
            &runtime_plan,
            request_id,
            "coding_workspace",
            &client,
            "agent_client_protocol",
            false,
            true,
            client_tools,
        )
        .await
    {
        Ok(upstream) => upstream,
        Err(err) => return Ok(Err(err)),
    };

    let stream = AcpBearChannelSseStream::new(
        upstream.bytes_stream(),
        AcpStreamContext {
            pool: state.sqlx_pool.clone(),
            user_id,
            bear_id: bear.id,
            acp_session_id: session_id.to_string(),
            request_id,
        },
        initial_adapter_events(missing_tools_scope),
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

fn map_bear_channel_event_to_acp_adapter_event(event: &serde_json::Value) -> Option<Bytes> {
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
        "client_tool_request" => {
            let call = event
                .get("call")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            serde_json::json!({
                "type": "client_tool_request",
                "request_id": event.get("request_id").cloned(),
                "session_id": event.get("session_id").cloned(),
                "conversation_id": event.get("conversation_id").cloned(),
                "call_id": call.get("id").cloned(),
                "tool_name": call.get("name").cloned(),
                "arguments": call.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({})),
                "descriptor": call.get("descriptor").cloned(),
                "approval_policy": call.get("approval_policy").cloned(),
                "timeout_ms": call.get("timeout_ms").cloned().unwrap_or_else(|| serde_json::json!(30000)),
            })
        }
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

/// Preview for log lines (prefix + `...`).
fn preview_id(s: &str) -> String {
    const PREFIX: usize = 12;
    let mut it = s.chars();
    let prefix: String = it.by_ref().take(PREFIX).collect();
    if it.next().is_some() {
        format!("{prefix}...")
    } else {
        s.to_string()
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
fn map_bear_channel_frame_to_acp_adapter_events(frame: &[u8]) -> Vec<Bytes> {
    let body = strip_trailing_sse_delimiter(frame);
    match parse_sse_event_body_to_json(body) {
        Ok(Some(value)) => map_bear_channel_event_to_acp_adapter_event(&value)
            .into_iter()
            .collect(),
        Ok(None) | Err(_) => Vec::new(),
    }
}

async fn map_bear_channel_frame_to_acp_adapter_events_with_persistence(
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
                "ACP upstream Codepool SSE event JSON parse failed"
            );
            return Ok(Vec::new());
        }
        Ok(None) => return Ok(Vec::new()),
        Ok(Some(v)) => v,
    };

    if value.get("type").and_then(|v| v.as_str()) == Some("client_tool_request") {
        let context_rid = context.request_id.to_string();
        let req_preview = preview_id(
            value
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or(&context_rid),
        );
        let call_preview = value
            .get("call")
            .and_then(|c| c.get("id"))
            .and_then(|v| v.as_str())
            .map(preview_id)
            .unwrap_or_else(|| "?".to_string());
        tracing::info!(
            request_id = %context.request_id,
            event_type = "client_tool_request",
            event_request_id = %req_preview,
            call_id = %call_preview,
            "ACP upstream Codepool event received"
        );
    }

    persist_stream_event_side_effects(&context, &value)
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let mut out = Vec::new();
    if let Some(bytes) = map_bear_channel_event_to_acp_adapter_event(&value) {
        if value.get("type").and_then(|v| v.as_str()) == Some("client_tool_request") {
            let call = value.get("call");
            let call_id = call
                .and_then(|c| c.get("id"))
                .and_then(|v| v.as_str())
                .map(preview_id)
                .unwrap_or_else(|| "?".to_string());
            let tool_name = call
                .and_then(|c| c.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                call_id = %call_id,
                tool_name = %tool_name,
                "ACP adapter-facing client tool request emitted"
            );
        }
        out.push(bytes);
    }
    Ok(out)
}

fn initial_adapter_events(missing_tools_scope: bool) -> Vec<Bytes> {
    if !missing_tools_scope {
        return Vec::new();
    }
    vec![Bytes::from(format!(
        "data: {}\n\n",
        serde_json::json!({
            "type": "status",
            "content": {
                "type": "text",
                "text": "ACP local editor tools are unavailable because this token lacks the acp:tools scope. Chat will continue, but the bear cannot request local file reads or writes from this editor session. Generate a new Den Code token to enable local tools."
            },
            "diagnostic": {
                "code": "acp_tools_scope_missing",
                "required_scope": "acp:tools"
            }
        })
    ))]
}

struct AcpBearChannelSseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    /// Complete upstream SSE event bodies (delimiter stripped), FIFO.
    pending_raw_frames: std::collections::VecDeque<Vec<u8>>,
    pending: std::collections::VecDeque<Bytes>,
    context: AcpStreamContext,
    persist_future:
        Option<Pin<Box<dyn Future<Output = Result<Vec<Bytes>, std::io::Error>> + Send>>>,
}

impl AcpBearChannelSseStream {
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

impl Stream for AcpBearChannelSseStream {
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
                map_bear_channel_frame_to_acp_adapter_events_with_persistence(frame_body, context)
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
                let message = format!("Codepool stream read failed: {err}");
                tracing::warn!(
                    request_id = %this.context.request_id,
                    acp_session_id = %this.context.acp_session_id,
                    error = %err,
                    "ACP upstream Codepool SSE stream read error"
                );
                let event = serde_json::json!({
                    "type": "error",
                    "message": "Codepool stream ended unexpectedly while BEARS was waiting for events.",
                    "detail": message,
                    "request_id": this.context.request_id.to_string(),
                    "diagnostic": {
                        "code": "codepool_stream_read_error",
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
                        "ACP upstream Codepool SSE stream ended with incomplete frame"
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
        let out = map_bear_channel_frame_to_acp_adapter_events(
            b"data: {\"type\":\"assistant_delta\",\"text\":\"hello\"}\n\n",
        );
        let text = String::from_utf8(out[0].to_vec()).unwrap();
        assert!(text.contains("\"type\":\"agent_message_chunk\""));
        assert!(text.contains("\"text\":\"hello\""));
    }

    #[test]
    fn maps_done_to_acp_adapter_event() {
        let out = map_bear_channel_frame_to_acp_adapter_events(
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
        let out = map_bear_channel_frame_to_acp_adapter_events(
            b"data: {\"type\":\"assistant_delta\",\"text\":\ndata: \"hello\"}\n\n",
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn sse_parser_rejects_invalid_json_with_parse_path_empty() {
        let body = br#"data: not-json"#;
        assert!(parse_sse_event_body_to_json(body).is_err());
        let out = map_bear_channel_frame_to_acp_adapter_events(b"data: not-json\n\n");
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
    fn builds_read_text_file_descriptor_only_when_authorized_and_supported() {
        let caps = serde_json::json!({ "fs": { "readTextFile": true } });
        let context = serde_json::json!({ "cwd": "/tmp/workspace" });
        let descriptors = authorized_client_tool_descriptors(true, "zed", &caps, &context);
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0]["name"], "acp_fs_read_text_file");
        assert_eq!(descriptors[0]["acp"]["method"], "fs/read_text_file");
        assert_eq!(descriptors[0]["approval_policy"], "never");

        assert!(authorized_client_tool_descriptors(false, "zed", &caps, &context).is_empty());
        assert!(authorized_client_tool_descriptors(
            true,
            "zed",
            &serde_json::json!({ "fs": { "readTextFile": false } }),
            &context
        )
        .is_empty());
        assert_eq!(
            authorized_client_tool_descriptors(
                true,
                "zed",
                &serde_json::json!({ "fs": { "read_text_file": true } }),
                &context
            )
            .len(),
            1
        );
    }

    #[test]
    fn builds_write_text_file_descriptor_only_when_authorized_and_supported() {
        std::env::set_var("ACP_WRITE_TOOLS_ENABLED", "true");
        let caps = serde_json::json!({ "fs": { "writeTextFile": true } });
        let context = serde_json::json!({ "cwd": "/tmp/workspace" });
        let descriptors = authorized_client_tool_descriptors(true, "zed", &caps, &context);
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0]["name"], "acp_fs_write_text_file");
        assert_eq!(descriptors[0]["acp"]["method"], "fs/write_text_file");
        assert_eq!(descriptors[0]["approval_policy"], "always");
        assert_eq!(
            descriptors[0]["permissions"],
            serde_json::json!(["filesystem", "write"])
        );

        assert!(authorized_client_tool_descriptors(false, "zed", &caps, &context).is_empty());
        assert!(authorized_client_tool_descriptors(
            true,
            "zed",
            &serde_json::json!({ "fs": { "writeTextFile": false } }),
            &context
        )
        .is_empty());
        assert_eq!(
            authorized_client_tool_descriptors(
                true,
                "zed",
                &serde_json::json!({ "fs": { "write_text_file": true } }),
                &context
            )
            .len(),
            1
        );
    }

    #[test]
    fn builds_read_and_write_descriptors_together() {
        std::env::set_var("ACP_WRITE_TOOLS_ENABLED", "true");
        let caps = serde_json::json!({ "fs": { "readTextFile": true, "writeTextFile": true } });
        let context = serde_json::json!({ "cwd": "/tmp/workspace" });
        let descriptors = authorized_client_tool_descriptors(true, "zed", &caps, &context);
        let names = descriptors
            .iter()
            .map(|descriptor| descriptor["name"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["acp_fs_read_text_file", "acp_fs_write_text_file"]
        );
    }

    #[test]
    fn initial_adapter_events_mentions_reads_and_writes_for_write_only_capability() {
        let events = initial_adapter_events(true);
        assert_eq!(events.len(), 1);
        let text = String::from_utf8(events[0].to_vec()).unwrap();
        assert!(text.contains("reads or writes"));
        assert!(text.contains("acp_tools_scope_missing"));
    }

    #[test]
    fn normalizes_acp_tool_result_statuses_for_codepool() {
        assert_eq!(normalize_codepool_tool_result_status("ok").unwrap(), "ok");
        assert_eq!(
            normalize_codepool_tool_result_status("error").unwrap(),
            "error"
        );
        assert_eq!(
            normalize_codepool_tool_result_status("cancelled").unwrap(),
            "cancelled"
        );
        assert_eq!(
            normalize_codepool_tool_result_status("timeout").unwrap(),
            "timeout"
        );
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
    fn maps_client_tool_request_to_adapter_event() {
        let out = map_bear_channel_frame_to_acp_adapter_events(
            br#"data: {"type":"client_tool_request","request_id":"req-1","session_id":"s-1","conversation_id":"default","call":{"id":"call-1","name":"acp_fs_read_text_file","arguments":{"path":"README.md"},"timeout_ms":30000}}

"#,
        );
        let value: serde_json::Value = serde_json::from_str(
            String::from_utf8(out[0].to_vec())
                .unwrap()
                .trim_start_matches("data: ")
                .trim(),
        )
        .unwrap();
        assert_eq!(value["type"], "client_tool_request");
        assert_eq!(value["call_id"], "call-1");
        assert_eq!(value["tool_name"], "acp_fs_read_text_file");
        assert_eq!(value["arguments"]["path"], "README.md");
    }
}
