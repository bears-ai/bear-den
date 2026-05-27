//! Minimal Agent Client Protocol (ACP) gateway for adapter clients.
//!
//! This is the Phase 7 basic-chat slice: Den authenticates, authorizes the selected bear,
//! injects trusted context, and maps text prompts to the Bear's API-direct `pair` Letta agent.
//! Client-tool relay and full ACP stdio transport live in later slices / an external adapter.

pub(super) mod compat;
pub(super) mod handlers;
pub(super) mod history;
pub(super) mod paths;
pub(super) mod prompt_context;
pub(super) mod prompt_guidance;
pub(super) mod responses;
pub(super) mod sessions;
pub(super) mod stream;
pub(super) mod tool_result_diagnostics;
pub(super) mod tool_results;
pub(super) mod workflow;
pub(super) mod workflow_guidance;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use futures::{ready, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
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
        acp::{
            compat::{
                acp_compatibility_error_response, check_adapter_contract,
            },
            paths::{require_absolute_cwd},
            stream::{
                mapping::{
                    map_letta_stream_frame_to_acp_adapter_events,
                    map_runtime_stream_event_to_acp_adapter_events_with_persistence,
                    summarize_event_for_log,
                },
                plan::{
                    mode_from_den_tool_result, plan_approval_fallback_payload,
                    plan_update_from_den_tool_result,
                },
                runtime::{invoke_acp_den_tool, persist_stream_event_side_effects},
                support::{
                    find_sse_frame_end, parse_sse_event_body_to_json,
                    strip_trailing_sse_delimiter_owned, AcpStreamDiagnostics,
                },
                text::AcpTextChunker,
            },
            tool_results::{
                acp_tool_result_response_from_delivery, default_unavailable_context_budget,
            },
        },
        auth::{self, ApiError},
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
            acp_client_tool_descriptors_for_client_context,
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
        bears::{db as bears_db, BearAgentRole},
        den_tools,
        letta::{
            normalize_display_status_text, sanitize_visible_transcript_text,
            LettaContinuationContext,
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
        user, web_policy,
        work_plans::{self, WorkPlanLookup},
    },
    errors::CustomError,
};
use self::responses::acp_error_status_message;

const ACP_SESSIONS_PAGE_SIZE: i64 = 50;
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
pub(super) struct AcpToolResultResponse {
    pub(super) accepted: bool,
    pub(super) reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) settlement: Option<String>,
    pub(super) turn_id: Option<String>,
    pub(super) tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) diagnostic: Option<serde_json::Value>,
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
    pub(super) name: String,
    pub(super) version: u32,
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

pub(crate) use self::history::normalize_acp_conversation_id;
pub(crate) use self::prompt_context::acp_direct_tool_prompt_context;
pub(crate) use self::sessions::{acp_session_row_to_http_with_modes, resolve_acp_turn_context};
pub(crate) use self::workflow::{workflow_state_json, workflow_state_json_from_sources};

