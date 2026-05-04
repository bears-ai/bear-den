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
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    future::Future,
    path::Path as FsPath,
    pin::Pin,
    sync::{Arc, Mutex},
    task::Poll,
};
use time::format_description::well_known::Rfc3339;
use tokio::sync::oneshot;
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

fn acp_debug_event_sample_chars() -> usize {
    std::env::var("ACP_DEBUG_EVENT_SAMPLE_CHARS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|n| n.clamp(128, 20_000))
        .unwrap_or(360)
}

#[derive(Debug, Clone, Copy)]
struct AcpToolDescriptor {
    provider_name: &'static str,
    canonical_name: &'static str,
    adapter_method: &'static str,
    client_method: &'static str,
    title: &'static str,
    kind: &'static str,
    risk: &'static str,
}

const ACP_READ_TEXT_FILE_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_read_text_file",
    canonical_name: "acp.fs.read_text_file",
    adapter_method: "bears/read_text_file",
    client_method: "fs/read_text_file",
    title: "Read file",
    kind: "read",
    risk: "read_only",
};

fn provider_tool_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn acp_read_text_file_client_tool_descriptor() -> serde_json::Value {
    debug_assert!(provider_tool_name_is_safe(
        ACP_READ_TEXT_FILE_TOOL.provider_name
    ));
    serde_json::json!({
        "name": ACP_READ_TEXT_FILE_TOOL.provider_name,
        "description": format!(
            "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Reads a UTF-8 text file from the user's editor workspace through the local adapter. Use only for user workspace files, not server files.",
            ACP_READ_TEXT_FILE_TOOL.canonical_name,
            "acp_client",
            ACP_READ_TEXT_FILE_TOOL.adapter_method,
            ACP_READ_TEXT_FILE_TOOL.client_method,
            ACP_READ_TEXT_FILE_TOOL.kind,
            ACP_READ_TEXT_FILE_TOOL.risk,
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute local file path under the workspace." },
                "line": { "type": "integer", "minimum": 1, "description": "Optional 1-based starting line." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 2000, "description": "Optional maximum number of lines." }
            },
            "required": ["path"]
        }
    })
}

#[derive(Debug)]
pub(crate) struct AcpToolTurn {
    user_id: i32,
    bear_id: Uuid,
    bear_slug: String,
    acp_session_id: String,
    request_id: Uuid,
    tool_call_id: String,
    tool_name: String,
    approval_request_id: Option<String>,
    settled: bool,
    result_tx: Option<oneshot::Sender<AcpToolResultRequest>>,
}

pub(crate) type AcpToolTurnRegistry = Arc<Mutex<HashMap<String, AcpToolTurn>>>;

pub(crate) fn new_acp_tool_turn_registry() -> AcpToolTurnRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

fn acp_tool_turn_key(session_id: &str, tool_call_id: &str) -> String {
    format!("{session_id}\n{tool_call_id}")
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/bears/{slug}/sessions", get(list_acp_sessions))
        .route("/bears/{slug}/sessions/{session_id}", get(get_acp_session))
        .route("/bears/{slug}/sessions/{session_id}/prompt", post(prompt))
        .route(
            "/bears/{slug}/sessions/{session_id}/tool-results/{tool_call_id}",
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

#[derive(Debug, Clone, Deserialize)]
struct AcpToolResultRequest {
    turn_id: Option<String>,
    request_id: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    approval_request_id: Option<String>,
    status: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    structured_content: serde_json::Value,
    #[serde(default)]
    diagnostic: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct AcpToolResultResponse {
    accepted: bool,
    reason: String,
    turn_id: Option<String>,
    tool_call_id: String,
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

fn is_acp_history_target(conversation_id: &str) -> bool {
    conversation_id == "default" || conversation_id.starts_with("conv-")
}

fn is_acp_archive_target(conversation_id: &str) -> bool {
    conversation_id.starts_with("conv-")
}

fn normalized_durable_acp_conversation_id(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| is_acp_history_target(s))
        .map(str::to_string)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpConversationSelectionSource {
    Explicit,
    Resolved,
    Stored,
    Generated,
}

impl AcpConversationSelectionSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Resolved => "resolved",
            Self::Stored => "stored",
            Self::Generated => "generated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcpConversationResolution {
    /// Den-persisted ACP session selection. May be `default`, `conv-*`, or pending `new-*`.
    session_selection: String,
    /// Known durable conversation id. Never a pending `new-*` placeholder.
    resolved_conversation_id: Option<String>,
    /// Letta path target. Never a pending `new-*` placeholder.
    upstream_target: String,
    selection_source: AcpConversationSelectionSource,
    history_target: Option<String>,
    archive_target: Option<String>,
    requires_belongs_to_bear_check: bool,
}

impl AcpConversationResolution {
    fn from_selection(
        session_selection: String,
        selection_source: AcpConversationSelectionSource,
        pair_agent_id: &str,
        existing_session: Option<&acp_sessions::AcpSessionRow>,
    ) -> Self {
        let resolved_conversation_id = if is_acp_history_target(&session_selection) {
            Some(session_selection.clone())
        } else if existing_session.is_some_and(|s| s.conversation_id.trim() == session_selection) {
            normalized_durable_acp_conversation_id(
                existing_session.and_then(|s| s.resolved_conversation_id.as_deref()),
            )
        } else {
            None
        };
        let upstream_target = if session_selection.starts_with("new-") {
            // `new-*` IDs are BEARS/ACP-local pending identifiers. Letta validates the path
            // parameter strictly (`default`, `conv-*`, or `agent-*`), so create/resume the
            // pending thread through the agent target and persist the real `conv-*` once the
            // stream emits `conversation_resolved`.
            pair_agent_id.to_string()
        } else {
            session_selection.clone()
        };
        let history_target = resolved_conversation_id
            .as_deref()
            .filter(|s| is_acp_history_target(s))
            .map(str::to_string);
        let archive_target = resolved_conversation_id
            .as_deref()
            .filter(|s| is_acp_archive_target(s))
            .map(str::to_string);
        let requires_belongs_to_bear_check = selection_source
            == AcpConversationSelectionSource::Explicit
            && session_selection.starts_with("conv-");

        Self {
            session_selection,
            resolved_conversation_id,
            upstream_target,
            selection_source,
            history_target,
            archive_target,
            requires_belongs_to_bear_check,
        }
    }
}

fn resolve_acp_prompt_conversation(
    requested_raw: Option<&str>,
    existing_session: Option<&acp_sessions::AcpSessionRow>,
    pair_agent_id: &str,
    generated_pending_id: String,
) -> Result<AcpConversationResolution, CustomError> {
    let requested = requested_raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| normalize_acp_conversation_id(Some(s)))
        .transpose()?
        // Older adapters hardcoded `conversation_id: "default"` for every prompt.
        // Treat that as omission so ACP session/new still creates a pending thread and
        // later prompts can use the stored/resolved conversation for this ACP session.
        .filter(|id| id != "default");

    let (session_selection, source) = if let Some(id) = requested {
        (id, AcpConversationSelectionSource::Explicit)
    } else if let Some(id) = existing_session
        .and_then(|s| normalized_durable_acp_conversation_id(s.resolved_conversation_id.as_deref()))
    {
        (id, AcpConversationSelectionSource::Resolved)
    } else if let Some(id) = existing_session
        .map(|s| s.conversation_id.trim())
        .filter(|s| !s.is_empty())
        .filter(|s| s.starts_with("conv-") || is_valid_pending_acp_conversation_id(s))
        .map(str::to_string)
    {
        (id, AcpConversationSelectionSource::Stored)
    } else {
        (
            generated_pending_id,
            AcpConversationSelectionSource::Generated,
        )
    };

    Ok(AcpConversationResolution::from_selection(
        session_selection,
        source,
        pair_agent_id,
        existing_session,
    ))
}

fn acp_archive_target_for_session(session: &acp_sessions::AcpSessionRow) -> Option<&str> {
    session
        .resolved_conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|s| is_acp_archive_target(s))
        .or_else(|| {
            let selection = session.conversation_id.trim();
            is_acp_archive_target(selection).then_some(selection)
        })
}

fn letta_conversation_id_from_create_response(value: &serde_json::Value) -> Option<String> {
    value
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| s.starts_with("conv-"))
        .map(str::to_string)
}

