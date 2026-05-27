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
use futures::{ready, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    path::Path as FsPath,
    pin::Pin,
    sync::Arc,
    task::Poll,
};
use time::format_description::well_known::Rfc3339;
use tokio::sync::{oneshot, Mutex as TokioMutex};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    api::{
        acp_stream_mapping::{
            map_letta_stream_frame_to_acp_adapter_events,
            map_runtime_stream_event_to_acp_adapter_events_with_persistence,
            summarize_event_for_log,
        },
        acp_stream_support::{
            find_sse_frame_end, parse_sse_event_body_to_json,
            strip_trailing_sse_delimiter_owned, AcpStreamDiagnostics,
        },
        auth::{self, ApiError},
        oauth::OAuthScope,
        service::ApiState,
    },
    core::{
        acp_letta_events::{acp_event_to_adapter_sse, AcpGatewayEvent},
        acp_plan_mode,
        acp_runtime::{
            ensure_acp_session_conversation, require_pair_runtime_binding,
            verify_acp_conversation_belongs_to_binding,
        },
        acp_sessions::{self, UpsertAcpSession},
        acp_tokens,
        acp_tool_turns::{
            AcpToolResultDelivery, AcpToolResultRequest, AcpToolTurnCoordinator,
            AcpToolTurnRegistration,
        },
        acp_tools::{
            acp_client_tool_descriptors_for_client_context, acp_diag_phase,
            acp_provider_tool_names_for_client_context, acp_tool_policy_json_for_provider,
            resolve_session_policy_for_mode, AcpToolStatus,
        },
        acp_turn_controller::{
            AcpActiveTurnCancelHandle, AcpToolExecutionRoute as ControllerToolExecutionRoute,
            AcpTurnController, AcpTurnPhase,
        },
        acp_turn_runner::{
            acp_cleanup_stale_runtime_state, continue_acp_turn_with_runtime,
            start_acp_turn_with_retries, AcpStaleRuntimeCleanupParams, AcpTurnContinueRequest,
            AcpTurnStartRequest, AcpTurnStreamContext,
        },
        archived_conversations,
        bears::{db as bears_db, BearAgentRole},
        den_tools::{self, DenToolChannelContext, DenToolInvocationContext},
        letta::{
            load_agent_conversations, normalize_display_status_text,
            sanitize_visible_transcript_text, LettaContinuationContext,
        },
        memory_manager_head::{write_memfs_role_memory_entry, MemfsWriteRoleMemoryEntryRequest},
        memory_proposals::{self, CreateMemoryProposal},
        pair_reflection::{self, CompletePairReflectionRun, CreatePairReflectionRun},
        reflection_conductor,
        role_runtime::{
            AcpTurnLifecycleContext, AcpTurnLifecycleRuntime, RoleRuntime, RoleTurnResult,
            RoleTurnScope, TurnResultReason, TurnResultStatus,
        },
        runtime_provider::{RoleRuntimeBinding, RuntimeContinuation},
        turn_state, user, web_policy,
        work_plans::{self, WorkPlanLookup, WorkPlanProjection},
    },
    errors::CustomError,
};

const ACP_SESSIONS_PAGE_SIZE: i64 = 50;
const BEARS_ACP_ADAPTER_CONTRACT_NAME: &str = "bears.acp.adapter";
const BEARS_ACP_ADAPTER_CONTRACT_CURRENT: u32 = 1;
const BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED: u32 = 1;
const BEARS_ACP_ADAPTER_CONTRACT_MAX_SUPPORTED: u32 = 1;
// Missing contract metadata is accepted for compatibility with already-running
// adapter processes. Set this to true only when a Den change is actually
// incompatible with adapters that do not send `adapter_contract`.
const BEARS_ACP_ADAPTER_CONTRACT_REQUIRED: bool = false;
fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn acp_stream_tokens_enabled() -> bool {
    env_flag("BEARS_ACP_STREAM_TOKENS")
}

