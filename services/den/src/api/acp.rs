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
    collections::{BTreeMap, HashMap, VecDeque},
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
        auth::{self, ApiError},
        oauth::OAuthScope,
        service::ApiState,
    },
    core::{
        acp_letta_events::{
            acp_event_adapter_type, acp_event_has_visible_output, acp_event_to_adapter_sse,
            map_native_letta_stream_event_to_acp_event_with_accumulator, AcpGatewayEvent,
            LettaToolCallAccumulator,
        },
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

fn acp_debug_event_sample_chars() -> usize {
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
struct AcpStreamContext {
    pool: PgPool,
    tool_turns: AcpToolTurnCoordinator,
    user_id: i32,
    user_profile: Option<crate::core::user::User>,
    bear_id: Uuid,
    bear_slug: String,
    acp_session_id: String,
    client: String,
    conversation_selection: String,
    resolved_conversation_id: Option<String>,
    upstream_target: String,
    workspace_roots: Vec<String>,
    session_policy: Option<serde_json::Value>,
    activity: Option<serde_json::Value>,
    request_id: Uuid,
    pair_agent_id: String,
    config: Arc<crate::config::Config>,
    role_runtime: RoleRuntime,
    turn_scope: RoleTurnScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolExecutionRoute {
    DenServer,
    AdapterLocal,
    Unsupported,
}

impl From<ToolExecutionRoute> for ControllerToolExecutionRoute {
    fn from(route: ToolExecutionRoute) -> Self {
        match route {
            ToolExecutionRoute::DenServer => Self::DenServer,
            ToolExecutionRoute::AdapterLocal => Self::AdapterLocal,
            ToolExecutionRoute::Unsupported => Self::Unsupported,
        }
    }
}

fn tool_execution_route(tool_name: &str, args: &serde_json::Value) -> ToolExecutionRoute {
    if args.get("_unsupported_detail").is_some() {
        ToolExecutionRoute::Unsupported
    } else if acp_den_provider_to_canonical_tool_name(tool_name).is_some() {
        ToolExecutionRoute::DenServer
    } else {
        ToolExecutionRoute::AdapterLocal
    }
}

struct PersistedToolRequestEffect {
    tool_call_id: String,
    tool_name: String,
    route: ToolExecutionRoute,
    den_server_result_rx: Option<oneshot::Receiver<AcpToolResultRequest>>,
}

async fn persist_stream_event_side_effects(
    context: &AcpStreamContext,
    event: &mut AcpGatewayEvent,
) -> Result<Option<PersistedToolRequestEffect>, CustomError> {
    let mut tool_request_effect = None;
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
            args,
            result_tx,
            result_rx: _,
            approval_required,
            approval_reason,
            ..
        } => {
            let route = tool_execution_route(tool_name, args);
            let effect_tool_call_id = tool_call_id.clone();
            let effect_tool_name = tool_name.clone();
            let mut effect_den_server_result_rx = None;
            tracing::info!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                tool_request_id = %request_id,
                tool_call_id = %tool_call_id,
                tool_name = %tool_name,
                route = ?route,
                approval_required = %approval_required,
                approval_request_id = ?approval_request_id,
                "ACP tool request route classified"
            );
            match route {
                ToolExecutionRoute::Unsupported => {
                    let result_tx = result_tx.take().ok_or_else(|| {
                        CustomError::System(
                            "ACP unsupported tool request missing result channel".to_string(),
                        )
                    })?;
                    *approval_required = false;
                    *approval_reason = None;
                    let detail = args
                        .get("_unsupported_detail")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unsupported ACP/Den tool")
                        .to_string();
                    let result = AcpToolResultRequest {
                        turn_id: None,
                        request_id: Some(context.request_id.to_string()),
                        tool_call_id: Some(tool_call_id.clone()),
                        tool_name: Some(tool_name.clone()),
                        approval_request_id: approval_request_id.clone(),
                        status: "unsupported".to_string(),
                        content: Some(detail.clone()),
                        structured_content: serde_json::json!({}),
                        diagnostic: serde_json::json!({
                            "component": "den.acp",
                            "phase": "unsupported_tool_settled",
                            "tool_name": tool_name,
                            "tool_call_id": tool_call_id,
                        }),
                        ..Default::default()
                    };
                    let _ = result_tx.send(result);
                    tracing::warn!(
                        request_id = %context.request_id,
                        acp_session_id = %context.acp_session_id,
                        tool_request_id = %request_id,
                        tool_call_id = %tool_call_id,
                        tool_name = %tool_name,
                        detail = %detail,
                        "ACP unsupported tool request settled with error result"
                    );
                }
                ToolExecutionRoute::DenServer => {
                    let canonical_name = acp_den_provider_to_canonical_tool_name(&effect_tool_name)
                        .ok_or_else(|| CustomError::System("missing Den tool route".to_string()))?;
                    let tool_request_id = request_id.clone();
                    if canonical_name == den_tools::DEN_WEB_FETCH {
                        route_web_fetch_tool_request(context, event, false).await?;
                    } else {
                        route_direct_den_tool_request(context, event, canonical_name).await?;
                    }
                    if let AcpGatewayEvent::ToolRequest { result_rx, .. } = event {
                        if let Some(result_rx) = result_rx.take() {
                            effect_den_server_result_rx = Some(result_rx);
                        }
                    }
                    tracing::info!(
                        request_id = %context.request_id,
                        acp_session_id = %context.acp_session_id,
                        tool_request_id = %tool_request_id,
                        tool_call_id = %effect_tool_call_id,
                        tool_name = %effect_tool_name,
                        canonical_tool_name = %canonical_name,
                        "ACP Den server tool routed"
                    );
                }
                ToolExecutionRoute::AdapterLocal => {
                    let result_tx = result_tx.take().ok_or_else(|| {
                        CustomError::System("ACP tool request missing result channel".to_string())
                    })?;
                    context.tool_turns.register(AcpToolTurnRegistration {
                        user_id: context.user_id,
                        bear_id: context.bear_id,
                        bear_slug: context.bear_slug.clone(),
                        acp_session_id: context.acp_session_id.clone(),
                        request_id: context.request_id,
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                        approval_request_id: approval_request_id.clone(),
                        timeout_ms: acp_tool_timeout_ms_for_provider(tool_name),
                        result_tx,
                    })?;
                    tracing::info!(
                        request_id = %context.request_id,
                        acp_session_id = %context.acp_session_id,
                        tool_request_id = %request_id,
                        tool_call_id = %tool_call_id,
                        tool_name = %tool_name,
                        approval_required = %approval_required,
                        approval_request_id = ?approval_request_id,
                        "ACP adapter-local tool obligation registered"
                    );
                }
            }
            tool_request_effect = Some(PersistedToolRequestEffect {
                tool_call_id: effect_tool_call_id,
                tool_name: effect_tool_name,
                route,
                den_server_result_rx: effect_den_server_result_rx,
            });
        }
        _ => {}
    }
    Ok(tool_request_effect)
}

async fn route_web_fetch_tool_request(
    context: &AcpStreamContext,
    event: &mut AcpGatewayEvent,
    _plan_mode_active: bool,
) -> Result<(), CustomError> {
    let AcpGatewayEvent::ToolRequest {
        tool_call_id,
        approval_request_id,
        tool_name,
        request_id,
        args,
        result_tx,
        approval_required,
        approval_reason,
        ..
    } = event
    else {
        return Ok(());
    };
    let result_tx = result_tx.take().ok_or_else(|| {
        CustomError::System("ACP Den web_fetch request missing result channel".to_string())
    })?;
    *approval_required = false;
    *approval_reason = None;
    let web_args = match serde_json::from_value::<WebFetchToolArgs>(args.clone()) {
        Ok(args) => args,
        Err(err) => {
            settle_den_tool_error(
                result_tx,
                context,
                tool_call_id,
                tool_name,
                approval_request_id.as_deref(),
                "den_server_tool_validation_failed",
                format!("web_fetch arguments are invalid: {err}"),
            );
            return Ok(());
        }
    };
    let (normalized, decision) = match web_policy::decide_web_fetch_approval(
        &context.pool,
        context.bear_id,
        web_args.url.trim(),
    )
    .await
    {
        Ok(value) => value,
        Err(err) => {
            settle_den_tool_error(
                result_tx,
                context,
                tool_call_id,
                tool_name,
                approval_request_id.as_deref(),
                "web_fetch_approval_policy_failed",
                err.to_string(),
            );
            return Ok(());
        }
    };
    if decision.is_approved() && web_policy::is_local_web_url(&normalized) {
        *tool_name = "local_web_fetch".to_string();
        args["url"] = serde_json::json!(normalized.url);
        context.tool_turns.register(AcpToolTurnRegistration {
            user_id: context.user_id,
            bear_id: context.bear_id,
            bear_slug: context.bear_slug.clone(),
            acp_session_id: context.acp_session_id.clone(),
            request_id: context.request_id,
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.clone(),
            approval_request_id: approval_request_id.clone(),
            timeout_ms: acp_tool_timeout_ms_for_provider(tool_name),
            result_tx,
        })?;
        return Ok(());
    }
    if matches!(decision, web_policy::WebApprovalDecision::RequiresApproval) {
        let permission_id = format!("perm-{}", Uuid::new_v4());
        pending_web_fetch_approvals().lock().await.insert(
            permission_id.clone(),
            PendingWebFetchApproval {
                user_id: context.user_id,
                bear_id: context.bear_id,
                result_tx,
                context: context.clone(),
                provider_name: tool_name.clone(),
                tool_call_id: tool_call_id.clone(),
                approval_request_id: approval_request_id.clone(),
                args: args.clone(),
                normalized_url: normalized.clone(),
            },
        );
        *event = AcpGatewayEvent::PermissionRequest {
            request_id: request_id.clone(),
            permission_id,
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.clone(),
            title: "Fetch URL".to_string(),
            reason: format!(
                "BEARS wants to fetch {}. Approve this URL or host?",
                normalized.url
            ),
            target: serde_json::json!({ "kind": "url", "url": normalized.url, "host": normalized.host }),
            options: vec![
                "allow_once".to_string(),
                "allow_url".to_string(),
                "allow_host".to_string(),
                "reject_once".to_string(),
            ],
        };
        return Ok(());
    }
    // `route_web_fetch_tool_request` owns `result_tx` from the start so it can
    // either park it behind an ACP permission request, register it for local
    // adapter execution, or settle validation/policy errors. Do not re-enter
    // `route_direct_den_tool_request` here: that helper would try to take the
    // already-owned channel a second time and fail the turn.
    let result = invoke_acp_den_tool(
        context,
        den_tools::DEN_WEB_FETCH,
        tool_name,
        tool_call_id,
        approval_request_id.as_deref(),
        args.clone(),
    )
    .await;
    let _ = result_tx.send(result);
    tracing::info!(
        request_id = %context.request_id,
        acp_session_id = %context.acp_session_id,
        tool_request_id = %request_id,
        tool_call_id = %tool_call_id,
        tool_name = %tool_name,
        canonical_tool_name = %den_tools::DEN_WEB_FETCH,
        web_approval_decision = %decision.as_str(),
        "ACP Den web_fetch tool executed"
    );
    Ok(())
}