fn acp_direct_tool_prompt_context(
    session_id: &str,
    cwd: &str,
    client_context: &serde_json::Value,
) -> String {
    let roots = client_context
        .get("workspace_roots")
        .or_else(|| client_context.get("workspaceRoots"))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| vec![cwd.to_string()]);
    format!(
        concat!(
            "\n\n<system-reminder>",
            "BEARS ACP direct local file reading is available for this turn. ",
            "If you need to read a workspace file, call client tool `fs_read_text_file`; Den will route it to the local ACP adapter method `bears/read_text_file` / ACP client method `fs/read_text_file`. ",
            "Use params {{\"path\":\"/absolute/path\",\"line\":1,\"limit\":400}}. Current ACP session id is `{session_id}`. ",
            "Use absolute paths under these workspace roots: {roots}. ",
            "Do not guess file contents; request the file read and use the returned content. ",
            "</system-reminder>"
        ),
        session_id = session_id,
        roots = roots.join(", "),
    )
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

fn letta_stream_text_preserving_whitespace(inner: &serde_json::Value) -> Option<String> {
    let content = inner.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = content.as_object() {
        if let Some(t) = obj.get("text").and_then(|x| x.as_str()) {
            return Some(t.to_string());
        }
    }
    let parts = content.as_array()?;
    let mut out = String::new();
    let mut found_text = false;
    for part in parts {
        if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
            found_text = true;
            out.push_str(t);
        }
    }
    found_text.then_some(out)
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