fn acp_text_chunk_chars() -> usize {
    std::env::var("BEARS_ACP_TEXT_CHUNK_CHARS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(64, 2048))
        .unwrap_or(384)
}

fn acp_max_thought_bytes_per_turn() -> usize {
    std::env::var("BEARS_ACP_MAX_THOUGHT_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1024, 1024 * 1024))
        .unwrap_or(128 * 1024)
}

fn acp_debug_ui_enabled() -> bool {
    env_flag("BEARS_ACP_DEBUG_UI")
}

fn acp_tool_timeout_ms_for_provider(tool_name: &str) -> u64 {
    std::env::var("BEARS_ACP_TOOL_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| value.clamp(1, 300_000))
        .unwrap_or_else(|| {
            acp_tool_policy_json_for_provider(tool_name)
                .get("tool_timeout_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30_000)
        })
}

pub(super) fn acp_debug_event_sample_chars() -> usize {
    std::env::var("ACP_DEBUG_EVENT_SAMPLE_CHARS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|n| n.clamp(128, 20_000))
        .unwrap_or(360)
}

type PendingWebFetchMap = Arc<TokioMutex<HashMap<String, PendingWebFetchApproval>>>;
static PENDING_WEB_FETCH_APPROVALS: std::sync::OnceLock<PendingWebFetchMap> =
    std::sync::OnceLock::new();

fn pending_web_fetch_approvals() -> PendingWebFetchMap {
    PENDING_WEB_FETCH_APPROVALS
        .get_or_init(|| Arc::new(TokioMutex::new(HashMap::new())))
        .clone()
}

struct PendingWebFetchApproval {
    user_id: i32,
    bear_id: Uuid,
    result_tx: oneshot::Sender<AcpToolResultRequest>,
    context: AcpStreamContext,
    provider_name: String,
    tool_call_id: String,
    approval_request_id: Option<String>,
    args: serde_json::Value,
    normalized_url: web_policy::NormalizedWebUrl,
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/bears/{slug}/sessions", get(list_acp_sessions))
        .route("/bears/{slug}/sessions/{session_id}", get(get_acp_session))
        .route(
            "/bears/{slug}/sessions/{session_id}/runtime",
            get(get_acp_session_runtime),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/mode",
            post(set_session_mode),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/adapter-environment",
            post(post_adapter_environment),
        )
        .route("/bears/{slug}/sessions/{session_id}/prompt", post(prompt))
        .route(
            "/bears/{slug}/sessions/{session_id}/tool-results/{tool_call_id}",
            post(tool_result),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/permissions/{permission_id}",
            post(permission_result),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/close",
            post(close_session),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/cancel",
            post(cancel_session),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/compact",
            post(compact_session),
        )
        .route("/bears/{slug}/conversations", get(conversations))
        .route(
            "/bears/{slug}/conversations/{conversation_id}/history",
            get(conversation_history),
        )
        .route("/bears/{slug}/auth-check", get(auth_check))
}

#[derive(Debug, Deserialize)]
struct AcpSetModeRequest {
    mode: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    adapter_contract: Option<AdapterContract>,
}

#[derive(Debug, Serialize)]
struct AcpSetModeResponse {
    requested_mode: String,
    effective_mode: String,
    session_policy: serde_json::Value,
    workflow_state: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_mode: Option<serde_json::Value>,
    message: String,
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
    /// Adapter-local mode selected before Den has necessarily persisted the ACP
    /// session binding. Den treats this as initial intent for new sessions only;
    /// existing sessions continue to use Den's stored current_mode/plan state.
    #[serde(default)]
    pub requested_mode: Option<String>,
    #[serde(default)]
    pub adapter_contract: Option<AdapterContract>,
}

#[derive(Debug, Serialize)]
struct AcpToolResultResponse {
    accepted: bool,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    settlement: Option<String>,
    turn_id: Option<String>,
    tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic: Option<serde_json::Value>,
}

#[cfg(test)]
impl AcpToolResultResponse {
    fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("AcpToolResultResponse serializes")
    }
}

#[derive(Debug, Deserialize)]
struct AcpPermissionDecisionRequest {
    decision: String,
    #[serde(default)]
    plan_mode_id: Option<Uuid>,
    #[serde(default)]
    adapter_contract: Option<AdapterContract>,
}

#[derive(Debug, Deserialize)]
struct AcpAdapterEnvironmentRequest {
    environment: serde_json::Value,
    #[serde(default)]
    conversation_title: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    adapter_contract: Option<AdapterContract>,
}

#[derive(Debug, Serialize)]
struct AcpPermissionDecisionResponse {
    accepted: bool,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_tool_request: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct AcpCloseSessionResponse {
    ok: bool,
    archived: bool,
    conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    unwedged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_state: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct AcpErrorResponse {
    error: String,
    error_code: &'static str,
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    adapter_contract_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum_adapter_contract_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_adapter_contract_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum_adapter_contract_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggested_action: Option<&'static str>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdapterContract {
    name: String,
    version: u32,
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
    id: Option<String>,
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
pub(crate) struct AcpSessionHttp {
    acp_session_id: String,
    runtime_session_id: String,
    conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_conversation_id: Option<String>,
    client: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_title_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_title_synced_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    closed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_mode: Option<serde_json::Value>,
    session_policy: serde_json::Value,
    workflow_state: serde_json::Value,
}

fn format_acp_session_timestamp(t: time::OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

fn tools_enabled_for_client(client: &str) -> bool {
    let normalized = normalize_acp_client(Some(client));
    matches!(
        normalized.as_str(),
        "zed" | "cursor" | "vscode" | "windsurf"
    )
}

fn normalize_acp_requested_mode(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ask" => Some("ask"),
        "plan" => Some("plan"),
        "write" => Some("write"),
        _ => None,
    }
}

fn requested_mode_from_prompt(
    body: &AcpPromptRequest,
) -> Result<Option<&'static str>, CustomError> {
    let Some(raw) = body
        .requested_mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    normalize_acp_requested_mode(raw).map(Some).ok_or_else(|| {
        CustomError::ValidationError("requested_mode must be one of ask, plan, write".to_string())
    })
}

#[derive(Debug, Clone)]
pub(crate) struct AcpResolvedTurnContext {
    pub(crate) policy: crate::core::acp_tools::AcpResolvedSessionPolicy,
    pub(crate) workflow_state: serde_json::Value,
    pub(crate) effective_mode: String,
}

pub(crate) fn resolve_acp_turn_context(
    row: &acp_sessions::AcpSessionRow,
    plan_mode_row: Option<&crate::core::acp_plan_mode::AcpPlanModeSessionRow>,
    activity_plan: Option<&WorkPlanProjection>,
) -> AcpResolvedTurnContext {
    let policy = resolve_session_policy_for_mode(
        &row.current_mode,
        plan_mode_row.map(|value| value.state.as_str()),
    );
    let workflow_state = workflow_state_json_from_sources(&policy, plan_mode_row, activity_plan);
    let effective_mode = policy.mode_label.to_ascii_lowercase();
    AcpResolvedTurnContext {
        policy,
        workflow_state,
        effective_mode,
    }
}

pub(crate) fn acp_session_row_to_http_with_modes(
    row: acp_sessions::AcpSessionRow,
    plan_mode: Option<serde_json::Value>,
) -> AcpSessionHttp {
    let plan_mode_row = plan_mode
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let turn_context = resolve_acp_turn_context(&row, plan_mode_row.as_ref(), None);
    AcpSessionHttp {
        acp_session_id: row.acp_session_id,
        runtime_session_id: row.runtime_session_id,
        conversation_id: row.conversation_id,
        resolved_conversation_id: row.resolved_conversation_id,
        client: row.client,
        cwd: row.cwd,
        title: row.conversation_title,
        conversation_title_updated_at: row
            .conversation_title_updated_at
            .map(format_acp_session_timestamp),
        conversation_title_synced_at: row
            .conversation_title_synced_at
            .map(format_acp_session_timestamp),
        closed_at: row.closed_at.map(format_acp_session_timestamp),
        archived_at: row.archived_at.map(format_acp_session_timestamp),
        created_at: format_acp_session_timestamp(row.created_at),
        updated_at: format_acp_session_timestamp(row.updated_at),
        plan_mode,
        session_policy: turn_context.policy.to_json(),
        workflow_state: turn_context.workflow_state,
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

fn adapter_contract_from_value(value: &serde_json::Value) -> Option<AdapterContract> {
    serde_json::from_value(value.get("adapter_contract")?.clone()).ok()
}

fn check_adapter_contract(contract: Option<&AdapterContract>) -> Result<(), AcpCompatibilityError> {
    let Some(contract) = contract else {
        if BEARS_ACP_ADAPTER_CONTRACT_REQUIRED {
            return Err(AcpCompatibilityError::AdapterOutOfDate { version: 0 });
        }
        return Ok(());
    };
    if contract.name != BEARS_ACP_ADAPTER_CONTRACT_NAME {
        return Err(AcpCompatibilityError::AdapterOutOfDate {
            version: contract.version,
        });
    }
    if contract.version < BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED {
        return Err(AcpCompatibilityError::AdapterOutOfDate {
            version: contract.version,
        });
    }
    if contract.version > BEARS_ACP_ADAPTER_CONTRACT_MAX_SUPPORTED {
        return Err(AcpCompatibilityError::DenOutOfDate {
            version: contract.version,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum AcpCompatibilityError {
    AdapterOutOfDate { version: u32 },
    DenOutOfDate { version: u32 },
}

fn acp_compatibility_error_response(err: AcpCompatibilityError, request_id: Uuid) -> Response {
    let (status, error_code, error, suggested_action, adapter_version) = match err {
        AcpCompatibilityError::AdapterOutOfDate { version } => (
            StatusCode::UPGRADE_REQUIRED,
            "adapter_out_of_date",
            "The BEARS ACP adapter is older than this Den server.",
            "Update bears-acp-adapter and restart your ACP client.",
            version,
        ),
        AcpCompatibilityError::DenOutOfDate { version } => (
            StatusCode::CONFLICT,
            "den_out_of_date",
            "This BEARS Den server is older than the ACP adapter.",
            "Deploy the matching BEARS Den server or use an older adapter.",
            version,
        ),
    };
    tracing::warn!(
        %request_id,
        error_code,
        adapter_contract_version = adapter_version,
        minimum_adapter_contract_version = BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED,
        current_adapter_contract_version = BEARS_ACP_ADAPTER_CONTRACT_CURRENT,
        "ACP adapter contract mismatch"
    );
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    let body = serde_json::to_string(&AcpErrorResponse {
        error: error.to_string(),
        error_code,
        request_id: request_id.to_string(),
        adapter_contract_version: Some(adapter_version),
        minimum_adapter_contract_version: Some(BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED),
        current_adapter_contract_version: Some(BEARS_ACP_ADAPTER_CONTRACT_CURRENT),
        maximum_adapter_contract_version: Some(BEARS_ACP_ADAPTER_CONTRACT_MAX_SUPPORTED),
        suggested_action: Some(suggested_action),
    })
    .unwrap_or_else(|_| "{\"error\":\"response serialization failed\"}".to_string());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn compatibility_tool_result_body(
    err: &AcpCompatibilityError,
    tool_call_id: &str,
    mut original: AcpToolResultRequest,
) -> AcpToolResultRequest {
    let (status, message, phase) = match err {
        AcpCompatibilityError::AdapterOutOfDate { .. } => (
            "error",
            "The BEARS ACP adapter is older than this Den server. Update bears-acp-adapter and restart your ACP client.",
            "adapter_contract_out_of_date",
        ),
        AcpCompatibilityError::DenOutOfDate { .. } => (
            "error",
            "This BEARS Den server is older than the ACP adapter. Deploy the matching Den server or use an older adapter.",
            "den_contract_out_of_date",
        ),
    };
    original.tool_call_id = Some(tool_call_id.to_string());
    original.status = status.to_string();
    original.content = Some(message.to_string());
    original.structured_content = serde_json::json!({});
    original.diagnostic = serde_json::json!({
        "component": "den.acp",
        "phase": phase,
        "tool_call_id": tool_call_id,
        "minimum_adapter_contract_version": BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED,
        "current_adapter_contract_version": BEARS_ACP_ADAPTER_CONTRACT_CURRENT,
        "maximum_adapter_contract_version": BEARS_ACP_ADAPTER_CONTRACT_MAX_SUPPORTED,
    });
    original
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
        adapter_contract_version: None,
        minimum_adapter_contract_version: None,
        current_adapter_contract_version: None,
        maximum_adapter_contract_version: None,
        suggested_action: None,
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
        adapter_contract_version: None,
        minimum_adapter_contract_version: None,
        current_adapter_contract_version: None,
        maximum_adapter_contract_version: None,
        suggested_action: None,
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

fn is_acp_archive_target(conversation_id: &str) -> bool {
    conversation_id.starts_with("conv-")
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

pub(crate) fn acp_pair_den_tool_descriptors() -> serde_json::Value {
    let descriptors = den_tools::builtin_den_tool_descriptors_for_role(BearAgentRole::Pair)
        .into_iter()
        .filter(|descriptor| {
            matches!(
                descriptor.name,
                den_tools::DEN_CONVERSATION_SET_TITLE
                    | den_tools::DEN_WEB_FETCH
                    | den_tools::DEN_WEB_SEARCH
                    | den_tools::DEN_SITUATION_GET
                    | den_tools::DEN_MEMORY_WRITE_ENTRY
                    | den_tools::DEN_MEMORY_STATUS
                    | den_tools::DEN_MEMORY_TREE
                    | den_tools::DEN_MEMORY_READ
                    | den_tools::DEN_MEMORY_SEARCH
                    | den_tools::DEN_MEMORY_REQUEST_REVIEW
                    | den_tools::DEN_WORK_PLAN_LIST
                    | den_tools::DEN_WORK_PLAN_GET_STATUS
                    | den_tools::DEN_WORK_PLAN_UPDATE
                    | den_tools::DEN_WORK_PLAN_REQUEST_HANDOFF
            )
        })
        .map(|descriptor| {
            serde_json::json!({
                "name": descriptor.provider_name,
                "description": format!(
                    "Den server tool ({}). {}",
                    descriptor.name, descriptor.description
                ),
                "parameters": descriptor.input_schema,
                "x-bears-domain": descriptor.domain,
                "x-bears-content-class": descriptor.content_class,
                "x-bears-display": descriptor.display,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!(descriptors)
}

fn acp_den_provider_to_canonical_tool_name(provider_name: &str) -> Option<&'static str> {
    den_tools::builtin_den_tool_descriptor_for_provider_name(provider_name)
        .map(|descriptor| descriptor.name)
}

fn merge_acp_pair_tool_descriptors(client_tools: serde_json::Value) -> serde_json::Value {
    let mut merged = client_tools.as_array().cloned().unwrap_or_default();
    if let Some(server_tools) = acp_pair_den_tool_descriptors().as_array() {
        merged.extend(server_tools.iter().cloned());
    }
    serde_json::json!(merged)
}

fn looks_like_letta_waiting_for_approval_error(err: &CustomError) -> bool {
    let message = format!("{err:#}").to_ascii_lowercase();
    message.contains("waiting for approval")
        || message.contains("please approve or deny")
        || message.contains("requires_approval")
}

fn looks_like_letta_no_active_runs_error(err: &CustomError) -> bool {
    let message = format!("{err:#}").to_ascii_lowercase();
    message.contains("no active runs to cancel")
}

async fn cancel_letta_runs_by_id_or_skip(
    letta: &crate::core::letta::LettaClient,
    pair_agent_id: &str,
    run_ids: &[String],
    reason: &str,
) -> serde_json::Value {
    if run_ids.is_empty() {
        return serde_json::json!({
            "ok": true,
            "skipped": true,
            "attempted": false,
            "run_ids": run_ids,
            "reason": "no_run_ids",
            "requested_reason": reason,
            "message": "Skipped Letta run cancellation because no run IDs were known; refusing agent-wide cancel for concurrent ACP safety.",
        });
    }
    match letta.cancel_agent_runs(pair_agent_id, run_ids).await {
        Ok(value) => serde_json::json!({
            "ok": true,
            "skipped": false,
            "attempted": true,
            "run_ids": run_ids,
            "result": value,
        }),
        Err(err) if looks_like_letta_no_active_runs_error(&err) => serde_json::json!({
            "ok": true,
            "skipped": false,
            "attempted": true,
            "run_ids": run_ids,
            "result": "no_active_runs",
        }),
        Err(err) => serde_json::json!({
            "ok": false,
            "skipped": false,
            "attempted": true,
            "run_ids": run_ids,
            "error": err.to_string(),
        }),
    }
}

pub(crate) fn workflow_state_json(
    policy: &crate::core::acp_tools::AcpResolvedSessionPolicy,
) -> serde_json::Value {
    workflow_state_json_from_sources(policy, None, None)
}

pub(crate) fn workflow_state_json_with_activity(
    policy: &crate::core::acp_tools::AcpResolvedSessionPolicy,
    activity_plan: Option<&WorkPlanProjection>,
) -> serde_json::Value {
    workflow_state_json_from_sources(policy, None, activity_plan)
}

pub(crate) fn workflow_state_json_from_sources(
    policy: &crate::core::acp_tools::AcpResolvedSessionPolicy,
    workplan_row: Option<&crate::core::acp_plan_mode::AcpPlanModeSessionRow>,
    activity_plan: Option<&WorkPlanProjection>,
) -> serde_json::Value {
    turn_state::turn_state_from_sources(policy, workplan_row, activity_plan)
}

fn render_turn_state_summary_with_activity(
    session_id: &str,
    roots: &[String],
    local_tool_names: &[&str],
    den_tool_names: &[&str],
    policy: &crate::core::acp_tools::AcpResolvedSessionPolicy,
    activity_plan: Option<&WorkPlanProjection>,
) -> String {
    let execution_unlocked = policy.tool_enablement.enables_non_read_tools();
    let turn_state = workflow_state_json_with_activity(policy, activity_plan);
    let activity_status = turn_state["activity"]["status"]
        .as_str()
        .unwrap_or("inactive");
    let activity_plan_id = turn_state["activity"]["plan_id"].as_str().unwrap_or("none");
    let current_item = turn_state["activity"]["current_item"]["title"]
        .as_str()
        .unwrap_or("none");
    format!(
        "<system-reminder>AUTHORITATIVE WORKFLOW STATE for this turn: permission_mode=`{}`; tool_classes={}; workplan.state=`{}`; workplan.approval_status={}; activity.status=`{}`; activity.plan_id=`{}`; activity.current_item=`{}`; memory.active_plan_write_allowed=false; execution.execution_unlocked={}; state_authority=current turn capabilities override prior-turn assumptions. BEARS ACP direct local workspace tools available this turn: {}. Server tools available to pair: {}. Current ACP session id is `{}`. Use absolute paths under these workspace roots: {}.</system-reminder>",
        policy.mode_label,
        policy.allowed_tool_classes().join(", "),
        turn_state["workplan"]["state"].as_str().unwrap_or("inactive"),
        turn_state["workplan"]["approval_status"]
            .as_str()
            .unwrap_or("inactive"),
        activity_status,
        activity_plan_id,
        current_item,
        execution_unlocked,
        local_tool_names.join(", "),
        den_tool_names.join(", "),
        session_id,
        roots.join(", "),
    )
}

#[cfg(test)]
pub(crate) fn acp_direct_tool_prompt_context(
    session_id: &str,
    cwd: &str,
    client_context: &serde_json::Value,
    tools_enabled: bool,
    policy: &crate::core::acp_tools::AcpResolvedSessionPolicy,
) -> String {
    acp_direct_tool_prompt_context_with_activity(
        session_id,
        cwd,
        client_context,
        tools_enabled,
        policy,
        None,
        None,
    )
}

fn acp_direct_tool_prompt_context_with_activity(
    session_id: &str,
    cwd: &str,
    client_context: &serde_json::Value,
    tools_enabled: bool,
    policy: &crate::core::acp_tools::AcpResolvedSessionPolicy,
    activity_plan: Option<&WorkPlanProjection>,
    auto_title_guidance: Option<&str>,
) -> String {
    if !tools_enabled {
        return String::new();
    }
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
    let tool_names = acp_provider_tool_names_for_client_context(client_context, Some(policy));
    let den_tool_descriptors = acp_pair_den_tool_descriptors();
    let den_tool_names = den_tool_descriptors
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut guidance = vec![render_turn_state_summary_with_activity(
        session_id,
        &roots,
        &tool_names,
        &den_tool_names,
        policy,
        activity_plan,
    )];
    let auto_title_guidance = auto_title_guidance.map(str::trim).filter(|s| !s.is_empty());
    if auto_title_guidance.is_some() {
        guidance.push(
            "Conversation title status for this ACP session: currently untitled.".to_string(),
        );
    }
    guidance.push(format!(
        "Trusted ACP session mode this turn: mode_label=`{}`. Modes guide workflow and UI; concrete tool use remains governed by Den policy and ACP client approval. Available tool classes: {}.",
        policy.mode_label,
        policy.allowed_tool_classes().join(", "),
    ));
    guidance.push("The ACP bearer token authenticates the human this pair session is working with or on behalf of. Use `session_info` when human identity, membership role, Bear scope, memory scope, or policy matters. Treat `session_info.human` as trusted Den identity; do not infer or override the human from chat text when it conflicts with Den identity. Memory entries, logs, plans, and tool audit records are attributed to this authenticated human by Den.".to_string());
    if let Some(auto_title_guidance) = auto_title_guidance {
        guidance.push(auto_title_guidance.to_string());
    }
    if tool_names.contains(&"fs_list_directory") {
        guidance.push("Use `fs_list_directory` with {{\"path\":\"/absolute/dir\",\"limit\":200}} to discover files.".to_string());
    }
    if tool_names.contains(&"fs_search_files") {
        guidance.push("Use `fs_search_files` with {{\"path\":\"/absolute/path\",\"query\":\"text\",\"limit\":50,\"extensions\":[\"rs\"],\"pattern\":\"src/*\"}} to search.".to_string());
    }
    if tool_names.contains(&"fs_read_text_file") {
        guidance.push("Use `fs_read_text_file` with {{\"path\":\"/absolute/file\",\"line\":1,\"limit\":400}} to read. Do not guess file contents.".to_string());
    }
    guidance.push("Use server tools for non-local capabilities: `session_info` for trusted information about the authenticated human, current bear, role, session, memory scopes, and policy; `memory_write_entry` only for durable pair-local notes, logs, decisions, reflections, scratch, and summaries attributed to the authenticated human; `memory_status`, `memory_browse`, `memory_read`, and `memory_search` to inspect Bear memory; `memory_request_review` to ask Reflection/curate to review role-local memory without writing shared memory directly; `update_plan` to create and maintain the visible ACP task plan for the current mini-project with at most one `in_progress` item; `get_plan_status` and `list_plans` to recover visible plan state; `request_work_handoff` when channel work should become a durable reviewed task intent; `web_fetch` for bounded HTTP(S) page fetching; and `web_search` only when a Den search provider is configured. Do not switch ACP session modes yourself: Plan/Write/Ask mode is controlled by the user or ACP client UI. When planning would help, use `update_plan` and concise prose while remaining in the current mode; do not use memory entry tools for active plans, task lists, observations, run results, Cabinet writes, or direct core updates.".to_string());
    guidance.push("Memory is Bear-scoped across Workplaces and may contain multiple work surfaces. A Workplace is the role-scoped memory surface; for pair, that is the `pair` workplace. For questions about the current project, repo, service, architecture, terminology, or prior local decisions, first identify the relevant current work surface from trusted session hints, workspace roots, repo clues, or explicit user references rather than treating all Bear memory as one flat pool.".to_string());
    guidance.push("Prefer work-surface-first retrieval for local-understanding questions: current conversation and trusted session info -> current Workplace and current work-surface hints -> current work-surface canonical anchors -> current work-surface role-local working memory -> Bear-global shared anchors -> broader Bear memory search -> local workspace artifacts -> general world knowledge.".to_string());
    guidance.push("Use `memory_browse`, `memory_read`, and `memory_search` not only to recall prior notes, but to learn the current work surface within the current Workplace. If canonical work-surface anchors exist, prefer them over broad memory search for questions like 'what do you know about this?' or 'how does this work here?'.".to_string());
    guidance.push("Use `session_info.work_surface` as the trusted Den briefing for current Workplace/work-surface hints when available. Treat its reference candidates as guidance to resolve the active work surface, then confirm against canonical anchors and explicit user intent.".to_string());
    if tool_names.contains(&"fs_edit_file") {
        guidance.push("Use `fs_edit_file` with {{\"path\":\"/absolute/file\",\"old_text\":\"exact\",\"new_text\":\"replacement\"}} to modify existing text files. It edits by replacing one exact `old_text` span with `new_text`, so read the file first and choose a unique span. Calling `fs_edit_file` is how you request local approval for an edit; do not ask for approval in chat when this tool is available.".to_string());
        guidance.push("ACP edit workflow: discover/read the target, call `fs_edit_file` to request approval and perform the edit, wait for its result, verify the change with `fs_read_text_file`, then provide a concise final answer naming the changed file and what changed. Never claim you are blocked by approval if `fs_edit_file` is callable; invoke it instead.".to_string());
    } else {
        guidance.push("No ACP edit tool is callable in this turn. Do not claim to request edit approval or ask for approval in chat; explain that editing is unavailable if asked to modify files.".to_string());
    }
    if tool_names.contains(&"fs_create_text_file") {
        guidance.push("Use `fs_create_text_file` with {{\"path\":\"/absolute/new-file.txt\",\"content\":\"text\"}} to create new UTF-8 text files. It will not overwrite existing files; use `create_parent_dirs:true` only when parent directories should be created.".to_string());
    }
    if tool_names.contains(&"fs_delete_path") {
        guidance.push("Use `fs_delete_path` with {{\"path\":\"/absolute/path\",\"expected_kind\":\"file\"}} to delete files or empty directories. For non-empty directories, `recursive:true` is required. Deleting workspace roots and sensitive paths is denied.".to_string());
    }
    guidance.push("Tool-loop rule: after any ACP tool result, continue from the returned content until the user's original request is complete. Do not stop merely because a tool succeeded. Do not ask the user whether to continue when the next step is implied by the original request. Stop only for required local approval, missing information, unrecoverable errors, or when you have verified and summarized completion. Never write textual tool-call syntax such as `to=functions...` or `functions.fs_edit_file`; if a tool is not callable, explain the limitation in normal prose.".to_string());
    format!(
        "\n\n<system-reminder>{}</system-reminder>",
        guidance.join(" ")
    )
}

async fn acp_plan_mode_prompt_context(
    state: &ApiState,
    bear_id: Uuid,
    user_id: i32,
    session_id: &str,
) -> Result<String, CustomError> {
    let plan_mode =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear_id, session_id).await?;
    let Some(plan_mode) = plan_mode else {
        return Ok(String::new());
    };
    let submitted_plan_present = plan_mode.plan_artifact_path.is_some();
    let approval_status = if plan_mode.state == "approved" {
        "approved_execution_unlocked"
    } else if plan_mode.state == "submitted" {
        "awaiting_human_approval"
    } else {
        plan_mode.state.as_str()
    };
    let execution_unlocked = plan_mode.state == "approved";
    Ok(format!(
        "\n\n<system-reminder>ACP workflow state for this session: workflow_id={} workflow_state={} submitted_plan_present={} approval_status={} execution_unlocked={}. Workflow state is authoritative; artifact path is audit context only. Plan mode is controlled by the user or ACP client UI, not by model tool calls. Keep planning visible with `update_plan` and concise prose. If implementation is requested but write tools are not callable this turn, explain that the user can switch the session to Write mode. Artifact path remains available for audit when needed: {}.</system-reminder>",
        plan_mode.id,
        plan_mode.state,
        submitted_plan_present,
        approval_status,
        execution_unlocked,
        plan_mode
            .plan_artifact_path
            .as_deref()
            .unwrap_or("not_submitted")
    ))
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

fn letta_messages_top_array(v: &serde_json::Value) -> &[serde_json::Value] {
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

fn map_acp_history_page(
    body: &serde_json::Value,
    page_limit: u32,
) -> (Vec<AcpConversationHistoryMessage>, bool, Option<String>) {
    let raw = letta_messages_top_array(body);
    let has_more = raw.len() >= page_limit as usize;
    // Letta history is requested with `order_by=created_at_desc`; the last raw row is the
    // oldest item in this page and therefore the correct `before` cursor for the next page.
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
        if message_type == "user_message" && !letta_user_message_role_is_human(inner, msg) {
            continue;
        }
        let Some(text) = letta_message_text(inner).or_else(|| letta_message_text(msg)) else {
            continue;
        };
        let text = sanitize_visible_transcript_text(&text);
        if text.trim().is_empty() {
            continue;
        }
        rows.push(AcpConversationHistoryMessage {
            id: letta_message_id_string(msg),
            role: role.to_string(),
            text,
            created_at: letta_message_created_at(msg),
        });
    }
    (rows, has_more, next_before)
}

async fn pending_session_title_update_event(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    bear_slug: &str,
    acp_session_id: &str,
) -> Result<Option<AcpGatewayEvent>, CustomError> {
    let Some(session) =
        acp_sessions::find_for_user_bear_session(pool, user_id, bear_slug, acp_session_id).await?
    else {
        return Ok(None);
    };
    if let Some(event) = session_title_update_event_from_row(&session) {
        acp_sessions::mark_title_synced(pool, user_id, bear_id, acp_session_id).await?;
        Ok(Some(event))
    } else {
        Ok(None)
    }
}

fn acp_auto_title_instruction(session: &acp_sessions::AcpSessionRow) -> Option<String> {
    let has_title = session
        .conversation_title
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if has_title {
        return None;
    }
    let has_conversation_binding = session
        .resolved_conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        || session.conversation_id.trim().starts_with("conv-")
        || session.conversation_id.trim().starts_with("new-");
    if !has_conversation_binding {
        return None;
    }
    Some(
        "This conversation is currently untitled. Once the main subject is clear enough to summarize in a short, specific title, proactively call `set_conversation_title` in that turn without waiting for the user to ask. Prefer doing this before or alongside your normal response when the topic first becomes clear. Do not title vague openings such as greetings when the subject is not yet clear, and do not automatically rename again after a title has been set unless the human asks for a rename or the existing title is clearly wrong.".to_string(),
    )
}

fn session_title_update_event_from_row(
    session: &acp_sessions::AcpSessionRow,
) -> Option<AcpGatewayEvent> {
    let title = session
        .conversation_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)?;
    let needs_sync = match (
        session.conversation_title_updated_at,
        session.conversation_title_synced_at,
    ) {
        (Some(updated), Some(synced)) => synced < updated,
        (Some(_), None) => true,
        _ => false,
    };
    needs_sync.then_some(AcpGatewayEvent::SessionInfoUpdate {
        title: Some(title),
        updated_at: session
            .conversation_title_updated_at
            .map(format_acp_session_timestamp),
        meta: None,
    })
}

async fn prompt(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpPromptRequest>,
) -> impl IntoResponse {
    let request_id = Uuid::new_v4();
    if let Err(err) = check_adapter_contract(body.adapter_contract.as_ref()) {
        return acp_compatibility_error_response(err, request_id);
    }
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
    Ok(authenticate_acp_code_token_with_auth(state, &token, slug)
        .await?
        .user_id)
}

async fn authenticate_acp_code_token_with_auth(
    state: &ApiState,
    token: &str,
    slug: &str,
) -> Result<acp_tokens::AcpTokenAuth, CustomError> {
    if !acp_tokens::is_acp_token(token) {
        return Err(CustomError::Authentication(
            "expected a bear-scoped BEARS ACP token".to_string(),
        ));
    }
    let auth = acp_tokens::authenticate_for_bear_slug_with_scopes(&state.sqlx_pool, token, slug)
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
    Ok(auth)
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
        acp_sessions::SessionListParams {
            user_id,
            bear_slug: &bear.slug,
            include_closed: query.include_closed,
            cwd_filter,
            limit: fetch_limit,
            cursor_updated_at: cursor.as_ref().map(|c| c.updated_at),
            cursor_id: cursor.as_ref().map(|c| c.id),
        },
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
        let plan_mode = acp_plan_mode::active_for_session(
            &state.sqlx_pool,
            user_id,
            bear.id,
            &row.acp_session_id,
        )
        .await?
        .map(serde_json::to_value)
        .transpose()?;
        sessions.push(acp_session_row_to_http_with_modes(row, plan_mode));
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

async fn get_acp_session_runtime(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match get_acp_session_runtime_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn get_acp_session_runtime_inner(
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
    let plan_mode =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id).await?;
    let activity_plan = work_plans::get_visible_work_plan(
        &state.sqlx_pool,
        bear.id,
        BearAgentRole::Pair,
        user_id,
        WorkPlanLookup {
            plan_id: None,
            source_conversation_id: row.resolved_conversation_id.clone().or_else(|| {
                let conversation_id = row.conversation_id.trim();
                conversation_id
                    .starts_with("conv-")
                    .then(|| conversation_id.to_string())
            }),
            source_acp_session_id: Some(session_id.to_string()),
        },
    )
    .await?;
    let turn_context = resolve_acp_turn_context(&row, plan_mode.as_ref(), activity_plan.as_ref());
    let role_scope = RoleTurnScope::acp_pair(
        bear.id,
        session_id.to_string(),
        row.resolved_conversation_id.clone(),
    );
    let role_runtime = RoleRuntime::with_turn_cancellations(
        state.acp_tool_turns.clone(),
        state.acp_turn_cancellations.clone(),
    );
    let runtime = role_runtime.tool_turn_runtime_snapshot(session_id, &state.acp_tool_turns);
    let active_turn = state
        .acp_tool_turns
        .active_turn_for_session(session_id)
        .map(|turn| turn.diagnostic());
    let stream_turn = state
        .acp_turn_cancellations
        .active_for_session(session_id)
        .map(|turn| {
            serde_json::json!({
                "acp_session_id": turn.acp_session_id,
                "request_id": turn.request_id,
                "conversation_id": turn.conversation_id,
                "run_ids": turn.run_ids,
            })
        });
    let pending = state
        .acp_tool_turns
        .pending_for_session(session_id)
        .into_iter()
        .map(|turn| turn.diagnostic())
        .collect::<Vec<_>>();
    let expired = state
        .acp_tool_turns
        .expired_pending_for_session(session_id)
        .into_iter()
        .map(|turn| turn.diagnostic())
        .collect::<Vec<_>>();
    let adapter_environment = if tools_enabled_for_client(&row.client) {
        row.adapter_environment.unwrap_or_else(|| {
            json!({
                "status": "unavailable",
                "note": "ACP adapter has not published an environment snapshot for this session yet.",
            })
        })
    } else {
        serde_json::json!({ "status": "not_applicable" })
    };
    Ok(Json(serde_json::json!({
        "ok": true,
        "bear_id": bear.id,
        "role": "pair",
        "channel_kind": "acp_session",
        "acp_session_id": session_id,
        "title": row.conversation_title,
        "conversation_title_updated_at": row
            .conversation_title_updated_at
            .map(format_acp_session_timestamp),
        "conversation_title_synced_at": row
            .conversation_title_synced_at
            .map(format_acp_session_timestamp),
        "conversation": {
            "session_selection": row.conversation_id,
            "resolved_conversation_id": row.resolved_conversation_id,
            "upstream_target": row.resolved_conversation_id
                .as_deref()
                .or_else(|| row.conversation_id.starts_with("conv-").then_some(row.conversation_id.as_str()))
                .unwrap_or("unresolved"),
        },
        "active_turn": {
            "active": active_turn.is_some(),
            "turn": active_turn,
        },
        "stream_turn": {
            "active": stream_turn.is_some(),
            "turn": stream_turn,
        },
        "pending_tools": pending,
        "expired_tools": expired,
        "tool_turns": role_runtime.pending_diagnostics(&role_scope),
        "runtime": runtime,
        "adapter_environment": adapter_environment,
        "context_budget": default_unavailable_context_budget(),
        "turn_state": turn_context.workflow_state,
        "session_policy": turn_context.policy.to_json(),
        "activity": activity_plan,
        "plan_mode": plan_mode,
    }))
    .into_response())
}

async fn set_session_mode(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpSetModeRequest>,
) -> Response {
    let request_id = Uuid::new_v4();
    match set_session_mode_inner(state, slug, session_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn post_adapter_environment(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpAdapterEnvironmentRequest>,
) -> Response {
    let request_id = Uuid::new_v4();
    match post_adapter_environment_inner(state, slug, session_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn post_adapter_environment_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
    body: AcpAdapterEnvironmentRequest,
) -> Result<Response, CustomError> {
    let token = auth::extract_bearer_token(&headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    let auth = authenticate_acp_code_token_with_auth(&state, &token, &slug).await?;
    if !acp_tokens::scopes_contains(&auth.scopes, acp_tokens::acp_tools_scope()) {
        return Err(CustomError::Authorization(
            "ACP token is missing required acp:tools scope".to_string(),
        ));
    }
    let session = acp_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        auth.user_id,
        &slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?;
    acp_sessions::update_adapter_environment(
        &state.sqlx_pool,
        auth.user_id,
        session.bear_id,
        &session_id,
        &body.environment,
    )
    .await?;
    let client_title = body
        .conversation_title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            body.environment
                .get("thread_title")
                .or_else(|| body.environment.get("conversation_title"))
                .or_else(|| body.environment.get("title"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    if client_title.is_some() {
        acp_sessions::update_client_conversation_title(
            &state.sqlx_pool,
            auth.user_id,
            session.bear_id,
            &session_id,
            client_title,
        )
        .await?;
    }
    Ok(Json(serde_json::json!({
        "accepted": true,
        "reason": "stored",
    }))
    .into_response())
}

async fn set_session_mode_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
    body: AcpSetModeRequest,
) -> Result<Response, CustomError> {
    if let Err(err) = check_adapter_contract(body.adapter_contract.as_ref()) {
        return Ok(acp_compatibility_error_response(err, Uuid::new_v4()));
    }
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
    let requested_mode = body.mode.trim().to_ascii_lowercase();
    if !matches!(requested_mode.as_str(), "ask" | "plan" | "write") {
        return Err(CustomError::ValidationError(
            "mode must be one of ask, plan, write".to_string(),
        ));
    }
    let existing =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await?;
    let Some(_existing) = existing else {
        return Err(CustomError::NotFound("ACP session not found".to_string()));
    };

    let reason = body
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("User selected ACP session mode");

    let effective_mode;
    let message;
    match requested_mode.as_str() {
        "plan" => {
            acp_plan_mode::enter_plan_mode(
                &state.sqlx_pool,
                acp_plan_mode::EnterPlanModeParams {
                    user_id,
                    bear_id: bear.id,
                    bear_slug: bear.slug.clone(),
                    acp_session_id: session_id.to_string(),
                    reason: reason.to_string(),
                    requested_by: acp_plan_mode::AcpPlanModeRequestedBy::User,
                    previous_permission_mode: Some("ask".to_string()),
                },
            )
            .await?;
            acp_sessions::set_current_mode(&state.sqlx_pool, user_id, bear.id, session_id, "plan")
                .await?;
            effective_mode = "plan".to_string();
            message = "Plan mode entered. Planning is active; concrete tool use remains governed by Den policy and ACP client approval.".to_string();
        }
        "ask" => {
            if let Some(active) =
                acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id)
                    .await?
            {
                acp_plan_mode::cancel_plan_mode(
                    &state.sqlx_pool,
                    user_id,
                    bear.id,
                    session_id,
                    Some(active.id),
                )
                .await?;
                message = "Plan mode cancelled; returned to Ask.".to_string();
            } else {
                message = "Returned to Ask according to Den session policy.".to_string();
            }
            acp_sessions::set_current_mode(&state.sqlx_pool, user_id, bear.id, session_id, "ask")
                .await?;
            effective_mode = "ask".to_string();
        }
        "write" => {
            let active_plan =
                acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id)
                    .await?;
            if let Some(active) = active_plan.as_ref() {
                match active.state.as_str() {
                    "submitted" => {
                        acp_plan_mode::approve_plan_mode(
                            &state.sqlx_pool,
                            user_id,
                            bear.id,
                            session_id,
                            active.id,
                        )
                        .await?;
                        message = "Write mode enabled by user request; the submitted plan was approved by the authenticated ACP human.".to_string();
                    }
                    "active" => {
                        acp_plan_mode::cancel_plan_mode(
                            &state.sqlx_pool,
                            user_id,
                            bear.id,
                            session_id,
                            Some(active.id),
                        )
                        .await?;
                        message = "Write mode enabled by user request; the unsubmitted plan draft was closed so the mode change could take effect.".to_string();
                    }
                    _ => {
                        message = "Write mode enabled by user request. Concrete tool use remains subject to Den policy and ACP client approval.".to_string();
                    }
                }
            } else {
                message = "Write mode enabled by user request. Concrete tool use remains subject to Den policy and ACP client approval.".to_string();
            }
            acp_sessions::set_current_mode(&state.sqlx_pool, user_id, bear.id, session_id, "write")
                .await?;
            effective_mode = "write".to_string();
            tracing::info!(
                bear_id = %bear.id,
                acp_session_id = %session_id,
                requested_mode = %requested_mode,
                effective_mode = %effective_mode,
                active_plan_id = ?active_plan.as_ref().map(|plan| plan.id),
                active_plan_state = ?active_plan.as_ref().map(|plan| plan.state.as_str()),
                "ACP session mode changed to write by authenticated user request"
            );
        }
        _ => unreachable!(),
    }

    let plan_mode_row =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id).await?;
    let plan_mode = plan_mode_row
        .clone()
        .map(serde_json::to_value)
        .transpose()?;
    let synthetic_row = acp_sessions::AcpSessionRow {
        current_mode: effective_mode.clone(),
        ..acp_sessions::find_for_user_bear_session(
            &state.sqlx_pool,
            user_id,
            &bear.slug,
            session_id,
        )
        .await?
        .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?
    };
    let turn_context = resolve_acp_turn_context(&synthetic_row, plan_mode_row.as_ref(), None);
    Ok(Json(AcpSetModeResponse {
        requested_mode,
        effective_mode: turn_context.effective_mode,
        session_policy: turn_context.policy.to_json(),
        workflow_state: turn_context.workflow_state,
        plan_mode,
        message,
    })
    .into_response())
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
    let plan_mode =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id).await?;
    let approval_fallback = plan_mode
        .as_ref()
        .filter(|plan| plan.state == "submitted")
        .map(plan_approval_fallback_payload);
    let mut response = serde_json::to_value(acp_session_row_to_http_with_modes(
        row,
        plan_mode.map(serde_json::to_value).transpose()?,
    ))?;
    if let Some(approval_fallback) = approval_fallback {
        response["approval_fallback"] = approval_fallback;
    }
    Ok(Json(response).into_response())
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
    let runtime_binding =
        require_pair_runtime_binding(&state.sqlx_pool, state.letta.as_ref(), &bear).await?;
    let agent_id = runtime_binding.binding_id;

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
    let runtime_binding =
        require_pair_runtime_binding(&state.sqlx_pool, state.letta.as_ref(), &bear).await?;
    let agent_id = runtime_binding.binding_id.clone();
    let conv_id = normalize_acp_conversation_id(Some(&conversation_id))?;
    if conv_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "history is only available for default or saved conv- conversations".to_string(),
        ));
    }
    if conv_id.starts_with("conv-") {
        verify_acp_conversation_belongs_to_binding(
            state.letta.as_ref(),
            &runtime_binding,
            &conv_id,
        )
        .await?;
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

async fn permission_result(
    State(state): State<ApiState>,
    Path((slug, session_id, permission_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpPermissionDecisionRequest>,
) -> Response {
    let request_id = Uuid::new_v4();
    if let Err(err) = check_adapter_contract(body.adapter_contract.as_ref()) {
        return acp_compatibility_error_response(err, request_id);
    }
    match permission_result_inner(state, slug, session_id, permission_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

async fn permission_result_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    permission_id: String,
    headers: HeaderMap,
    body: AcpPermissionDecisionRequest,
) -> Result<Response, CustomError> {
    let token = auth::extract_bearer_token(&headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    let auth = authenticate_acp_code_token_with_auth(&state, &token, &slug).await?;
    if !acp_tokens::scopes_contains(&auth.scopes, acp_tokens::acp_tools_scope()) {
        return Err(CustomError::Authorization(
            "ACP token is missing required acp:tools scope".to_string(),
        ));
    }
    if let Some(plan_mode_id) = body.plan_mode_id.or_else(|| {
        permission_id
            .strip_prefix("plan-mode-")
            .and_then(|raw| Uuid::parse_str(raw).ok())
    }) {
        let session = acp_sessions::find_for_user_bear_session(
            &state.sqlx_pool,
            auth.user_id,
            &slug,
            &session_id,
        )
        .await?
        .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?;
        let decision = body.decision.trim().to_ascii_lowercase();
        if decision == "timeout" {
            acp_sessions::set_current_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                "plan",
            )
            .await?;
            let row = acp_plan_mode::get_by_id_for_bear(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                plan_mode_id,
            )
            .await?;
            let policy = resolve_session_policy_for_mode("plan", Some("submitted"));
            return Ok(Json(serde_json::json!({
                "accepted": true,
                "reason": "plan_mode_approval_request_timed_out",
                "local_tool_request": serde_json::Value::Null,
                "effective_mode": "plan",
                "session_policy": policy.to_json(),
                "plan_mode": row,
                "workflow_state": row.as_ref().map(|plan| workflow_state_json_from_sources(&policy, Some(plan), None)).unwrap_or_else(|| workflow_state_json(&policy)),
                "approval_fallback": row.as_ref().filter(|plan| plan.state == "submitted").map(plan_approval_fallback_payload),
                "message": "The transient ACP approval request timed out, but the submitted plan remains pending. The user may approve it through Den UI, ACP client mode controls, or a new ACP approval request."
            }))
            .into_response());
        }
        let row = if matches!(
            decision.as_str(),
            "approve" | "approved" | "allow" | "allow_once"
        ) {
            acp_plan_mode::approve_plan_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                plan_mode_id,
            )
            .await?
        } else {
            acp_plan_mode::reject_plan_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                plan_mode_id,
            )
            .await?
        };
        let effective_mode = if row.state == "approved" {
            acp_sessions::set_current_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                "write",
            )
            .await?;
            "write"
        } else {
            acp_sessions::set_current_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                "plan",
            )
            .await?;
            "plan"
        };
        let policy = resolve_session_policy_for_mode(effective_mode, Some(row.state.as_str()));
        return Ok(Json(serde_json::json!({
            "accepted": true,
            "reason": format!("plan_mode_{}", row.state),
            "local_tool_request": serde_json::Value::Null,
            "effective_mode": effective_mode,
            "session_policy": policy.to_json(),
            "plan_mode": row,
            "workflow_state": workflow_state_json_from_sources(&policy, Some(&row), None),
            "approval_fallback": if row.state == "submitted" { Some(plan_approval_fallback_payload(&row)) } else { None },
        }))
        .into_response());
    }

    let pending = pending_web_fetch_approvals()
        .lock()
        .await
        .remove(&permission_id)
        .ok_or_else(|| CustomError::NotFound("pending permission request not found".to_string()))?;
    if pending.user_id != auth.user_id
        || pending.context.bear_slug != slug
        || pending.context.acp_session_id != session_id
    {
        return Err(CustomError::Authorization(
            "permission request does not belong to this ACP session".to_string(),
        ));
    }
    let decision = body.decision.as_str();
    if matches!(decision, "allow_url" | "allow_host") {
        let scope_kind = if decision == "allow_url" {
            "url"
        } else {
            "host"
        };
        let scope_value = if scope_kind == "url" {
            pending.normalized_url.url.clone()
        } else {
            pending.normalized_url.host.clone()
        };
        web_policy::record_web_approval(
            &state.sqlx_pool,
            pending.bear_id,
            scope_kind,
            &scope_value,
            Some(auth.user_id),
            "acp",
            None,
        )
        .await?;
    }
    if matches!(decision, "allow_once" | "allow_url" | "allow_host")
        && web_policy::is_local_web_url(&pending.normalized_url)
    {
        pending
            .context
            .tool_turns
            .register(AcpToolTurnRegistration {
                user_id: pending.user_id,
                bear_id: pending.bear_id,
                bear_slug: pending.context.bear_slug.clone(),
                acp_session_id: pending.context.acp_session_id.clone(),
                request_id: pending.context.request_id,
                tool_call_id: pending.tool_call_id.clone(),
                // Register the original Den/model-facing provider name. The adapter
                // executes `local_web_fetch`, but the result must settle the original
                // Letta `web_fetch` tool call and pass coordinator name validation.
                tool_name: pending.provider_name.clone(),
                approval_request_id: pending.approval_request_id.clone(),
                timeout_ms: acp_tool_policy_json_for_provider(&pending.provider_name)
                    .get("tool_timeout_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(30_000),
                result_tx: pending.result_tx,
            })?;
        return Ok(Json(AcpPermissionDecisionResponse {
            accepted: true,
            reason: "local_tool_required".to_string(),
            local_tool_request: Some(serde_json::json!({
                "tool_call_id": pending.tool_call_id,
                "tool_name": "local_web_fetch",
                "result_tool_name": pending.provider_name,
                "args": { "url": pending.normalized_url.url },
                "policy": { "max_bytes": 262144, "total_timeout_ms": 120000 }
            })),
        })
        .into_response());
    }
    let result = if matches!(decision, "allow_once" | "allow_url" | "allow_host") {
        invoke_acp_den_tool(
            &pending.context,
            den_tools::DEN_WEB_FETCH,
            &pending.provider_name,
            &pending.tool_call_id,
            pending.approval_request_id.as_deref(),
            pending.args,
        )
        .await
    } else {
        AcpToolResultRequest {
            turn_id: None,
            request_id: Some(pending.context.request_id.to_string()),
            tool_call_id: Some(pending.tool_call_id.clone()),
            tool_name: Some(pending.provider_name.clone()),
            approval_request_id: pending.approval_request_id.clone(),
            status: "permission_denied".to_string(),
            content: Some(if decision == "timeout" {
                "web_fetch permission timed out".to_string()
            } else {
                "web_fetch permission denied".to_string()
            }),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({ "component": "den.acp", "phase": "web_fetch_permission_denied" }),
            ..Default::default()
        }
    };
    let _ = pending.result_tx.send(result);
    Ok(Json(AcpPermissionDecisionResponse {
        accepted: true,
        reason: "delivered".to_string(),
        local_tool_request: None,
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
    let contract = body
        .adapter_contract
        .as_ref()
        .and_then(adapter_contract_from_value);
    if let Err(err) = check_adapter_contract(contract.as_ref()) {
        let token = match auth::extract_bearer_token(&headers) {
            Ok(token) => token,
            Err(auth_err) => return api_auth_error_response(auth_err, request_id),
        };
        if let Ok(auth) = authenticate_acp_code_token_with_auth(&state, &token, &slug).await {
            let synthetic = compatibility_tool_result_body(&err, &tool_call_id, body);
            let _ = state.acp_tool_turns.deliver_result(
                auth.user_id,
                &slug,
                &session_id,
                &tool_call_id,
                synthetic,
            );
        }
        return acp_compatibility_error_response(err, request_id);
    }
    match tool_result_inner(state, slug, session_id, tool_call_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

fn default_unavailable_context_budget() -> serde_json::Value {
    serde_json::json!({
        "status": "unavailable",
        "reason": "Letta/provider context usage data is not wired into Den session_info yet",
        "source": "den.acp",
    })
}

fn late_result_settlement_from_status(status: &str) -> &'static str {
    match status {
        "timeout" => "timed_out",
        "cancelled" => "cancelled",
        "ok" | "error" | "unsupported" => "already_settled",
        _ => "unknown",
    }
}

fn acp_tool_result_response_from_delivery(
    delivery: AcpToolResultDelivery,
    session_id: &str,
    tool_call_id_param: String,
    parsed_status: AcpToolStatus,
    tool_turns: &AcpToolTurnCoordinator,
) -> AcpToolResultResponse {
    match delivery {
        AcpToolResultDelivery::Delivered { body, .. } => AcpToolResultResponse {
            accepted: true,
            reason: "delivered".to_string(),
            settlement: None,
            turn_id: body.turn_id,
            tool_call_id: tool_call_id_param,
            diagnostic: Some(serde_json::json!({
                "component": "den.acp",
                "phase": acp_diag_phase::DEN_RESULT_DELIVERED,
                "status": parsed_status.as_str(),
            })),
        },
        AcpToolResultDelivery::TurnMissing {
            turn_id,
            tool_call_id,
        } => AcpToolResultResponse {
            accepted: false,
            reason: "late_result_ignored".to_string(),
            settlement: Some("unknown".to_string()),
            turn_id,
            tool_call_id,
            diagnostic: Some(serde_json::json!({
                "component": "den.acp",
                "phase": "late_tool_result_ignored",
            })),
        },
        AcpToolResultDelivery::AlreadySettled {
            turn_id,
            tool_call_id,
        } => AcpToolResultResponse {
            accepted: false,
            reason: "late_result_ignored".to_string(),
            settlement: Some("already_settled".to_string()),
            turn_id,
            tool_call_id: tool_call_id.clone(),
            diagnostic: tool_turns
                .recently_settled(session_id, &tool_call_id)
                .map(|cached| cached.diagnostic()),
        },
        AcpToolResultDelivery::RecentlySettled {
            turn_id,
            tool_call_id,
            cached,
        } => AcpToolResultResponse {
            accepted: false,
            reason: "late_result_ignored".to_string(),
            settlement: Some(late_result_settlement_from_status(&cached.status).to_string()),
            turn_id,
            tool_call_id,
            diagnostic: Some(cached.diagnostic()),
        },
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
    let token = auth::extract_bearer_token(&headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    let auth = authenticate_acp_code_token_with_auth(&state, &token, &slug).await?;
    if !acp_tokens::scopes_contains(&auth.scopes, acp_tokens::acp_tools_scope()) {
        return Err(CustomError::Authorization(
            "ACP token is missing required acp:tools scope".to_string(),
        ));
    }
    let user_id = auth.user_id;
    let parsed_status = AcpToolStatus::parse(&body.status).ok_or_else(|| {
        CustomError::ValidationError(format!("invalid ACP tool result status: {}", body.status))
    })?;
    let delivery =
        state
            .acp_tool_turns
            .deliver_result(user_id, &slug, &session_id, &tool_call_id, body)?;
    match delivery {
        AcpToolResultDelivery::Delivered {
            mut body,
            request_id,
            bear_id,
            tool_name,
        } => {
            body.status = parsed_status.as_str().to_string();
            tracing::info!(
                request_id = %request_id,
                bear_id = %bear_id,
                acp_session_id = %session_id,
                tool_call_id = %tool_call_id,
                tool_name = %tool_name,
                body_request_id = body.request_id.as_deref(),
                body_tool_call_id = body.tool_call_id.as_deref(),
                body_approval_request_id = body.approval_request_id.as_deref(),
                status = %parsed_status.as_str(),
                content_bytes = body.content.as_deref().map(str::len).unwrap_or(0),
                structured_content_bytes = body.structured_content.to_string().len(),
                diagnostic = ?body.diagnostic,
                phase = acp_diag_phase::DEN_RESULT_DELIVERED,
                "ACP tool result received"
            );
            Ok(Json(acp_tool_result_response_from_delivery(
                AcpToolResultDelivery::Delivered {
                    body,
                    request_id,
                    bear_id,
                    tool_name,
                },
                &session_id,
                tool_call_id,
                parsed_status,
                &state.acp_tool_turns,
            ))
            .into_response())
        }
        delivery @ (AcpToolResultDelivery::TurnMissing { .. }
        | AcpToolResultDelivery::AlreadySettled { .. }
        | AcpToolResultDelivery::RecentlySettled { .. }) => {
            Ok(Json(acp_tool_result_response_from_delivery(
                delivery,
                &session_id,
                tool_call_id,
                parsed_status,
                &state.acp_tool_turns,
            ))
            .into_response())
        }
    }
}

async fn compact_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let response_request_id = Uuid::new_v4();
    let contract = adapter_contract_from_value(&body);
    if let Err(err) = check_adapter_contract(contract.as_ref()) {
        return acp_compatibility_error_response(err, response_request_id);
    }
    match compact_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

async fn compact_session_inner(
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
        return Err(CustomError::NotFound("ACP session not found".to_string()));
    };
    let conversation_id = session
        .resolved_conversation_id
        .as_deref()
        .or_else(|| {
            let selection = session.conversation_id.trim();
            selection.starts_with("conv-").then_some(selection)
        })
        .ok_or_else(|| {
            CustomError::ValidationError(
                "ACP session has no resolved Letta conversation to compact".to_string(),
            )
        })?;
    let compact_result = state.letta.compact_conversation(conversation_id).await?;
    tracing::warn!(
        acp_session_id = %session_id,
        bear_id = %session.bear_id,
        conversation_id,
        "ACP session compact requested; no stale approval recovery attempted because compaction does not resolve pending Letta approvals"
    );
    Ok(Json(serde_json::json!({
        "ok": true,
        "compacted": true,
        "acp_session_id": session_id,
        "conversation_id": conversation_id,
        "approval_recovery": {
            "attempted": false,
            "reason": "compaction_only"
        },
        "compact_result": compact_result,
    }))
    .into_response())
}

async fn close_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let response_request_id = Uuid::new_v4();
    let contract = body
        .as_ref()
        .and_then(|Json(value)| adapter_contract_from_value(value));
    if let Err(err) = check_adapter_contract(contract.as_ref()) {
        return acp_compatibility_error_response(err, response_request_id);
    }
    match close_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

async fn cancel_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let response_request_id = Uuid::new_v4();
    let contract = body
        .as_ref()
        .and_then(|Json(value)| adapter_contract_from_value(value));
    if let Err(err) = check_adapter_contract(contract.as_ref()) {
        return acp_compatibility_error_response(err, response_request_id);
    }
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
        return Ok(Json(serde_json::json!({
            "ok": true,
            "cancelled": false,
            "stream_turn": serde_json::Value::Null,
        }))
        .into_response());
    };
    let stream_cancel = state
        .acp_turn_cancellations
        .cancel_session(&session.acp_session_id);
    let active = state
        .acp_tool_turns
        .cancel_active_turn(&session.acp_session_id);
    let pair_agent_id =
        bears_db::role_agent_id(&state.sqlx_pool, session.bear_id, BearAgentRole::Pair)
            .await?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    let run_ids = stream_cancel
        .as_ref()
        .map(|turn| turn.run_ids.clone())
        .unwrap_or_default();
    if stream_cancel.is_some() && run_ids.is_empty() {
        tracing::warn!(
            acp_session_id = %session.acp_session_id,
            bear_id = %session.bear_id,
            conversation_id = %session.conversation_id,
            active_request_id = ?stream_cancel.as_ref().map(|turn| turn.request_id),
            active_conversation_id = ?stream_cancel.as_ref().and_then(|turn| turn.conversation_id.clone()),
            pair_agent_id = ?pair_agent_id,
            "ACP cancel found an active stream but no Letta run_ids; skipped upstream cancel to avoid agent-wide cancellation"
        );
    }
    let cancel_result = if let Some(agent_id) = pair_agent_id.as_deref() {
        cancel_letta_runs_by_id_or_skip(
            state.letta.as_ref(),
            agent_id,
            &run_ids,
            "explicit_acp_session_cancel",
        )
        .await
    } else {
        serde_json::json!({ "ok": false, "error": "pair role agent id is missing" })
    };
    state
        .acp_tool_turns
        .cleanup_session(&session.acp_session_id);
    tracing::info!(
        bear_id = %session.bear_id,
        acp_session_id = %session.acp_session_id,
        conversation_id = %session.conversation_id,
        active_request_id = ?active.as_ref().map(|turn| turn.request_id).or_else(|| stream_cancel.as_ref().map(|turn| turn.request_id)),
        active_conversation_id = ?active.as_ref().and_then(|turn| turn.conversation_id.clone()).or_else(|| stream_cancel.as_ref().and_then(|turn| turn.conversation_id.clone())),
        pair_agent_id = ?pair_agent_id,
        run_ids = ?run_ids,
        cancel_result = %cancel_result,
        "ACP cancel requested; cancelled active pair turn and cleaned session tool state"
    );
    Ok(Json(serde_json::json!({
        "ok": true,
        "cancelled": active.is_some() || stream_cancel.is_some(),
        "active_turn": active.map(|turn| turn.diagnostic()),
        "stream_turn": stream_cancel.map(|turn| serde_json::json!({
            "acp_session_id": turn.acp_session_id,
            "request_id": turn.request_id,
            "conversation_id": turn.conversation_id,
            "run_ids": turn.run_ids,
        })),
        "cancel_result": cancel_result,
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
            unwedged: None,
            workflow_state: None,
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
    if let Err(err) = run_pair_reflection_summary(&state, &session, "session_close").await {
        tracing::warn!(
            bear_id = %session.bear_id,
            acp_session_id = %session.acp_session_id,
            error = %err,
            "Pair reflection summary failed during ACP close"
        );
    }
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

    let active_plan_mode = acp_plan_mode::active_for_session(
        &state.sqlx_pool,
        user_id,
        session.bear_id,
        &session.acp_session_id,
    )
    .await?;
    let turn_context = resolve_acp_turn_context(&session, active_plan_mode.as_ref(), None);
    Ok(Json(AcpCloseSessionResponse {
        ok: true,
        archived,
        conversation_id: archive_target.map(str::to_string),
        unwedged: None,
        workflow_state: Some(turn_context.workflow_state),
    })
    .into_response())
}

async fn run_pair_reflection_summary(
    state: &ApiState,
    session: &acp_sessions::AcpSessionRow,
    trigger: &str,
) -> Result<(), CustomError> {
    let conversation_id = session
        .resolved_conversation_id
        .as_deref()
        .or_else(|| {
            session
                .conversation_id
                .trim()
                .strip_prefix("")
                .filter(|_| false)
        })
        .or_else(|| {
            let raw = session.conversation_id.trim();
            raw.starts_with("conv-").then_some(raw)
        });
    let messages_value = if state.letta.is_enabled() {
        if let Some(conversation_id) = conversation_id {
            state
                .letta
                .list_conversation_messages(conversation_id, None, 20, None, false)
                .await
                .ok()
        } else {
            None
        }
    } else {
        None
    };
    let message_summaries = summarize_letta_messages(messages_value.as_ref());
    let run = pair_reflection::create_run(
        &state.sqlx_pool,
        CreatePairReflectionRun {
            bear_id: session.bear_id,
            user_id: session.user_id,
            acp_session_id: &session.acp_session_id,
            conversation_id,
            trigger,
            considered_message_count: message_summaries.len() as i32,
            considered_memory_paths: Vec::new(),
            diagnostic: serde_json::json!({
                "phase": "pair_reflection_started",
                "conversation_id": conversation_id,
                "message_count": message_summaries.len(),
            }),
        },
    )
    .await?;
    let body = pair_reflection::render_pair_summary_markdown(
        &session.acp_session_id,
        conversation_id,
        trigger,
        &message_summaries,
    );
    let request = MemfsWriteRoleMemoryEntryRequest {
        kind: "summary".to_string(),
        title: pair_reflection::summary_title_for_session(&session.acp_session_id),
        body,
        tags: vec!["pair-reflection".to_string(), "session-summary".to_string()],
        refs: None,
        lifecycle: Some(serde_json::json!({
            "scope": "role-local",
            "retention": "durable",
            "promotion": "maybe",
            "status": "active"
        })),
        source: Some(serde_json::json!({
            "human": { "user_id": session.user_id, "authenticated_by": "acp_token" },
            "session": {
                "acp_session_id": session.acp_session_id,
                "conversation_id": conversation_id,
                "trigger": trigger
            },
            "reflection_run_id": run.id,
        })),
        author: None,
        conversation_id: conversation_id.map(str::to_string),
        session_id: Some(session.acp_session_id.clone()),
        acp_session_id: Some(session.acp_session_id.clone()),
        conversation_selection: Some(session.conversation_id.clone()),
        runtime_target: conversation_id.map(str::to_string),
        role_agent_id: None,
        agent_role: Some(pair_reflection::pair_reflection_role().as_str().to_string()),
        request_id: None,
    };
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| {
            CustomError::System(format!("MemFS pair reflection client build failed: {e}"))
        })?;
    let write_response = write_memfs_role_memory_entry(
        &http,
        &state.config.letta_memfs_service_url,
        session.bear_id,
        BearAgentRole::Pair.as_str(),
        &request,
    )
    .await?;
    let Some(write_response) = write_response else {
        pair_reflection::complete_run(
            &state.sqlx_pool,
            CompletePairReflectionRun {
                id: run.id,
                status: "skipped",
                summary_path: None,
                summary_commit: None,
                diagnostic: serde_json::json!({"reason": "MemFS sidecar not configured"}),
            },
        )
        .await?;
        return Ok(());
    };
    let completed_run = pair_reflection::complete_run(
        &state.sqlx_pool,
        CompletePairReflectionRun {
            id: run.id,
            status: "completed",
            summary_path: Some(&write_response.path),
            summary_commit: write_response.canonical_tip.as_deref(),
            diagnostic: serde_json::json!({
                "phase": "pair_reflection_completed",
                "path": write_response.path,
                "commit": write_response.canonical_tip,
            }),
        },
    )
    .await?;

    let pair_agent_id =
        bears_db::role_agent_id(&state.sqlx_pool, session.bear_id, BearAgentRole::Pair)
            .await?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    let proposal = memory_proposals::create(
        &state.sqlx_pool,
        CreateMemoryProposal {
            bear_id: session.bear_id,
            source_role: BearAgentRole::Pair,
            source_agent_id: pair_agent_id.clone(),
            source_paths: vec![write_response.path.clone()],
            source_refs: serde_json::json!({
                "acp_session_id": session.acp_session_id,
                "conversation_id": conversation_id,
                "reflection_run_id": completed_run.id,
            }),
            suggested_action: "unspecified",
            target_ref: None,
            title: &format!("Review pair reflection summary: {}", session.acp_session_id),
            summary: "Pair reflection created a durable session summary; review for useful shared/work-visible knowledge.",
            rationale: "Pair reflection summaries may contain durable decisions, lessons, or work-visible knowledge that should be curated beyond pair-local memory.",
            proposed_content: None,
            proposed_patch: None,
            refs: serde_json::json!({
                "summary_path": write_response.path,
                "summary_commit": write_response.canonical_tip,
                "reflection_run_id": completed_run.id,
            }),
            sensitivity: "normal",
            requires_human: false,
        },
    )
    .await?;

    let reflection_date = time::OffsetDateTime::now_utc().date();
    let conversation_key = format!("memory_curate:{reflection_date}");
    reflection_conductor::enqueue_memory_curate_for_proposals(
        &state.sqlx_pool,
        reflection_conductor::ProposalEnqueueParams {
            bear_id: session.bear_id,
            role_agent_id: pair_agent_id.as_deref(),
            conversation_id,
            conversation_key: Some(&conversation_key),
            conversation_date: Some(reflection_date),
            trigger: "pair_reflection",
            proposal_ids: vec![proposal.id],
        },
    )
    .await?;
    Ok(())
}

fn summarize_letta_messages(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let messages = value
        .get("messages")
        .or_else(|| value.get("data"))
        .or_else(|| value.get("items"))
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array());
    let Some(messages) = messages else {
        return Vec::new();
    };
    messages
        .iter()
        .rev()
        .filter_map(|message| {
            let role = message
                .get("role")
                .or_else(|| message.get("message_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("message");
            let content = message
                .get("content")
                .and_then(|v| v.as_str())
                .or_else(|| message.get("text").and_then(|v| v.as_str()))
                .unwrap_or("")
                .trim();
            if content.is_empty() {
                None
            } else {
                Some(format!("{role}: {}", truncate_for_reflection(content, 300)))
            }
        })
        .take(20)
        .collect()
}

fn truncate_for_reflection(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
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
    let token = auth::extract_bearer_token(&headers)?;
    let auth = authenticate_acp_code_token_with_auth(&state, &token, &slug)
        .await
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    let user_id = auth.user_id;
    let tools_enabled = acp_tokens::scopes_contains(&auth.scopes, acp_tokens::acp_tools_scope());
    if !tools_enabled {
        tracing::info!(bear_slug = %slug, user_id = user_id, "ACP token lacks acp:tools; local client tools disabled for prompt");
    }
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

    let pair_runtime_binding =
        match require_pair_runtime_binding(&state.sqlx_pool, state.letta.as_ref(), &bear).await {
            Ok(binding) => binding,
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
    let is_new_session_binding = existing_session.is_none();
    let requested_initial_mode = match requested_mode_from_prompt(&body) {
        Ok(mode) => mode,
        Err(err) => return Ok(Err(err)),
    };
    let generated_conversation_id = new_acp_conversation_id(&client);
    let (conversation_resolution, ensure_conversation_result) = ensure_acp_session_conversation(
        state.letta.as_ref(),
        crate::core::runtime_contracts::EnsureConversationRequest {
            bear_id: bear.id,
            role: "pair".to_string(),
            acp_session_id: session_id.to_string(),
            requested_selection: body.conversation_id.clone(),
            binding: pair_runtime_binding.clone(),
        },
        existing_session.as_ref(),
        generated_conversation_id,
    )
    .await
    .map_err(|err| {
        let (status, code, message) = acp_error_status_message(&err);
        ApiError::new(status, code, message)
    })?;
    if conversation_resolution.requires_belongs_to_bear_check {
        verify_acp_conversation_belongs_to_binding(
            state.letta.as_ref(),
            &pair_runtime_binding,
            &conversation_resolution.session_selection,
        )
        .await
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    }
    if ensure_conversation_result.created {
        tracing::info!(
            %request_id,
            acp_session_id = %session_id,
            bear_id = %bear.id,
            pending_conversation_id = %conversation_resolution.session_selection,
            resolved_conversation_id = %ensure_conversation_result.conversation.id,
            "ACP created fresh Letta conversation for new session"
        );
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
            resolved_conversation_id: conversation_resolution
                .resolved_conversation
                .as_ref()
                .map(|conversation| conversation.id.clone()),
            client: client.clone(),
            cwd: Some(cwd.clone()),
            current_mode: if is_new_session_binding {
                requested_initial_mode.map(str::to_string)
            } else {
                None
            },
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
    if is_new_session_binding {
        match requested_initial_mode {
            Some("plan") => {
                acp_plan_mode::enter_plan_mode(
                    &state.sqlx_pool,
                    acp_plan_mode::EnterPlanModeParams {
                        user_id,
                        bear_id: bear.id,
                        bear_slug: bear.slug.clone(),
                        acp_session_id: session_id.to_string(),
                        reason: "Client selected ACP Plan mode before first prompt".to_string(),
                        requested_by: acp_plan_mode::AcpPlanModeRequestedBy::User,
                        previous_permission_mode: Some("ask".to_string()),
                    },
                )
                .await
                .map_err(|err| {
                    let (status, code, message) = acp_error_status_message(&err);
                    ApiError::new(status, code, message)
                })?;
                acp_sessions::set_current_mode(
                    &state.sqlx_pool,
                    user_id,
                    bear.id,
                    session_id,
                    "plan",
                )
                .await
                .map_err(|err| {
                    let (status, code, message) = acp_error_status_message(&err);
                    ApiError::new(status, code, message)
                })?;
            }
            Some("write") => {
                acp_sessions::set_current_mode(
                    &state.sqlx_pool,
                    user_id,
                    bear.id,
                    session_id,
                    "write",
                )
                .await
                .map_err(|err| {
                    let (status, code, message) = acp_error_status_message(&err);
                    ApiError::new(status, code, message)
                })?;
            }
            _ => {}
        }
    }
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        bear_slug = %bear.slug,
        bear_id = %bear.id,
        role = "pair",
        compatibility_binding_id = %pair_runtime_binding.binding_id,
        client = %client,
        cwd = %cwd,
        requested_conversation_id = body.conversation_id.as_deref().map(str::trim),
        conversation_id = %conversation_resolution.session_selection,
        conversation_selection_source = %conversation_resolution.selection_source.as_str(),
        resolved_conversation_id = conversation_resolution
            .resolved_conversation
            .as_ref()
            .map(|conversation| conversation.id.as_str()),
        history_target = conversation_resolution
            .history_target
            .as_ref()
            .map(|conversation| conversation.id.as_str()),
        archive_target = conversation_resolution
            .archive_target
            .as_ref()
            .map(|conversation| conversation.id.as_str()),
        letta_conversation_id = %conversation_resolution.upstream_target,
        "ACP gateway routing prompt to pair role via Letta API"
    );
    let active_plan_mode =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id)
            .await
            .map_err(|err| {
                let (status, code, message) = acp_error_status_message(&err);
                ApiError::new(status, code, message)
            })?;
    let session_mode =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await
            .map_err(|err| {
                ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "database",
                    err.to_string(),
                )
            })?
            .map(|session| session.current_mode)
            .unwrap_or_else(|| "ask".to_string());
    let synthetic_session_row = acp_sessions::AcpSessionRow {
        id: Uuid::nil(),
        user_id,
        bear_id: bear.id,
        bear_slug: bear.slug.clone(),
        acp_session_id: session_id.to_string(),
        runtime_session_id: "runtime-test".to_string(),
        conversation_id: conversation_resolution.session_selection.clone(),
        resolved_conversation_id: conversation_resolution
            .resolved_conversation
            .as_ref()
            .map(|conversation| conversation.id.clone()),
        client: client.clone(),
        cwd: Some(cwd.clone()),
        adapter_environment: None,
        current_mode: session_mode,
        conversation_title: acp_sessions::find_for_user_bear_session(
            &state.sqlx_pool,
            user_id,
            &bear.slug,
            session_id,
        )
        .await
        .map_err(|err| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "database",
                err.to_string(),
            )
        })?
        .and_then(|session| session.conversation_title),
        conversation_title_updated_at: None,
        conversation_title_synced_at: None,
        closed_at: None,
        archived_at: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    let auto_title_guidance = acp_auto_title_instruction(&synthetic_session_row);
    let resolved_policy =
        resolve_acp_turn_context(&synthetic_session_row, active_plan_mode.as_ref(), None).policy;
    let plan_mode_context = acp_plan_mode_prompt_context(&state, bear.id, user_id, session_id)
        .await
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    let current_activity_plan = work_plans::get_visible_work_plan(
        &state.sqlx_pool,
        bear.id,
        BearAgentRole::Pair,
        user_id,
        WorkPlanLookup {
            plan_id: None,
            source_conversation_id: None,
            source_acp_session_id: Some(session_id.to_string()),
        },
    )
    .await
    .map_err(|err| {
        let (status, code, message) = acp_error_status_message(&err);
        ApiError::new(status, code, message)
    })?;
    let tool_prompt_context = acp_direct_tool_prompt_context_with_activity(
        session_id,
        &cwd,
        &body.client_context,
        tools_enabled,
        &resolved_policy,
        current_activity_plan.as_ref(),
        auto_title_guidance.as_deref(),
    );
    let merged_client_tool_descriptors = tools_enabled.then(|| {
        merge_acp_pair_tool_descriptors(acp_client_tool_descriptors_for_client_context(
            &body.client_context,
            Some(&resolved_policy),
        ))
    });
    let auto_title_tool_advertised = merged_client_tool_descriptors
        .as_ref()
        .and_then(|value| value.as_array())
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|name| name == den_tools::DEN_CONVERSATION_SET_TITLE_PROVIDER)
            })
        });
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        auto_title_guidance_injected = auto_title_guidance.is_some(),
        auto_title_tool_advertised,
        current_conversation_title = synthetic_session_row.conversation_title.as_deref(),
        resolved_conversation_id = synthetic_session_row.resolved_conversation_id.as_deref(),
        conversation_id = %synthetic_session_row.conversation_id,
        "ACP auto-title prompt state"
    );
    let plans = current_activity_plan
        .clone()
        .into_iter()
        .collect::<Vec<_>>();
    let activity_context = work_plans::render_workboard_prompt_context(&plans);
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        prompt_message_len = prompt.len(),
        plan_mode_context_len = plan_mode_context.len(),
        activity_context_len = activity_context.len(),
        tool_prompt_context_len = tool_prompt_context.len(),
        prompt_has_trusted_mode_suffix = prompt.contains("Trusted ACP session mode this turn:"),
        prompt_has_system_reminder = prompt.contains("<system-reminder>"),
        "ACP prompt context assembly lengths"
    );
    let mut initial_events = Vec::new();
    let mut session_info_event_sent = false;
    if let Some(conversation) = conversation_resolution.resolved_conversation.clone() {
        initial_events.push(AcpGatewayEvent::ConversationResolved {
            conversation_id: conversation.id,
        });
    }
    if let Some(title_event) = pending_session_title_update_event(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &bear.slug,
        session_id,
    )
    .await
    .map_err(|err| {
        let (status, code, message) = acp_error_status_message(&err);
        ApiError::new(status, code, message)
    })? {
        session_info_event_sent = true;
        initial_events.push(title_event);
    }
    if let Some(plan_event) = current_activity_plan
        .clone()
        .map(AcpGatewayEvent::PlanUpdate)
    {
        initial_events.push(plan_event);
    }
    let turn_runtime_context =
        format!("{plan_mode_context}{activity_context}{tool_prompt_context}");
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        upstream_user_prompt_len = prompt.len(),
        turn_runtime_context_len = turn_runtime_context.len(),
        turn_runtime_context_has_trusted_mode_suffix =
            turn_runtime_context.contains("Trusted ACP session mode this turn:"),
        turn_runtime_context_has_system_reminder =
            turn_runtime_context.contains("<system-reminder>"),
        runtime_context_sent_as_user_content = false,
        "ACP final upstream prompt assembly"
    );
    let workspace_roots = body
        .client_context
        .get("workspace_roots")
        .or_else(|| body.client_context.get("workspaceRoots"))
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
    let client_tool_descriptors = merged_client_tool_descriptors.clone();
    let stream_tokens = acp_stream_tokens_enabled();
    let turn_lifecycle = AcpTurnLifecycleRuntime::new(
        state.acp_tool_turns.clone(),
        state.acp_turn_cancellations.clone(),
    );
    let lifecycle_lease = match turn_lifecycle.acquire_pair_turn(
        AcpTurnLifecycleContext {
            bear_id: bear.id,
            acp_session_id: session_id.to_string(),
            resolved_conversation_id: conversation_resolution
                .resolved_conversation
                .as_ref()
                .map(|conversation| conversation.id.clone()),
        },
        request_id,
    ) {
        Ok(lease) => lease,
        Err(err) => return Ok(Err(err)),
    };
    let role_runtime = lifecycle_lease.role_runtime.clone();
    let turn_scope = lifecycle_lease.turn_scope.clone();
    let active_turn_guard = lifecycle_lease.active_turn_guard;
    let cancel_handle = lifecycle_lease.cancel_handle;
    let cancel_rx = lifecycle_lease.cancel_rx;

    let upstream = match start_acp_turn_with_retries(AcpTurnStartRequest {
        state: &state,
        request_id,
        session_id,
        bear_id: bear.id,
        binding: &pair_runtime_binding,
        upstream_target: &conversation_resolution.upstream_target,
        prompt,
        client_tools: client_tool_descriptors.clone(),
        runtime_context_len: turn_runtime_context.len(),
        stream_tokens,
    })
    .await
    {
        Ok(upstream) => upstream,
        Err(err) => return Ok(Err(err)),
    };

    let session_policy = resolved_policy.to_json();
    let activity = current_activity_plan
        .as_ref()
        .map(|plan| serde_json::json!(plan));
    let stream = AcpLettaSseStream::new(
        upstream.bytes_stream().map(|item| item.map_err(Into::into)),
        AcpStreamContext {
            pool: state.sqlx_pool.clone(),
            tool_turns: state.acp_tool_turns.clone(),
            user_id,
            user_profile: user::user_by_id(&state.sqlx_pool, user_id).await.ok(),
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            client: client.clone(),
            conversation_selection: conversation_resolution.session_selection.clone(),
            resolved_conversation_id: conversation_resolution
                .resolved_conversation
                .as_ref()
                .map(|conversation| conversation.id.clone()),
            upstream_target: conversation_resolution.upstream_target.clone(),
            workspace_roots: workspace_roots.clone(),
            session_policy: Some(session_policy),
            activity,
            request_id,
            pair_agent_id: pair_runtime_binding.binding_id.clone(),
            config: state.config.clone(),
            role_runtime,
            turn_scope,
        },
        initial_events,
        session_info_event_sent,
        state.letta.clone(),
        LettaContinuationContext {
            conversation_id: conversation_resolution.upstream_target.clone(),
            agent_id: Some(pair_runtime_binding.binding_id.clone()),
            client_tools: client_tool_descriptors,
            stream_tokens,
            max_steps: 4,
        },
        active_turn_guard,
    )
    .with_cancel_registration(cancel_handle, cancel_rx);
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
pub(super) struct AcpStreamContext {
    pub(super) pool: PgPool,
    pub(super) tool_turns: AcpToolTurnCoordinator,
    pub(super) user_id: i32,
    pub(super) user_profile: Option<crate::core::user::User>,
    pub(super) bear_id: Uuid,
    pub(super) bear_slug: String,
    pub(super) acp_session_id: String,
    pub(super) client: String,
    pub(super) conversation_selection: String,
    pub(super) resolved_conversation_id: Option<String>,
    pub(super) upstream_target: String,
    pub(super) workspace_roots: Vec<String>,
    pub(super) session_policy: Option<serde_json::Value>,
    pub(super) activity: Option<serde_json::Value>,
    pub(super) request_id: Uuid,
    pub(super) pair_agent_id: String,
    pub(super) config: Arc<crate::config::Config>,
    pub(super) role_runtime: RoleRuntime,
    pub(super) turn_scope: RoleTurnScope,
}