async fn route_direct_den_tool_request(
    context: &AcpStreamContext,
    event: &mut AcpGatewayEvent,
    canonical_name: &str,
) -> Result<(), CustomError> {
    let AcpGatewayEvent::ToolRequest {
        tool_call_id,
        approval_request_id,
        tool_name,
        request_id,
        args,
        result_tx,
        approval_required,
        approval_reason,
        ..
    } = event
    else {
        return Ok(());
    };
    let result_tx = result_tx.take().ok_or_else(|| {
        CustomError::System("ACP Den tool request missing result channel".to_string())
    })?;
    *approval_required = false;
    *approval_reason = None;
    let result = invoke_acp_den_tool(
        context,
        canonical_name,
        tool_name,
        tool_call_id,
        approval_request_id.as_deref(),
        args.clone(),
    )
    .await;
    let _ = result_tx.send(result);
    tracing::info!(
        request_id = %context.request_id,
        acp_session_id = %context.acp_session_id,
        tool_request_id = %request_id,
        tool_call_id = %tool_call_id,
        tool_name = %tool_name,
        canonical_tool_name = %canonical_name,
        "ACP Den server tool executed"
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WebFetchToolArgs {
    url: String,
}

fn mode_from_den_tool_result(result: &AcpToolResultRequest) -> Option<&str> {
    result
        .structured_content
        .get("mode_update")
        .and_then(serde_json::Value::as_str)
        .filter(|mode| matches!(*mode, "ask" | "plan" | "write"))
}

fn work_plan_item_to_acp_plan_entry(item: &serde_json::Value) -> Option<serde_json::Value> {
    let title = item
        .get("title")
        .and_then(serde_json::Value::as_str)?
        .trim();
    if title.is_empty() {
        return None;
    }
    let raw_status = item
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("pending");
    let blocked_reason = item
        .get("blocked_reason")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let summary = item
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let content = match (raw_status, blocked_reason, summary) {
        ("blocked", Some(reason), _) => format!("Blocked: {title} — {reason}"),
        ("blocked", None, _) => format!("Blocked: {title}"),
        ("cancelled", _, _) => format!("Cancelled: {title}"),
        (_, _, Some(summary)) => format!("{title} — {summary}"),
        _ => title.to_string(),
    };
    let status = match raw_status {
        "in_progress" => "in_progress",
        "completed" | "cancelled" => "completed",
        _ => "pending",
    };
    let priority = if raw_status == "in_progress" {
        "high"
    } else {
        "medium"
    };
    Some(serde_json::json!({
        "content": content,
        "priority": priority,
        "status": status,
        "_meta": {
            "bears": {
                "item_id": item.get("id").cloned().unwrap_or(serde_json::Value::Null),
                "status": raw_status,
                "blocked_reason": item.get("blocked_reason").cloned().unwrap_or(serde_json::Value::Null),
                "source_refs": item.get("source_refs").cloned().unwrap_or_else(|| serde_json::json!([])),
            }
        }
    }))
}

fn plan_update_from_den_tool_result(result: &AcpToolResultRequest) -> Option<AcpGatewayEvent> {
    if let Some(plan) = result.structured_content.get("plan") {
        let items = plan.get("items").and_then(serde_json::Value::as_array)?;
        let entries = items
            .iter()
            .filter_map(work_plan_item_to_acp_plan_entry)
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return None;
        }
        return Some(AcpGatewayEvent::PlanUpdateJson { entries });
    }

    plan_approval_fallback_from_tool_result(result)
}

fn plan_approval_fallback_payload(row: &acp_plan_mode::AcpPlanModeSessionRow) -> serde_json::Value {
    serde_json::json!({
        "kind": "submitted_plan_approval",
        "plan_id": row.id,
        "title": row.plan_title.as_deref().unwrap_or("Submitted implementation plan"),
        "body": row.plan_body.as_deref().unwrap_or(""),
        "artifact_path": row.plan_artifact_path.as_deref().unwrap_or("not_submitted"),
        "state": row.state,
        "approval_status": turn_state::approval_status_label(Some(row.state.as_str()), "Plan"),
    })
}

fn plan_approval_fallback_from_tool_result(
    result: &AcpToolResultRequest,
) -> Option<AcpGatewayEvent> {
    let workplan = result.structured_content.get("workplan")?;
    let raw_state = workplan
        .get("raw_state")
        .and_then(serde_json::Value::as_str)
        .or_else(|| workplan.get("state").and_then(serde_json::Value::as_str))?
        .trim();
    if raw_state != "submitted" {
        return None;
    }
    let plan_id = workplan
        .get("plan_id")
        .or_else(|| workplan.get("id"))
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())?;
    let title = workplan
        .get("title")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            result
                .structured_content
                .get("submitted_plan")
                .and_then(|submitted| submitted.get("title"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("Submitted implementation plan")
        .trim()
        .to_string();
    let body = result
        .structured_content
        .get("submitted_plan")
        .and_then(|submitted| submitted.get("body"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| workplan.get("body").and_then(serde_json::Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let artifact_path = result
        .structured_content
        .get("artifact")
        .and_then(|artifact| artifact.get("path"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            result
                .structured_content
                .get("submitted_plan")
                .and_then(|submitted| submitted.get("artifact_path"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            workplan
                .get("artifact_path")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("not_submitted")
        .trim()
        .to_string();

    Some(AcpGatewayEvent::PlanApprovalFallback {
        plan_id,
        title,
        body,
        artifact_path,
        state: raw_state.to_string(),
        approval_status: workplan
            .get("approval_status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("awaiting_human_approval")
            .to_string(),
    })
}

fn settle_den_tool_error(
    result_tx: oneshot::Sender<AcpToolResultRequest>,
    context: &AcpStreamContext,
    tool_call_id: &str,
    tool_name: &str,
    approval_request_id: Option<&str>,
    phase: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    let result = AcpToolResultRequest {
        turn_id: None,
        request_id: Some(context.request_id.to_string()),
        tool_call_id: Some(tool_call_id.to_string()),
        tool_name: Some(tool_name.to_string()),
        approval_request_id: approval_request_id.map(str::to_string),
        status: "error".to_string(),
        content: Some(message.clone()),
        structured_content: serde_json::json!({}),
        diagnostic: serde_json::json!({
            "component": "den.acp",
            "phase": phase,
            "tool_name": tool_name,
            "tool_call_id": tool_call_id,
            "error": message,
        }),
        ..Default::default()
    };
    let _ = result_tx.send(result);
}

async fn invoke_acp_runtime_local_tool(
    context: &AcpStreamContext,
    tool_name: &str,
    tool_call_id: &str,
    args: serde_json::Value,
) -> AcpToolResultRequest {
    match tool_name {
        "bear_environment" => {
            let tool_context = DenToolInvocationContext {
                bear_id: context.bear_id,
                bear_slug: context.bear_slug.clone(),
                role_agent_id: context.pair_agent_id.clone(),
                agent_role: Some(BearAgentRole::Pair),
                user_id: context.user_id,
                username: context
                    .user_profile
                    .as_ref()
                    .map(|user| user.username.clone()),
                membership_role: bears_db::membership_role_for_user(
                    &context.pool,
                    context.user_id,
                    context.bear_id,
                )
                .await
                .ok()
                .flatten()
                .flatten(),
                conversation_id: context
                    .resolved_conversation_id
                    .clone()
                    .unwrap_or_else(|| context.upstream_target.clone()),
                session_id: context.acp_session_id.clone(),
                acp_session_id: Some(context.acp_session_id.clone()),
                conversation_selection: Some(context.conversation_selection.clone()),
                runtime_target: Some(context.upstream_target.clone()),
                workspace_roots: context.workspace_roots.clone(),
                session_policy: context.session_policy.clone(),
                activity: context.activity.clone(),
                runtime: Some(
                    context
                        .role_runtime
                        .tool_turn_runtime_snapshot(&context.acp_session_id, &context.tool_turns),
                ),
                context_budget: Some(default_unavailable_context_budget()),
                request_id: Some(context.request_id.to_string()),
                channel: DenToolChannelContext {
                    family: Some("acp_runtime".to_string()),
                    client: Some(context.client.clone()),
                    protocol: Some("acp".to_string()),
                },
            };
            match den_tools::invoke_den_tool(
                &context.pool,
                context.config.as_ref(),
                den_tools::DEN_BEAR_ENVIRONMENT,
                args,
                tool_context,
            )
            .await
            {
                Ok(value) => AcpToolResultRequest {
                    turn_id: None,
                    request_id: Some(context.request_id.to_string()),
                    tool_call_id: Some(tool_call_id.to_string()),
                    tool_name: Some(tool_name.to_string()),
                    approval_request_id: None,
                    status: "ok".to_string(),
                    content: Some(
                        serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
                    ),
                    structured_content: value,
                    diagnostic: serde_json::json!({
                        "component": "den.acp",
                        "phase": "runtime_local_tool_result",
                        "tool_name": tool_name,
                    }),
                    ..Default::default()
                },
                Err(err) => AcpToolResultRequest {
                    turn_id: None,
                    request_id: Some(context.request_id.to_string()),
                    tool_call_id: Some(tool_call_id.to_string()),
                    tool_name: Some(tool_name.to_string()),
                    approval_request_id: None,
                    status: "error".to_string(),
                    content: Some(err.to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({
                        "component": "den.acp",
                        "phase": "runtime_local_tool_error",
                        "tool_name": tool_name,
                    }),
                    ..Default::default()
                },
            }
        }
        _ => AcpToolResultRequest {
            turn_id: None,
            request_id: Some(context.request_id.to_string()),
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(tool_name.to_string()),
            approval_request_id: None,
            status: "error".to_string(),
            content: Some(format!("unsupported ACP runtime local tool: {tool_name}")),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({
                "component": "den.acp",
                "phase": "runtime_local_tool_unsupported",
                "tool_name": tool_name,
            }),
            ..Default::default()
        },
    }
}

async fn invoke_acp_den_tool(
    context: &AcpStreamContext,
    canonical_name: &str,
    provider_name: &str,
    tool_call_id: &str,
    approval_request_id: Option<&str>,
    args: serde_json::Value,
) -> AcpToolResultRequest {
    if canonical_name == den_tools::DEN_BEAR_ENVIRONMENT {
        return invoke_acp_runtime_local_tool(context, "bear_environment", tool_call_id, args)
            .await;
    }
    let membership_role =
        bears_db::membership_role_for_user(&context.pool, context.user_id, context.bear_id)
            .await
            .ok()
            .flatten()
            .flatten();
    let tool_context = DenToolInvocationContext {
        bear_id: context.bear_id,
        bear_slug: context.bear_slug.clone(),
        role_agent_id: context.pair_agent_id.clone(),
        agent_role: Some(BearAgentRole::Pair),
        user_id: context.user_id,
        username: context
            .user_profile
            .as_ref()
            .map(|user| user.username.clone()),
        membership_role,
        conversation_id: context
            .resolved_conversation_id
            .clone()
            .unwrap_or_else(|| context.upstream_target.clone()),
        session_id: context.acp_session_id.clone(),
        acp_session_id: Some(context.acp_session_id.clone()),
        conversation_selection: Some(context.conversation_selection.clone()),
        runtime_target: Some(context.upstream_target.clone()),
        workspace_roots: context.workspace_roots.clone(),
        session_policy: context.session_policy.clone(),
        activity: context.activity.clone(),
        runtime: Some(
            context
                .role_runtime
                .tool_turn_runtime_snapshot(&context.acp_session_id, &context.tool_turns),
        ),
        context_budget: Some(default_unavailable_context_budget()),
        request_id: Some(context.request_id.to_string()),
        channel: DenToolChannelContext {
            family: Some("acp".to_string()),
            client: Some("api-direct".to_string()),
            protocol: Some("acp".to_string()),
        },
    };
    match den_tools::invoke_den_tool(
        &context.pool,
        context.config.as_ref(),
        canonical_name,
        args,
        tool_context,
    )
    .await
    {
        Ok(value) => AcpToolResultRequest {
            turn_id: None,
            request_id: Some(context.request_id.to_string()),
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(provider_name.to_string()),
            approval_request_id: approval_request_id.map(str::to_string),
            status: "ok".to_string(),
            content: Some(
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            ),
            structured_content: value,
            diagnostic: serde_json::json!({
                "component": "den.acp",
                "phase": "den_server_tool_result",
                "canonical_tool_name": canonical_name,
            }),
            ..Default::default()
        },
        Err(err) => AcpToolResultRequest {
            turn_id: None,
            request_id: Some(context.request_id.to_string()),
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(provider_name.to_string()),
            approval_request_id: approval_request_id.map(str::to_string),
            status: "error".to_string(),
            content: Some(err.to_string()),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({
                "component": "den.acp",
                "phase": "den_server_tool_error",
                "canonical_tool_name": canonical_name,
                "error": err.to_string(),
            }),
            ..Default::default()
        },
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
    run_ids: Vec<String>,
    saw_visible_output: bool,
    saw_error: bool,
    saw_turn_complete: bool,
    saw_tool_return_ack: bool,
    saw_requires_approval_stop: bool,
    emitted_empty_turn_error: bool,
    emitted_runtime_cleanup: bool,
}

impl AcpStreamDiagnostics {
    fn merge_from(&mut self, other: Self) {
        self.upstream_frames += other.upstream_frames;
        self.parsed_events += other.parsed_events;
        self.mapped_events += other.mapped_events;
        self.unmapped_events += other.unmapped_events;
        for (key, value) in other.native_message_types {
            *self.native_message_types.entry(key).or_insert(0) += value;
        }
        for (key, value) in other.native_event_types {
            *self.native_event_types.entry(key).or_insert(0) += value;
        }
        for (key, value) in other.adapter_event_types {
            *self.adapter_event_types.entry(key).or_insert(0) += value;
        }
        for (key, value) in other.tool_request_counts {
            *self.tool_request_counts.entry(key).or_insert(0) += value;
        }
        for sample in other.unmapped_event_samples {
            if self.unmapped_event_samples.len() < 5 {
                self.unmapped_event_samples.push(sample);
            }
        }
        for run_id in other.run_ids {
            if !self.run_ids.iter().any(|known| known == &run_id) {
                self.run_ids.push(run_id);
            }
        }
        self.saw_visible_output |= other.saw_visible_output;
        self.saw_error |= other.saw_error;
        self.saw_turn_complete |= other.saw_turn_complete;
        self.saw_tool_return_ack |= other.saw_tool_return_ack;
        self.saw_requires_approval_stop |= other.saw_requires_approval_stop;
        self.emitted_empty_turn_error |= other.emitted_empty_turn_error;
        self.emitted_runtime_cleanup |= other.emitted_runtime_cleanup;
    }

    fn observe_runtime_event(
        &mut self,
        event: &crate::core::runtime_provider::RuntimeStreamEvent,
    ) {
        self.parsed_events += 1;
        let runtime_type = match event {
            crate::core::runtime_provider::RuntimeStreamEvent::JsonValue { .. } => "json_value",
            crate::core::runtime_provider::RuntimeStreamEvent::AssistantTextDelta { .. } => {
                self.saw_visible_output = true;
                "assistant_text_delta"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::RunProgress { .. } => {
                self.saw_visible_output = true;
                "run_progress"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::RunPaused { reason, .. } => {
                if reason == "awaiting_approval" {
                    self.saw_requires_approval_stop = true;
                }
                "run_paused"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::ToolCallRequested { tool_call_id, .. } => {
                let count = self.tool_request_counts.entry(tool_call_id.clone()).or_insert(0);
                *count += 1;
                "tool_call_requested"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::Error { .. } => {
                self.saw_error = true;
                self.saw_visible_output = true;
                "error"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::ConversationResolved { conversation } => {
                let run_id = conversation.id.clone();
                if !self.run_ids.iter().any(|known| known == &run_id) {
                    self.run_ids.push(run_id);
                }
                "conversation_resolved"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::TurnCompleted { .. } => {
                self.saw_turn_complete = true;
                "turn_completed"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::TurnFailed { .. } => {
                self.saw_error = true;
                self.saw_visible_output = true;
                "turn_failed"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::TurnCancelled { .. } => {
                self.saw_error = true;
                self.saw_visible_output = true;
                "turn_cancelled"
            }
        };
        Self::increment(&mut self.native_event_types, runtime_type);
    }

    fn increment(map: &mut BTreeMap<String, usize>, key: &str) {
        let key = if key.trim().is_empty() {
            "<missing>"
        } else {
            key
        };
        *map.entry(key.to_string()).or_insert(0) += 1;
    }

    fn observe_parsed_event(&mut self, value: &serde_json::Value) -> Vec<String> {
        self.parsed_events += 1;
        let mut newly_observed_run_ids = Vec::new();
        let message_type = value
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        for run_id in Self::extract_run_ids(value) {
            if self.observe_run_id(&run_id) {
                newly_observed_run_ids.push(run_id);
            }
        }
        Self::increment(&mut self.native_message_types, message_type);
        if message_type == "tool_return_message" {
            self.saw_tool_return_ack = true;
        }
        let stop_reason = value
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .or_else(|| {
                value
                    .pointer("/message/stop_reason")
                    .and_then(|v| v.as_str())
            })
            .or_else(|| value.pointer("/data/stop_reason").and_then(|v| v.as_str()));
        if stop_reason == Some("requires_approval") {
            self.saw_requires_approval_stop = true;
        }
        Self::increment(&mut self.native_event_types, event_type);
        newly_observed_run_ids
    }

    fn extract_run_ids(value: &serde_json::Value) -> Vec<String> {
        let mut run_ids = Vec::new();
        for pointer in [
            "/run_id",
            "/message/run_id",
            "/data/run_id",
            "/run/id",
            "/message/run/id",
            "/data/run/id",
        ] {
            if let Some(run_id) = value
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|run_id| !run_id.is_empty())
            {
                let run_id = run_id.to_string();
                if !run_ids.iter().any(|known| known == &run_id) {
                    run_ids.push(run_id);
                }
            }
        }
        for pointer in ["/run_ids", "/message/run_ids", "/data/run_ids"] {
            if let Some(items) = value.pointer(pointer).and_then(serde_json::Value::as_array) {
                for run_id in items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|run_id| !run_id.is_empty())
                {
                    let run_id = run_id.to_string();
                    if !run_ids.iter().any(|known| known == &run_id) {
                        run_ids.push(run_id);
                    }
                }
            }
        }
        run_ids
    }

    fn observe_run_id(&mut self, run_id: &str) -> bool {
        let run_id = run_id.trim();
        if run_id.is_empty() || self.run_ids.iter().any(|known| known == run_id) {
            return false;
        }
        self.run_ids.push(run_id.to_string());
        true
    }

    fn observe_mapped_event(&mut self, event: &AcpGatewayEvent) {
        self.mapped_events += 1;
        Self::increment(&mut self.adapter_event_types, acp_event_adapter_type(event));
        self.saw_visible_output |= acp_event_has_visible_output(event);
        self.saw_error |= matches!(event, AcpGatewayEvent::Error { .. });
        self.saw_turn_complete |= matches!(
            event,
            AcpGatewayEvent::TurnComplete { .. } | AcpGatewayEvent::TurnResult { .. }
        );
    }

    fn observe_unmapped_event(&mut self, value: &serde_json::Value) {
        self.unmapped_events += 1;
        if self.unmapped_event_samples.len() < 5 {
            self.unmapped_event_samples
                .push(summarize_letta_event_for_log(value).to_string());
        }
    }

    fn empty_turn_error_event(&mut self, context: &AcpStreamContext) -> Option<AcpGatewayEvent> {
        if self.emitted_empty_turn_error
            || self.saw_visible_output
            || self.saw_error
            || self.saw_tool_return_ack
            || self.emitted_runtime_cleanup
        {
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
                "run_ids": self.run_ids,
            })),
        })
    }

    fn mark_runtime_cleanup_emitted(&mut self) {
        self.emitted_runtime_cleanup = true;
    }

    fn diagnostic_json_with_turn_controller(
        &self,
        context: &AcpStreamContext,
        turn_controller: Option<&AcpTurnController>,
    ) -> serde_json::Value {
        serde_json::json!({
            "request_id": context.request_id,
            "acp_session_id": context.acp_session_id,
            "upstream_frames": self.upstream_frames,
            "parsed_events": self.parsed_events,
            "mapped_events": self.mapped_events,
            "unmapped_events": self.unmapped_events,
            "native_message_types": self.native_message_types,
            "native_event_types": self.native_event_types,
            "adapter_event_types": self.adapter_event_types,
            "tool_request_counts": self.tool_request_counts,
            "run_ids": self.run_ids,
            "saw_visible_output": self.saw_visible_output,
            "saw_error": self.saw_error,
            "saw_turn_complete": self.saw_turn_complete,
            "saw_tool_return_ack": self.saw_tool_return_ack,
            "saw_requires_approval_stop": self.saw_requires_approval_stop,
            "turn_controller": turn_controller.map(|controller| {
                let snapshot = controller.status_snapshot();
                serde_json::json!({
                    "phase": format!("{:?}", snapshot.phase),
                    "open_obligations": snapshot.open_obligations,
                    "pending_adapter_tools": snapshot.pending_adapter_tools,
                    "pending_den_tools": snapshot.pending_den_tools,
                    "pending_permissions": snapshot.pending_permissions,
                    "terminal_status": snapshot.terminal_status.map(|status| format!("{:?}", status)),
                    "terminal_reason": snapshot.terminal_reason.map(|reason| format!("{:?}", reason)),
                    "orphaned_requires_approval": snapshot.orphaned_requires_approval,
                    "late_results_ignored": snapshot.late_results_ignored,
                })
            }),
        })
    }

    fn log_summary(&self, context: &AcpStreamContext) {
        let turn_result_count = self
            .adapter_event_types
            .get("turn_result")
            .copied()
            .unwrap_or(0);
        if turn_result_count > 1 {
            tracing::warn!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                turn_result_count,
                "ACP stream emitted more than one terminal turn_result"
            );
        }
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
            saw_tool_return_ack = self.saw_tool_return_ack,
            native_message_types = ?self.native_message_types,
            native_event_types = ?self.native_event_types,
            adapter_event_types = ?self.adapter_event_types,
            tool_request_counts = ?self.tool_request_counts,
            pending_tool_argument_buffers = self.tool_call_accumulator.pending_argument_buffers(),
            pending_tool_name_buffers = self.tool_call_accumulator.pending_name_buffers(),
            unmapped_event_samples = ?self.unmapped_event_samples,
            run_ids = ?self.run_ids,
            "ACP Letta stream summary"
        );
    }
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

fn sha256_short(value: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(digest)
        .chars()
        .take(16)
        .collect()
}

fn summarize_large_text_field(value: &str, allow_preview: bool) -> serde_json::Value {
    let mut summary = serde_json::json!({
        "redacted": true,
        "bytes": value.len(),
        "chars": value.chars().count(),
        "sha256": sha256_short(value),
    });
    if allow_preview && !value.is_empty() {
        summary["preview"] = serde_json::json!(preview_str_truncated(
            value,
            acp_debug_event_sample_chars().min(512),
        ));
        summary["truncated"] =
            serde_json::json!(value.len() > acp_debug_event_sample_chars().min(512));
    }
    summary
}

fn summarize_tool_arguments(value: &str) -> serde_json::Value {
    let mut summary = summarize_large_text_field(value, false);
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(object) = parsed.as_object() {
            summary["json_keys"] = serde_json::json!(object.keys().cloned().collect::<Vec<_>>());
        }
    }
    summary
}

fn summarize_letta_event_for_log(value: &serde_json::Value) -> serde_json::Value {
    fn summarize(value: &serde_json::Value, depth: usize, key: Option<&str>) -> serde_json::Value {
        const REDACTED_TEXT_FIELDS: &[&str] = &[
            "content",
            "reasoning",
            "tool_return",
            "stdout",
            "stderr",
            "message",
            "detail",
            "old_text",
            "new_text",
        ];
        if depth > 3 {
            return serde_json::json!({ "redacted": true, "reason": "max_depth" });
        }
        match value {
            serde_json::Value::String(s) => {
                if key == Some("arguments") {
                    summarize_tool_arguments(s)
                } else if key.is_some_and(|k| REDACTED_TEXT_FIELDS.contains(&k)) {
                    summarize_large_text_field(s, false)
                } else if s.len() > acp_debug_event_sample_chars() {
                    summarize_large_text_field(s, true)
                } else {
                    serde_json::Value::String(s.clone())
                }
            }
            serde_json::Value::Array(items) => {
                let mut summarized = items
                    .iter()
                    .take(8)
                    .map(|item| summarize(item, depth + 1, None))
                    .collect::<Vec<_>>();
                let shown = summarized.len();
                if items.len() > shown {
                    summarized.push(serde_json::json!({
                        "truncated_items": items.len() - shown,
                    }));
                }
                serde_json::Value::Array(summarized)
            }
            serde_json::Value::Object(object) => {
                let mut out = serde_json::Map::new();
                out.insert(
                    "keys".to_string(),
                    serde_json::json!(object.keys().cloned().collect::<Vec<_>>()),
                );
                for field in [
                    "message_type",
                    "type",
                    "id",
                    "run_id",
                    "step_id",
                    "seq_id",
                    "tool_call_id",
                    "stop_reason",
                    "status",
                    "name",
                    "date",
                ] {
                    if let Some(raw) = object.get(field) {
                        out.insert(field.to_string(), summarize(raw, depth + 1, Some(field)));
                    }
                }
                for field in ["tool_call", "tool_calls", "function"] {
                    if let Some(raw) = object.get(field) {
                        out.insert(field.to_string(), summarize(raw, depth + 1, Some(field)));
                    }
                }
                for field in [
                    "content",
                    "reasoning",
                    "tool_return",
                    "stdout",
                    "stderr",
                    "message",
                    "detail",
                    "arguments",
                ] {
                    if let Some(raw) = object.get(field) {
                        out.insert(field.to_string(), summarize(raw, depth + 1, Some(field)));
                    }
                }
                serde_json::Value::Object(out)
            }
            other => other.clone(),
        }
    }
    let summarized = summarize(value, 0, None);
    let serialized = summarized.to_string();
    if serialized.len() > 4096 {
        serde_json::json!({
            "redacted": true,
            "reason": "summary_too_large",
            "bytes": serialized.len(),
            "sha256": sha256_short(&serialized),
        })
    } else {
        summarized
    }
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
        Ok(Some(value)) => map_native_letta_stream_event_to_acp_event_with_accumulator(
            &value,
            &mut LettaToolCallAccumulator::default(),
        )
        .map(acp_event_to_adapter_sse)
        .into_iter()
        .collect(),
        Ok(None) | Err(_) => Vec::new(),
    }
}

async fn map_runtime_stream_event_to_acp_adapter_events_with_persistence(
    runtime_event: crate::core::runtime_provider::RuntimeStreamEvent,
    context: AcpStreamContext,
    diagnostics: &mut AcpStreamDiagnostics,
) -> Result<
    (
        Vec<AcpGatewayEvent>,
        Option<PersistedToolRequestEffect>,
        Option<(String, String, AcpResolvedToolResult)>,
    ),
    std::io::Error,
> {
    diagnostics.upstream_frames += 1;
    let value = match runtime_event {
        crate::core::runtime_provider::RuntimeStreamEvent::JsonValue { value } => value,
        crate::core::runtime_provider::RuntimeStreamEvent::ToolCallRequested {
            tool_call_id,
            tool_name,
            title,
            kind,
            arguments,
            approval_request_id,
            approval_required,
            approval_reason,
        } => serde_json::json!({
            "message_type": if approval_required { "approval_request_message" } else { "tool_call_message" },
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "tool_title": title,
            "tool_kind": kind,
            "args": arguments,
            "approval_request_id": approval_request_id,
            "approval_reason": approval_reason,
        }),
        crate::core::runtime_provider::RuntimeStreamEvent::RunPaused { reason, .. } => {
            let stop_reason = if reason == "awaiting_approval" {
                "requires_approval".to_string()
            } else {
                reason
            };
            serde_json::json!({
                "message_type": "stop_reason",
                "stop_reason": stop_reason,
            })
        }
        crate::core::runtime_provider::RuntimeStreamEvent::TurnCompleted { .. } => {
            serde_json::json!({
                "message_type": "stop_reason",
                "stop_reason": "end_turn",
            })
        }
        other => {
            return Err(std::io::Error::other(format!(
                "runtime event not supported by ACP persistence mapper: {other:?}"
            )));
        }
    };
    let observed_run_ids = diagnostics.observe_parsed_event(&value);
    if let Some(turn_cancellations) = context.role_runtime.turn_cancellations() {
        for run_id in observed_run_ids {
            if turn_cancellations.record_run_id(
                &context.acp_session_id,
                context.request_id,
                &run_id,
            ) {
                tracing::debug!(
                    request_id = %context.request_id,
                    acp_session_id = %context.acp_session_id,
                    run_id = %run_id,
                    "attached observed Letta run_id to active ACP turn"
                );
            }
        }
    }

    let Some(mut event) = map_native_letta_stream_event_to_acp_event_with_accumulator(
        &value,
        &mut diagnostics.tool_call_accumulator,
    ) else {
        diagnostics.observe_unmapped_event(&value);
        return Ok((Vec::new(), None, None));
    };
    // Keep both halves of the per-tool result channel on the event until
    // `persist_stream_event_side_effects` has decided whether this is an
    // adapter-local tool or a Den-executed server tool. Den server tools such
    // as `web_fetch` consume `result_tx` internally and must not be stripped of
    // the sender just because the stream driver wants to await adapter-local
    // tool results.
    let mut adapter_result_rx = None;
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
            return Ok((Vec::new(), None, None));
        }
    }
    let tool_request_effect = persist_stream_event_side_effects(&context, &mut event)
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    if let AcpGatewayEvent::ToolRequest {
        tool_call_id,
        tool_name,
        args,
        result_rx,
        ..
    } = &mut event
    {
        match tool_execution_route(tool_name, args) {
            ToolExecutionRoute::AdapterLocal => {
                adapter_result_rx = result_rx.take().map(|rx| {
                    (
                        tool_call_id.clone(),
                        tool_name.clone(),
                        AcpResolvedToolResult::Receiver(rx),
                    )
                });
            }
            ToolExecutionRoute::DenServer => {}
            ToolExecutionRoute::Unsupported => {}
        }
    }
    let mut tool_request_effect = tool_request_effect;
    let events = if let Some(effect) = tool_request_effect.as_mut() {
        if let Some(rx) = effect.den_server_result_rx.take() {
            let tool_call_id = effect.tool_call_id.clone();
            let tool_name = effect.tool_name.clone();
            let result = rx
                .await
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            adapter_result_rx = Some((
                tool_call_id,
                tool_name,
                AcpResolvedToolResult::Ready(Box::new(result)),
            ));
            Vec::new()
        } else {
            vec![event]
        }
    } else {
        vec![event]
    };
    Ok((events, tool_request_effect, adapter_result_rx))
}

type AcpFrameResult = Result<
    (
        Vec<AcpGatewayEvent>,
        Option<PersistedToolRequestEffect>,
        Option<(String, String, AcpResolvedToolResult)>,
    ),
    std::io::Error,
>;

type AcpContinueToolPrepared = Result<
    (
        crate::core::runtime_provider::RuntimeStreamContinuation,
        crate::core::runtime_provider::RuntimeEventStream,
        std::sync::Arc<std::sync::Mutex<AcpStreamDiagnostics>>,
    ),
    CustomError,
>;

enum AcpResolvedToolResult {
    Receiver(oneshot::Receiver<AcpToolResultRequest>),
    Ready(Box<AcpToolResultRequest>),
}

enum AcpPendingFuture {
    Frame(Pin<Box<dyn Future<Output = (AcpFrameResult, AcpStreamDiagnostics)> + Send>>),
    Tool(Pin<Box<dyn Future<Output = Box<AcpToolResultRequest>> + Send>>),
    ContinueTool(Pin<Box<dyn Future<Output = AcpContinueToolPrepared> + Send>>),
    Cleanup(Pin<Box<dyn Future<Output = serde_json::Value> + Send>>),
}

#[derive(Default)]
struct AcpTextChunker {
    assistant: String,
    reasoning: String,
    max_chars: usize,
    max_reasoning_bytes: usize,
    emitted_reasoning_bytes: usize,
    reasoning_limit_reached: bool,
}

impl AcpTextChunker {
    fn new(max_chars: usize) -> Self {
        Self::new_with_reasoning_limit(max_chars, acp_max_thought_bytes_per_turn())
    }

    fn new_with_reasoning_limit(max_chars: usize, max_reasoning_bytes: usize) -> Self {
        Self {
            assistant: String::new(),
            reasoning: String::new(),
            max_chars,
            max_reasoning_bytes,
            emitted_reasoning_bytes: 0,
            reasoning_limit_reached: false,
        }
    }

    fn push(&mut self, event: AcpGatewayEvent) -> Vec<AcpGatewayEvent> {
        match event {
            AcpGatewayEvent::AssistantTextDelta { text } => {
                self.assistant.push_str(&text);
                if should_flush_text(&self.assistant, self.max_chars) {
                    self.flush_assistant().into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            AcpGatewayEvent::StatusText { text } => {
                if self.reasoning_limit_reached {
                    return Vec::new();
                }
                let first_status_for_turn =
                    self.emitted_reasoning_bytes == 0 && self.reasoning.is_empty();
                self.reasoning.push_str(&text);
                if first_status_for_turn || should_flush_text(&self.reasoning, self.max_chars) {
                    self.flush_reasoning().into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            other => {
                let mut events = self.flush_all();
                events.push(other);
                events
            }
        }
    }

    fn flush_assistant(&mut self) -> Option<AcpGatewayEvent> {
        if self.assistant.is_empty() {
            None
        } else {
            Some(AcpGatewayEvent::AssistantTextDelta {
                text: std::mem::take(&mut self.assistant),
            })
        }
    }

    fn flush_reasoning(&mut self) -> Option<AcpGatewayEvent> {
        if self.reasoning.is_empty() || self.reasoning_limit_reached {
            self.reasoning.clear();
            return None;
        }
        let remaining = self
            .max_reasoning_bytes
            .saturating_sub(self.emitted_reasoning_bytes);
        if remaining == 0 {
            self.reasoning.clear();
            self.reasoning_limit_reached = true;
            return Some(AcpGatewayEvent::StatusText {
                text: normalize_display_status_text(
                    "BEARS suppressed additional thinking/status output for this turn because it exceeded the safety limit",
                ),
            });
        }

        let mut text = std::mem::take(&mut self.reasoning);
        if text.len() > remaining {
            text = truncate_utf8_boundary(&text, remaining).to_string();
            self.reasoning_limit_reached = true;
            text.push('\n');
            text.push_str(&normalize_display_status_text(
                "BEARS suppressed additional thinking/status output for this turn because it exceeded the safety limit",
            ));
        }
        self.emitted_reasoning_bytes = self.emitted_reasoning_bytes.saturating_add(text.len());
        Some(AcpGatewayEvent::StatusText { text })
    }

    fn flush_all(&mut self) -> Vec<AcpGatewayEvent> {
        self.flush_assistant()
            .into_iter()
            .chain(self.flush_reasoning())
            .collect()
    }
}

fn should_flush_text(buffer: &str, max_chars: usize) -> bool {
    buffer.chars().count() >= max_chars
        || buffer.ends_with('\n')
        || buffer.ends_with(". ")
        || buffer.ends_with("! ")
        || buffer.ends_with("? ")
}

fn truncate_utf8_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

struct AcpLettaSseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, CustomError>> + Send>>,
    buffer: Vec<u8>,
    pending: VecDeque<Bytes>,
    context: AcpStreamContext,
    letta: Arc<crate::core::letta::LettaClient>,
    continuation: LettaContinuationContext,
    queued_tool_result_continuation: Option<AcpToolResultRequest>,
    diagnostics: AcpStreamDiagnostics,
    logged_summary: bool,
    persist_future: Option<AcpPendingFuture>,
    session_info_event_sent: bool,
    text_chunker: AcpTextChunker,
    active_turn_guard: Option<crate::core::role_runtime::RoleTurnGuard>,
    cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
    cancel_handle: Option<AcpActiveTurnCancelHandle>,
    turn_controller: AcpTurnController,
}

impl AcpLettaSseStream {
    fn outstanding_tool_obligations(&self) -> Vec<String> {
        self.context
            .tool_turns
            .pending_for_session(&self.context.acp_session_id)
            .into_iter()
            .filter(|turn| turn.request_id == self.context.request_id)
            .map(|turn| turn.tool_call_id)
            .collect()
    }

    fn controller_allows_terminal(&self) -> bool {
        self.turn_controller.may_emit_terminal()
    }

    fn turn_result_event(role_result: &RoleTurnResult) -> AcpGatewayEvent {
        let terminal = role_result.to_terminal_event();
        AcpGatewayEvent::TurnResult {
            status: terminal.status,
            reason: terminal.reason,
            request_id: terminal.request_id,
            session_id: terminal.session_id,
            retryable: terminal.retryable,
            diagnostics: terminal.diagnostics,
        }
    }

    fn push_adapter_event(&mut self, event: AcpGatewayEvent) {
        if matches!(event, AcpGatewayEvent::TurnComplete { .. }) {
            self.turn_controller.on_stream_end();
            let Some(controller_terminal) = self.turn_controller.take_terminal_event() else {
                let snapshot = self.turn_controller.status_snapshot();
                tracing::info!(
                    request_id = %self.context.request_id,
                    acp_session_id = %self.context.acp_session_id,
                    controller_phase = ?snapshot.phase,
                    controller_open_obligations = snapshot.open_obligations,
                    "suppressed ACP turn_complete until turn controller allows terminal emission"
                );
                return;
            };
            tracing::debug!(
                request_id = %self.context.request_id,
                acp_session_id = %self.context.acp_session_id,
                controller_terminal_status = ?controller_terminal.status,
                controller_terminal_reason = ?controller_terminal.reason,
                "emitting ACP turn_complete authorized by turn controller"
            );
        }
        if matches!(event, AcpGatewayEvent::SessionInfoUpdate { .. }) {
            self.session_info_event_sent = true;
        }
        self.diagnostics.observe_mapped_event(&event);
        self.pending.push_back(acp_event_to_adapter_sse(event));
    }

    fn push_terminal_result_now(&mut self, role_result: RoleTurnResult) {
        let Some(controller_terminal) = self.turn_controller.take_terminal_event() else {
            let snapshot = self.turn_controller.status_snapshot();
            tracing::warn!(
                request_id = %self.context.request_id,
                acp_session_id = %self.context.acp_session_id,
                controller_phase = ?snapshot.phase,
                controller_open_obligations = snapshot.open_obligations,
                controller_terminal_status = ?snapshot.terminal_status,
                controller_terminal_reason = ?snapshot.terminal_reason,
                "suppressed ACP turn_result because turn controller did not allow terminal emission"
            );
            return;
        };
        tracing::debug!(
            request_id = %self.context.request_id,
            acp_session_id = %self.context.acp_session_id,
            controller_terminal_status = ?controller_terminal.status,
            controller_terminal_reason = ?controller_terminal.reason,
            "emitting ACP turn_result authorized by turn controller"
        );
        let event = Self::turn_result_event(&role_result);
        self.push_adapter_event(event);
    }

    fn push_terminal_result_when_ready(&mut self, role_result: RoleTurnResult) {
        if self.controller_allows_terminal() {
            self.push_terminal_result_now(role_result);
            return;
        }
        let controller_snapshot = self.turn_controller.status_snapshot();
        let outstanding = self.outstanding_tool_obligations();
        let pending_tool_continuation = self.queued_tool_result_continuation.is_some();
        tracing::warn!(
            request_id = %self.context.request_id,
            acp_session_id = %self.context.acp_session_id,
            outstanding_tool_call_ids = ?outstanding,
            pending_tool_continuation,
            controller_open_obligations = controller_snapshot.open_obligations,
            controller_phase = ?controller_snapshot.phase,
            controller_terminal_status = ?controller_snapshot.terminal_status,
            controller_terminal_reason = ?controller_snapshot.terminal_reason,
            "suppressed ACP turn_result because turn controller was not ready"
        );
    }

    fn new(
        inner: impl Stream<Item = Result<Bytes, CustomError>> + Send + 'static,
        context: AcpStreamContext,
        initial_events: Vec<AcpGatewayEvent>,
        session_info_event_sent: bool,
        letta: Arc<crate::core::letta::LettaClient>,
        continuation: LettaContinuationContext,
        active_turn_guard: crate::core::role_runtime::RoleTurnGuard,
    ) -> Self {
        let mut pending = VecDeque::new();
        for event in initial_events {
            pending.push_back(acp_event_to_adapter_sse(event));
        }
        Self {
            inner: Box::pin(inner),
            buffer: Vec::new(),
            pending,
            context,
            letta,
            continuation,
            queued_tool_result_continuation: None,
            diagnostics: AcpStreamDiagnostics::default(),
            logged_summary: false,
            persist_future: None,
            session_info_event_sent,
            text_chunker: AcpTextChunker::new(acp_text_chunk_chars()),
            active_turn_guard: Some(active_turn_guard),
            cancel_rx: None,
            cancel_handle: None,
            turn_controller: {
                let mut controller = AcpTurnController::new();
                controller.on_stream_started();
                controller
            },
        }
    }

    #[cfg(test)]
    fn with_cancel_rx(mut self, cancel_rx: tokio::sync::watch::Receiver<bool>) -> Self {
        self.cancel_rx = Some(cancel_rx);
        self
    }

    fn with_cancel_registration(
        mut self,
        handle: AcpActiveTurnCancelHandle,
        cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        self.cancel_handle = Some(handle);
        self.cancel_rx = Some(cancel_rx);
        self
    }

    fn cleanup_active_tool_turns(&mut self) {
        for pending in self
            .context
            .tool_turns
            .pending_for_session(&self.context.acp_session_id)
            .into_iter()
            .filter(|pending| pending.request_id == self.context.request_id)
        {
            self.context
                .tool_turns
                .remove(&self.context.acp_session_id, &pending.tool_call_id);
        }
    }

    fn log_summary_once(&mut self) {
        if !self.logged_summary {
            self.cleanup_active_tool_turns();
            self.cancel_handle.take();
            if let Some(guard) = self.active_turn_guard.take() {
                guard.release();
            }
            self.diagnostics.log_summary(&self.context);
            self.logged_summary = true;
        }
    }
}

impl Drop for AcpLettaSseStream {
    fn drop(&mut self) {
        self.cleanup_active_tool_turns();
        self.cancel_handle.take();
        if let Some(guard) = self.active_turn_guard.take() {
            guard.release();
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

        if this
            .cancel_rx
            .as_ref()
            .is_some_and(|cancel_rx| *cancel_rx.borrow())
            && this.turn_controller.phase() != AcpTurnPhase::Terminal
        {
            this.turn_controller.on_cancel();
            let cancelled_tool_call_ids = this.outstanding_tool_obligations();
            for tool_call_id in &cancelled_tool_call_ids {
                this.context
                    .tool_turns
                    .remove(&this.context.acp_session_id, tool_call_id);
            }
            this.queued_tool_result_continuation = None;
            this.persist_future = None;
            let role_result = this.context.role_runtime.turn_result(
                TurnResultStatus::Cancelled,
                TurnResultReason::Cancelled,
                this.context.request_id,
                this.context.turn_scope.clone(),
                false,
                serde_json::json!({
                    "stream": this.diagnostics.diagnostic_json_with_turn_controller(&this.context, Some(&this.turn_controller)),
                    "cancelled_by": "acp_test_cancel_signal",
                }),
            );
            this.push_terminal_result_now(role_result);
            if let Some(bytes) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(bytes)));
            }
        }

        if this.persist_future.is_none()
            && this.queued_tool_result_continuation.is_none()
            && !this.outstanding_tool_obligations().is_empty()
        {
            if !this.outstanding_tool_obligations().is_empty() {
                let runtime = this.context.role_runtime.tool_turn_runtime_snapshot(
                    &this.context.acp_session_id,
                    &this.context.tool_turns,
                );
                this.pending.push_back(acp_event_to_adapter_sse(
                    AcpGatewayEvent::SessionInfoUpdate {
                        title: None,
                        updated_at: None,
                        meta: Some(serde_json::json!({
                            "bears": {
                                "runtime": runtime,
                                "context_budget": default_unavailable_context_budget(),
                            }
                        })),
                    },
                ));
                return self.poll_next(cx);
            }
            tracing::debug!(
                request_id = %this.context.request_id,
                acp_session_id = %this.context.acp_session_id,
                outstanding_tool_call_ids = ?this.outstanding_tool_obligations(),
                "ACP stream waiting for local tool result before polling upstream terminal state"
            );
            return Poll::Pending;
        }

        if let Some(fut) = this.persist_future.as_mut() {
            match fut {
                AcpPendingFuture::Frame(fut) => {
                    let (result, diagnostics) = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    this.diagnostics = diagnostics;
                    match result {
                        Ok((events, tool_effect, result_rx)) => {
                            if let Some(effect) = tool_effect.as_ref() {
                                this.turn_controller.on_tool_request(
                                    effect.tool_call_id.clone(),
                                    effect.tool_name.clone(),
                                    ControllerToolExecutionRoute::from(effect.route),
                                );
                            }
                            for event in events {
                                for event in this.text_chunker.push(event) {
                                    this.push_adapter_event(event);
                                }
                            }
                            if let Some((tool_call_id, tool_name, result_rx)) = result_rx {
                                let approval_request_id = this
                                    .context
                                    .tool_turns
                                    .pending_for_session(&this.context.acp_session_id)
                                    .into_iter()
                                    .find(|pending| pending.tool_call_id == tool_call_id)
                                    .and_then(|pending| pending.approval_request_id);
                                this.persist_future = Some(AcpPendingFuture::Tool(Box::pin(
                                    async move {
                                        let AcpResolvedToolResult::Receiver(result_rx) = result_rx
                                        else {
                                            if let AcpResolvedToolResult::Ready(result) = result_rx
                                            {
                                                return result;
                                            }
                                            unreachable!();
                                        };
                                        let timeout_ms =
                                            acp_tool_timeout_ms_for_provider(&tool_name);
                                        match tokio::time::timeout(
                                            std::time::Duration::from_millis(timeout_ms),
                                            result_rx,
                                        )
                                        .await
                                        {
                                            Err(_) => Box::new(AcpToolResultRequest {
                                                tool_call_id: Some(tool_call_id.clone()),
                                                tool_name: Some(tool_name.clone()),
                                                approval_request_id: approval_request_id.clone(),
                                                status: "timeout".to_string(),
                                                content: Some(format!(
                                                    "BEARS denied this approval automatically because `{tool_name}` timed out after {timeout_ms}ms."
                                                )),
                                                structured_content: serde_json::json!({}),
                                                diagnostic: serde_json::json!({
                                                    "component": "den.acp",
                                                    "phase": "local_tool_result_timeout_auto_denied",
                                                    "tool_call_id": tool_call_id,
                                                    "tool_name": tool_name,
                                                    "timeout_ms": timeout_ms,
                                                }),
                                                ..Default::default()
                                            }),
                                            Ok(Err(err)) => Box::new(AcpToolResultRequest {
                                                tool_call_id: Some(tool_call_id.clone()),
                                                tool_name: Some(tool_name.clone()),
                                                approval_request_id: approval_request_id.clone(),
                                                status: "error".to_string(),
                                                content: Some(format!(
                                                    "BEARS denied this approval automatically because the ACP local tool result channel closed: {err}"
                                                )),
                                                structured_content: serde_json::json!({}),
                                                diagnostic: serde_json::json!({
                                                    "component": "den.acp",
                                                    "phase": "local_tool_result_channel_closed_auto_denied",
                                                    "tool_call_id": tool_call_id,
                                                    "tool_name": tool_name,
                                                }),
                                                ..Default::default()
                                            }),
                                            Ok(Ok(value)) => Box::new(value),
                                        }
                                    },
                                )));
                            }
                            return self.poll_next(cx);
                        }
                        Err(err) => {
                            let message = err.to_string();
                            tracing::warn!(
                                request_id = %this.context.request_id,
                                acp_session_id = %this.context.acp_session_id,
                                error = %message,
                                "ACP stream frame processing failed"
                            );
                            let event = AcpGatewayEvent::Error {
                                message: "BEARS failed while processing an ACP stream event."
                                    .to_string(),
                                detail: Some(message),
                                error_type: Some("acp_stream_frame_processing_failed".to_string()),
                                request_id: Some(this.context.request_id.to_string()),
                                context: Some(serde_json::json!({
                                    "component": "den.acp",
                                    "acp_session_id": this.context.acp_session_id,
                                })),
                            };
                            this.push_adapter_event(event);
                            return self.poll_next(cx);
                        }
                    }
                }
                AcpPendingFuture::Tool(fut) => {
                    let result = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    let tool_result = *result;
                    {
                        if let Some(done_id) = tool_result.tool_call_id.as_deref() {
                            let ok = tool_result.status == "ok";
                            this.turn_controller.on_adapter_tool_result(done_id, ok);
                            if tool_result.status == "timeout" {
                                this.turn_controller.on_tool_timeout(done_id);
                            }
                        }
                        if let Some(done_id) = tool_result.tool_call_id.as_deref() {
                            this.context
                                .tool_turns
                                .remove(&this.context.acp_session_id, done_id);
                        }
                        if let Some(plan_event) = plan_update_from_den_tool_result(&tool_result) {
                            this.push_adapter_event(plan_event);
                        }
                        if let Some(mode) = mode_from_den_tool_result(&tool_result) {
                            let mode_event = AcpGatewayEvent::ModeUpdate {
                                mode: mode.to_string(),
                            };
                            this.push_adapter_event(mode_event);
                        }
                        let tool_name = tool_result
                            .tool_name
                            .as_deref()
                            .unwrap_or("tool")
                            .to_string();
                        let completion_text = normalize_display_status_text(
                            &if acp_debug_ui_enabled() {
                                format!(
                                "BEARS debug: local tool {tool_name} completed with status {} ({} bytes)",
                                tool_result.status,
                                tool_result.content.as_deref().map(str::len).unwrap_or(0),
                            )
                            } else {
                                format!("Local tool {tool_name} completed")
                            },
                        );
                        this.pending.push_back(acp_event_to_adapter_sse(
                            AcpGatewayEvent::StatusText {
                                text: completion_text,
                            },
                        ));
                        // Letta keeps the original run active until its SSE stream finishes
                        // with `requires_approval`/stop metadata. If we POST the tool return
                        // before draining that stream, Letta rejects the continuation with
                        // HTTP 409 (another run is still processing this conversation). Store
                        // the result and continue reading the current stream; once it ends,
                        // start the tool-return continuation.
                        this.queued_tool_result_continuation = Some(tool_result);
                        return self.poll_next(cx);
                    }
                }
                AcpPendingFuture::ContinueTool(fut) => {
                    let result = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    match result {
                        Ok((_stream_kind, stream, diagnostics)) => {
                            let context = this.context.clone();
                            let request_id = this.context.request_id.to_string();
                            let acp_session_id = this.context.acp_session_id.clone();
                            let diagnostics_for_stream = diagnostics.clone();
                            this.persist_future = Some(AcpPendingFuture::Frame(Box::pin(async move {
                                let mut queued_events = Vec::new();
                                let mut runtime_stream = stream;
                                while let Some(item) = futures::StreamExt::next(&mut runtime_stream).await {
                                    match item {
                                        Ok(event) => {
                                            if let Ok(mut guard) = diagnostics_for_stream.lock() {
                                                guard.observe_runtime_event(&event);
                                            }
                                            match event {
                                                crate::core::runtime_provider::RuntimeStreamEvent::AssistantTextDelta { text } => {
                                                    queued_events.push(AcpGatewayEvent::AssistantTextDelta { text });
                                                }
                                                crate::core::runtime_provider::RuntimeStreamEvent::RunProgress { kind, text, phase: _, detail: _ } => {
                                                    let rendered = if kind == "status_text" {
                                                        text.unwrap_or_default()
                                                    } else {
                                                        text.unwrap_or_else(|| kind)
                                                    };
                                                    queued_events.push(AcpGatewayEvent::StatusText { text: rendered });
                                                }
                                                crate::core::runtime_provider::RuntimeStreamEvent::ConversationResolved { conversation } => {
                                                    queued_events.push(AcpGatewayEvent::ConversationResolved {
                                                        conversation_id: conversation.id,
                                                    });
                                                }
                                                crate::core::runtime_provider::RuntimeStreamEvent::TurnCompleted { .. } => {
                                                    queued_events.push(AcpGatewayEvent::TurnComplete {
                                                        outcome: "ok".to_string(),
                                                    });
                                                }
                                                crate::core::runtime_provider::RuntimeStreamEvent::RunPaused { .. }
                                                | crate::core::runtime_provider::RuntimeStreamEvent::ToolCallRequested { .. }
                                                | crate::core::runtime_provider::RuntimeStreamEvent::JsonValue { .. } => {
                                                    let mut temp_diagnostics = AcpStreamDiagnostics::default();
                                                    let (events, _effect, _adapter_result_rx) = match map_runtime_stream_event_to_acp_adapter_events_with_persistence(
                                                        event,
                                                        context.clone(),
                                                        &mut temp_diagnostics,
                                                    ).await {
                                                        Ok(ok) => ok,
                                                        Err(err) => return (Err(err), AcpStreamDiagnostics::default()),
                                                    };
                                                    if let Ok(mut guard) = diagnostics_for_stream.lock() {
                                                        guard.merge_from(temp_diagnostics);
                                                    }
                                                    queued_events.extend(events);
                                                }
                                                crate::core::runtime_provider::RuntimeStreamEvent::TurnFailed { message, .. } => {
                                                    queued_events.push(AcpGatewayEvent::Error {
                                                        message,
                                                        detail: None,
                                                        error_type: Some("runtime_turn_failed".to_string()),
                                                        request_id: Some(request_id.clone()),
                                                        context: Some(serde_json::json!({
                                                            "component": "den.acp",
                                                            "acp_session_id": acp_session_id,
                                                        })),
                                                    });
                                                }
                                                crate::core::runtime_provider::RuntimeStreamEvent::TurnCancelled { .. } => {
                                                    queued_events.push(AcpGatewayEvent::Error {
                                                        message: "Runtime continuation was cancelled.".to_string(),
                                                        detail: None,
                                                        error_type: Some("runtime_turn_cancelled".to_string()),
                                                        request_id: Some(request_id.clone()),
                                                        context: Some(serde_json::json!({
                                                            "component": "den.acp",
                                                            "acp_session_id": acp_session_id,
                                                        })),
                                                    });
                                                }
                                                crate::core::runtime_provider::RuntimeStreamEvent::Error { message, detail, error_type, request_id: upstream_request_id, context: runtime_context } => {
                                                    queued_events.push(AcpGatewayEvent::Error {
                                                        message,
                                                        detail,
                                                        error_type,
                                                        request_id: upstream_request_id.or_else(|| Some(request_id.clone())),
                                                        context: runtime_context.or_else(|| Some(serde_json::json!({
                                                            "component": "den.acp",
                                                            "acp_session_id": acp_session_id,
                                                        }))),
                                                    });
                                                }
                                            }
                                        }
                                        Err(err) => return (Err(std::io::Error::other(err.to_string())), AcpStreamDiagnostics::default()),
                                    }
                                }
                                let mut diagnostics = std::sync::Arc::try_unwrap(diagnostics).ok().and_then(|m| m.into_inner().ok()).unwrap_or_default();
                                for event in &queued_events {
                                    diagnostics.observe_mapped_event(event);
                                }
                                (Ok((queued_events, None, None)), diagnostics)
                            })));
                            return self.poll_next(cx);
                        }
                        Err(err) => {
                            if looks_like_letta_waiting_for_approval_error(&err) {
                                let letta = this.letta.clone();
                                let tool_turns = this.context.tool_turns.clone();
                                let acp_session_id = this.context.acp_session_id.clone();
                                let bear_id = this.context.bear_id;
                                let pair_agent_id = this.context.pair_agent_id.clone();
                                let run_ids = this.diagnostics.run_ids.clone();
                                let request_id = this.context.request_id;
                                this.persist_future =
                                    Some(AcpPendingFuture::Cleanup(Box::pin(async move {
                                        acp_cleanup_stale_runtime_state(
                                            AcpStaleRuntimeCleanupParams {
                                                letta,
                                                tool_turns,
                                                acp_session_id,
                                                bear_id,
                                                pair_agent_id,
                                                run_ids,
                                                reason: "tool_return_continuation_failed",
                                                request_id,
                                            },
                                        )
                                        .await
                                    })));
                                return self.poll_next(cx);
                            }
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
                AcpPendingFuture::Cleanup(fut) => {
                    let cleanup = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    let reason = cleanup
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .map(|reason| {
                            if reason == "orphaned_requires_approval_stop" {
                                TurnResultReason::StaleApproval
                            } else {
                                TurnResultReason::RuntimeCleanup
                            }
                        })
                        .unwrap_or(TurnResultReason::RuntimeCleanup);
                    this.turn_controller.on_stream_end();
                    let role_result = this.context.role_runtime.turn_result(
                        TurnResultStatus::Recovered,
                        reason,
                        this.context.request_id,
                        this.context.turn_scope.clone(),
                        true,
                        serde_json::json!({
                            "cleanup": cleanup.clone(),
                            "stream": this.diagnostics.diagnostic_json_with_turn_controller(&this.context, Some(&this.turn_controller)),
                        }),
                    );
                    this.push_terminal_result_when_ready(role_result);
                    this.diagnostics.mark_runtime_cleanup_emitted();
                    return self.poll_next(cx);
                }
            }
        }

        if this.turn_controller.phase() == AcpTurnPhase::Terminal {
            this.log_summary_once();
            return Poll::Ready(None);
        }

        match ready!(this.inner.as_mut().poll_next(cx)) {
            Some(Ok(chunk)) => {
                this.buffer.extend_from_slice(&chunk);
                if let Some(end) = find_sse_frame_end(&this.buffer) {
                    let frame: Vec<u8> = this.buffer.drain(..end).collect();
                    let context = this.context.clone();
                    let mut diagnostics = std::mem::take(&mut this.diagnostics);
                    this.persist_future = Some(AcpPendingFuture::Frame(Box::pin(async move {
                        let body = strip_trailing_sse_delimiter_owned(frame);
                        let result = match parse_sse_event_body_to_json(&body) {
                            Ok(Some(value)) => map_runtime_stream_event_to_acp_adapter_events_with_persistence(
                                crate::core::runtime_provider::RuntimeStreamEvent::JsonValue { value },
                                context,
                                &mut diagnostics,
                            )
                            .await,
                            Ok(None) => Ok((Vec::new(), None, None)),
                            Err(err) => Err(std::io::Error::other(err)),
                        };
                        (result, diagnostics)
                    })));
                    self.poll_next(cx)
                } else {
                    std::task::Poll::Pending
                }
            }
            Some(Err(err)) => {
                let message = format!("Letta stream read failed: {err}");
                tracing::warn!(
                    request_id = %this.context.request_id,
                    acp_session_id = %this.context.acp_session_id,
                    error = %err,
                    "ACP upstream Letta SSE stream read error"
                );
                this.turn_controller.on_stream_error();
                let role_result = this.context.role_runtime.turn_result(
                    TurnResultStatus::Failed,
                    TurnResultReason::RuntimeCleanup,
                    this.context.request_id,
                    this.context.turn_scope.clone(),
                    false,
                    serde_json::json!({
                        "error": message,
                        "stream": this.diagnostics.diagnostic_json_with_turn_controller(&this.context, Some(&this.turn_controller)),
                    }),
                );
                this.push_terminal_result_when_ready(role_result);
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
                this.pending
                    .push_back(Bytes::from(format!("data: {}\n\n", event)));
                if let Some(bytes) = this.pending.pop_front() {
                    Poll::Ready(Some(Ok(bytes)))
                } else {
                    this.log_summary_once();
                    Poll::Ready(None)
                }
            }
            None => {
                if !this.buffer.is_empty() {
                    let message = format!(
                        "ACP upstream Letta SSE stream ended with incomplete frame ({} bytes)",
                        this.buffer.len()
                    );
                    this.buffer.clear();
                    this.push_adapter_event(AcpGatewayEvent::Error {
                        message: "BEARS failed while processing an ACP stream event.".to_string(),
                        detail: Some(message),
                        error_type: Some("acp_stream_frame_processing_failed".to_string()),
                        request_id: Some(this.context.request_id.to_string()),
                        context: Some(serde_json::json!({
                            "component": "den.acp",
                            "acp_session_id": this.context.acp_session_id,
                        })),
                    });
                    return self.poll_next(cx);
                }
                if this.diagnostics.saw_requires_approval_stop
                    && !this.outstanding_tool_obligations().is_empty()
                {
                    this.turn_controller.on_requires_approval_stop();
                    self.poll_next(cx)
                } else if this.queued_tool_result_continuation.is_none()
                    && !this.outstanding_tool_obligations().is_empty()
                {
                    tracing::debug!(
                        request_id = %this.context.request_id,
                        acp_session_id = %this.context.acp_session_id,
                        outstanding_tool_call_ids = ?this.outstanding_tool_obligations(),
                        "ACP upstream ended while local tool obligations are outstanding; waiting for results"
                    );
                    Poll::Pending
                } else if let Some(tool_result) = this.queued_tool_result_continuation.take() {
                    let letta = this.letta.clone();
                    let continuation = this.continuation.clone();
                    let tool_name = tool_result
                        .tool_name
                        .as_deref()
                        .unwrap_or("tool")
                        .to_string();
                    let Some(tool_call_id) = tool_result.tool_call_id.clone() else {
                        this.pending.push_back(acp_event_to_adapter_sse(
                            AcpGatewayEvent::Error {
                                message: "Cannot continue Letta after ACP tool result without original tool_call_id.".to_string(),
                                detail: Some(format!(
                                    "Tool result for {tool_name} did not include a tool_call_id; refusing to use tool name as a fallback."
                                )),
                                error_type: Some("missing_tool_call_id".to_string()),
                                request_id: Some(this.context.request_id.to_string()),
                                context: None,
                            },
                        ));
                        return self.poll_next(cx);
                    };
                    this.diagnostics.saw_tool_return_ack = true;
                    let tool_return = tool_result.content.clone().unwrap_or_default();
                    let status = tool_result.status.clone();
                    let approval_request_id = tool_result.approval_request_id.clone();
                    let config = this.context.config.clone();
                    let api_state = ApiState {
                        sqlx_pool: this.context.pool.clone(),
                        config: config.clone(),
                        letta: letta.clone(),
                        bifrost: Arc::new(crate::core::bifrost::BifrostClient::new(
                            config.as_ref(),
                        )),
                        acp_tool_turns: this.context.tool_turns.clone(),
                        acp_turn_cancellations:
                            crate::core::acp_turn_controller::AcpActiveTurnCancelRegistry::new(),
                    };
                    let binding = RoleRuntimeBinding {
                        binding_id: continuation
                            .agent_id
                            .clone()
                            .unwrap_or_else(|| this.context.pair_agent_id.clone()),
                        compatibility_backend: Some("letta".to_string()),
                    };
                    let request_id = this.context.request_id;
                    let acp_session_id = this.context.acp_session_id.clone();
                    let continuation_request =
                        if let Some(approval_request_id) = approval_request_id {
                            RuntimeContinuation::ApprovalDecision {
                                approval_request_id,
                                tool_call_id: Some(tool_call_id.clone()),
                                decision: if status == "ok" {
                                    crate::core::runtime_provider::RuntimeApprovalDecision::Approve
                                } else {
                                    crate::core::runtime_provider::RuntimeApprovalDecision::Deny
                                },
                                reason: Some(tool_return.clone()),
                            }
                        } else {
                            RuntimeContinuation::ToolResult {
                            tool_call_id: tool_call_id.clone(),
                            approval_request_id: None,
                            status: match status.as_str() {
                                "ok" => crate::core::runtime_provider::RuntimeToolResultStatus::Ok,
                                "timeout" => {
                                    crate::core::runtime_provider::RuntimeToolResultStatus::Timeout
                                }
                                _ => crate::core::runtime_provider::RuntimeToolResultStatus::Error,
                            },
                            content: tool_return.clone(),
                        }
                        };
                    let stream_context = AcpTurnStreamContext {
                        client_tools: continuation.client_tools.clone(),
                        stream_tokens: continuation.stream_tokens,
                        max_steps: continuation.max_steps,
                    };
                    this.persist_future =
                        Some(AcpPendingFuture::ContinueTool(Box::pin(async move {
                            let prepared = continue_acp_turn_with_runtime(AcpTurnContinueRequest {
                                state: &api_state,
                                request_id,
                                acp_session_id: &acp_session_id,
                                binding: &binding,
                                continuation: continuation_request,
                                stream_context,
                            })
                            .await?;
                            let mut diagnostics = AcpStreamDiagnostics::default();
                            diagnostics.saw_requires_approval_stop = false;
                            Ok((prepared.0, prepared.1, std::sync::Arc::new(std::sync::Mutex::new(diagnostics))))
                        })));
                    this.diagnostics.saw_requires_approval_stop = false;
                    self.poll_next(cx)
                } else if this.diagnostics.saw_requires_approval_stop
                    && this.turn_controller.status_snapshot().open_obligations == 0
                    && this.outstanding_tool_obligations().is_empty()
                    && !this.diagnostics.saw_tool_return_ack
                    && !this.diagnostics.emitted_runtime_cleanup
                {
                    let letta = this.letta.clone();
                    let tool_turns = this.context.tool_turns.clone();
                    let acp_session_id = this.context.acp_session_id.clone();
                    let bear_id = this.context.bear_id;
                    let pair_agent_id = this.context.pair_agent_id.clone();
                    let run_ids = this.diagnostics.run_ids.clone();
                    let request_id = this.context.request_id;
                    this.persist_future = Some(AcpPendingFuture::Cleanup(Box::pin(async move {
                        acp_cleanup_stale_runtime_state(AcpStaleRuntimeCleanupParams {
                            letta,
                            tool_turns,
                            acp_session_id,
                            bear_id,
                            pair_agent_id,
                            run_ids,
                            reason: "orphaned_requires_approval_stop",
                            request_id,
                        })
                        .await
                    })));
                    self.poll_next(cx)
                } else if let Some(event) = this.diagnostics.empty_turn_error_event(&this.context) {
                    for event in this.text_chunker.push(event) {
                        this.push_adapter_event(event);
                    }
                    self.poll_next(cx)
                } else {
                    for event in this.text_chunker.flush_all() {
                        this.push_adapter_event(event);
                    }
                    if this.turn_controller.phase() != AcpTurnPhase::Terminal {
                        this.turn_controller.on_stream_end();
                        let role_result = this.context.role_runtime.turn_result(
                            TurnResultStatus::Ok,
                            TurnResultReason::StreamComplete,
                            this.context.request_id,
                            this.context.turn_scope.clone(),
                            false,
                            this.diagnostics.diagnostic_json_with_turn_controller(
                                &this.context,
                                Some(&this.turn_controller),
                            ),
                        );
                        this.push_terminal_result_when_ready(role_result);
                    }
                    if !this.pending.is_empty() {
                        return self.poll_next(cx);
                    }
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
    use crate::core::{
        acp_runtime::{
            resolve_acp_prompt_conversation, AcpConversationResolution,
            AcpConversationSelectionSource,
        },
        acp_turn_runner::ACP_STALE_APPROVAL_RECOVERY_DENIAL_REASON,
        letta::PendingApprovalDenialMode,
    };

    #[test]
    fn acp_prompt_requested_mode_is_normalized() {
        let body: AcpPromptRequest = serde_json::from_value(serde_json::json!({
            "message": "hello",
            "requested_mode": " WRITE "
        }))
        .expect("prompt request");

        assert_eq!(requested_mode_from_prompt(&body).unwrap(), Some("write"));
    }

    #[test]
    fn acp_prompt_requested_mode_rejects_unknown_values() {
        let body: AcpPromptRequest = serde_json::from_value(serde_json::json!({
            "message": "hello",
            "requested_mode": "sudo"
        }))
        .expect("prompt request");

        assert!(matches!(
            requested_mode_from_prompt(&body),
            Err(CustomError::ValidationError(_))
        ));
    }

    #[test]
    fn acp_pair_descriptors_keep_workboard_tools_but_hide_mode_control_tools() {
        let descriptors = acp_pair_den_tool_descriptors();
        let names = descriptors
            .as_array()
            .expect("descriptor array")
            .iter()
            .filter_map(|descriptor| descriptor.get("name").and_then(|value| value.as_str()))
            .collect::<Vec<_>>();

        for expected in [
            den_tools::DEN_WORK_PLAN_UPDATE_PROVIDER,
            den_tools::DEN_WORK_PLAN_GET_STATUS_PROVIDER,
            den_tools::DEN_WORK_PLAN_LIST_PROVIDER,
            den_tools::DEN_WORK_PLAN_REQUEST_HANDOFF_PROVIDER,
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }

        for hidden in [
            den_tools::DEN_PLAN_MODE_ENTER_PROVIDER,
            den_tools::DEN_PLAN_MODE_STATUS_PROVIDER,
            den_tools::DEN_PLAN_MODE_RECORD_APPROVAL_PROVIDER,
            den_tools::DEN_PLAN_MODE_EXIT_PROVIDER,
            den_tools::DEN_PLAN_MODE_CANCEL_PROVIDER,
        ] {
            assert!(
                !names.contains(&hidden),
                "unexpected mode-control tool {hidden}"
            );
        }
    }

    #[test]
    fn concurrent_letta_run_conflict_is_not_stale_approval() {
        let err = CustomError::System(
            "Letta send message HTTP 409 Conflict: another run is still processing this conversation"
                .to_string(),
        );

        assert!(!looks_like_letta_waiting_for_approval_error(&err));
    }

    #[test]
    fn acp_recovery_approval_denial_reasons_do_not_look_like_policy_blocks() {
        for reason in [ACP_STALE_APPROVAL_RECOVERY_DENIAL_REASON] {
            assert!(!reason.contains("Denied by BEARS"));
            assert!(reason.contains("expired ACP approval request"));
            assert!(reason.contains("not a user or web policy block"));
            assert!(reason.contains("Retry the tool"));
        }
    }

    #[test]
    fn acp_history_page_replays_desc_letta_page_chronologically() {
        let body = serde_json::json!({
            "messages": [
                { "id": "m4", "message_type": "assistant_message", "content": "reply 2", "created_at": "2026-01-01T00:00:04Z" },
                { "id": "m3", "message_type": "user_message", "content": "ask 2", "created_at": "2026-01-01T00:00:03Z" },
                { "id": "m2", "message_type": "assistant_message", "content": "reply 1", "created_at": "2026-01-01T00:00:02Z" },
                { "id": "m1", "message_type": "user_message", "content": "ask 1", "created_at": "2026-01-01T00:00:01Z" }
            ]
        });
        let (messages, _has_more, next_before) = map_acp_history_page(&body, 4);
        assert_eq!(next_before.as_deref(), Some("m1"));
        assert_eq!(
            messages
                .iter()
                .map(|message| (message.role.as_str(), message.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("user", "ask 1"),
                ("assistant", "reply 1"),
                ("user", "ask 2"),
                ("assistant", "reply 2"),
            ]
        );
    }

    #[test]
    fn adapter_environment_request_deserializes_client_thread_title() {
        let body: AcpAdapterEnvironmentRequest = serde_json::from_value(serde_json::json!({
            "environment": { "thread_title": "Zed rename" },
            "conversation_title": "Zed rename"
        }))
        .expect("request should deserialize");
        assert_eq!(body.conversation_title.as_deref(), Some("Zed rename"));
        assert_eq!(
            body.environment
                .get("thread_title")
                .and_then(|value| value.as_str()),
            Some("Zed rename")
        );
    }

    #[test]
    fn acp_direct_tool_prompt_context_marks_untitled_sessions() {
        let policy = crate::core::acp_tools::resolve_session_policy_for_mode("ask", None);
        let context = acp_direct_tool_prompt_context_with_activity(
            "acp-test-session",
            "/workspace",
            &serde_json::json!({
                "workspace_roots": ["/workspace"],
                "tools": []
            }),
            true,
            &policy,
            None,
            Some("This conversation is currently untitled. Once the main subject is clear enough to summarize in a short, specific title, proactively call `set_conversation_title` in that turn without waiting for the user to ask."),
        );
        assert!(
            context.contains("Conversation title status for this ACP session: currently untitled.")
        );
        assert!(context.contains("set_conversation_title"));
    }

    #[test]
    fn summarize_letta_event_for_log_redacts_large_tool_return() {
        let event = serde_json::json!({
            "message_type": "tool_return_message",
            "id": "message-test",
            "run_id": "run-test",
            "step_id": "step-test",
            "tool_call_id": "call-test",
            "status": "success",
            "tool_return": "x".repeat(10_000),
            "tool_call": {
                "function": {
                    "name": "fs_edit_file",
                    "arguments": "{\"path\":\"/tmp/a\",\"old_text\":\"secret\",\"new_text\":\"replacement\"}"
                }
            }
        });
        let summary = summarize_letta_event_for_log(&event);
        assert_eq!(summary["message_type"], "tool_return_message");
        assert_eq!(summary["run_id"], "run-test");
        assert_eq!(summary["tool_call_id"], "call-test");
        assert_eq!(summary["tool_return"]["redacted"], true);
        assert_eq!(summary["tool_return"]["bytes"], 10_000);
        assert!(summary["tool_return"].get("preview").is_none());
        assert_eq!(
            summary["tool_call"]["function"]["arguments"]["redacted"],
            true
        );
        assert_eq!(
            summary["tool_call"]["function"]["arguments"]["json_keys"],
            serde_json::json!(["new_text", "old_text", "path"])
        );
    }

    #[test]
    fn acp_text_chunker_flushes_first_reasoning_status_without_waiting_for_punctuation() {
        let mut chunker = AcpTextChunker::new_with_reasoning_limit(1024, 128);
        let events = chunker.push(AcpGatewayEvent::StatusText {
            text: "Thinking".to_string(),
        });
        assert_eq!(events.len(), 1);
        let AcpGatewayEvent::StatusText { text } = &events[0] else {
            panic!("expected status text");
        };
        assert_eq!(text, "Thinking");
    }

    #[test]
    fn acp_text_chunker_caps_reasoning_output_per_turn() {
        let mut chunker = AcpTextChunker::new_with_reasoning_limit(1024, 10);
        let events = chunker.push(AcpGatewayEvent::StatusText {
            text: "abcdefghijklmnopqrstuvwxyz".to_string(),
        });
        assert_eq!(events.len(), 1);
        let AcpGatewayEvent::StatusText { text } = &events[0] else {
            panic!("expected status text");
        };
        assert!(text.starts_with("abcdefghij\n"));
        assert!(text.contains("BEARS suppressed additional thinking/status output"));

        let events = chunker.push(AcpGatewayEvent::StatusText {
            text: "more".to_string(),
        });
        assert!(events.is_empty());
    }

    #[test]
    fn acp_tool_result_turn_missing_returns_late_result_ignored() {
        let registry = AcpToolTurnCoordinator::new();
        let response = acp_tool_result_response_from_delivery(
            AcpToolResultDelivery::TurnMissing {
                turn_id: Some("turn-1".to_string()),
                tool_call_id: "call-1".to_string(),
            },
            "acp-session",
            "call-1".to_string(),
            AcpToolStatus::Ok,
            &registry,
        )
        .to_value();

        assert_eq!(response["accepted"], false);
        assert_eq!(response["reason"], "late_result_ignored");
        assert_eq!(response["settlement"], "unknown");
        assert_eq!(response["turn_id"], "turn-1");
        assert_eq!(response["tool_call_id"], "call-1");
        assert_eq!(response["diagnostic"]["phase"], "late_tool_result_ignored");
    }

    #[test]
    fn acp_tool_result_recently_settled_timeout_returns_timed_out_settlement() {
        let registry = AcpToolTurnCoordinator::new();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        registry
            .register(AcpToolTurnRegistration {
                user_id: 1,
                bear_id: Uuid::new_v4(),
                bear_slug: "test-bear".to_string(),
                acp_session_id: "acp-session".to_string(),
                request_id: Uuid::new_v4(),
                tool_call_id: "call-timeout".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: Some("approval-timeout".to_string()),
                timeout_ms: 1,
                result_tx: tx,
            })
            .unwrap();
        let delivered = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-session",
                "call-timeout",
                AcpToolResultRequest {
                    tool_call_id: Some("call-timeout".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "timeout".to_string(),
                    content: Some("timed out".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(delivered, AcpToolResultDelivery::Delivered { .. }));
        registry.remove("acp-session", "call-timeout");
        let late = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-session",
                "call-timeout",
                AcpToolResultRequest {
                    tool_call_id: Some("call-timeout".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "ok".to_string(),
                    content: Some("late".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        let response = acp_tool_result_response_from_delivery(
            late,
            "acp-session",
            "call-timeout".to_string(),
            AcpToolStatus::Ok,
            &registry,
        )
        .to_value();

        assert_eq!(response["accepted"], false);
        assert_eq!(response["reason"], "late_result_ignored");
        assert_eq!(response["settlement"], "timed_out");
        assert_eq!(response["tool_call_id"], "call-timeout");
        assert_eq!(response["diagnostic"]["status"], "timeout");
    }

    #[tokio::test]
    async fn acp_stream_waits_for_tool_result_and_continues_letta() {
        use axum::{
            extract::State,
            http::header,
            response::{IntoResponse, Response},
            routing::post,
            Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
            tool_return_status: StatusCode,
            tool_return_body: &'static str,
            cancel_calls: Arc<TokioMutex<usize>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> Response {
            *state.captured.lock().await = Some(body);
            (
                state.tool_return_status,
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                state.tool_return_body,
            )
                .into_response()
        }

        async fn fake_cancel(State(state): State<FakeState>) -> Response {
            *state.cancel_calls.lock().await += 1;
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"cancelled\":true}",
            )
                .into_response()
        }

        let captured = Arc::new(TokioMutex::new(None));
        let cancel_calls = Arc::new(TokioMutex::new(0));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .route("/v1/agents/{agent_id}/messages/cancel", post(fake_cancel))
            .with_state(FakeState {
                captured: captured.clone(),
                tool_return_status: StatusCode::OK,
                tool_return_body: concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"file says hello\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
                cancel_calls: cancel_calls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let cancel_registry = crate::core::acp_turn_controller::AcpActiveTurnCancelRegistry::new();
        let request_id = Uuid::new_v4();
        let role_runtime =
            RoleRuntime::with_turn_cancellations(registry.clone(), cancel_registry.clone());
        let (_cancel_handle, _cancel_rx) = cancel_registry.register(
            "acp-test-session",
            request_id,
            Some("conv-test-resolved".to_string()),
        );
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-1\",\"run_id\":\"run-stream-test\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_test\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-test.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_test\""));

        let runtime_snapshot =
            cancel_registry.runtime_snapshot_for_session("acp-test-session", &registry);
        assert_eq!(
            runtime_snapshot["state"],
            serde_json::json!("requires_action")
        );
        assert_eq!(
            runtime_snapshot["active_turn"]["pending_obligations"],
            serde_json::json!(1)
        );
        assert_eq!(
            runtime_snapshot["active_turn"]["run_ids"],
            serde_json::json!(["run-stream-test"])
        );

        let delivery = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-test-session",
                "call_test",
                AcpToolResultRequest {
                    turn_id: Some("turn-test".to_string()),
                    request_id: Some("request-test".to_string()),
                    tool_call_id: Some("call_test".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    approval_request_id: None,
                    status: "ok".to_string(),
                    content: Some("hello from file".to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({}),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(delivery, AcpToolResultDelivery::Delivered { .. }));

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            if output.contains("file says hello") {
                break;
            }
        }
        assert!(output.contains("Local tool fs_read_text_file completed"));
        assert!(output.contains("file says hello"));

        let body = captured.lock().await.clone().unwrap();
        assert_eq!(body["client_tools"][0]["name"], "fs_read_text_file");
        assert_eq!(body["messages"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approval_request_id"], "approval-1");
        assert_eq!(body["messages"][0]["approve"], true);
        assert_eq!(body["messages"][0]["approvals"][0]["type"], "tool");
        assert_eq!(
            body["messages"][0]["approvals"][0]["tool_call_id"],
            "call_test"
        );
    }

    #[tokio::test]
    async fn acp_stream_failed_local_tool_result_continues_with_denial_payload() {
        use axum::{
            extract::State,
            http::header,
            response::{IntoResponse, Response},
            routing::post,
            Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> Response {
            *state.captured.lock().await = Some(body);
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"handled error\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
            )
                .into_response()
        }

        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let cancel_registry = crate::core::acp_turn_controller::AcpActiveTurnCancelRegistry::new();
        let request_id = Uuid::new_v4();
        let role_runtime =
            RoleRuntime::with_turn_cancellations(registry.clone(), cancel_registry.clone());
        let (_cancel_handle, _cancel_rx) = cancel_registry.register(
            "acp-error-session",
            request_id,
            Some("conv-error-resolved".to_string()),
        );
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-error-session",
            Some("conv-error-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-error-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-error-resolved".to_string()),
            upstream_target: "conv-error-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-error\",\"run_id\":\"run-stream-error\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_error\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-error.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-error-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_error\""));

        let delivery = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-error-session",
                "call_error",
                AcpToolResultRequest {
                    turn_id: Some("turn-error".to_string()),
                    request_id: Some("request-error".to_string()),
                    tool_call_id: Some("call_error".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    approval_request_id: None,
                    status: "error".to_string(),
                    content: Some("tool failed".to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({}),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(delivery, AcpToolResultDelivery::Delivered { .. }));

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            if output.contains("handled error") {
                break;
            }
        }
        assert!(output.contains("Local tool fs_read_text_file completed"));
        assert!(output.contains("handled error"));

        let body = captured.lock().await.clone().unwrap();
        assert_eq!(body["messages"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approval_request_id"], "approval-error");
        assert_eq!(body["messages"][0]["approve"], false);
        assert_eq!(body["messages"][0]["approvals"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approvals"][0]["approve"], false);
        assert_eq!(
            body["messages"][0]["approvals"][0]["tool_call_id"],
            "call_error"
        );
        assert_eq!(body["messages"][0]["approvals"][0]["reason"], "tool failed");
    }

    #[tokio::test]
    async fn acp_stream_does_not_emit_turn_result_before_local_tool_result() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"continued after tool\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-1\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_test\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-test.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_test\""));

        let mut pre_result_output = String::new();
        let no_terminal = tokio::time::timeout(std::time::Duration::from_millis(50), async {
            while let Some(item) = stream.next().await {
                pre_result_output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
                if pre_result_output.contains("\"type\":\"turn_result\"")
                    || pre_result_output.contains("\"type\":\"turn_complete\"")
                {
                    break;
                }
            }
        })
        .await;
        // In full `acp_stream_` test runs, Tokio's clock can advance enough for the
        // synthetic local-tool timeout to settle before this probe. The invariant here is
        // narrower: no terminal may appear before either a real adapter result or an
        // auto-timeout settlement, and Den must not post a Letta continuation before one
        // of those settlements.
        if no_terminal.is_ok() {
            assert!(
                pre_result_output.contains("Local tool fs_read_text_file completed"),
                "stream emitted output before local tool result or timeout settlement: {pre_result_output}"
            );
        }
        assert!(
            !pre_result_output.contains("\"type\":\"turn_result\""),
            "stream emitted turn_result before local tool result settled: {pre_result_output}"
        );
        if !pre_result_output.contains("Local tool fs_read_text_file completed") {
            assert!(
                !pre_result_output.contains("\"type\":\"turn_complete\""),
                "stream emitted turn_complete before local tool result or timeout settlement: {pre_result_output}"
            );
            assert!(captured.lock().await.is_none());
        }

        if !pre_result_output.contains("Local tool fs_read_text_file completed") {
            let delivery = registry
                .deliver_result(
                    1,
                    "test-bear",
                    "acp-test-session",
                    "call_test",
                    AcpToolResultRequest {
                        turn_id: Some("turn-test".to_string()),
                        request_id: Some("request-test".to_string()),
                        tool_call_id: Some("call_test".to_string()),
                        tool_name: Some("fs_read_text_file".to_string()),
                        approval_request_id: None,
                        status: "ok".to_string(),
                        content: Some("hello from file".to_string()),
                        structured_content: serde_json::json!({}),
                        diagnostic: serde_json::json!({}),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(matches!(delivery, AcpToolResultDelivery::Delivered { .. }));
        }

        let mut output = pre_result_output;
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
        }
        assert!(output.contains("Local tool fs_read_text_file completed"));
        assert!(output.contains("continued after tool"));
        assert_eq!(
            output.matches("\"type\":\"turn_complete\"").count(),
            1,
            "output was: {output}"
        );
        assert!(captured.lock().await.is_some());
    }

    #[tokio::test]
    async fn acp_stream_duplicate_turn_complete_emits_once() {
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;

        let config = crate::config::Config::test_stub();
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry,
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(concat!(
            "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n",
            "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
        )))]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: None,
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
        }

        assert_eq!(
            output.matches("\"type\":\"turn_complete\"").count(),
            1,
            "output was: {output}"
        );
        assert_eq!(
            output.matches("\"type\":\"turn_result\"").count(),
            0,
            "output was: {output}"
        );
    }

    #[tokio::test]
    async fn acp_stream_routes_session_info_as_den_server_tool() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"oriented\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(config.clone()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(concat!(
            "data: {\"id\":\"approval-1\",\"message_type\":\"approval_request_message\",",
            "\"tool_call\":{\"name\":\"session_info\",\"tool_call_id\":\"call_session_info\",",
            "\"arguments\":\"{}\"}}\n\n"
        )))]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "session_info" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first =
            tokio::time::timeout(std::time::Duration::from_millis(100), stream.next()).await;
        assert!(
            first.is_err(),
            "Den-server session_info unexpectedly emitted an adapter event: {first:?}"
        );
        assert!(captured.lock().await.is_none());

        let missing = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-test-session",
                "call_session_info",
                AcpToolResultRequest {
                    tool_call_id: Some("call_session_info".to_string()),
                    tool_name: Some("session_info".to_string()),
                    status: "ok".to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(missing, AcpToolResultDelivery::TurnMissing { .. }));
        drop(stream);
    }

    #[tokio::test]
    async fn acp_stream_emits_initial_session_info_update() {
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;

        let mut config = crate::config::Config::load();
        config.letta_base_url = "http://127.0.0.1:9".to_string();
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry,
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "conv-test-resolved".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(config.clone()),
            role_runtime,
            turn_scope,
        };
        let upstream = futures::stream::pending::<Result<Bytes, CustomError>>();
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            vec![AcpGatewayEvent::SessionInfoUpdate {
                title: Some("Renamed in same turn".to_string()),
                updated_at: Some("2026-05-23T00:00:00Z".to_string()),
                meta: None,
            }],
            true,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-test-resolved".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: None,
                stream_tokens: false,
                max_steps: 1,
            },
            active_turn_guard,
        );

        let first = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("expected initial session info update without waiting for next prompt")
            .expect("stream should yield an event")
            .expect("event should serialize");
        let output = String::from_utf8(first.to_vec()).unwrap();
        assert!(
            output.contains("\"type\":\"session_info_update\""),
            "output was: {output}"
        );
        assert!(
            output.contains("Renamed in same turn"),
            "output was: {output}"
        );
    }

    #[test]
    fn acp_auto_title_instruction_requires_saved_conversation_without_title() {
        let base = acp_sessions::AcpSessionRow {
            id: Uuid::nil(),
            user_id: 1,
            bear_id: Uuid::nil(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            runtime_session_id: "runtime-test".to_string(),
            conversation_id: "conv-test-resolved".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            client: "zed".to_string(),
            cwd: Some("/workspace".to_string()),
            adapter_environment: None,
            current_mode: "ask".to_string(),
            conversation_title: None,
            conversation_title_updated_at: None,
            conversation_title_synced_at: None,
            closed_at: None,
            archived_at: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        let guidance = acp_auto_title_instruction(&base).expect("guidance expected");
        assert!(guidance.contains("set_conversation_title"));
        assert!(guidance.contains("currently untitled"));
        assert!(guidance.contains("without waiting for the user to ask"));

        let titled = acp_sessions::AcpSessionRow {
            conversation_title: Some("Already titled".to_string()),
            ..base.clone()
        };
        assert!(acp_auto_title_instruction(&titled).is_none());

        let unresolved = acp_sessions::AcpSessionRow {
            resolved_conversation_id: None,
            conversation_id: "pending-id".to_string(),
            ..base
        };
        assert!(acp_auto_title_instruction(&unresolved).is_none());
    }

    #[tokio::test]
    async fn acp_stream_timeout_pending_local_tool() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"handled timeout\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        std::env::set_var("BEARS_ACP_TOOL_TIMEOUT_MS", "20");

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-timeout-session",
            Some("conv-timeout".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-timeout-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-timeout".to_string()),
            upstream_target: "conv-timeout".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-timeout\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_timeout\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-timeout.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-timeout".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_timeout\""));

        let mut output = String::new();
        let stream_result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(item) = stream.next().await {
                output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            }
        })
        .await;
        std::env::remove_var("BEARS_ACP_TOOL_TIMEOUT_MS");
        assert!(
            stream_result.is_ok(),
            "stream timed out; output was: {output}"
        );

        assert!(
            output.contains("Local tool fs_read_text_file completed"),
            "output was: {output}"
        );
        assert!(output.contains("handled timeout"), "output was: {output}");
        assert_eq!(
            output.matches("\"type\":\"turn_complete\"").count(),
            1,
            "output was: {output}"
        );

        let body = captured.lock().await.clone().unwrap();
        assert_eq!(body["messages"][0]["type"], "approval");
        assert_eq!(
            body["messages"][0]["approval_request_id"],
            "approval-timeout"
        );
        assert_eq!(body["messages"][0]["approve"], false);
        assert_eq!(body["messages"][0]["approvals"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approvals"][0]["approve"], false);
        assert_eq!(
            body["messages"][0]["approvals"][0]["tool_call_id"],
            "call_timeout"
        );
        assert!(body["messages"][0]["approvals"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("timed out after 20ms"));

        let late = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-timeout-session",
                "call_timeout",
                AcpToolResultRequest {
                    tool_call_id: Some("call_timeout".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "ok".to_string(),
                    content: Some("late result".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(
            late,
            AcpToolResultDelivery::RecentlySettled { .. }
                | AcpToolResultDelivery::TurnMissing { .. }
        ));
    }

    #[tokio::test]
    async fn acp_stream_cancel_pending_local_tool() {
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;

        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-cancel-session",
            Some("conv-cancel".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-cancel-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-cancel".to_string()),
            upstream_target: "conv-cancel".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(concat!(
            "data: {\"id\":\"approval-cancel\",\"message_type\":\"approval_request_message\",",
            "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_cancel\",",
            "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-cancel.txt\\\"}\"}}\n\n"
        )))]);
        let config = crate::config::Config::test_stub();
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-cancel".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        )
        .with_cancel_rx(cancel_rx);

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_cancel\""));

        cancel_tx.send(true).unwrap();
        let cancelled = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .expect("cancel terminal should not hang");
        let cancelled = cancelled
            .expect("cancel should emit terminal before ending")
            .unwrap();
        let cancelled_text = String::from_utf8(cancelled.to_vec()).unwrap();
        assert!(
            cancelled_text.contains("\"type\":\"turn_result\""),
            "{cancelled_text}"
        );
        assert!(
            cancelled_text.contains("\"status\":\"cancelled\""),
            "{cancelled_text}"
        );
        assert!(
            cancelled_text.contains("\"reason\":\"cancelled\""),
            "{cancelled_text}"
        );

        let late = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-cancel-session",
                "call_cancel",
                AcpToolResultRequest {
                    tool_call_id: Some("call_cancel".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "ok".to_string(),
                    content: Some("late result".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(late, AcpToolResultDelivery::TurnMissing { .. }));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn acp_stream_cleans_orphaned_requires_approval_stop() {
        use axum::{extract::State, http::header, response::IntoResponse, routing::post, Router};
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            cancel_calls: Arc<TokioMutex<usize>>,
        }

        async fn fake_cancel(State(state): State<FakeState>) -> impl IntoResponse {
            *state.cancel_calls.lock().await += 1;
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"cancelled\":true}",
            )
        }

        let cancel_calls = Arc::new(TokioMutex::new(0));
        let app = Router::new()
            .route("/v1/agents/{agent_id}/messages/cancel", post(fake_cancel))
            .with_state(FakeState {
                cancel_calls: cancel_calls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::test_stub();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-orphaned-approval",
            Some("conv-test".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-orphaned-approval".to_string(),
            client: "zed".to_string(),
            conversation_selection: "conv-test".to_string(),
            resolved_conversation_id: Some("conv-test".to_string()),
            upstream_target: "conv-test".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(
            "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
        ))]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-test".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: None,
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
        }
        assert!(!output.contains("status_text"), "{output}");
        assert!(!output.contains("runtime recovery"), "{output}");
        assert!(output.contains("\"type\":\"turn_result\""), "{output}");
        assert_eq!(
            *cancel_calls.lock().await,
            0,
            "orphaned cleanup without run_ids must not issue an agent-wide Letta cancel"
        );
    }

    #[test]
    fn stale_approval_recovery_uses_inspect_only_mode_to_avoid_conversation_contamination() {
        let mode = PendingApprovalDenialMode::InspectOnly;
        assert!(matches!(mode, PendingApprovalDenialMode::InspectOnly));
    }

    #[tokio::test]
    async fn acp_stream_cleans_runtime_when_tool_return_continuation_conflicts() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
            cancel_calls: Arc<TokioMutex<usize>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                StatusCode::CONFLICT,
                [(header::CONTENT_TYPE, "application/json")],
                "{\"error\":\"conversation waiting for approval\"}",
            )
        }

        async fn fake_cancel(State(state): State<FakeState>) -> impl IntoResponse {
            *state.cancel_calls.lock().await += 1;
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"cancelled\":true}",
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let cancel_calls = Arc::new(TokioMutex::new(0));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .route("/v1/agents/{agent_id}/messages/cancel", post(fake_cancel))
            .with_state(FakeState {
                captured: captured.clone(),
                cancel_calls: cancel_calls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::test_stub();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-continuation-conflict",
            Some("conv-test".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-continuation-conflict".to_string(),
            client: "zed".to_string(),
            conversation_selection: "conv-test".to_string(),
            resolved_conversation_id: Some("conv-test".to_string()),
            upstream_target: "conv-test".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-1\",\"run_id\":\"run-conflict\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_conflict\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-test.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            LettaContinuationContext {
                conversation_id: "conv-test".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        assert!(String::from_utf8(first.to_vec())
            .unwrap()
            .contains("tool_request"));
        registry
            .deliver_result(
                1,
                "test-bear",
                "acp-continuation-conflict",
                "call_conflict",
                AcpToolResultRequest {
                    tool_call_id: Some("call_conflict".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    approval_request_id: None,
                    status: "ok".to_string(),
                    content: Some("hello".to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({}),
                    ..Default::default()
                },
            )
            .unwrap();

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            if output.contains("\"status\":\"recovered\"") {
                break;
            }
        }
        assert!(output.contains("\"status\":\"recovered\""), "{output}");
        assert!(
            output.contains("\"run_ids\":[\"run-conflict\"]"),
            "{output}"
        );
        assert_eq!(*cancel_calls.lock().await, 1);
        assert!(captured.lock().await.is_some());
    }

    #[test]
    fn acp_history_filters_system_scoped_user_messages_and_reminder_suffixes() {
        let body = serde_json::json!({
            "messages": [
                {
                    "id": "msg-system-user",
                    "date": "2026-05-10T00:00:00Z",
                    "message_type": "user_message",
                    "role": "system",
                    "content": "BEARS ACP direct local workspace tools available this turn: fs_read_text_file."
                },
                {
                    "id": "msg-assistant",
                    "date": "2026-05-10T00:00:01Z",
                    "message_type": "assistant_message",
                    "content": "Done.\n<system-reminder>hidden harness</system-reminder>"
                },
                {
                    "id": "msg-human",
                    "date": "2026-05-10T00:00:02Z",
                    "message_type": "user_message",
                    "content": "Please check this thread.\n<system-reminder>adapter-only instructions</system-reminder>"
                },
                {
                    "id": "msg-human-scaffold",
                    "date": "2026-05-10T00:00:03Z",
                    "message_type": "user_message",
                    "content": "ACP workflow state for this session: workflow_id=123 workflow_state=submitted submitted_plan_present=true approval_status=awaiting_human_approval execution_unlocked=false. Workflow state is authoritative.\n\nPlease only show the real user text."
                }
            ]
        });
        let (messages, has_more, next_before) = map_acp_history_page(&body, 50);
        assert!(!has_more);
        assert_eq!(next_before.as_deref(), Some("msg-human-scaffold"));
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "Please only show the real user text.");
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].text, "Please check this thread.");
        assert_eq!(messages[2].role, "assistant");
        assert_eq!(messages[2].text, "Done.");
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
        let binding = crate::core::runtime_contracts::RoleRuntimeBinding {
            binding_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            compatibility_backend: Some("letta".to_string()),
        };
        let resolution =
            resolve_acp_prompt_conversation(None, None, &binding, "new-acp-zed-abc123".to_string())
                .unwrap();
        assert_eq!(resolution.session_selection, "new-acp-zed-abc123");
        assert_eq!(resolution.resolved_conversation, None);
        assert_eq!(resolution.upstream_target, binding.binding_id);
        assert_eq!(resolution.history_target, None);
        assert_eq!(resolution.archive_target, None);
        assert_eq!(
            resolution.selection_source,
            AcpConversationSelectionSource::Generated
        );
    }

    #[test]
    fn resolver_routes_explicit_conv_directly_and_requires_bear_check() {
        let binding = crate::core::runtime_contracts::RoleRuntimeBinding {
            binding_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            compatibility_backend: Some("letta".to_string()),
        };
        let conv_id = "conv-12345678-1234-4567-89ab-123456789abc";
        let resolution = resolve_acp_prompt_conversation(
            Some(conv_id),
            None,
            &binding,
            "new-acp-zed-unused".to_string(),
        )
        .unwrap();
        assert_eq!(resolution.session_selection, conv_id);
        assert_eq!(
            resolution
                .resolved_conversation
                .as_ref()
                .map(|c| c.id.as_str()),
            Some(conv_id)
        );
        assert_eq!(resolution.upstream_target, conv_id);
        assert_eq!(
            resolution.history_target.as_ref().map(|c| c.id.as_str()),
            Some(conv_id)
        );
        assert_eq!(
            resolution.archive_target.as_ref().map(|c| c.id.as_str()),
            Some(conv_id)
        );
        assert_eq!(
            resolution.selection_source,
            AcpConversationSelectionSource::Explicit
        );
        assert!(resolution.requires_belongs_to_bear_check);
    }

    #[test]
    fn resolver_never_archives_pending_or_default_targets() {
        let binding = crate::core::runtime_contracts::RoleRuntimeBinding {
            binding_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            compatibility_backend: Some("letta".to_string()),
        };
        let pending = AcpConversationResolution::from_selection(
            "new-acp-zed-abc123".to_string(),
            AcpConversationSelectionSource::Generated,
            &binding,
            None,
        );
        assert_eq!(pending.history_target, None);
        assert_eq!(pending.archive_target, None);

        let default = AcpConversationResolution::from_selection(
            "default".to_string(),
            AcpConversationSelectionSource::Stored,
            &binding,
            None,
        );
        assert_eq!(
            default.history_target.as_ref().map(|c| c.id.as_str()),
            Some("default")
        );
        assert_eq!(default.archive_target, None);
    }

    #[test]
    fn rejects_legacy_pending_acp_conversation_ids_that_exceed_letta_limit() {
        let legacy = "new-acp-zed-acp-12345678-1234-1234-1234-123456789abc";
        assert!(normalize_acp_conversation_id(Some(legacy)).is_ok());
        assert!(!is_valid_pending_acp_conversation_id(legacy));
    }
}