async fn tool_result(
    State(state): State<ApiState>,
    Path((slug, session_id, tool_call_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpToolResultRequest>,
) -> Response {
    let request_id = Uuid::new_v4();
    match tool_result_inner(state, slug, session_id, tool_call_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn tool_result_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    tool_call_id: String,
    headers: HeaderMap,
    body: AcpToolResultRequest,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let key = acp_tool_turn_key(&session_id, &tool_call_id);
    let mut turns = state
        .acp_tool_turns
        .lock()
        .map_err(|_| CustomError::System("ACP tool turn registry lock poisoned".to_string()))?;
    let Some(turn) = turns.get_mut(&key) else {
        return Ok(Json(AcpToolResultResponse {
            accepted: false,
            reason: "turn_missing".to_string(),
            turn_id: body.turn_id,
            tool_call_id,
        })
        .into_response());
    };
    if turn.user_id != user_id
        || turn.bear_slug != slug
        || turn.acp_session_id != session_id
        || turn.tool_call_id != tool_call_id
    {
        return Err(CustomError::Authorization(
            "tool result does not match the authenticated ACP session".to_string(),
        ));
    }
    if let Some(body_tool_call_id) = body.tool_call_id.as_deref().filter(|s| !s.is_empty()) {
        if body_tool_call_id != turn.tool_call_id {
            return Err(CustomError::ValidationError(format!(
                "tool result call id mismatch: expected {}, got {}",
                turn.tool_call_id, body_tool_call_id
            )));
        }
    }
    if let Some(body_approval_request_id) = body
        .approval_request_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        if turn.approval_request_id.as_deref() != Some(body_approval_request_id) {
            return Err(CustomError::ValidationError(format!(
                "tool result approval request id mismatch: expected {:?}, got {}",
                turn.approval_request_id, body_approval_request_id
            )));
        }
    }
    if let Some(body_tool_name) = body.tool_name.as_deref().filter(|s| !s.is_empty()) {
        if body_tool_name != turn.tool_name {
            return Err(CustomError::ValidationError(format!(
                "tool result name mismatch: expected {}, got {}",
                turn.tool_name, body_tool_name
            )));
        }
    }
    if turn.settled {
        return Ok(Json(AcpToolResultResponse {
            accepted: false,
            reason: "already_settled".to_string(),
            turn_id: body.turn_id,
            tool_call_id,
        })
        .into_response());
    }
    let mut body = body;
    if body
        .tool_call_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_none()
    {
        body.tool_call_id = Some(turn.tool_call_id.clone());
    }
    if body
        .approval_request_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_none()
    {
        body.approval_request_id = turn.approval_request_id.clone();
    }
    turn.settled = true;
    let result_tx = turn.result_tx.take();
    tracing::info!(
        request_id = %turn.request_id,
        bear_id = %turn.bear_id,
        acp_session_id = %session_id,
        tool_call_id = %tool_call_id,
        tool_name = %turn.tool_name,
        body_request_id = body.request_id.as_deref(),
        body_tool_call_id = body.tool_call_id.as_deref(),
        body_approval_request_id = body.approval_request_id.as_deref(),
        status = %body.status,
        content_bytes = body.content.as_deref().map(str::len).unwrap_or(0),
        structured_content_bytes = body.structured_content.to_string().len(),
        diagnostic = ?body.diagnostic,
        "ACP tool result received"
    );
    if let Some(result_tx) = result_tx {
        let _ = result_tx.send(body.clone());
    }
    Ok(Json(AcpToolResultResponse {
        accepted: true,
        reason: "delivered".to_string(),
        turn_id: body.turn_id,
        tool_call_id,
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
    let archive_target = acp_archive_target_for_session(&session);
    let mut archived = false;
    if let Some(archive_target) = archive_target.filter(|_| state.letta.is_enabled()) {
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
        conversation_id: archive_target.map(str::to_string),
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
    let generated_conversation_id = new_acp_conversation_id(&client);
    let mut conversation_resolution = resolve_acp_prompt_conversation(
        body.conversation_id.as_deref(),
        existing_session.as_ref(),
        &pair_agent_id,
        generated_conversation_id,
    )
    .map_err(|err| {
        let (_, _, message) = acp_error_status_message(&err);
        ApiError::new(StatusCode::BAD_REQUEST, "validation", message)
    })?;
    if conversation_resolution.requires_belongs_to_bear_check {
        verify_acp_conversation_belongs_to_bear(
            &state,
            &pair_agent_id,
            &conversation_resolution.session_selection,
        )
        .await
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    }
    if conversation_resolution
        .session_selection
        .starts_with("new-")
        && conversation_resolution.resolved_conversation_id.is_none()
    {
        let created = state
            .letta
            .create_conversation_for_agent(&pair_agent_id)
            .await
            .map_err(|err| {
                let (status, code, message) = acp_error_status_message(&err);
                ApiError::new(status, code, message)
            })?;
        let conv_id = letta_conversation_id_from_create_response(&created).ok_or_else(|| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "letta_create_conversation",
                format!(
                    "Letta create conversation response did not contain a conv-* id: {created}"
                ),
            )
        })?;
        tracing::info!(
            %request_id,
            acp_session_id = %session_id,
            bear_id = %bear.id,
            pending_conversation_id = %conversation_resolution.session_selection,
            resolved_conversation_id = %conv_id,
            "ACP created fresh Letta conversation for new session"
        );
        conversation_resolution.resolved_conversation_id = Some(conv_id.clone());
        conversation_resolution.history_target = Some(conv_id.clone());
        conversation_resolution.archive_target = Some(conv_id.clone());
        conversation_resolution.upstream_target = conv_id;
    }
    let runtime_session_id = format!("acp-api-direct:{client}:{}:{session_id}", bear.id);
    acp_sessions::upsert_session(
        &state.sqlx_pool,
        UpsertAcpSession {
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            runtime_session_id: runtime_session_id.clone(),
            conversation_id: conversation_resolution.session_selection.clone(),
            resolved_conversation_id: conversation_resolution.resolved_conversation_id.clone(),
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
        requested_conversation_id = body.conversation_id.as_deref().map(str::trim),
        conversation_id = %conversation_resolution.session_selection,
        conversation_selection_source = %conversation_resolution.selection_source.as_str(),
        resolved_conversation_id = conversation_resolution.resolved_conversation_id.as_deref(),
        history_target = conversation_resolution.history_target.as_deref(),
        archive_target = conversation_resolution.archive_target.as_deref(),
        letta_conversation_id = %conversation_resolution.upstream_target,
        "ACP gateway routing prompt to pair role via Letta API"
    );
    let tool_prompt_context =
        acp_direct_tool_prompt_context(session_id, &cwd, &body.client_context);
    let prompt_with_tool_context = format!("{prompt}{tool_prompt_context}");
    let upstream = match state
        .letta
        .post_agent_messages_streaming(
            &pair_agent_id,
            &prompt_with_tool_context,
            Some(serde_json::json!([
                acp_read_text_file_client_tool_descriptor()
            ])),
        )
        .await
    {
        Ok(upstream) => upstream,
        Err(err) => return Ok(Err(err)),
    };

    let stream = AcpLettaSseStream::new(
        upstream.bytes_stream(),
        AcpStreamContext {
            pool: state.sqlx_pool.clone(),
            tool_turns: state.acp_tool_turns.clone(),
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            request_id,
        },
        state.letta.clone(),
        pair_agent_id.clone(),
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
    tool_turns: AcpToolTurnRegistry,
    user_id: i32,
    bear_id: Uuid,
    bear_slug: String,
    acp_session_id: String,
    request_id: Uuid,
}

#[derive(Debug)]
enum AcpGatewayEvent {
    AssistantTextDelta {
        text: String,
    },
    StatusText {
        text: String,
    },
    TurnComplete {
        outcome: String,
    },
    Error {
        message: String,
        detail: Option<String>,
        error_type: Option<String>,
        request_id: Option<String>,
        context: Option<serde_json::Value>,
    },
    ToolRequest {
        request_id: String,
        turn_id: String,
        tool_call_id: String,
        approval_request_id: Option<String>,
        tool_name: String,
        title: String,
        kind: String,
        args: serde_json::Value,
        approval_required: bool,
        approval_reason: Option<String>,
        result_tx: Option<oneshot::Sender<AcpToolResultRequest>>,
        result_rx: Option<oneshot::Receiver<AcpToolResultRequest>>,
    },
    ConversationResolved {
        conversation_id: String,
    },
}

async fn persist_stream_event_side_effects(
    context: &AcpStreamContext,
    event: &mut AcpGatewayEvent,
) -> Result<(), CustomError> {
    match event {
        AcpGatewayEvent::ConversationResolved { conversation_id } => {
            acp_sessions::mark_resolved(
                &context.pool,
                context.user_id,
                context.bear_id,
                &context.acp_session_id,
                conversation_id,
            )
            .await?;
        }
        AcpGatewayEvent::ToolRequest {
            tool_call_id,
            approval_request_id,
            tool_name,
            request_id,
            result_tx,
            ..
        } => {
            let result_tx = result_tx.take().ok_or_else(|| {
                CustomError::System("ACP tool request missing result channel".to_string())
            })?;
            let key = acp_tool_turn_key(&context.acp_session_id, tool_call_id);
            let mut turns = context.tool_turns.lock().map_err(|_| {
                CustomError::System("ACP tool turn registry lock poisoned".to_string())
            })?;
            turns.insert(
                key,
                AcpToolTurn {
                    user_id: context.user_id,
                    bear_id: context.bear_id,
                    bear_slug: context.bear_slug.clone(),
                    acp_session_id: context.acp_session_id.clone(),
                    request_id: context.request_id,
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    approval_request_id: approval_request_id.clone(),
                    settled: false,
                    result_tx: Some(result_tx),
                },
            );
            tracing::info!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                tool_request_id = %request_id,
                tool_call_id = %tool_call_id,
                tool_name = %tool_name,
                "ACP tool request registered"
            );
        }
        _ => {}
    }
    Ok(())
}

fn map_native_letta_stream_event_to_acp_event(
    event: &serde_json::Value,
) -> Option<AcpGatewayEvent> {
    let inner = letta_inner_for_acp_history(event);
    let message_type = inner
        .get("message_type")
        .and_then(|v| v.as_str())
        .or_else(|| event.get("message_type").and_then(|v| v.as_str()))
        .unwrap_or("");
    match message_type {
        "assistant_message" => Some(AcpGatewayEvent::AssistantTextDelta {
            text: letta_stream_text_preserving_whitespace(inner)
                .or_else(|| letta_stream_text_preserving_whitespace(event))
                .unwrap_or_default(),
        }),
        "reasoning_message" => Some(AcpGatewayEvent::StatusText {
            text: inner
                .get("reasoning")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| {
                    event
                        .get("reasoning")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .or_else(|| letta_stream_text_preserving_whitespace(inner))
                .or_else(|| letta_stream_text_preserving_whitespace(event))
                .unwrap_or_default(),
        }),
        "error_message" => Some(AcpGatewayEvent::Error {
            message: event
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Upstream error")
                .to_string(),
            detail: event
                .get("detail")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            error_type: event
                .get("error_type")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            request_id: event
                .get("request_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            context: event.get("context").cloned(),
        }),
        "stop_reason" => {
            let stop_reason = inner
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .or_else(|| event.get("stop_reason").and_then(|v| v.as_str()))
                .unwrap_or("unknown");
            if stop_reason == "end_turn" {
                Some(AcpGatewayEvent::TurnComplete {
                    outcome: "ok".to_string(),
                })
            } else if stop_reason == "requires_approval" {
                None
            } else {
                Some(AcpGatewayEvent::Error {
                    message: format!(
                        "Letta stopped before producing assistant output: {stop_reason}"
                    ),
                    detail: None,
                    error_type: Some(stop_reason.to_string()),
                    request_id: None,
                    context: None,
                })
            }
        }
        "tool_call_message" | "approval_request_message" | "function_call" => {
            native_letta_tool_request_event(
                event,
                inner,
                message_type == "approval_request_message",
            )
        }
        _ => native_letta_conversation_resolved_event(event),
    }
}

fn native_letta_tool_request_event(
    event: &serde_json::Value,
    inner: &serde_json::Value,
    approval_required: bool,
) -> Option<AcpGatewayEvent> {
    native_letta_tool_request_event_with_args(event, inner, approval_required, None, None)
}

fn native_letta_tool_request_event_with_args(
    event: &serde_json::Value,
    inner: &serde_json::Value,
    approval_required: bool,
    args_override: Option<serde_json::Value>,
    tool_name_override: Option<&str>,
) -> Option<AcpGatewayEvent> {
    let tool_call = inner
        .get("tool_call")
        .or_else(|| event.get("tool_call"))
        .or_else(|| {
            inner
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .and_then(|items| items.first())
        })
        .or_else(|| {
            event
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .and_then(|items| items.first())
        });
    let tool_name = tool_name_override.or_else(|| {
        tool_call
            .and_then(|v| v.get("name"))
            .or_else(|| {
                tool_call
                    .and_then(|v| v.get("function"))
                    .and_then(|f| f.get("name"))
            })
            .or_else(|| inner.get("tool_name"))
            .or_else(|| inner.get("name"))
            .or_else(|| event.get("tool_name"))
            .or_else(|| event.get("name"))
            .and_then(|v| v.as_str())
    })?;
    let normalized_tool = match tool_name {
        "bears/read_text_file" | "fs.read_text_file" | "fs_read_text_file" | "read_text_file" => {
            "fs_read_text_file"
        }
        _ => {
            return Some(AcpGatewayEvent::Error {
                message: format!("Letta requested unsupported ACP local tool: {tool_name}"),
                detail: Some(
                    "Only fs_read_text_file is wired for the current ACP local tool slice."
                        .to_string(),
                ),
                error_type: Some("unsupported_tool".to_string()),
                request_id: None,
                context: Some(serde_json::json!({ "tool_name": tool_name })),
            })
        }
    };
    let args_raw = tool_call
        .and_then(|v| v.get("input"))
        .or_else(|| tool_call.and_then(|v| v.get("arguments")))
        .or_else(|| tool_call.and_then(|v| v.get("args")))
        .or_else(|| {
            tool_call
                .and_then(|v| v.get("function"))
                .and_then(|f| f.get("arguments"))
        })
        .or_else(|| inner.get("input"))
        .or_else(|| inner.get("args"))
        .or_else(|| inner.get("arguments"))
        .or_else(|| event.get("input"))
        .or_else(|| event.get("args"))
        .or_else(|| event.get("arguments"));
    let args = if let Some(args) = args_override {
        args
    } else {
        match args_raw {
            Some(v) => {
                if v.is_null() {
                    // Letta may emit an initial tool-call shell with null arguments, then stream
                    // the real arguments in later events. Treat null as incomplete, not invalid.
                    return None;
                }
                if let Some(s) = v.as_str() {
                    // Letta may stream tool-call arguments incrementally. Do not emit a tool
                    // request until the JSON arguments parse completely.
                    let parsed = serde_json::from_str::<serde_json::Value>(s).ok()?;
                    if parsed.is_null() {
                        return None;
                    }
                    parsed
                } else {
                    v.clone()
                }
            }
            None => return None,
        }
    };
    if normalized_tool == "fs_read_text_file" && args.get("path").and_then(|v| v.as_str()).is_none()
    {
        // Do not fail on still-empty/non-object incremental chunks; wait for a complete object.
        if !args.is_object() || args.as_object().is_some_and(|m| m.is_empty()) {
            return None;
        }
        return Some(AcpGatewayEvent::Error {
            message: "Letta requested fs_read_text_file without a path argument.".to_string(),
            detail: Some(format!(
                "Parsed arguments did not contain string field `path`; args={}",
                preview_str_truncated(&args.to_string(), 240)
            )),
            error_type: Some("invalid_tool_arguments".to_string()),
            request_id: None,
            context: Some(serde_json::json!({
                "tool_name": tool_name,
                "tool_call_id": tool_call
                    .and_then(|v| v.get("tool_call_id"))
                    .or_else(|| tool_call.and_then(|v| v.get("id")))
                    .and_then(|v| v.as_str()),
                "args": args,
            })),
        });
    }
    let tool_call_id = tool_call
        .and_then(|v| v.get("tool_call_id"))
        .or_else(|| tool_call.and_then(|v| v.get("id")))
        .or_else(|| {
            tool_call
                .and_then(|v| v.get("function"))
                .and_then(|f| f.get("tool_call_id"))
        })
        .or_else(|| inner.get("tool_call_id"))
        .or_else(|| inner.get("id"))
        .or_else(|| event.get("tool_call_id"))
        .or_else(|| event.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("call-{}", Uuid::new_v4()));
    let approval_request_id = (approval_required).then(|| {
        event
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("approval-{}", Uuid::new_v4()))
    });
    let request_id = event
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let turn_id = event
        .get("turn_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let (result_tx, result_rx) = oneshot::channel();
    Some(AcpGatewayEvent::ToolRequest {
        request_id,
        turn_id,
        tool_call_id,
        approval_request_id,
        tool_name: normalized_tool.to_string(),
        title: ACP_READ_TEXT_FILE_TOOL.title.to_string(),
        kind: ACP_READ_TEXT_FILE_TOOL.kind.to_string(),
        args,
        approval_required,
        approval_reason: approval_required
            .then(|| "Letta requested approval before running this local ACP tool.".to_string()),
        result_tx: Some(result_tx),
        result_rx: Some(result_rx),
    })
}

#[derive(Debug, Default)]
struct LettaToolCallAccumulator {
    names: BTreeMap<String, String>,
    argument_buffers: BTreeMap<String, String>,
    emitted: BTreeMap<String, usize>,
}

impl LettaToolCallAccumulator {
    fn pending_argument_buffers(&self) -> usize {
        self.argument_buffers.len()
    }

    fn pending_name_buffers(&self) -> usize {
        self.names.len()
    }

    fn observe(&mut self, event: &serde_json::Value) -> Option<AcpGatewayEvent> {
        let inner = letta_inner_for_acp_history(event);
        let message_type = inner
            .get("message_type")
            .and_then(|v| v.as_str())
            .or_else(|| event.get("message_type").and_then(|v| v.as_str()))
            .unwrap_or("");
        if !matches!(
            message_type,
            "tool_call_message" | "approval_request_message" | "function_call"
        ) {
            return None;
        }
        let tool_call = inner
            .get("tool_call")
            .or_else(|| event.get("tool_call"))
            .or_else(|| {
                inner
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .and_then(|items| items.first())
            })
            .or_else(|| {
                event
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .and_then(|items| items.first())
            });
        let tool_call_id = tool_call
            .and_then(|v| v.get("tool_call_id"))
            .or_else(|| tool_call.and_then(|v| v.get("id")))
            .or_else(|| inner.get("tool_call_id"))
            .or_else(|| inner.get("id"))
            .or_else(|| event.get("tool_call_id"))
            .or_else(|| event.get("id"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("unknown-{}", Uuid::new_v4()));
        if self.emitted.contains_key(&tool_call_id) {
            return None;
        }
        if let Some(name) = tool_call_name(tool_call, inner, event) {
            self.names.insert(tool_call_id.clone(), name.to_string());
        }
        let args = self.parse_args_fragment(&tool_call_id, tool_call, inner, event)?;
        let tool_name = self.names.get(&tool_call_id).map(String::as_str)?;
        let mapped = native_letta_tool_request_event_with_args(
            event,
            inner,
            message_type == "approval_request_message",
            Some(args),
            Some(tool_name),
        );
        if mapped.is_some() {
            self.names.remove(&tool_call_id);
            self.argument_buffers.remove(&tool_call_id);
            *self.emitted.entry(tool_call_id).or_insert(0) += 1;
        }
        mapped
    }

    fn parse_args_fragment(
        &mut self,
        tool_call_id: &str,
        tool_call: Option<&serde_json::Value>,
        inner: &serde_json::Value,
        event: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let args_raw = tool_call_args_raw(tool_call, inner, event)?;
        if args_raw.is_null() {
            return None;
        }
        if let Some(fragment) = args_raw.as_str() {
            let buffer = self
                .argument_buffers
                .entry(tool_call_id.to_string())
                .or_default();
            buffer.push_str(fragment);
            match serde_json::from_str::<serde_json::Value>(buffer) {
                Ok(value) if !value.is_null() => Some(value),
                _ => None,
            }
        } else {
            Some(args_raw.clone())
        }
    }
}

fn tool_call_name<'a>(
    tool_call: Option<&'a serde_json::Value>,
    inner: &'a serde_json::Value,
    event: &'a serde_json::Value,
) -> Option<&'a str> {
    tool_call
        .and_then(|v| v.get("name"))
        .or_else(|| {
            tool_call
                .and_then(|v| v.get("function"))
                .and_then(|f| f.get("name"))
        })
        .or_else(|| inner.get("tool_name"))
        .or_else(|| inner.get("name"))
        .or_else(|| event.get("tool_name"))
        .or_else(|| event.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn tool_call_args_raw<'a>(
    tool_call: Option<&'a serde_json::Value>,
    inner: &'a serde_json::Value,
    event: &'a serde_json::Value,
) -> Option<&'a serde_json::Value> {
    tool_call
        .and_then(|v| v.get("input"))
        .or_else(|| tool_call.and_then(|v| v.get("arguments")))
        .or_else(|| tool_call.and_then(|v| v.get("args")))
        .or_else(|| {
            tool_call
                .and_then(|v| v.get("function"))
                .and_then(|f| f.get("arguments"))
        })
        .or_else(|| inner.get("input"))
        .or_else(|| inner.get("args"))
        .or_else(|| inner.get("arguments"))
        .or_else(|| event.get("input"))
        .or_else(|| event.get("args"))
        .or_else(|| event.get("arguments"))
}

fn map_native_letta_stream_event_to_acp_event_with_diagnostics(
    event: &serde_json::Value,
    diagnostics: &mut AcpStreamDiagnostics,
) -> Option<AcpGatewayEvent> {
    if let Some(mapped) = diagnostics.tool_call_accumulator.observe(event) {
        return Some(mapped);
    }
    map_native_letta_stream_event_to_acp_event(event)
}

fn native_letta_conversation_resolved_event(event: &serde_json::Value) -> Option<AcpGatewayEvent> {
    let conversation_id = event
        .get("conversation_id")
        .or_else(|| event.get("conversationId"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| s.starts_with("conv-"))?;
    let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let message_type = event
        .get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if ty == "conversation_resolved" || message_type == "conversation_resolved" {
        Some(AcpGatewayEvent::ConversationResolved {
            conversation_id: conversation_id.to_string(),
        })
    } else {
        None
    }
}

fn acp_event_adapter_type(event: &AcpGatewayEvent) -> &'static str {
    match event {
        AcpGatewayEvent::AssistantTextDelta { .. } => "assistant_text_delta",
        AcpGatewayEvent::StatusText { .. } => "status_text",
        AcpGatewayEvent::TurnComplete { .. } => "turn_complete",
        AcpGatewayEvent::Error { .. } => "error",
        AcpGatewayEvent::ToolRequest { .. } => "tool_request",
        AcpGatewayEvent::ConversationResolved { .. } => "conversation_resolved",
    }
}

fn acp_event_has_visible_output(event: &AcpGatewayEvent) -> bool {
    match event {
        AcpGatewayEvent::AssistantTextDelta { text } | AcpGatewayEvent::StatusText { text } => {
            !text.is_empty()
        }
        AcpGatewayEvent::Error { .. } => true,
        AcpGatewayEvent::TurnComplete { .. }
        | AcpGatewayEvent::ToolRequest { .. }
        | AcpGatewayEvent::ConversationResolved { .. } => false,
    }
}

#[derive(Debug, Default)]
struct AcpStreamDiagnostics {
    upstream_frames: usize,
    parsed_events: usize,
    mapped_events: usize,
    unmapped_events: usize,
    native_message_types: BTreeMap<String, usize>,
    native_event_types: BTreeMap<String, usize>,
    adapter_event_types: BTreeMap<String, usize>,
    tool_request_counts: BTreeMap<String, usize>,
    tool_call_accumulator: LettaToolCallAccumulator,
    unmapped_event_samples: Vec<String>,
    saw_visible_output: bool,
    saw_error: bool,
    saw_turn_complete: bool,
    emitted_empty_turn_error: bool,
}

impl AcpStreamDiagnostics {
    fn increment(map: &mut BTreeMap<String, usize>, key: &str) {
        let key = if key.trim().is_empty() {
            "<missing>"
        } else {
            key
        };
        *map.entry(key.to_string()).or_insert(0) += 1;
    }

    fn observe_parsed_event(&mut self, value: &serde_json::Value) {
        self.parsed_events += 1;
        let message_type = value
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        Self::increment(&mut self.native_message_types, message_type);
        Self::increment(&mut self.native_event_types, event_type);
    }

    fn observe_mapped_event(&mut self, event: &AcpGatewayEvent) {
        self.mapped_events += 1;
        Self::increment(&mut self.adapter_event_types, acp_event_adapter_type(event));
        self.saw_visible_output |= acp_event_has_visible_output(event);
        self.saw_error |= matches!(event, AcpGatewayEvent::Error { .. });
        self.saw_turn_complete |= matches!(event, AcpGatewayEvent::TurnComplete { .. });
    }

    fn observe_unmapped_event(&mut self, value: &serde_json::Value) {
        self.unmapped_events += 1;
        if self.unmapped_event_samples.len() < 5 {
            self.unmapped_event_samples.push(preview_str_truncated(
                &value.to_string(),
                acp_debug_event_sample_chars(),
            ));
        }
    }

    fn empty_turn_error_event(&mut self, context: &AcpStreamContext) -> Option<AcpGatewayEvent> {
        if self.emitted_empty_turn_error || self.saw_visible_output || self.saw_error {
            return None;
        }
        self.emitted_empty_turn_error = true;
        let detail = format!(
            "Letta stream ended without displayable assistant/status/error output. upstream_frames={}, parsed_events={}, mapped_events={}, unmapped_events={}, message_types={:?}, event_types={:?}",
            self.upstream_frames,
            self.parsed_events,
            self.mapped_events,
            self.unmapped_events,
            self.native_message_types,
            self.native_event_types,
        );
        Some(AcpGatewayEvent::Error {
            message: "Letta completed the turn without producing displayable ACP output."
                .to_string(),
            detail: Some(detail),
            error_type: Some("empty_mapped_turn".to_string()),
            request_id: Some(context.request_id.to_string()),
            context: Some(serde_json::json!({
                "acp_session_id": context.acp_session_id,
                "unmapped_event_samples": self.unmapped_event_samples,
            })),
        })
    }

    fn log_summary(&self, context: &AcpStreamContext) {
        tracing::info!(
            request_id = %context.request_id,
            acp_session_id = %context.acp_session_id,
            upstream_frames = self.upstream_frames,
            parsed_events = self.parsed_events,
            mapped_events = self.mapped_events,
            unmapped_events = self.unmapped_events,
            saw_visible_output = self.saw_visible_output,
            saw_error = self.saw_error,
            saw_turn_complete = self.saw_turn_complete,
            native_message_types = ?self.native_message_types,
            native_event_types = ?self.native_event_types,
            adapter_event_types = ?self.adapter_event_types,
            tool_request_counts = ?self.tool_request_counts,
            pending_tool_argument_buffers = self.tool_call_accumulator.pending_argument_buffers(),
            pending_tool_name_buffers = self.tool_call_accumulator.pending_name_buffers(),
            unmapped_event_samples = ?self.unmapped_event_samples,
            "ACP Letta stream summary"
        );
    }
}

fn acp_event_to_adapter_sse(event: AcpGatewayEvent) -> Bytes {
    let mapped = match event {
        AcpGatewayEvent::AssistantTextDelta { text } => serde_json::json!({
            "type": "assistant_text_delta",
            "text": text,
        }),
        AcpGatewayEvent::StatusText { text } => serde_json::json!({
            "type": "status_text",
            "text": text,
        }),
        AcpGatewayEvent::TurnComplete { outcome } => serde_json::json!({
            "type": "turn_complete",
            "outcome": outcome,
        }),
        AcpGatewayEvent::Error {
            message,
            detail,
            error_type,
            request_id,
            context,
        } => {
            let mut mapped = serde_json::json!({
                "type": "error",
                "message": message,
                "detail": detail,
                "error_type": error_type,
            });
            if let Some(context) = context {
                mapped["context"] = context;
            }
            if let Some(request_id) = request_id {
                mapped["request_id"] = serde_json::json!(request_id);
            }
            mapped
        }
        AcpGatewayEvent::ToolRequest {
            request_id,
            turn_id,
            tool_call_id,
            approval_request_id,
            tool_name,
            title,
            kind,
            args,
            approval_required,
            approval_reason,
            result_tx: _,
            result_rx: _,
        } => serde_json::json!({
            "type": "tool_request",
            "request_id": request_id,
            "turn_id": turn_id,
            "tool_call_id": tool_call_id,
            "approval_request_id": approval_request_id,
            "tool_name": tool_name,
            "title": title,
            "kind": kind,
            "args": args,
            "approval": {
                "required": approval_required,
                "reason": approval_reason,
            },
            "diagnostic": {
                "component": "den.acp",
                "transport_version": 3,
            },
        }),
        AcpGatewayEvent::ConversationResolved { conversation_id } => serde_json::json!({
            "type": "conversation_resolved",
            "conversation_id": conversation_id,
        }),
    };
    Bytes::from(format!("data: {}\n\n", mapped))
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
        Ok(Some(value)) => map_native_letta_stream_event_to_acp_event(&value)
            .map(acp_event_to_adapter_sse)
            .into_iter()
            .collect(),
        Ok(None) | Err(_) => Vec::new(),
    }
}

async fn map_letta_stream_frame_to_acp_adapter_events_with_persistence(
    frame: Vec<u8>,
    context: AcpStreamContext,
    diagnostics: &mut AcpStreamDiagnostics,
) -> Result<
    (
        Vec<Bytes>,
        Option<(String, oneshot::Receiver<AcpToolResultRequest>)>,
    ),
    std::io::Error,
> {
    diagnostics.upstream_frames += 1;
    let body = strip_trailing_sse_delimiter_owned(frame);
    let value = match parse_sse_event_body_to_json(&body) {
        Err(msg) => {
            tracing::warn!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                error = %msg,
                "ACP upstream Letta SSE event JSON parse failed"
            );
            return Ok((Vec::new(), None));
        }
        Ok(None) => return Ok((Vec::new(), None)),
        Ok(Some(v)) => v,
    };
    diagnostics.observe_parsed_event(&value);

    let Some(mut event) =
        map_native_letta_stream_event_to_acp_event_with_diagnostics(&value, diagnostics)
    else {
        diagnostics.observe_unmapped_event(&value);
        return Ok((Vec::new(), None));
    };
    let result_rx = if let AcpGatewayEvent::ToolRequest {
        tool_call_id,
        result_rx,
        ..
    } = &mut event
    {
        result_rx.take().map(|rx| (tool_call_id.clone(), rx))
    } else {
        None
    };
    if let AcpGatewayEvent::ToolRequest { tool_call_id, .. } = &event {
        let count = diagnostics
            .tool_request_counts
            .entry(tool_call_id.clone())
            .or_insert(0);
        *count += 1;
        if *count > 1 {
            tracing::debug!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                tool_call_id = %tool_call_id,
                duplicate_count = *count,
                "ignoring duplicate streamed ACP tool request"
            );
            return Ok((Vec::new(), None));
        }
    }
    diagnostics.observe_mapped_event(&event);
    persist_stream_event_side_effects(&context, &mut event)
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok((vec![acp_event_to_adapter_sse(event)], result_rx))
}

enum AcpPendingFuture {
    Frame(
        Pin<
            Box<
                dyn Future<
                        Output = (
                            Result<
                                (
                                    Vec<Bytes>,
                                    Option<(String, oneshot::Receiver<AcpToolResultRequest>)>,
                                ),
                                std::io::Error,
                            >,
                            AcpStreamDiagnostics,
                        ),
                    > + Send,
            >,
        >,
    ),
    Tool(Pin<Box<dyn Future<Output = Result<AcpToolResultRequest, String>> + Send>>),
    ContinueTool(Pin<Box<dyn Future<Output = Result<reqwest::Response, CustomError>> + Send>>),
}

struct AcpLettaSseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    /// Complete upstream SSE event bodies (delimiter stripped), FIFO.
    pending_raw_frames: VecDeque<Vec<u8>>,
    pending: VecDeque<Bytes>,
    context: AcpStreamContext,
    letta: Arc<crate::core::letta::LettaClient>,
    pair_agent_id: String,
    pending_tool_result: Option<AcpToolResultRequest>,
    diagnostics: AcpStreamDiagnostics,
    logged_summary: bool,
    active_tool_call_ids: Vec<String>,
    persist_future: Option<AcpPendingFuture>,
}

impl AcpLettaSseStream {
    fn new(
        inner: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        context: AcpStreamContext,
        letta: Arc<crate::core::letta::LettaClient>,
        pair_agent_id: String,
    ) -> Self {
        Self {
            inner: Box::pin(inner),
            buffer: Vec::new(),
            pending_raw_frames: VecDeque::new(),
            pending: VecDeque::new(),
            context,
            letta,
            pair_agent_id,
            pending_tool_result: None,
            diagnostics: AcpStreamDiagnostics::default(),
            logged_summary: false,
            active_tool_call_ids: Vec::new(),
            persist_future: None,
        }
    }

    fn cleanup_active_tool_turns(&mut self) {
        if self.active_tool_call_ids.is_empty() {
            return;
        }
        if let Ok(mut turns) = self.context.tool_turns.lock() {
            for tool_call_id in self.active_tool_call_ids.drain(..) {
                turns.remove(&acp_tool_turn_key(
                    &self.context.acp_session_id,
                    &tool_call_id,
                ));
            }
        }
    }

    fn log_summary_once(&mut self) {
        if !self.logged_summary {
            self.cleanup_active_tool_turns();
            self.diagnostics.log_summary(&self.context);
            self.logged_summary = true;
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
            match fut {
                AcpPendingFuture::Frame(fut) => {
                    let (result, diagnostics) = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    this.diagnostics = diagnostics;
                    match result {
                        Ok((bytes, result_rx)) => {
                            for item in bytes {
                                this.pending.push_back(item);
                            }
                            if let Some((tool_call_id, result_rx)) = result_rx {
                                this.active_tool_call_ids.push(tool_call_id);
                                this.persist_future =
                                    Some(AcpPendingFuture::Tool(Box::pin(async move {
                                        tokio::time::timeout(
                                            std::time::Duration::from_secs(30),
                                            result_rx,
                                        )
                                        .await
                                        .map_err(|_| {
                                            "timed out waiting for ACP local tool result"
                                                .to_string()
                                        })?
                                        .map_err(|err| err.to_string())
                                    })));
                            }
                            return self.poll_next(cx);
                        }
                        Err(err) => return Poll::Ready(Some(Err(err))),
                    }
                }
                AcpPendingFuture::Tool(fut) => {
                    let result = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    match result {
                        Ok(tool_result) => {
                            if let Some(done_id) = tool_result.tool_call_id.as_deref() {
                                this.active_tool_call_ids.retain(|id| id != done_id);
                                if let Ok(mut turns) = this.context.tool_turns.lock() {
                                    turns.remove(&acp_tool_turn_key(
                                        &this.context.acp_session_id,
                                        done_id,
                                    ));
                                }
                            }
                            let tool_name = tool_result
                                .tool_name
                                .as_deref()
                                .unwrap_or("tool")
                                .to_string();
                            this.pending.push_back(acp_event_to_adapter_sse(
                                AcpGatewayEvent::StatusText {
                                    text: format!(
                                    "Local tool {tool_name} completed with status {} ({} bytes)",
                                    tool_result.status,
                                    tool_result.content.as_deref().map(str::len).unwrap_or(0),
                                ),
                                },
                            ));
                            // Letta keeps the original run active until its SSE stream finishes
                            // with `requires_approval`/stop metadata. If we POST the tool return
                            // before draining that stream, Letta rejects the continuation with
                            // HTTP 409 (another run is still processing this conversation). Store
                            // the result and continue reading the current stream; once it ends,
                            // start the tool-return continuation.
                            this.pending_tool_result = Some(tool_result);
                            return self.poll_next(cx);
                        }
                        Err(err) => {
                            this.pending.push_back(acp_event_to_adapter_sse(
                                AcpGatewayEvent::Error {
                                    message: "ACP local tool result was not delivered before the turn could continue.".to_string(),
                                    detail: Some(err),
                                    error_type: Some("tool_result_channel_closed".to_string()),
                                    request_id: Some(this.context.request_id.to_string()),
                                    context: None,
                                },
                            ));
                            return self.poll_next(cx);
                        }
                    }
                }
                AcpPendingFuture::ContinueTool(fut) => {
                    let result = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    match result {
                        Ok(response) => {
                            this.inner = Box::pin(response.bytes_stream());
                            return self.poll_next(cx);
                        }
                        Err(err) => {
                            this.pending.push_back(acp_event_to_adapter_sse(
                                AcpGatewayEvent::Error {
                                    message:
                                        "Failed to continue Letta after ACP local tool result."
                                            .to_string(),
                                    detail: Some(err.to_string()),
                                    error_type: Some("letta_tool_return_failed".to_string()),
                                    request_id: Some(this.context.request_id.to_string()),
                                    context: None,
                                },
                            ));
                            return self.poll_next(cx);
                        }
                    }
                }
            }
        }

        if let Some(frame_body) = this.pending_raw_frames.pop_front() {
            let context = this.context.clone();
            let mut diagnostics = std::mem::take(&mut this.diagnostics);
            this.persist_future = Some(AcpPendingFuture::Frame(Box::pin(async move {
                let result = map_letta_stream_frame_to_acp_adapter_events_with_persistence(
                    frame_body,
                    context,
                    &mut diagnostics,
                )
                .await;
                (result, diagnostics)
            })));
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
                } else if let Some(tool_result) = this.pending_tool_result.take() {
                    let letta = this.letta.clone();
                    let pair_agent_id = this.pair_agent_id.clone();
                    let tool_name = tool_result
                        .tool_name
                        .as_deref()
                        .unwrap_or("tool")
                        .to_string();
                    let tool_call_id = tool_result
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| tool_name.clone());
                    let tool_return = tool_result.content.clone().unwrap_or_default();
                    let status = tool_result.status.clone();
                    let approval_request_id = tool_result.approval_request_id.clone();
                    this.persist_future =
                        Some(AcpPendingFuture::ContinueTool(Box::pin(async move {
                            letta
                                .post_agent_tool_returns_streaming(
                                    &pair_agent_id,
                                    &tool_call_id,
                                    approval_request_id.as_deref(),
                                    &status,
                                    &tool_return,
                                )
                                .await
                        })));
                    self.poll_next(cx)
                } else if let Some(event) = this.diagnostics.empty_turn_error_event(&this.context) {
                    this.diagnostics.observe_mapped_event(&event);
                    this.pending.push_back(acp_event_to_adapter_sse(event));
                    self.poll_next(cx)
                } else {
                    this.log_summary_once();
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
    fn acp_read_text_file_provider_name_is_safe() {
        assert!(provider_tool_name_is_safe(
            ACP_READ_TEXT_FILE_TOOL.provider_name
        ));
        assert!(!provider_tool_name_is_safe("fs.read_text_file"));
        assert!(!provider_tool_name_is_safe("fs/read_text_file"));
    }

    #[test]
    fn acp_read_text_file_descriptor_uses_provider_name_only() {
        let descriptor = acp_read_text_file_client_tool_descriptor();
        assert_eq!(descriptor["name"], ACP_READ_TEXT_FILE_TOOL.provider_name);
        assert_ne!(descriptor["name"], ACP_READ_TEXT_FILE_TOOL.canonical_name);
        assert_ne!(descriptor["name"], ACP_READ_TEXT_FILE_TOOL.client_method);
        assert!(!descriptor
            .to_string()
            .contains("\"name\":\"fs.read_text_file\""));
    }

    #[test]
    fn serializes_internal_assistant_chunk_to_acp_adapter_event() {
        let text = String::from_utf8(
            acp_event_to_adapter_sse(AcpGatewayEvent::AssistantTextDelta {
                text: "hello".to_string(),
            })
            .to_vec(),
        )
        .unwrap();
        assert!(text.contains("\"type\":\"assistant_text_delta\""));
        assert!(text.contains("\"text\":\"hello\""));
    }

    #[test]
    fn serializes_internal_done_to_acp_adapter_event() {
        let text = String::from_utf8(
            acp_event_to_adapter_sse(AcpGatewayEvent::TurnComplete {
                outcome: "ok".to_string(),
            })
            .to_vec(),
        )
        .unwrap();
        assert!(text.contains("\"type\":\"turn_complete\""));
    }

    #[test]
    fn native_parser_ignores_bears_shaped_assistant_delta() {
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"type\":\"assistant_delta\",\"text\":\"hello\"}\n\n",
        );
        assert!(out.is_empty());
    }

    #[test]
    fn maps_native_letta_assistant_message_to_acp_adapter_event() {
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"message_type\":\"assistant_message\",\"content\":\"hello from native Letta\"}\n\n",
        );
        let text = String::from_utf8(out[0].to_vec()).unwrap();
        assert!(text.contains("\"type\":\"assistant_text_delta\""));
        assert!(text.contains("hello from native Letta"));
    }

    #[test]
    fn maps_native_letta_assistant_chunks_without_stripping_spaces() {
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"message_type\":\"assistant_message\",\"content\":\" broke\"}\n\n",
        );
        let text = String::from_utf8(out[0].to_vec()).unwrap();
        assert!(text.contains("\"text\":\" broke\""));
    }

    #[test]
    fn maps_realistic_fixture_approval_request_to_tool_request() {
        let event: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_approval_request.json"
        ))
        .unwrap();
        let mapped = map_native_letta_stream_event_to_acp_event(&event).unwrap();
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_call_id,
                approval_request_id,
                tool_name,
                args,
                approval_required,
                ..
            } => {
                assert_eq!(tool_call_id, "call_fixture_read");
                assert_eq!(
                    approval_request_id.as_deref(),
                    Some("message-approval-fixture")
                );
                assert_eq!(tool_name, "fs_read_text_file");
                assert_eq!(args["path"], "/tmp/acp-fixture.txt");
                assert!(approval_required);
            }
            other => panic!("expected tool request, got {other:?}"),
        }
    }

    #[test]
    fn partial_fixture_does_not_emit_tool_request_until_arguments_complete() {
        let null_args: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_tool_call_delta_null_args.json"
        ))
        .unwrap();
        assert!(map_native_letta_stream_event_to_acp_event(&null_args).is_none());

        let partial: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_tool_call_delta_partial.json"
        ))
        .unwrap();
        assert!(map_native_letta_stream_event_to_acp_event(&partial).is_none());

        let complete: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_tool_call_delta_complete.json"
        ))
        .unwrap();
        let mapped = map_native_letta_stream_event_to_acp_event(&complete).unwrap();
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_call_id, args, ..
            } => {
                assert_eq!(tool_call_id, "call_fixture_partial");
                assert_eq!(args["path"], "/tmp/acp-complete.txt");
            }
            other => panic!("expected tool request, got {other:?}"),
        }
    }

    #[test]
    fn maps_nested_letta_approval_request_to_tool_request() {
        let event = serde_json::json!({
            "id": "message-approval-1",
            "message_type": "approval_request_message",
            "tool_call": {
                "name": "fs_read_text_file",
                "tool_call_id": "call_abc123",
                "arguments": "{\"path\":\"/tmp/readme.md\",\"line\":1,\"limit\":20}"
            }
        });
        let mapped = map_native_letta_stream_event_to_acp_event(&event).unwrap();
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_call_id,
                approval_request_id,
                tool_name,
                args,
                approval_required,
                ..
            } => {
                assert_eq!(tool_call_id, "call_abc123");
                assert_eq!(approval_request_id.as_deref(), Some("message-approval-1"));
                assert_eq!(tool_name, "fs_read_text_file");
                assert_eq!(args["path"], "/tmp/readme.md");
                assert!(approval_required);
            }
            other => panic!("expected tool request, got {other:?}"),
        }
    }

    #[test]
    fn accumulates_split_tool_argument_fragments() {
        let first: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_tool_call_delta_split_1.json"
        ))
        .unwrap();
        let second: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_tool_call_delta_split_2.json"
        ))
        .unwrap();
        let mut diagnostics = AcpStreamDiagnostics::default();
        assert!(map_native_letta_stream_event_to_acp_event_with_diagnostics(
            &first,
            &mut diagnostics,
        )
        .is_none());
        let mapped =
            map_native_letta_stream_event_to_acp_event_with_diagnostics(&second, &mut diagnostics)
                .unwrap();
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_call_id, args, ..
            } => {
                assert_eq!(tool_call_id, "call_fixture_split");
                assert_eq!(args["path"], "/tmp/split.txt");
            }
            other => panic!("expected tool request, got {other:?}"),
        }
    }

    #[test]
    fn accumulates_tool_name_across_argument_chunks() {
        let first: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_tool_call_delta_split_name_only_first.json"
        ))
        .unwrap();
        let second: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_tool_call_delta_split_args_no_name.json"
        ))
        .unwrap();
        let mut diagnostics = AcpStreamDiagnostics::default();
        assert!(map_native_letta_stream_event_to_acp_event_with_diagnostics(
            &first,
            &mut diagnostics,
        )
        .is_none());
        let mapped =
            map_native_letta_stream_event_to_acp_event_with_diagnostics(&second, &mut diagnostics)
                .unwrap();
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_call_id,
                args,
                tool_name,
                ..
            } => {
                assert_eq!(tool_call_id, "call_fixture_split_name");
                assert_eq!(tool_name, "fs_read_text_file");
                assert_eq!(args["path"], "/tmp/name-buffer.txt");
            }
            other => panic!("expected tool request, got {other:?}"),
        }
    }

    #[test]
    fn maps_letta_input_shape_fixture_to_tool_request() {
        let event: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_approval_request_input_shape.json"
        ))
        .unwrap();
        let mapped = map_native_letta_stream_event_to_acp_event(&event).unwrap();
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_call_id, args, ..
            } => {
                assert_eq!(tool_call_id, "call_fixture_input_shape");
                assert_eq!(args["path"], "/tmp/input-shape.txt");
                assert_eq!(args["line"], 3);
            }
            other => panic!("expected tool request, got {other:?}"),
        }
    }

    #[test]
    fn maps_letta_function_shape_fixture_to_tool_request() {
        let event: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acp_letta_approval_request_function_shape.json"
        ))
        .unwrap();
        let mapped = map_native_letta_stream_event_to_acp_event(&event).unwrap();
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_call_id, args, ..
            } => {
                assert_eq!(tool_call_id, "call_fixture_function_shape");
                assert_eq!(args["path"], "/tmp/function-shape.txt");
            }
            other => panic!("expected tool request, got {other:?}"),
        }
    }

    #[test]
    fn maps_letta_tool_calls_array_to_tool_request() {
        let event = serde_json::json!({
            "message_type": "tool_call_message",
            "tool_calls": [{
                "name": "fs_read_text_file",
                "tool_call_id": "call_array",
                "arguments": {"path":"/tmp/a.txt"}
            }]
        });
        let mapped = map_native_letta_stream_event_to_acp_event(&event).unwrap();
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_call_id,
                args,
                approval_required,
                ..
            } => {
                assert_eq!(tool_call_id, "call_array");
                assert_eq!(args["path"], "/tmp/a.txt");
                assert!(!approval_required);
            }
            other => panic!("expected tool request, got {other:?}"),
        }
    }

    #[test]
    fn invalid_read_file_tool_arguments_are_visible_errors() {
        let event = serde_json::json!({
            "message_type": "approval_request_message",
            "id": "message-missing-path",
            "tool_call": {
                "name": "fs_read_text_file",
                "tool_call_id": "call_missing_path",
                "arguments": "{}"
            }
        });
        let mapped = map_native_letta_stream_event_to_acp_event(&event).unwrap();
        match mapped {
            AcpGatewayEvent::Error {
                error_type,
                message,
                ..
            } => {
                assert_eq!(error_type.as_deref(), Some("invalid_tool_arguments"));
                assert!(message.contains("without a path"));
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn maps_native_letta_stop_reason_end_turn_to_done() {
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n",
        );
        let text = String::from_utf8(out[0].to_vec()).unwrap();
        assert!(text.contains("\"type\":\"turn_complete\""));
    }

    #[test]
    fn sse_parser_joins_multiple_data_lines_into_one_json_value() {
        let body = br#"data: {"message_type":"assistant_message","content":
data: "hello"}"#;
        let v = parse_sse_event_body_to_json(body).unwrap().unwrap();
        assert_eq!(v["message_type"], "assistant_message");
        assert_eq!(v["content"], "hello");
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"message_type\":\"assistant_message\",\"content\":\ndata: \"hello\"}\n\n",
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
    fn resolver_maps_pending_acp_selection_to_letta_agent_target() {
        let agent_id = "agent-12345678-1234-4567-89ab-123456789abc";
        let resolution =
            resolve_acp_prompt_conversation(None, None, agent_id, "new-acp-zed-abc123".to_string())
                .unwrap();
        assert_eq!(resolution.session_selection, "new-acp-zed-abc123");
        assert_eq!(resolution.resolved_conversation_id, None);
        assert_eq!(resolution.upstream_target, agent_id);
        assert_eq!(resolution.history_target, None);
        assert_eq!(resolution.archive_target, None);
        assert_eq!(
            resolution.selection_source,
            AcpConversationSelectionSource::Generated
        );
    }

    #[test]
    fn resolver_routes_explicit_conv_directly_and_requires_bear_check() {
        let agent_id = "agent-12345678-1234-4567-89ab-123456789abc";
        let conv_id = "conv-12345678-1234-4567-89ab-123456789abc";
        let resolution = resolve_acp_prompt_conversation(
            Some(conv_id),
            None,
            agent_id,
            "new-acp-zed-unused".to_string(),
        )
        .unwrap();
        assert_eq!(resolution.session_selection, conv_id);
        assert_eq!(
            resolution.resolved_conversation_id.as_deref(),
            Some(conv_id)
        );
        assert_eq!(resolution.upstream_target, conv_id);
        assert_eq!(resolution.history_target.as_deref(), Some(conv_id));
        assert_eq!(resolution.archive_target.as_deref(), Some(conv_id));
        assert_eq!(
            resolution.selection_source,
            AcpConversationSelectionSource::Explicit
        );
        assert!(resolution.requires_belongs_to_bear_check);
    }

    #[test]
    fn resolver_never_archives_pending_or_default_targets() {
        let pending = AcpConversationResolution::from_selection(
            "new-acp-zed-abc123".to_string(),
            AcpConversationSelectionSource::Generated,
            "agent-12345678-1234-4567-89ab-123456789abc",
            None,
        );
        assert_eq!(pending.history_target, None);
        assert_eq!(pending.archive_target, None);

        let default = AcpConversationResolution::from_selection(
            "default".to_string(),
            AcpConversationSelectionSource::Stored,
            "agent-12345678-1234-4567-89ab-123456789abc",
            None,
        );
        assert_eq!(default.history_target.as_deref(), Some("default"));
        assert_eq!(default.archive_target, None);
    }

    #[test]
    fn rejects_legacy_pending_acp_conversation_ids_that_exceed_letta_limit() {
        let legacy = "new-acp-zed-acp-12345678-1234-1234-1234-123456789abc";
        assert!(normalize_acp_conversation_id(Some(legacy)).is_ok());
        assert!(!is_valid_pending_acp_conversation_id(legacy));
    }
}