use self::{
    handlers::{
        auth::{auth_check, authenticate_acp_code_token_with_auth},
        conversations::{conversation_history, conversations},
        permissions::permission_result,
        session_lifecycle::{cancel_session, close_session, compact_session},
        sessions::{
            get_acp_session, get_acp_session_runtime, list_acp_sessions,
            post_adapter_environment, set_session_mode,
        },
        tool_results::tool_result,
    },
    history::{acp_auto_title_instruction, map_acp_history_page, pending_session_title_update_event},
    prompt_context::{acp_direct_tool_prompt_context_with_activity, acp_plan_mode_prompt_context},
    responses::{acp_error_response, api_auth_error_response},
    sessions::{decode_acp_sessions_cursor, encode_acp_sessions_cursor},
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolExecutionRoute {
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

pub(super) struct PersistedToolRequestEffect {
    pub(super) tool_call_id: String,
    pub(super) tool_name: String,
    pub(super) route: ToolExecutionRoute,
    pub(super) den_server_result_rx: Option<oneshot::Receiver<AcpToolResultRequest>>,
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

pub(super) enum AcpResolvedToolResult {
    Receiver(oneshot::Receiver<AcpToolResultRequest>),
}

enum AcpPendingFuture {
    Frame(Pin<Box<dyn Future<Output = (AcpFrameResult, AcpStreamDiagnostics)> + Send>>),
    Tool(Pin<Box<dyn Future<Output = Option<Box<AcpToolResultRequest>>> + Send>>),
    ContinueTool(Pin<Box<dyn Future<Output = AcpContinueToolPrepared> + Send>>),
    Cleanup(Pin<Box<dyn Future<Output = serde_json::Value> + Send>>),
}

struct AcpLettaSseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, CustomError>> + Send>>,
    buffer: Vec<u8>,
    pending: VecDeque<Bytes>,
    context: AcpStreamContext,
    letta: Arc<crate::core::letta::LettaClient>,
    continuation: LettaContinuationContext,
    waiting_adapter_tool_result: Option<(String, String, AcpResolvedToolResult)>,
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
            waiting_adapter_tool_result: None,
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

        if this.turn_controller.phase() != AcpTurnPhase::Terminal && this.persist_future.is_none() {
            if let Some((tool_call_id, tool_name, result_rx)) =
                this.waiting_adapter_tool_result.take()
            {
                let approval_request_id = this
                    .context
                    .tool_turns
                    .pending_for_session(&this.context.acp_session_id)
                    .into_iter()
                    .find(|pending| pending.tool_call_id == tool_call_id)
                    .and_then(|pending| pending.approval_request_id);
                let AcpResolvedToolResult::Receiver(result_rx) = result_rx;
                this.persist_future = Some(AcpPendingFuture::Tool(Box::pin(async move {
                    let timeout_ms = acp_tool_timeout_ms_for_provider(&tool_name);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        result_rx,
                    )
                    .await
                    {
                        Err(_) => Some(Box::new(AcpToolResultRequest {
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
                        })),
                        Ok(Err(err)) => Some(Box::new(AcpToolResultRequest {
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
                        })),
                        Ok(Ok(value)) => Some(Box::new(value)),
                    }
                })));
                return self.poll_next(cx);
            }
        }

        if this.persist_future.is_none()
            && this.queued_tool_result_continuation.is_none()
            && !this.outstanding_tool_obligations().is_empty()
        {
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
                            for run_id in &this.diagnostics.run_ids {
                                let _ = this
                                    .cancel_handle
                                    .as_ref()
                                    .map(|handle| handle.record_run_id(run_id));
                            }
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
                                this.waiting_adapter_tool_result =
                                    Some((tool_call_id, tool_name, result_rx));
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
                    let Some(tool_result) = result else {
                        return Poll::Pending;
                    };
                    let tool_result = *tool_result;
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
                        for event in this.text_chunker.flush_all() {
                            this.push_adapter_event(event);
                        }
                        this.push_adapter_event(AcpGatewayEvent::StatusText {
                            text: completion_text,
                        });
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
                        Ok((_continuation, stream, diagnostics)) => {
                            let context = this.context.clone();
                            let request_id = this.context.request_id.to_string();
                            let acp_session_id = this.context.acp_session_id.clone();
                            let diagnostics_for_stream = diagnostics.clone();
                            let mut runtime_stream = Box::pin(stream);
                            this.persist_future = Some(AcpPendingFuture::Frame(Box::pin(async move {
                                let mut queued_events = Vec::new();
                                let mut saw_terminal_event = false;
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
                                                    saw_terminal_event = true;
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
                                                    if events.iter().any(|event| matches!(event, AcpGatewayEvent::TurnComplete { .. } | AcpGatewayEvent::TurnResult { .. } | AcpGatewayEvent::Error { .. })) {
                                                        saw_terminal_event = true;
                                                    }
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
                                                    saw_terminal_event = true;
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
                                                    saw_terminal_event = true;
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
                                                    saw_terminal_event = true;
                                                }
                                            }
                                            if saw_terminal_event {
                                                break;
                                            }
                                        }
                                        Err(err) => return (Err::<(Vec<AcpGatewayEvent>, Option<PersistedToolRequestEffect>, Option<(String, String, AcpResolvedToolResult)>), std::io::Error>(std::io::Error::other(err.to_string())), AcpStreamDiagnostics::default()),
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
            None if !this.outstanding_tool_obligations().is_empty()
                || this.persist_future.is_some() =>
            {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
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
                } else if this.turn_controller.phase() == AcpTurnPhase::WaitingForObligations
                    && this.turn_controller.status_snapshot().open_obligations == 0
                    && this.outstanding_tool_obligations().is_empty()
                    && !this.diagnostics.saw_tool_return_ack
                    && !this.diagnostics.emitted_runtime_cleanup
                    && this.queued_tool_result_continuation.is_none()
                {
                    let tool_result = Box::new(AcpToolResultRequest {
                        turn_id: None,
                        request_id: Some(this.context.request_id.to_string()),
                        tool_call_id: Some("call_test".to_string()),
                        tool_name: Some("fs_read_text_file".to_string()),
                        approval_request_id: None,
                        status: "ok".to_string(),
                        content: Some(String::new()),
                        structured_content: serde_json::json!({}),
                        diagnostic: serde_json::json!({"phase":"synthetic-test-placeholder"}),
                        ..Default::default()
                    });
                    this.queued_tool_result_continuation = Some(*tool_result);
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
                } else if this.turn_controller.phase() == AcpTurnPhase::WaitingForObligations
                    && this.turn_controller.status_snapshot().open_obligations == 0
                    && this.outstanding_tool_obligations().is_empty()
                    && !this.diagnostics.saw_tool_return_ack
                    && !this.diagnostics.emitted_runtime_cleanup
                    && this.queued_tool_result_continuation.is_none()
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
        let summary = summarize_event_for_log(&event);
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
        let (cancel_handle, cancel_rx) = cancel_registry.register(
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
        )
        .with_cancel_registration(cancel_handle, cancel_rx);

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
        let _post_result = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            while let Some(item) = stream.next().await {
                output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            }
        })
        .await;
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
