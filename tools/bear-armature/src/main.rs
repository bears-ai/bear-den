mod approvals;
mod bearwire;
mod headless;
mod json_rpc;
mod paths;
mod projection_dispatcher;
mod tool_tasks;
mod tools;
mod update;

fn classify_prompt_failure(error_chain: &str) -> (&'static str, String) {
    if error_chain.contains("Den API connectivity failure:") {
        (
            "den_api_unavailable",
            "Den could not continue this turn because its API is unavailable. Check your connection or try again shortly.".to_string(),
        )
    } else if error_chain.starts_with("BEARS run failed:")
        || error_chain.starts_with("BEARS run failed (")
        || error_chain.starts_with("BEARS prompt stopped:")
    {
        ("bearwire_run_failed", error_chain.to_string())
    } else if error_chain.contains("BearWire delivery deadline expired") {
        (
            "bearwire_delivery_deadline",
            "Den did not reach a terminal outcome before the work-surface delivery deadline. This prompt was stopped; try again or inspect the run status before retrying.".to_string(),
        )
    } else if error_chain.contains("Den BearWire delivery ended before a terminal run event")
        || error_chain.contains("Den BearWire delivery ended without visible output, tool activity, or a terminal run event")
    {
        (
            "bearwire_missing_terminal_event",
            "Den stopped delivering this turn before it reported a final outcome. No further work was performed after the last reported tool result. Try again; if it repeats, provide the run ID from the work-surface logs when reporting it.".to_string(),
        )
    } else if error_chain.contains("BearWire event page HTTP")
        || error_chain.contains("parse BearWire event page JSON")
        || error_chain.contains("event handling failed")
    {
        (
            "bearwire_event_delivery",
            "Den could not finish delivering this turn to the work surface. Try again; if it repeats, check the Den and work-surface logs.".to_string(),
        )
    } else {
        (
            "unclassified_prompt_failure",
            "Den could not complete this turn. Please try again or start a fresh turn.".to_string(),
        )
    }
}

#[cfg(test)]
mod prompt_failure_tests {
    use super::{
        classify_completed_turn_without_text, classify_prompt_failure, CompletedTurnWithoutText,
    };

    #[test]
    fn classifies_completed_turn_without_text() {
        assert_eq!(
            classify_completed_turn_without_text(false, true),
            CompletedTurnWithoutText::Expected
        );
        assert_eq!(
            classify_completed_turn_without_text(true, true),
            CompletedTurnWithoutText::Anomalous
        );
        assert_eq!(
            classify_completed_turn_without_text(false, false),
            CompletedTurnWithoutText::Anomalous
        );
    }

    #[test]
    fn classifies_event_delivery_failure() {
        let (kind, message) =
            classify_prompt_failure("parse BearWire event page JSON: unexpected end of JSON input");
        assert_eq!(kind, "bearwire_event_delivery");
        assert!(message.contains("work surface"));
    }

    #[test]
    fn classifies_delivery_deadline_as_a_terminal_prompt_failure() {
        let (kind, message) = classify_prompt_failure(
            "BearWire delivery deadline expired after 600s while the run remained nonterminal",
        );
        assert_eq!(kind, "bearwire_delivery_deadline");
        assert!(message.contains("prompt was stopped"));
    }

    #[test]
    fn preserves_bearwire_failure_message() {
        let (kind, message) = classify_prompt_failure(
            "BEARS run failed (continuation_watchdog_timeout): Den did not resume after a tool result.\n\nRun: `run-1`",
        );
        assert_eq!(kind, "bearwire_run_failed");
        assert!(message.contains("continuation_watchdog_timeout"));
        assert!(message.contains("run-1"));
    }
}

use agent_client_protocol::schema::{
    AgentCapabilities, AuthEnvVar, AuthMethod, AuthMethodEnvVar, AuthenticateResponse,
    AvailableCommand, AvailableCommandsUpdate, CloseSessionResponse, ConfigOptionUpdate,
    ContentBlock, ContentChunk, CreateTerminalRequest, CreateTerminalResponse, CurrentModeUpdate,
    Diff, EnvVariable, Implementation, InitializeResponse, ListSessionsResponse,
    LoadSessionResponse, McpCapabilities, NewSessionResponse, PlanEntry, PlanEntryPriority,
    PlanEntryStatus, PromptCapabilities, PromptResponse, ProtocolVersion, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalRequest, RequestPermissionRequest, ResumeSessionResponse,
    SessionCapabilities, SessionCloseCapabilities, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionInfo, SessionInfoUpdate,
    SessionListCapabilities, SessionMode, SessionModeState, SessionResumeCapabilities,
    SessionUpdate, StopReason, Terminal, TerminalOutputRequest, TerminalOutputResponse, ToolCall,
    ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
};
use anyhow::{anyhow, bail, Context, Result};
use bearwire_protocol::surface::SurfaceHistoryEvent;

use approvals::{
    approval_url_host_scope, parse_permission_decision, permission_class_for_tool,
    permission_options_for_context, ApprovalCache, ApprovalScope, ApprovalTarget,
    PermissionDecision,
};
use axum::{extract::State, response::IntoResponse};

use http::StatusCode;
#[cfg(test)]
use json_rpc::capture_json_output_for_test;
use json_rpc::{id_key, write_json, JsonRpcTransport};
use paths::{
    ensure_path_allowed_for_session, file_uri_or_path_to_path, is_absolute_local_path,
    normalize_requested_tool_path, resolve_requested_tool_path,
};
use projection_dispatcher::AcpProjectionDispatcher;

use reqwest::Url;
use rmcp::{
    handler::server::{
        common::{schema_for_type, FromContextPart},
        router::Router as McpRouter,
        wrapper::Parameters,
        ServerHandler,
    },
    model::{
        CallToolResult, Content, Implementation as McpImplementation, ServerCapabilities,
        ServerInfo, Tool as McpTool,
    },
    transport::{
        streamable_http_server::session::local::LocalSessionManager, StreamableHttpServerConfig,
        StreamableHttpService,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(test)]
use std::fs;
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    env,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock, RwLock,
    },
};
use tokio::{
    io::{self, AsyncBufReadExt, BufReader},
    sync::{broadcast, mpsc, Mutex as TokioMutex},
    time::{timeout, Duration},
};
use tool_tasks::{log_tool_task_phase, ToolExecutionOwnership, ToolTaskPhase, ToolTaskRegistry};
use tools::chrome::{
    chrome_capability_status_line, chrome_tools_available, handle_chrome_console_messages,
    handle_chrome_network_requests, handle_chrome_open, handle_chrome_screenshot,
    handle_chrome_snapshot,
};
use tower_service::Service;

use tools::adapter_env::{collect_bear_environment, fetch_den_runtime_state};
use tools::fs::{
    handle_apply_patch, handle_copy_path, handle_create_directory, handle_create_text_file,
    handle_delete_path, handle_find_paths_blocking, handle_list_directory_blocking,
    handle_move_path, handle_read_text_file, handle_replace_text, handle_search_files_blocking,
    handle_stat, ReplaceTextArgs, ReplaceTextPlan,
};
use tools::git::{
    handle_git_add, handle_git_commit, handle_git_diff, handle_git_log, handle_git_restore,
    handle_git_show, handle_git_stash, handle_git_status,
};
use tools::mcp::{
    host_browser_bridge_config_from_env, host_browser_bridge_env_summary, parse_acp_mcp_servers,
    summarize_acp_mcp_servers_param, McpRegistry, McpSourceConfig,
};
use tools::process::handle_process_run;
use tools::terminal::{handle_terminal_run_command, TerminalCommandValidation};
use tools::web::handle_local_web_fetch;
use update::{run_update_command, update_doctor_line, UpdateCommand, UpdateOptions};

use uuid::Uuid;

#[derive(Clone, Debug)]
struct Config {
    api_url: String,
    bear: String,
    token: String,
    client: String,
}

#[derive(Clone, Debug)]
struct RuntimeConfig {
    config: Option<Config>,
    diagnostics: Vec<String>,
    check_server: bool,
    doctor: bool,
    headless: bool,
    update_command: Option<UpdateCommand>,
    browser_bridge: Option<BrowserBridgeConfig>,
    api_url: String,
    bear: String,
    token_env: String,
    client: String,
}

#[derive(Clone, Debug)]
struct BrowserBridgeConfig {
    bind: String,
    token: String,
    path: String,
    allowed_origins: Vec<String>,
}

#[derive(Clone, Default)]
struct AdapterState {
    client_capabilities: Value,
    session_contexts: HashMap<String, SessionContext>,
    transport: JsonRpcTransport,
}

#[derive(Clone)]
struct AdapterSharedState {
    transport: JsonRpcTransport,
    client_capabilities: Arc<TokioMutex<Value>>,
    session_contexts: Arc<TokioMutex<HashMap<String, SessionContext>>>,
    last_plan_update_hashes: Arc<TokioMutex<HashMap<String, u64>>>,
    surface_tool_statuses: Arc<TokioMutex<HashMap<String, SurfaceToolStatus>>>,
    tool_tasks: ToolTaskRegistry,
    mcp_registry: McpRegistry,
    approval_cache: ApprovalCache,
    cancellation_tx: broadcast::Sender<CancellationNotice>,
    active_prompts: Arc<TokioMutex<HashMap<String, ActivePromptTurn>>>,
    projection_dispatcher: AcpProjectionDispatcher,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancellationScope {
    PromptAndTools,
    ToolsOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancellationOrigin {
    SessionCancel,
    SessionClose,
    Steering,
    RunTerminal,
    DeliveryDeadline,
}

#[derive(Clone, Debug)]
struct CancellationNotice {
    session_id: String,
    turn_token: Option<Uuid>,
    conversation_id: Option<String>,
    scope: CancellationScope,
    origin: CancellationOrigin,
}

#[derive(Clone, Debug)]
struct ActivePromptTurn {
    token: Uuid,
    conversation_id: Option<String>,
    response: PromptResponseGuard,
}

#[derive(Clone, Debug)]
pub(crate) struct PromptResponseGuard {
    id: Value,
    sent: Arc<AtomicBool>,
}

impl PromptResponseGuard {
    fn new(id: Value) -> Self {
        Self {
            id,
            sent: Arc::new(AtomicBool::new(false)),
        }
    }

    fn claim(&self) -> Option<Value> {
        (!self.sent.swap(true, Ordering::AcqRel)).then(|| self.id.clone())
    }
}

#[allow(dead_code)]
#[derive(Default)]
struct SseFrameOutcome {
    saw_visible_output: bool,
    saw_tool_activity: bool,
    saw_error: bool,
    saw_done: bool,
    recover_and_retry: bool,
    saw_cancellation_error: bool,
    terminal_outcome: Option<String>,
    recovery_hint: Option<String>,
    terminal_user_message: Option<String>,
    upstream_errors: Vec<String>,
}

fn env_bool(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn init_armature_tracing() {
    if !env_bool("BEARS_ARMATURE_TRACE") {
        return;
    }
    let filter = env::var("BEARS_ARMATURE_TRACE_FILTER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "bear_armature::lifecycle=debug".to_string());
    let Ok(filter) = tracing_subscriber::EnvFilter::try_new(filter.clone()) else {
        eprintln!(
            "bear-armature: invalid BEARS_ARMATURE_TRACE_FILTER={filter:?}; tracing disabled"
        );
        return;
    };
    let _ = tracing_subscriber::fmt()
        .compact()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true)
        .without_time()
        .try_init();
    tracing::info!(
        target: "bear_armature::lifecycle",
        "armature lifecycle tracing enabled"
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BearDebugMode {
    Off,
    On,
    Verbose,
}

impl BearDebugMode {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "0" | "false" | "no" | "off" => Some(Self::Off),
            "1" | "true" | "yes" | "on" => Some(Self::On),
            "verbose" | "debug" | "trace" => Some(Self::Verbose),
            _ => None,
        }
    }

    fn from_env() -> Self {
        env::var("BEAR_DEBUG")
            .ok()
            .and_then(|value| Self::parse(&value))
            .unwrap_or(Self::Off)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Verbose => "verbose",
        }
    }

    fn shows_thoughts(self) -> bool {
        matches!(self, Self::On | Self::Verbose)
    }

    fn is_verbose(self) -> bool {
        matches!(self, Self::Verbose)
    }
}

static BEAR_DEBUG_MODE: OnceLock<RwLock<BearDebugMode>> = OnceLock::new();

fn bear_debug_lock() -> &'static RwLock<BearDebugMode> {
    BEAR_DEBUG_MODE.get_or_init(|| RwLock::new(BearDebugMode::from_env()))
}

fn bear_debug_mode() -> BearDebugMode {
    bear_debug_lock()
        .read()
        .map(|guard| *guard)
        .unwrap_or_else(|_| BearDebugMode::from_env())
}

fn set_bear_debug_mode(mode: BearDebugMode) {
    if let Ok(mut guard) = bear_debug_lock().write() {
        *guard = mode;
    }
}

pub(crate) fn bear_debug_verbose() -> bool {
    bear_debug_mode().is_verbose()
}

#[derive(Clone, Debug, Default)]
struct SessionContext {
    cwd: String,
    roots: Vec<String>,
    raw: Value,
    mcp_sources: Vec<McpSourceConfig>,
    conversation_id: Option<String>,
    resolved_conversation_id: Option<String>,
    thread_title: Option<String>,
    current_mode: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct ToolPolicy {
    max_lines: Option<usize>,
    max_entries: Option<usize>,
    max_results: Option<usize>,
    max_bytes: Option<u64>,
    recursive_default: Option<bool>,
    include_hidden_default: Option<bool>,
    execution_target: Option<String>,
    approval_policy: Option<String>,
    sensitive_path_policy: Option<String>,
    target_policy: Option<Value>,
    max_replacements: Option<usize>,
    create_files: Option<bool>,
    allow_multiple: Option<bool>,
    deny_hidden_paths: Option<bool>,
    total_timeout_ms: Option<u64>,
    permission_timeout_ms: Option<u64>,
}

const MODE_ASK: &str = "ask";
const MODE_PLAN: &str = "plan";
const MODE_WRITE: &str = "write";
const FOCUS_TITLE_PREFIX: &str = "⌖ ";
const DEN_ACP_ADAPTER_CONTRACT_NAME: &str = "bears.acp.adapter";
const DEN_ACP_ADAPTER_CONTRACT_VERSION: u32 = 1;
const LOCAL_DEN_INSPECTION_TIMEOUT: Duration = Duration::from_secs(2);

fn project_focused_acp_title(title: Option<String>) -> Option<String> {
    title.map(|title| {
        let bare = title.strip_prefix(FOCUS_TITLE_PREFIX).unwrap_or(&title);
        format!("{FOCUS_TITLE_PREFIX}{bare}")
    })
}

pub(crate) fn adapter_version() -> &'static str {
    env!("DEN_ACP_ADAPTER_VERSION")
}

impl ToolPolicy {
    fn risk(&self) -> &str {
        let _execution_target = self.execution_target.as_deref().unwrap_or("armature_local");
        let _approval_policy = self.approval_policy.as_deref().unwrap_or("required");
        if self.create_files.is_some()
            || self.allow_multiple.is_some()
            || self.max_replacements.is_some()
        {
            "writes_workspace"
        } else {
            "read_only"
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalToolStatus {
    Ok,
    Error,
    PermissionDenied,
    Timeout,
    Cancelled,
    Unsupported,
}

impl LocalToolStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::PermissionDenied => "permission_denied",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug)]
struct LocalToolError {
    status: LocalToolStatus,
    message: String,
    diagnostic: Value,
}

fn session_config_options_for_mode(mode: &str) -> Vec<SessionConfigOption> {
    let mode = normalize_mode(mode);
    vec![mode_config_option(mode)]
}

fn mode_config_option(mode: &str) -> SessionConfigOption {
    let mode = normalize_mode(mode);
    SessionConfigOption::select(
        "mode",
        "Session Mode",
        mode,
        vec![
            SessionConfigSelectOption::new(MODE_ASK, "Ask")
                .description("Mutation gate closed; read, search, and inspect only."),
            SessionConfigSelectOption::new(MODE_PLAN, "Plan").description(
                "Mutation gate review_required; read-only until the plan is approved.",
            ),
            SessionConfigSelectOption::new(MODE_WRITE, "Write").description(
                "Mutation gate open; workspace changes are allowed subject to approval policy.",
            ),
        ],
    )
    .description("Reflects trusted Den session policy for mutation-gate state.")
    .category(SessionConfigOptionCategory::Mode)
}

fn model_config_option(model_state: &Value) -> Option<SessionConfigOption> {
    let effective = model_state
        .get("effective_model")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let selected = if model_state.get("selection_mode").and_then(Value::as_str) == Some("explicit")
    {
        model_state
            .get("selected_model")
            .or_else(|| model_state.get("requested_model"))
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .to_string()
    } else {
        "auto".to_string()
    };
    let mut options = vec![SessionConfigSelectOption::new(
        "auto",
        "Default",
    )
    .description(format!(
        "Use the current stance/Bear default model for this ACP conversation. Current effective model: {effective}."
    ))];
    if let Some(items) = model_state.get("model_options").and_then(Value::as_array) {
        for item in items {
            let Some(handle) = item.get("handle").and_then(Value::as_str) else {
                continue;
            };
            let label = item.get("label").and_then(Value::as_str).unwrap_or(handle);
            options.push(SessionConfigSelectOption::new(
                handle.to_string(),
                label.to_string(),
            ));
        }
    }
    Some(
        SessionConfigOption::select("model", "Model", selected, options)
            .description(
                "Conversation-scoped model selection. Auto inherits the stance/Bear default.",
            )
            .category(SessionConfigOptionCategory::Mode),
    )
}

fn session_config_options_for_context(
    context: Option<&SessionContext>,
    mode: &str,
) -> Vec<SessionConfigOption> {
    let mut options = vec![mode_config_option(mode)];
    if let Some(model_state) = context.and_then(|ctx| ctx.raw.get("model_selection")) {
        if let Some(option) = model_config_option(model_state) {
            options.push(option);
        }
    }
    options
}

fn normalize_mode(mode: &str) -> &'static str {
    match mode.trim().to_ascii_lowercase().as_str() {
        MODE_PLAN => MODE_PLAN,
        MODE_WRITE => MODE_WRITE,
        _ => MODE_ASK,
    }
}

fn set_context_mode(
    context: &mut SessionContext,
    mode: &str,
    source: &str,
    pending_den_sync: bool,
) -> &'static str {
    let mode = normalize_mode(mode);
    context.current_mode = Some(mode.to_string());
    if !context.raw.is_object() {
        context.raw = json!({});
    }
    context.raw["session_mode"] = json!({
        "requested_mode": mode,
        "effective_mode": mode,
        "source": source,
        "pending_den_sync": pending_den_sync,
    });
    mode
}

async fn remember_session_mode(
    shared_state: &AdapterSharedState,
    adapter_state: &mut AdapterState,
    session_id: &str,
    mode: &str,
    source: &str,
    pending_den_sync: bool,
) -> &'static str {
    let mode = normalize_mode(mode);
    if let Some(context) = adapter_state.session_contexts.get_mut(session_id) {
        set_context_mode(context, mode, source, pending_den_sync);
    }
    if let Some(context) = shared_state
        .session_contexts
        .lock()
        .await
        .get_mut(session_id)
    {
        set_context_mode(context, mode, source, pending_den_sync);
    }
    mode
}

fn session_modes_for_mode(mode: &str) -> SessionModeState {
    let mode = normalize_mode(mode);
    SessionModeState::new(
        mode,
        vec![
            SessionMode::new(MODE_ASK, "Ask")
                .description("Mutation gate closed; read, search, and inspect only."),
            SessionMode::new(MODE_PLAN, "Plan").description(
                "Mutation gate review_required; read-only until the plan is approved.",
            ),
            SessionMode::new(MODE_WRITE, "Write").description(
                "Mutation gate open; workspace changes are allowed subject to approval policy.",
            ),
        ],
    )
}

fn infer_mode_from_plan_mode_state(plan_mode: Option<&Value>) -> &'static str {
    match plan_mode
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
    {
        Some("active" | "submitted") => MODE_PLAN,
        Some("approved") => MODE_WRITE,
        _ => MODE_ASK,
    }
}

fn session_id_from_config_params(params: &Value) -> Result<&str> {
    params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session config params missing sessionId"))
}

fn config_value_from_params(params: &Value) -> Result<&str> {
    params
        .get("value")
        .and_then(|value| {
            if let Some(raw) = value.as_str() {
                Some(raw)
            } else {
                value.get("value").and_then(Value::as_str)
            }
        })
        .ok_or_else(|| anyhow!("session config params missing value"))
}

fn mode_value_from_config_params(params: &Value) -> Result<&str> {
    params
        .get("value")
        .and_then(|value| {
            if let Some(raw) = value.as_str() {
                Some(raw)
            } else {
                value.get("value").and_then(Value::as_str)
            }
        })
        .ok_or_else(|| anyhow!("session config params missing mode value"))
}

fn plan_entry_from_work_plan_item(item: &Value) -> Option<PlanEntry> {
    let title = item.get("title").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }
    let raw_status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending");
    let blocked_reason = item
        .get("blocked_reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let summary = item
        .get("summary")
        .and_then(Value::as_str)
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
        "in_progress" => PlanEntryStatus::InProgress,
        "completed" | "cancelled" => PlanEntryStatus::Completed,
        _ => PlanEntryStatus::Pending,
    };
    let priority = if raw_status == "in_progress" {
        PlanEntryPriority::High
    } else {
        PlanEntryPriority::Medium
    };
    Some(PlanEntry::new(content, priority, status))
}

fn plan_entry_from_acp_plan_item(item: &Value) -> Option<PlanEntry> {
    let content = item.get("content").and_then(Value::as_str)?.trim();
    if content.is_empty() {
        return None;
    }
    let priority = match item
        .get("priority")
        .and_then(Value::as_str)
        .unwrap_or("medium")
    {
        "high" => PlanEntryPriority::High,
        "low" => PlanEntryPriority::Low,
        _ => PlanEntryPriority::Medium,
    };
    let status = match item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending")
    {
        "in_progress" => PlanEntryStatus::InProgress,
        "completed" => PlanEntryStatus::Completed,
        _ => PlanEntryStatus::Pending,
    };
    Some(PlanEntry::new(content, priority, status))
}

pub(crate) fn plan_entries_from_plan_update_event(event: &Value) -> Vec<PlanEntry> {
    event
        .get("entries")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    plan_entry_from_acp_plan_item(item)
                        .or_else(|| plan_entry_from_work_plan_item(item))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn submitted_plan_fallback_entry(value: &Value) -> Option<PlanEntry> {
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Submitted implementation plan");
    Some(PlanEntry::new(
        format!("Review submitted implementation plan: {title}"),
        PlanEntryPriority::High,
        PlanEntryStatus::InProgress,
    ))
}

fn plan_entries_from_den_session(den: &Value) -> Vec<PlanEntry> {
    if let Some(fallback) = den.get("approval_fallback") {
        if let Some(entry) = submitted_plan_fallback_entry(fallback) {
            return vec![entry];
        }
    }
    den.get("plan_mode")
        .filter(|plan| plan.get("state").and_then(Value::as_str) == Some("submitted"))
        .and_then(|plan| {
            submitted_plan_fallback_entry(&json!({
                "title": plan.get("plan_title").cloned().unwrap_or(Value::Null),
            }))
        })
        .map(|entry| vec![entry])
        .unwrap_or_default()
}

fn plan_approval_fallback_message(value: &Value) -> Option<String> {
    let plan_id = value.get("plan_id").and_then(Value::as_str)?;
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Submitted implementation plan");
    let artifact_path = value
        .get("artifact_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("not_submitted");
    let body = value
        .get("body")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Plan body is unavailable; use the artifact path for audit context.");
    Some(format!(
        "\n\n## Submitted implementation plan awaiting approval\n\n**{title}**\n\nArtifact: `{artifact_path}`\nPlan ID: `{plan_id}`\n\n{body}\n\nUse the approval target if your ACP client shows one, or reply `approved` / `go ahead` to approve this submitted plan."
    ))
}

async fn surface_submitted_plan_fallback(session_id: &str, den: &Value) -> Result<()> {
    let entries = plan_entries_from_den_session(den);
    if !entries.is_empty() {
        send_plan_update(session_id, entries).await?;
    }
    if let Some(fallback) = den.get("approval_fallback") {
        if let Some(message) = plan_approval_fallback_message(fallback) {
            send_agent_message_chunk(session_id, &message).await?;
        }
    }
    Ok(())
}

fn plan_entries_from_work_plan_args(args: &Value) -> Vec<PlanEntry> {
    args.get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(plan_entry_from_work_plan_item)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn plan_entries_hash(entries: &[PlanEntry]) -> Result<u64> {
    let value = serde_json::to_string(entries)?;
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    Ok(hasher.finish())
}

async fn should_send_plan_update(
    shared_state: &AdapterSharedState,
    session_id: &str,
    entries: &[PlanEntry],
) -> Result<bool> {
    let hash = plan_entries_hash(entries)?;
    let mut hashes = shared_state.last_plan_update_hashes.lock().await;
    if hashes.get(session_id).copied() == Some(hash) {
        return Ok(false);
    }
    hashes.insert(session_id.to_string(), hash);
    Ok(true)
}

async fn send_available_commands_update(session_id: &str) -> Result<()> {
    let commands = local_slash_available_commands();
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": serde_json::to_value(SessionUpdate::AvailableCommandsUpdate(
                AvailableCommandsUpdate::new(commands)
            ))?,
        }),
    )
    .await
}

async fn refresh_slash_commands_for_session(session_id: &str) {
    if let Err(err) = send_available_commands_update(session_id).await {
        eprintln!(
            "bear-armature: failed to refresh slash commands session_id={session_id} error={err:#}"
        );
    }
}

async fn refresh_slash_commands_for_all_sessions(shared_state: &AdapterSharedState) {
    let session_ids: Vec<String> = shared_state
        .session_contexts
        .lock()
        .await
        .keys()
        .cloned()
        .collect();
    for session_id in session_ids {
        refresh_slash_commands_for_session(&session_id).await;
    }
}

fn spawn_adapter_environment_publish(
    config: Config,
    session_id: String,
    adapter_state: AdapterState,
    conversation_title: Option<String>,
) {
    tokio::spawn(async move {
        let snapshot = match collect_bear_environment(
            &adapter_state,
            &session_id,
            Some(&config),
            None,
            &json!({
                "include_session_mcp": true,
                "include_client_capabilities": true,
                "include_raw_context": true,
                "inspect_den": false,
            }),
        )
        .await
        {
            Ok(snapshot) => snapshot,
            Err(err) => {
                eprintln!(
                    "bear-armature: failed to collect adapter environment for publish session_id={} error={err:#}",
                    session_id
                );
                return;
            }
        };
        if let Err(err) = post_adapter_environment(
            &config,
            &session_id,
            snapshot,
            conversation_title.as_deref(),
        )
        .await
        {
            if bear_debug_verbose() {
                eprintln!(
                    "bear-armature: failed to publish adapter environment session_id={} error={err:#}",
                    session_id
                );
            }
        }
    });
}

async fn send_session_info_update(
    session_id: &str,
    title: Option<String>,
    updated_at: Option<String>,
) -> Result<()> {
    write_notification(
        "session/update",
        acp_session_info_update_payload(session_id, title, updated_at)?,
    )
    .await
}

fn acp_session_info_update_payload(
    session_id: &str,
    title: Option<String>,
    updated_at: Option<String>,
) -> Result<Value> {
    let mut update = SessionInfoUpdate::new();
    if let Some(title) = title {
        update = update.title(title);
    }
    if let Some(updated_at) = updated_at {
        update = update.updated_at(updated_at);
    }
    Ok(json!({
        "sessionId": session_id,
        "update": serde_json::to_value(SessionUpdate::SessionInfoUpdate(update))?,
    }))
}

async fn send_den_runtime_session_info_update(
    session_id: &str,
    runtime: Option<Value>,
    context_budget: Option<Value>,
) -> Result<()> {
    let mut bears = serde_json::Map::new();
    if let Some(runtime) = runtime {
        bears.insert("runtime".to_string(), runtime);
    }
    if let Some(context_budget) = context_budget {
        bears.insert("context_budget".to_string(), context_budget);
    }
    if bears.is_empty() {
        return Ok(());
    }
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "session_info_update",
                "_meta": {
                    "bears": Value::Object(bears),
                }
            }
        }),
    )
    .await
}

fn context_budget_token_count(context_budget: &Value, key: &str) -> Option<u64> {
    context_budget.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|count| u64::try_from(count).ok()))
    })
}

fn acp_usage_update_payload(session_id: &str, context_budget: Value) -> Option<Value> {
    let used = context_budget_token_count(&context_budget, "estimated_total_tokens")
        .or_else(|| context_budget_token_count(&context_budget, "estimated_input_tokens"))?;
    let size = context_budget_token_count(&context_budget, "context_window")?;
    if size == 0 {
        return None;
    }
    Some(json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "usage_update",
            "used": used,
            "size": size,
            "_meta": {
                "bears": {
                    "context_budget": context_budget,
                }
            }
        }
    }))
}

async fn send_context_budget_usage_update(session_id: &str, context_budget: Value) -> Result<()> {
    let Some(payload) = acp_usage_update_payload(session_id, context_budget) else {
        return Ok(());
    };
    write_notification("session/update", payload).await
}

fn acp_plan_update_payload(session_id: &str, entries: Vec<PlanEntry>) -> Result<Value> {
    Ok(json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "plan",
            "entries": entries
                .into_iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?,
        },
    }))
}

async fn send_plan_update(session_id: &str, entries: Vec<PlanEntry>) -> Result<()> {
    let entry_count = entries.len();
    write_notification(
        "session/update",
        acp_plan_update_payload(session_id, entries)?,
    )
    .await?;
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: debug sent ACP plan update session_id={} entry_count={}",
            session_id, entry_count
        );
    }
    Ok(())
}

async fn remember_session_model(
    shared_state: &AdapterSharedState,
    adapter_state: &mut AdapterState,
    session_id: &str,
    model_state: Value,
) {
    if let Some(context) = adapter_state.session_contexts.get_mut(session_id) {
        context.raw["model_selection"] = model_state.clone();
    }
    if let Some(context) = shared_state
        .session_contexts
        .lock()
        .await
        .get_mut(session_id)
    {
        context.raw["model_selection"] = model_state;
    }
}

async fn notify_config_options_for_session(
    shared_state: &AdapterSharedState,
    session_id: &str,
) -> Result<()> {
    let context = shared_state
        .session_contexts
        .lock()
        .await
        .get(session_id)
        .cloned();
    let mode = context
        .as_ref()
        .and_then(|ctx| ctx.current_mode.as_deref())
        .map(normalize_mode)
        .unwrap_or(MODE_ASK);
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": serde_json::to_value(SessionUpdate::ConfigOptionUpdate(
                ConfigOptionUpdate::new(session_config_options_for_context(context.as_ref(), mode))
            ))?,
        }),
    )
    .await
}

pub(crate) async fn sync_session_model_from_den(
    http: &reqwest::Client,
    config: Option<&Config>,
    shared_state: &AdapterSharedState,
    adapter_state: &mut AdapterState,
    session_id: &str,
) -> Result<Option<Value>> {
    let Some(config) = config else {
        return Ok(None);
    };
    let result = bearwire::rpc_call(
        http,
        config,
        "session.model.get",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
        }),
    )
    .await?;
    remember_session_model(shared_state, adapter_state, session_id, result.clone()).await;
    notify_config_options_for_session(shared_state, session_id).await?;
    Ok(Some(result))
}

async fn notify_mode_state(session_id: &str, mode: &str) -> Result<()> {
    let mode = normalize_mode(mode);
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": serde_json::to_value(SessionUpdate::ConfigOptionUpdate(
                ConfigOptionUpdate::new(session_config_options_for_mode(mode))
            ))?,
        }),
    )
    .await?;
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": serde_json::to_value(SessionUpdate::CurrentModeUpdate(
                CurrentModeUpdate::new(mode)
            ))?,
        }),
    )
    .await
}

impl LocalToolError {
    fn error(message: impl Into<String>) -> Self {
        Self {
            status: LocalToolStatus::Error,
            message: message.into(),
            diagnostic: json!({}),
        }
    }

    fn permission_denied(message: impl Into<String>) -> Self {
        Self {
            status: LocalToolStatus::PermissionDenied,
            message: message.into(),
            diagnostic: json!({
                "component": "bear-armature",
                "phase": "adapter_permission_denied",
                "reason": "client_permission_rejected",
            }),
        }
    }

    fn cancelled(message: impl Into<String>) -> Self {
        Self {
            status: LocalToolStatus::Cancelled,
            message: message.into(),
            diagnostic: json!({
                "component": "bear-armature",
                "phase": "adapter_cancelled",
                "reason": "session_cancelled",
            }),
        }
    }

    fn timeout(message: impl Into<String>) -> Self {
        Self {
            status: LocalToolStatus::Timeout,
            message: message.into(),
            diagnostic: json!({
                "component": "bear-armature",
                "phase": "adapter_permission_timeout",
                "reason": "client_permission_timeout",
            }),
        }
    }

    fn status_str(&self) -> &'static str {
        self.status.as_str()
    }
}

impl std::fmt::Display for LocalToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LocalToolError {}

impl From<anyhow::Error> for LocalToolError {
    fn from(err: anyhow::Error) -> Self {
        Self::error(format!("{err:#}"))
    }
}

#[derive(Debug)]
enum InboundMessage {
    Request(Value),
    Response { id: Value, value: Value },
}

#[derive(Debug)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    params: Value,
}

#[derive(Clone, Debug)]
struct ServerVersion {
    service: String,
    version: String,
    git_sha: String,
    built_at_utc: String,
}

impl ServerVersion {
    fn summary(&self) -> String {
        format!(
            "Den server version: service={}, version={}, git_sha={}, built_at_utc={}",
            self.service, self.version, self.git_sha, self.built_at_utc
        )
    }
}

fn truncate_for_log(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }

    let end = s
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|&index| index <= max)
        .last()
        .unwrap_or(0);
    format!("{}...", &s[..end])
}

fn summarize_mcp_for_log(mcp: Option<&Value>) -> Value {
    let Some(mcp) = mcp else {
        return Value::Null;
    };
    let servers = mcp
        .get("servers")
        .and_then(Value::as_array)
        .map(|servers| {
            servers
                .iter()
                .map(|server| {
                    json!({
                        "name": server.get("name").and_then(Value::as_str),
                        "status": server.get("status").and_then(Value::as_str),
                        "transport": server.get("transport").and_then(Value::as_str),
                        "tool_count": server.get("tool_count").and_then(Value::as_u64),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let tool_names = mcp
        .get("client_tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "server_count": servers.len(),
        "servers": servers,
        "tool_count": tool_names.len(),
        "tool_names": tool_names,
    })
}

#[derive(Default)]
struct SseStreamDiagnostics {
    frames: usize,
    events: usize,
    fetch_errors: usize,
    event_errors: usize,
    event_error_samples: Vec<String>,
    event_types: HashMap<String, usize>,
    unknown_event_samples: Vec<String>,
    saw_turn_complete: bool,
    saw_visible_output: bool,
    saw_tool_activity: bool,
    saw_error: bool,
}

impl SseStreamDiagnostics {
    fn observe_event(&mut self, event: &Value) {
        self.events += 1;
        let ty = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("<missing>")
            .to_string();
        *self.event_types.entry(ty).or_insert(0) += 1;
    }

    fn observe_unknown(&mut self, event: &Value) {
        if self.unknown_event_samples.len() < 5 {
            self.unknown_event_samples
                .push(truncate_for_log(&event.to_string(), 360));
        }
    }

    fn observe_event_error(&mut self, event: &Value, error: &anyhow::Error) {
        self.event_errors += 1;
        if self.event_error_samples.len() < 5 {
            let event_type = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("<missing>");
            self.event_error_samples.push(format!(
                "type={event_type} error={} event={}",
                truncate_for_log(&error.to_string(), 240),
                truncate_for_log(&event.to_string(), 360),
            ));
        }
    }

    fn summary(&self) -> String {
        format!(
            "frames={}, events={}, fetch_errors={}, event_errors={}, event_error_samples={:?}, event_types={:?}, unknown_samples={:?}, saw_turn_complete={}, saw_visible_output={}, saw_tool_activity={}, saw_error={}",
            self.frames,
            self.events,
            self.fetch_errors,
            self.event_errors,
            self.event_error_samples,
            self.event_types,
            self.unknown_event_samples,
            self.saw_turn_complete,
            self.saw_visible_output,
            self.saw_tool_activity,
            self.saw_error,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletedTurnWithoutText {
    Expected,
    Anomalous,
}

fn classify_completed_turn_without_text(
    saw_error: bool,
    saw_tool_activity: bool,
) -> CompletedTurnWithoutText {
    if saw_error || !saw_tool_activity {
        CompletedTurnWithoutText::Anomalous
    } else {
        CompletedTurnWithoutText::Expected
    }
}

#[derive(Clone, Default)]
struct BrowserBridgeServer;

#[derive(Debug, Deserialize, JsonSchema)]
struct ChromeOpenArgs {
    url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChromeListArgs {
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChromeScreenshotArgs {
    #[serde(default)]
    format: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    chrome: String,
}

impl ServerHandler for BrowserBridgeServer {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::default();
        capabilities.tools = Some(Default::default());
        ServerInfo::new(capabilities)
        .with_server_info(
            McpImplementation::new("bears-host-browser-bridge", adapter_version())
                .with_title("BEARS Host Browser MCP Bridge")
                .with_description("Browser-only MCP bridge served by bear-armature."),
        )
        .with_instructions("This MCP server exposes browser-only tools from the BEARS host browser bridge. It can inspect and control the local browser via Chrome DevTools Protocol, but it does not provide host filesystem, host shell, or host git access.")
    }
}

async fn browser_bridge_tool_result(
    value: Result<Value>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut result = CallToolResult::default();
    match value {
        Ok(value) => {
            result.content = vec![Content::text(value.to_string())];
            result.structured_content = Some(value);
            result.is_error = Some(false);
        }
        Err(err) => {
            result.content = vec![Content::text(format!("browser bridge tool error: {err:#}"))];
            result.structured_content = Some(json!({ "ok": false, "error": format!("{err:#}") }));
            result.is_error = Some(true);
        }
    }
    Ok(result)
}

fn browser_bridge_authorized(
    headers: &axum::http::HeaderMap,
    config: &BrowserBridgeConfig,
) -> bool {
    let auth = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value: &axum::http::HeaderValue| value.to_str().ok())
        .unwrap_or("");
    let expected = format!("Bearer {}", config.token);
    auth == expected
}

type BrowserBridgeHttpService =
    Arc<TokioMutex<StreamableHttpService<McpRouter<BrowserBridgeServer>, LocalSessionManager>>>;

fn browser_bridge_router(
    config: BrowserBridgeConfig,
    service: BrowserBridgeHttpService,
) -> axum::Router {
    use axum::{
        routing::{any, get},
        Router,
    };

    let mcp_path = config.path.clone();
    Router::new()
        .route(
            "/health",
            get({
                move || async move {
                    axum::Json(HealthResponse {
                        ok: true,
                        service: "bears-host-browser-bridge",
                        chrome: chrome_capability_status_line(),
                    })
                }
            }),
        )
        .route(&mcp_path, any(browser_bridge_mcp_handler))
        .with_state((config, service))
}

async fn browser_bridge_mcp_handler(
    State((config, service)): State<(BrowserBridgeConfig, BrowserBridgeHttpService)>,
    request: axum::extract::Request,
) -> impl axum::response::IntoResponse {
    if !browser_bridge_authorized(request.headers(), &config) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let mut service = service.lock().await;
    match service.call(request).await {
        Ok(response) => response.into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("browser bridge transport error: {err:#}"),
        )
            .into_response(),
    }
}

async fn run_browser_bridge(config: BrowserBridgeConfig) -> Result<()> {
    let session_manager = Arc::new(LocalSessionManager::default());
    let service = Arc::new(TokioMutex::new(StreamableHttpService::new(
        || {
            let router = McpRouter::new(BrowserBridgeServer)
                .with_tool(route_browser_open())
                .with_tool(route_browser_snapshot())
                .with_tool(route_browser_console_messages())
                .with_tool(route_browser_network_requests())
                .with_tool(route_browser_screenshot());
            Ok(router)
        },
        session_manager,
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true)
            .with_allowed_hosts(["127.0.0.1", "localhost", "::1"])
            .with_allowed_origins(config.allowed_origins.clone()),
    )));

    let mcp_path = config.path.clone();
    let bind = config.bind.clone();
    let app = browser_bridge_router(config.clone(), service.clone());

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind browser bridge listener on {bind}"))?;
    eprintln!(
        "bear-armature: browser-bridge listening addr={} path={} chrome={} origins={:?}",
        bind,
        mcp_path,
        chrome_capability_status_line(),
        config.allowed_origins
    );
    axum::serve(listener, app)
        .await
        .context("serve browser bridge HTTP")
}

fn browser_tool(
    name: &'static str,
    description: &'static str,
    input_schema: std::sync::Arc<serde_json::Map<String, Value>>,
) -> McpTool {
    let mut tool = McpTool::default();
    tool.name = name.into();
    tool.description = Some(description.into());
    tool.input_schema = input_schema;
    tool
}

fn route_browser_open() -> rmcp::handler::server::router::tool::ToolRoute<BrowserBridgeServer> {
    rmcp::handler::server::router::tool::ToolRoute::new_dyn(
        browser_tool(
            "browser_open",
            "Open a URL in the host browser and focus the new tab/target.",
            schema_for_type::<ChromeOpenArgs>(),
        ),
        |mut context| {
            Box::pin(async move {
                let Parameters(args): Parameters<ChromeOpenArgs> =
                    Parameters::from_context_part(&mut context)?;
                browser_bridge_tool_result(
                    handle_chrome_open(&json!({ "url": args.url }), &ToolPolicy::default()).await,
                )
                .await
            })
        },
    )
}

fn route_browser_snapshot() -> rmcp::handler::server::router::tool::ToolRoute<BrowserBridgeServer> {
    rmcp::handler::server::router::tool::ToolRoute::new_dyn(
        browser_tool(
            "browser_snapshot",
            "Capture an accessibility-tree text snapshot of the active browser page.",
            schema_for_type::<()>(),
        ),
        |_context| {
            Box::pin(async move {
                browser_bridge_tool_result(
                    handle_chrome_snapshot(&Value::Null, &ToolPolicy::default()).await,
                )
                .await
            })
        },
    )
}

fn route_browser_console_messages(
) -> rmcp::handler::server::router::tool::ToolRoute<BrowserBridgeServer> {
    rmcp::handler::server::router::tool::ToolRoute::new_dyn(
        browser_tool(
            "browser_console_messages",
            "List recent console and log events from the active browser page.",
            schema_for_type::<ChromeListArgs>(),
        ),
        |mut context| {
            Box::pin(async move {
                let Parameters(args): Parameters<ChromeListArgs> =
                    Parameters::from_context_part(&mut context)?;
                browser_bridge_tool_result(
                    handle_chrome_console_messages(
                        &json!({ "limit": args.limit }),
                        &ToolPolicy::default(),
                    )
                    .await,
                )
                .await
            })
        },
    )
}

fn route_browser_network_requests(
) -> rmcp::handler::server::router::tool::ToolRoute<BrowserBridgeServer> {
    rmcp::handler::server::router::tool::ToolRoute::new_dyn(
        browser_tool(
            "browser_network_requests",
            "List recent network request events from the active browser page.",
            schema_for_type::<ChromeListArgs>(),
        ),
        |mut context| {
            Box::pin(async move {
                let Parameters(args): Parameters<ChromeListArgs> =
                    Parameters::from_context_part(&mut context)?;
                browser_bridge_tool_result(
                    handle_chrome_network_requests(
                        &json!({ "limit": args.limit }),
                        &ToolPolicy::default(),
                    )
                    .await,
                )
                .await
            })
        },
    )
}

fn route_browser_screenshot() -> rmcp::handler::server::router::tool::ToolRoute<BrowserBridgeServer>
{
    rmcp::handler::server::router::tool::ToolRoute::new_dyn(
        browser_tool(
            "browser_screenshot",
            "Capture a screenshot of the active browser page.",
            schema_for_type::<ChromeScreenshotArgs>(),
        ),
        |mut context| {
            Box::pin(async move {
                let Parameters(args): Parameters<ChromeScreenshotArgs> =
                    Parameters::from_context_part(&mut context)?;
                browser_bridge_tool_result(
                    handle_chrome_screenshot(
                        &json!({ "format": args.format }),
                        &ToolPolicy::default(),
                    )
                    .await,
                )
                .await
            })
        },
    )
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("bear-armature: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    init_armature_tracing();
    let mut runtime = RuntimeConfig::from_env_and_args()?;
    eprintln!(
        "bear-armature: starting version={} build_git_sha={} built_at_utc={} local_head_sha={} ACP sessions=list/resume/load supported direct_tools={}",
        adapter_version(),
        env!("DEN_ACP_ADAPTER_GIT_SHA"),
        env!("DEN_ACP_ADAPTER_BUILT_AT_UTC"),
        local_head_sha(),
        direct_tools_context()
    );
    eprintln!(
        "bear-armature: chrome tools {}",
        chrome_capability_status_line()
    );
    if let Some(browser_bridge) = runtime.browser_bridge.clone() {
        run_browser_bridge(browser_bridge).await?;
        return Ok(());
    }

    if !runtime.doctor && runtime.update_command.is_none() {
        if runtime.is_configured() {
            eprintln!("bear-armature: configuration looks valid");
        } else {
            eprintln!("{}", runtime.configuration_error_message());
        }
    }

    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        // Prompt responses are long-lived SSE streams. Do not set a global
        // per-request timeout here; it would abort healthy turns that spend
        // several minutes in local tool execution or model continuation.
        // Specific non-streaming operations use their own timeouts where needed.
        .build()
        .context("build HTTP client")?;

    if let Some(update_command) = runtime.update_command.clone() {
        run_update_command(&http, update_command).await?;
        return Ok(());
    }

    if runtime.doctor {
        run_doctor(&http, &runtime).await?;
        return Ok(());
    }

    if runtime.check_server {
        let Some(config) = runtime.config.as_ref() else {
            return Err(anyhow!(runtime.configuration_error_message()));
        };
        check_server_version(&http, config).await?;
        return Ok(());
    }

    if runtime.headless {
        return headless::run_headless(&http, &runtime).await;
    }

    let (inbound_tx, mut inbound_rx) = mpsc::channel::<InboundMessage>(128);
    let mut adapter_state = AdapterState::default();
    let approval_cache = ApprovalCache::load_for_runtime(&runtime).await;
    let (cancellation_tx, _) = broadcast::channel(64);
    let shared_state = AdapterSharedState {
        transport: adapter_state.transport.clone(),
        client_capabilities: Arc::new(TokioMutex::new(Value::Null)),
        session_contexts: Arc::new(TokioMutex::new(HashMap::new())),
        last_plan_update_hashes: Arc::new(TokioMutex::new(HashMap::new())),
        surface_tool_statuses: Arc::new(TokioMutex::new(HashMap::new())),
        tool_tasks: ToolTaskRegistry::default(),
        mcp_registry: McpRegistry::default(),
        approval_cache,
        cancellation_tx,
        active_prompts: Arc::new(TokioMutex::new(HashMap::new())),
        projection_dispatcher: AcpProjectionDispatcher::default(),
    };
    tokio::spawn(read_stdin_messages(
        inbound_tx,
        adapter_state.transport.clone(),
    ));

    while let Some(message) = inbound_rx.recv().await {
        let value = match message {
            InboundMessage::Request(value) => value,
            InboundMessage::Response { id, value } => {
                if !adapter_state
                    .transport
                    .route_response(&id, value.clone())
                    .await
                {
                    let diagnostics = adapter_state.transport.diagnostics().await;
                    let pending = diagnostics
                        .pending
                        .iter()
                        .map(|p| {
                            format!(
                                "{}:{} elapsed_ms={} timeout_ms={}",
                                p.id, p.method, p.elapsed_ms, p.timeout_ms
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let recent_timeouts = diagnostics
                        .recent_timeouts
                        .iter()
                        .map(|t| {
                            format!(
                                "{}:{} elapsed_ms={} timeout_ms={}",
                                t.id, t.method, t.elapsed_ms, t.timeout_ms
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    eprintln!(
                        "bear-armature: unmatched JSON-RPC response id={} value={} pending=[{}] recent_timeouts=[{}]",
                        id_key(&id),
                        truncate_for_log(&value.to_string(), 1200),
                        pending,
                        recent_timeouts,
                    );
                }
                continue;
            }
        };
        let request = match request_from_value(value) {
            Ok(request) => request,
            Err(err) => {
                write_response(
                    None,
                    Err(json_rpc_error(
                        -32700,
                        "Parse error",
                        Some(json!(err.to_string())),
                    )),
                )
                .await?;
                continue;
            }
        };

        let request_method = request.method.clone();
        let request_id = request.id.clone();
        let request_session_id = request
            .params
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.trim().is_empty())
            .map(str::to_owned);
        if let Err(err) = handle_request(
            &http,
            &mut runtime,
            &mut adapter_state,
            &shared_state,
            request,
        )
        .await
        {
            tracing::error!(
                target: "bear_armature::lifecycle",
                request_method,
                request_id = ?request_id,
                session_id = ?request_session_id,
                error = %format!("{err:#}"),
                "ACP request handling failed before a response could be confirmed"
            );
            eprintln!("bear-armature: request handling failed: {err:#}");
        }
    }

    Ok(())
}

impl BrowserBridgeConfig {
    fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut bind =
            env::var("DEN_HOST_BROWSER_MCP_BIND").unwrap_or_else(|_| "127.0.0.1:3766".to_string());
        let mut token = env::var("DEN_HOST_BROWSER_MCP_TOKEN").unwrap_or_default();
        let mut path = env::var("DEN_HOST_BROWSER_MCP_PATH").unwrap_or_else(|_| "/mcp".to_string());
        let mut allowed_origins = env::var("DEN_HOST_BROWSER_MCP_ALLOWED_ORIGINS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--bind" => bind = require_arg_value("--bind", args.next())?,
                "--token" => token = require_arg_value("--token", args.next())?,
                "--path" => path = require_arg_value("--path", args.next())?,
                "--allow-origin" => {
                    allowed_origins.push(require_arg_value("--allow-origin", args.next())?)
                }
                "--help" | "-h" => {
                    print_browser_bridge_help_to_stderr();
                    std::process::exit(0);
                }
                unknown => bail!(
                    "unknown browser-bridge argument {unknown:?}; use `bear-armature browser-bridge --help`"
                ),
            }
        }

        bind = bind.trim().to_string();
        token = token.trim().to_string();
        if bind.is_empty() {
            bail!("browser-bridge requires a non-empty bind address; pass --bind <host:port>");
        }
        if token.is_empty() {
            bail!(
                "browser-bridge requires a bearer token; set DEN_HOST_BROWSER_MCP_TOKEN or pass --token <token>"
            );
        }
        path = normalize_browser_bridge_path(&path);
        allowed_origins.retain(|origin| !origin.trim().is_empty());

        Ok(Self {
            bind,
            token,
            path,
            allowed_origins,
        })
    }
}

fn args_look_like_legacy_acp(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--api-url"
                | "--bear"
                | "--token"
                | "--token-env"
                | "--client"
                | "--check-config"
                | "--check-server"
        )
    })
}

fn env_looks_like_acp_configured() -> bool {
    let api_url = env::var("DEN_API_URL").unwrap_or_default();
    let bear = env::var("BEAR_SLUG").unwrap_or_default();
    if api_url.trim().is_empty() || bear.trim().is_empty() {
        return false;
    }
    if !env::var("DEN_TOKEN").unwrap_or_default().trim().is_empty() {
        return true;
    }
    let token_env = env::var("DEN_TOKEN_ENV").unwrap_or_default();
    !token_env.trim().is_empty()
        && env::var(&token_env)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
}

struct AcpConnectionArgs {
    api_url: String,
    bear: String,
    token: String,
    token_env: String,
    client: String,
    check_config: bool,
    check_server: bool,
    doctor: bool,
}

fn parse_acp_connection_args(mut args: impl Iterator<Item = String>) -> Result<AcpConnectionArgs> {
    let mut api_url = env::var("DEN_API_URL").unwrap_or_default();
    let mut bear = env::var("BEAR_SLUG").unwrap_or_default();
    let mut token = env::var("DEN_TOKEN").unwrap_or_default();
    let mut token_env = env::var("DEN_TOKEN_ENV").unwrap_or_default();
    let mut client = env::var("DEN_ACP_CLIENT").unwrap_or_else(|_| "zed".to_string());
    let mut check_config = false;
    let mut check_server = false;
    let mut doctor = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--api-url" => api_url = require_arg_value("--api-url", args.next())?,
            "--bear" => bear = require_arg_value("--bear", args.next())?,
            "--token" => token = require_arg_value("--token", args.next())?,
            "--token-env" => token_env = require_arg_value("--token-env", args.next())?,
            "--client" => client = require_arg_value("--client", args.next())?,
            "--check-config" => check_config = true,
            "--check-server" => check_server = true,
            "doctor" | "--doctor" => doctor = true,
            "--version" | "-V" => {
                print_version_to_stderr();
                std::process::exit(0);
            }
            "--help" | "-h" => {
                print_acp_help_to_stderr();
                std::process::exit(0);
            }
            unknown => {
                return Err(anyhow!(
                    "unknown ACP argument {unknown:?}; use `bear-armature acp --help`"
                ));
            }
        }
    }

    Ok(AcpConnectionArgs {
        api_url,
        bear,
        token,
        token_env,
        client,
        check_config,
        check_server,
        doctor,
    })
}

impl RuntimeConfig {
    fn from_env_and_args() -> Result<Self> {
        let args: Vec<String> = env::args().skip(1).collect();
        let mut update_command: Option<UpdateCommand> = None;
        let mut browser_bridge: Option<BrowserBridgeConfig> = None;

        if args.len() == 1 {
            match args[0].as_str() {
                "--version" | "-V" => {
                    print_version_to_stderr();
                    std::process::exit(0);
                }
                "--help" | "-h" => {
                    print_help_to_stderr();
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        let first = args.first().cloned();
        let mut headless = false;
        let acp_args = match first.as_deref() {
            Some("browser-bridge") => {
                browser_bridge = Some(BrowserBridgeConfig::from_args(args.into_iter().skip(1))?);
                Vec::new()
            }
            Some("update-check") => {
                update_command = Some(UpdateCommand::Check(UpdateOptions::from_args(
                    args.into_iter().skip(1),
                )?));
                Vec::new()
            }
            Some("update") => {
                update_command = Some(UpdateCommand::Update(UpdateOptions::from_args(
                    args.into_iter().skip(1),
                )?));
                Vec::new()
            }
            Some("doctor") | Some("--doctor") => Vec::new(),
            Some("acp") => args.into_iter().skip(1).collect(),
            Some("headless") => {
                headless = true;
                args.into_iter().skip(1).collect()
            }
            Some(_) if args_look_like_legacy_acp(&args) => args,
            Some(unknown) => {
                return Err(anyhow!(
                    "unknown subcommand {unknown:?}; use `bear-armature --help`"
                ));
            }
            None if env_looks_like_acp_configured() => Vec::new(),
            None => {
                print_help_to_stderr();
                std::process::exit(0);
            }
        };

        let AcpConnectionArgs {
            mut api_url,
            mut bear,
            mut token,
            token_env,
            mut client,
            check_config,
            check_server,
            doctor,
        } = if browser_bridge.is_some() || update_command.is_some() {
            AcpConnectionArgs {
                api_url: env::var("DEN_API_URL").unwrap_or_default(),
                bear: env::var("BEAR_SLUG").unwrap_or_default(),
                token: env::var("DEN_TOKEN").unwrap_or_default(),
                token_env: env::var("DEN_TOKEN_ENV").unwrap_or_default(),
                client: env::var("DEN_ACP_CLIENT").unwrap_or_else(|_| "zed".to_string()),
                check_config: false,
                check_server: false,
                doctor: matches!(first.as_deref(), Some("doctor") | Some("--doctor")),
            }
        } else if matches!(first.as_deref(), Some("doctor") | Some("--doctor")) {
            AcpConnectionArgs {
                api_url: env::var("DEN_API_URL").unwrap_or_default(),
                bear: env::var("BEAR_SLUG").unwrap_or_default(),
                token: env::var("DEN_TOKEN").unwrap_or_default(),
                token_env: env::var("DEN_TOKEN_ENV").unwrap_or_default(),
                client: env::var("DEN_ACP_CLIENT").unwrap_or_else(|_| "zed".to_string()),
                check_config: false,
                check_server: false,
                doctor: true,
            }
        } else {
            parse_acp_connection_args(acp_args.into_iter())?
        };

        let mut diagnostics = Vec::new();
        let token_env = token_env.trim().to_string();
        if !token_env.is_empty() {
            match env::var(&token_env) {
                Ok(value) => token = value,
                Err(_) => diagnostics.push(format!(
                    "DEN_TOKEN_ENV points at {token_env:?}, but that environment variable is not set. Export {token_env} or change --token-env."
                )),
            }
        }

        api_url = api_url.trim().trim_end_matches('/').to_string();
        bear = bear.trim().to_string();
        token = token.trim().to_string();
        client = normalize_client(&client);

        validate_api_url(&api_url, &mut diagnostics);
        if bear.is_empty() {
            diagnostics.push("Missing bear slug. Set BEAR_SLUG or pass --bear <slug>.".to_string());
        }
        if token.is_empty() {
            diagnostics.push(
                "Missing Den bearer token. Set DEN_TOKEN, set DEN_TOKEN_ENV to the name of an environment variable containing the token, pass --token <token>, or pass --token-env <env-var>. Den armature tokens include the armature:chat scope."
                    .to_string(),
            );
        }

        let config = if diagnostics.is_empty() {
            Some(Config {
                api_url: api_url.clone(),
                bear: bear.clone(),
                token,
                client: client.clone(),
            })
        } else {
            None
        };

        let runtime = Self {
            config,
            diagnostics,
            check_server,
            doctor,
            headless,
            update_command: update_command.clone(),
            browser_bridge: browser_bridge.clone(),
            api_url,
            bear,
            token_env,
            client,
        };
        if browser_bridge.is_some() || update_command.is_some() {
            return Ok(runtime);
        }

        if check_config {
            if runtime.is_configured() {
                eprintln!("bear-armature: configuration looks valid");
                std::process::exit(0);
            }
            eprintln!("{}", runtime.configuration_error_message());
            std::process::exit(2);
        }

        Ok(runtime)
    }

    fn is_configured(&self) -> bool {
        self.config.is_some()
    }

    fn token_is_present(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|config| !config.token.trim().is_empty())
    }

    fn should_advertise_auth_method(&self) -> bool {
        !self.token_is_present()
    }

    fn configuration_error_message(&self) -> String {
        let mut message = String::from(
            "Bear Den ACP adapter: configuration is incomplete, so prompts cannot be sent to Den yet:",
        );
        for diagnostic in &self.diagnostics {
            message.push_str("\n  - ");
            message.push_str(diagnostic);
        }
        message.push_str(
            "\n\nExample:\n  DEN_API_URL=https://api.bears.example\n  BEAR_SLUG=my-bear\n  DEN_TOKEN=...\n\nFor Zed, put those values in the custom agent server env block, or run with --token-env DEN_TOKEN so the token can stay outside editor settings.",
        );
        message
    }
}

fn validate_api_url(api_url: &str, diagnostics: &mut Vec<String>) {
    if api_url.is_empty() {
        diagnostics.push(
            "Missing Den API URL. Set DEN_API_URL or pass --api-url <url>. Use the API origin reachable from your editor process, for example https://api.bears.example."
                .to_string(),
        );
        return;
    }

    let parsed = match Url::parse(api_url) {
        Ok(url) => url,
        Err(err) => {
            diagnostics.push(format!(
                "Invalid Den API URL {api_url:?}: {err}. Include the scheme, for example https://api.bears.example."
            ));
            return;
        }
    };

    match parsed.scheme() {
        "http" | "https" => {}
        scheme => diagnostics.push(format!(
            "Invalid Den API URL scheme {scheme:?}. Use http:// for local development or https:// for deployed API servers."
        )),
    }

    if parsed.host_str().is_none() {
        diagnostics.push(format!(
            "Invalid Den API URL {api_url:?}: it does not contain a host name."
        ));
    }

    if parsed.path().contains("/acp/") {
        diagnostics.push(
            "DEN_API_URL should be the Den API origin only, not the full ACP prompt endpoint. Use a value like https://api.bears.example, not a URL containing /acp/bears/..."
                .to_string(),
        );
    }
}

fn require_arg_value(flag: &str, value: Option<String>) -> Result<String> {
    value.ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn print_version_to_stderr() {
    eprintln!(
        "bear-armature {}\nBuild git SHA: {}\nLocal HEAD SHA: {}\nACP sessions: list/resume/load; conversations bound via Den\nDirect tools: {}\nChrome tools: {}",
        adapter_version(),
        env!("DEN_ACP_ADAPTER_GIT_SHA"),
        local_head_sha(),
        direct_tools_context(),
        chrome_capability_status_line()
    );
}

fn local_head_sha() -> String {
    Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn normalize_browser_bridge_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        "/mcp".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.trim_end_matches('/').to_string()
    } else {
        format!("/{}", trimmed.trim_end_matches('/'))
    }
}

fn print_browser_bridge_help_to_stderr() {
    eprintln!(
        "bear-armature browser-bridge\n\nUsage: bear-armature browser-bridge [--bind 127.0.0.1:3766] [--path /mcp] [--token <token>] [--allow-origin <origin>]...\n\nOptions:\n  --bind <host:port>      Bind address for the host browser MCP bridge HTTP server\n  --path <path>           MCP HTTP path, default /mcp\n  --token <token>         Required bearer token for Authorization: Bearer <token>\n  --allow-origin <url>    Allowed Origin value for browser requests; repeatable\n  --help                  Show this help\n\nEnvironment fallbacks:\n  DEN_HOST_BROWSER_MCP_BIND\n  DEN_HOST_BROWSER_MCP_PATH\n  DEN_HOST_BROWSER_MCP_TOKEN\n  DEN_HOST_BROWSER_MCP_ALLOWED_ORIGINS  comma-separated list",
    );
}

fn print_acp_help_to_stderr() {
    eprintln!(
        "bear-armature acp\n\nUsage: bear-armature acp --api-url <url> --bear <slug> [--client zed] [--token-env DEN_TOKEN]\n\n\
Options:\n  --api-url <url>        Den API origin, for example https://api.bears.example\n  --bear <slug>          Bear slug to chat with\n  --token <token>        Den armature token with armature:chat scope\n  --token-env <env-var>  Read the Den bearer token from this environment variable\n  --client <name>        Client label: zed, opencode, or acp_adapter\n  --check-config         Validate configuration and exit without starting ACP stdio\n  --check-server         Fetch Den /version and exit without starting ACP stdio\n  --version              Show version/build behavior and exit\n  --help                 Show this help\n\n\
Environment fallbacks:\n  DEN_API_URL\n  BEAR_SLUG\n  DEN_TOKEN\n  DEN_TOKEN_ENV\n  DEN_ACP_CLIENT\n\n\
Legacy editors may still invoke `bear-armature --api-url ... --bear ...` without the `acp` subcommand.\n\
DEN_API_URL should be the API origin only, not the full /acp/bears/... endpoint."
    );
}

static HEADLESS_MODE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Marks this process as a headless sandbox run (no ACP client on stdin).
/// Permission requests are then auto-decided by the headless policy instead
/// of an editor round-trip.
pub(crate) fn set_headless_mode() {
    let _ = HEADLESS_MODE.set(true);
}

pub(crate) fn headless_mode() -> bool {
    *HEADLESS_MODE.get().unwrap_or(&false)
}

fn print_help_to_stderr() {
    eprintln!(
        "bear-armature {}\nBuild git SHA: {}\nLocal HEAD SHA: {}\nACP sessions: list/resume/load; conversations bound via Den\n\n\
Subcommands:\n  acp                    Run ACP stdio mode (explicit)\n  headless               Execute one Den work order in a sandbox (no editor; env-driven)\n  doctor                 Run user-friendly setup checks and exit\n  update-check           Check for a newer signed macOS package\n  update                 Download, verify, and install/open a newer macOS package\n  browser-bridge         Serve browser-only MCP tools over local Streamable HTTP\n\n\
Usage: bear-armature acp --api-url <url> --bear <slug> [--client zed] [--token-env DEN_TOKEN]\n       bear-armature doctor\n       bear-armature update-check [--channel stable]\n       bear-armature update [--open|--install|--download-only] [--yes]\n       bear-armature browser-bridge [--bind 127.0.0.1:3766] [--path /mcp] [--token <token>]\n\n\
Legacy usage (still supported):\n  bear-armature --api-url <url> --bear <slug> [--client zed] [--token-env DEN_TOKEN]\n\n\
Global options:\n  --version              Show version/build behavior and exit\n  --help                 Show this help\n\n\
Environment fallbacks:\n  DEN_API_URL\n  BEAR_SLUG\n  DEN_TOKEN\n  DEN_TOKEN_ENV\n  DEN_ACP_CLIENT\n  BEAR_ARMATURE_UPDATE_CHANNEL / BEARS_ACP_UPDATE_CHANNEL\n  BEAR_ARMATURE_UPDATE_MANIFEST_URL / BEARS_ACP_UPDATE_MANIFEST_URL\n\n\
DEN_API_URL should be the API origin only, not the full /acp/bears/... endpoint.",
        adapter_version(),
        env!("DEN_ACP_ADAPTER_GIT_SHA"),
        local_head_sha()
    );
}

fn normalize_client(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "zed" => "zed".to_string(),
        "opencode" => "opencode".to_string(),
        _ => "acp_adapter".to_string(),
    }
}

async fn read_stdin_messages(tx: mpsc::Sender<InboundMessage>, transport: JsonRpcTransport) {
    let stdin = BufReader::new(io::stdin());
    let mut lines = stdin.lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(line) {
                    Ok(value) => {
                        if value.get("method").and_then(Value::as_str).is_some() {
                            if tx.send(InboundMessage::Request(value)).await.is_err() {
                                break;
                            }
                        } else if let Some(id) = value.get("id").cloned() {
                            if transport.route_response(&id, value.clone()).await {
                                // Matched an adapter-originated request.
                            } else if tx
                                .send(InboundMessage::Response { id, value })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        } else if tx.send(InboundMessage::Request(value)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = write_response(
                            None,
                            Err(json_rpc_error(
                                -32700,
                                "Parse error",
                                Some(json!(err.to_string())),
                            )),
                        )
                        .await;
                    }
                }
            }
            Ok(None) => break,
            Err(err) => {
                eprintln!("bear-armature: failed to read stdin: {err:#}");
                break;
            }
        }
    }
}

fn request_from_value(value: Value) -> Result<JsonRpcRequest> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("JSON-RPC request is missing method"))?
        .to_string();
    let id = value.get("id").cloned();
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    Ok(JsonRpcRequest { id, method, params })
}

async fn handle_request(
    http: &reqwest::Client,
    runtime: &mut RuntimeConfig,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    request: JsonRpcRequest,
) -> Result<()> {
    match request.method.as_str() {
        "initialize" => {
            adapter_state.client_capabilities = normalize_client_capabilities(
                request
                    .params
                    .get("clientCapabilities")
                    .or_else(|| request.params.get("capabilities"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            *shared_state.client_capabilities.lock().await =
                adapter_state.client_capabilities.clone();
            if let Some(id) = request.id {
                write_response(id, Ok(initialize_result(runtime)?)).await?;
            }
        }
        "bears/read_text_file" => {
            if let Some(id) = request.id {
                match handle_direct_read_text_file(
                    adapter_state,
                    request.params,
                    &ToolPolicy::default(),
                )
                .await
                {
                    Ok(result) => write_response(id, Ok(result)).await?,
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32004,
                                "BEARS read_text_file failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                    }
                }
            }
        }
        "authenticate" => {
            if let Some(id) = request.id {
                match handle_authenticate(http, runtime, request.params).await {
                    Ok(()) => {
                        write_response(id, Ok(serde_json::to_value(AuthenticateResponse::new())?))
                            .await?
                    }
                    Err(err) => {
                        refresh_slash_commands_for_all_sessions(shared_state).await;
                        write_response(id, Err(authenticate_json_rpc_error(&err, runtime))).await?;
                    }
                }
            }
        }
        "session/new" => {
            if let Some(id) = request.id {
                let session_id = format!("acp-{}", Uuid::new_v4());
                let context = match session_context_from_params(&request.params) {
                    Ok(context) => context,
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32602,
                                "Invalid session params",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                        return Ok(());
                    }
                };
                let mcp_context = shared_state
                    .mcp_registry
                    .configure_session(&session_id, context.mcp_sources.clone())
                    .await?;
                let mut context = context;
                context.raw["mcp"] = mcp_context;
                ensure_session_context_capabilities(&mut context);
                if bear_debug_verbose() {
                    eprintln!(
                        "bear-armature: session/new session_id={} cwd={} roots={} direct_tools={} mcp={}",
                        session_id,
                        context.cwd,
                        context.roots.join(","),
                        context
                            .raw
                            .get("direct_tools")
                            .cloned()
                            .unwrap_or(Value::Null),
                        summarize_mcp_for_log(context.raw.get("mcp"))
                    );
                }
                let mode = MODE_ASK;
                let conversation_id = prompt_conversation_id_from_params(&request.params);
                if let Some(config) = runtime.config.as_ref() {
                    if bearwire::enabled() {
                        if let Err(err) = bearwire::post_session_open(
                            http,
                            config,
                            &session_id,
                            context.raw.clone(),
                            conversation_id.as_deref(),
                            mode,
                        )
                        .await
                        {
                            write_response(
                                id,
                                Err(json_rpc_error(
                                    -32003,
                                    "BEARS session creation failed",
                                    Some(json!({ "message": format!("{err:#}") })),
                                )),
                            )
                            .await?;
                            return Ok(());
                        }
                    }
                }
                shared_state
                    .session_contexts
                    .lock()
                    .await
                    .insert(session_id.clone(), context.clone());
                adapter_state
                    .session_contexts
                    .insert(session_id.clone(), context);
                if let Some(config) = runtime.config.as_ref() {
                    spawn_adapter_environment_publish(
                        config.clone(),
                        session_id.clone(),
                        adapter_state.clone(),
                        None,
                    );
                }
                let response = NewSessionResponse::new(session_id.clone())
                    .config_options(session_config_options_for_mode(mode))
                    .modes(session_modes_for_mode(mode))
                    .meta(Some(serde_json::Map::from_iter([(
                        "bears".to_string(),
                        json!({
                            "effectiveMode": mode,
                            "source": "adapter.session_new_default",
                            "note": "New ACP sessions default to Ask until Den session policy says otherwise."
                        }),
                    )])));
                write_response(id, Ok(serde_json::to_value(response)?)).await?;
            }
        }
        "session/set_config_option" => {
            if let Some(id) = request.id.clone() {
                let config_id = request
                    .params
                    .get("configId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if config_id != "mode" && config_id != "model" {
                    write_response(
                        id,
                        Err(json_rpc_error(
                            -32602,
                            "Unsupported config option",
                            Some(json!({ "configId": config_id })),
                        )),
                    )
                    .await?;
                    return Ok(());
                }
                let session_id = match session_id_from_config_params(&request.params) {
                    Ok(session_id) => session_id,
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32602,
                                "Invalid session config params",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                        return Ok(());
                    }
                };
                if config_id == "model" {
                    let requested_model = match config_value_from_params(&request.params) {
                        Ok(value) => value,
                        Err(err) => {
                            write_response(
                                id,
                                Err(json_rpc_error(
                                    -32602,
                                    "Invalid session config params",
                                    Some(json!({ "message": format!("{err:#}") })),
                                )),
                            )
                            .await?;
                            return Ok(());
                        }
                    };
                    let config = match runtime.config.as_ref() {
                        Some(config) => config,
                        None => {
                            write_response(
                                id,
                                Err(json_rpc_error(-32002, "Den is not configured", None)),
                            )
                            .await?;
                            return Ok(());
                        }
                    };
                    let selection_mode = if requested_model == "auto" {
                        "auto"
                    } else {
                        "explicit"
                    };
                    let den_response = bearwire::rpc_call(
                        http,
                        config,
                        "session.model.set",
                        json!({
                            "bear_slug": config.bear,
                            "session_id": session_id,
                            "selection_mode": selection_mode,
                            "model": if requested_model == "auto" { Value::Null } else { json!(requested_model) },
                        }),
                    )
                    .await?;
                    remember_session_model(
                        shared_state,
                        adapter_state,
                        session_id,
                        den_response.clone(),
                    )
                    .await;
                    notify_config_options_for_session(shared_state, session_id).await?;
                    let context = shared_state
                        .session_contexts
                        .lock()
                        .await
                        .get(session_id)
                        .cloned();
                    let mode = context
                        .as_ref()
                        .and_then(|ctx| ctx.current_mode.as_deref())
                        .unwrap_or(MODE_ASK);
                    write_response(
                        id,
                        Ok(json!({
                            "configOptions": session_config_options_for_context(context.as_ref(), mode),
                            "_meta": { "bears": { "model": den_response } }
                        })),
                    )
                    .await?;
                    return Ok(());
                }

                let requested_mode = match mode_value_from_config_params(&request.params) {
                    Ok(MODE_ASK | MODE_PLAN | MODE_WRITE) => {
                        mode_value_from_config_params(&request.params)?
                    }
                    Ok(other) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32602,
                                "Unsupported mode",
                                Some(json!({ "mode": other, "supported": [MODE_ASK, MODE_PLAN, MODE_WRITE] })),
                            )),
                        )
                        .await?;
                        return Ok(());
                    }
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32602,
                                "Invalid session config params",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                        return Ok(());
                    }
                };
                let (mode, den_response) = request_den_session_mode(
                    http,
                    runtime.config.as_ref(),
                    session_id,
                    requested_mode,
                )
                .await?;
                let pending_den_sync = den_response
                    .get("deferred")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                remember_session_mode(
                    shared_state,
                    adapter_state,
                    session_id,
                    mode,
                    if pending_den_sync {
                        "adapter.pending_den_session_mode"
                    } else {
                        "den.session_policy"
                    },
                    pending_den_sync,
                )
                .await;
                if bear_debug_verbose() {
                    eprintln!(
                        "bear-armature: session/set_config_option mode request session_id={} requested_mode={} effective_mode={} den_message={}",
                        session_id,
                        requested_mode,
                        mode,
                        den_response
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("<none>")
                    );
                }
                if requested_mode != mode {
                    eprintln!(
                        "bear-armature: Den adjusted client-requested mode={} for session_id={} to effective mode={}",
                        requested_mode, session_id, mode
                    );
                    let deferred = den_response
                        .get("deferred")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !deferred {
                        eprintln!(
                            "bear-armature: mode request adjusted session_id={} requested_mode={} effective_mode={} message={}",
                            session_id,
                            requested_mode,
                            mode,
                            den_response
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Den session policy adjusted the requested mode.")
                        );
                    }
                }
                notify_mode_state(session_id, mode).await?;
                send_available_commands_update(session_id).await?;
                write_response(
                    id,
                    Ok(json!({
                        "configOptions": session_config_options_for_mode(mode),
                        "_meta": {
                            "bears": {
                                "requestedMode": requested_mode,
                                "effectiveMode": mode,
                                "source": "den.session_policy",
                                "denResponse": den_response
                            }
                        }
                    })),
                )
                .await?;
            }
        }
        "session/set_mode" => {
            if let Some(id) = request.id.clone() {
                let session_id = request
                    .params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let mode = request
                    .params
                    .get("modeId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !matches!(mode, MODE_ASK | MODE_PLAN | MODE_WRITE) || session_id.is_empty() {
                    write_response(
                        id,
                        Err(json_rpc_error(
                            -32602,
                            "Invalid session mode params",
                            Some(json!({ "mode": mode, "supported": [MODE_ASK, MODE_PLAN, MODE_WRITE] })),
                        )),
                    )
                    .await?;
                    return Ok(());
                }
                let requested_mode = mode;
                let (mode, den_response) = request_den_session_mode(
                    http,
                    runtime.config.as_ref(),
                    session_id,
                    requested_mode,
                )
                .await?;
                let pending_den_sync = den_response
                    .get("deferred")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                remember_session_mode(
                    shared_state,
                    adapter_state,
                    session_id,
                    mode,
                    if pending_den_sync {
                        "adapter.pending_den_session_mode"
                    } else {
                        "den.session_policy"
                    },
                    pending_den_sync,
                )
                .await;
                if bear_debug_verbose() {
                    eprintln!(
                        "bear-armature: session/set_mode request session_id={} requested_mode={} effective_mode={} den_message={}",
                        session_id,
                        requested_mode,
                        mode,
                        den_response
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("<none>")
                    );
                }
                if requested_mode != mode {
                    eprintln!(
                        "bear-armature: Den adjusted client-requested legacy mode={} for session_id={} to effective mode={}",
                        requested_mode, session_id, mode
                    );
                    let deferred = den_response
                        .get("deferred")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !deferred {
                        eprintln!(
                            "bear-armature: mode request adjusted session_id={} requested_mode={} effective_mode={} message={}",
                            session_id,
                            requested_mode,
                            mode,
                            den_response
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Den session policy adjusted the requested mode.")
                        );
                    }
                }
                notify_mode_state(session_id, mode).await?;
                send_available_commands_update(session_id).await?;
                write_response(
                    id,
                    Ok(json!({
                        "modes": session_modes_for_mode(mode),
                        "_meta": {
                            "bears": {
                                "requestedMode": requested_mode,
                                "effectiveMode": mode,
                                "source": "den.session_policy",
                                "denResponse": den_response
                            }
                        }
                    })),
                )
                .await?;
            }
        }
        "session/list" => {
            if let Some(id) = request.id.clone() {
                let Some(config) = runtime.config.as_ref() else {
                    write_response(
                        id,
                        Err(configuration_error(Some(json!({
                            "message": runtime.configuration_error_message(),
                            "problems": runtime.diagnostics,
                        })))),
                    )
                    .await?;
                    return Ok(());
                };
                if let Err(err) = validate_den_code_token(http, config).await {
                    refresh_slash_commands_for_all_sessions(shared_state).await;
                    write_response(
                        id,
                        Err(auth_check_json_rpc_error(
                            &err,
                            Some("Generate a fresh Den Code token for this bear."),
                        )),
                    )
                    .await?;
                    return Ok(());
                }
                if let Err(err) = validate_optional_cwd_filter(&request.params) {
                    write_response(
                        id,
                        Err(json_rpc_error(
                            -32602,
                            "Invalid session/list params",
                            Some(json!({ "message": format!("{err:#}") })),
                        )),
                    )
                    .await?;
                    return Ok(());
                }
                match den_list_acp_sessions(http, config, &request.params).await {
                    Ok(den) => {
                        let mapped = map_den_sessions_list_to_acp(&den)?;
                        write_response(id, Ok(mapped)).await?;
                    }
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32003,
                                "BEARS session list failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                    }
                }
            }
        }
        "session/resume" => {
            if let Some(id) = request.id.clone() {
                let Some(config) = runtime.config.as_ref() else {
                    write_response(
                        id,
                        Err(configuration_error(Some(json!({
                            "message": runtime.configuration_error_message(),
                            "problems": runtime.diagnostics,
                        })))),
                    )
                    .await?;
                    return Ok(());
                };
                match restore_session_from_den(
                    http,
                    config,
                    adapter_state,
                    shared_state,
                    &request.params,
                )
                .await
                {
                    Ok((mode, context_budget, _den)) => {
                        let response = ResumeSessionResponse::new()
                            .config_options(session_config_options_for_mode(mode))
                            .modes(session_modes_for_mode(mode));
                        write_response(id, Ok(serde_json::to_value(response)?)).await?;
                        let session_id = request
                            .params
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if !session_id.is_empty() {
                            send_available_commands_update(session_id).await?;
                            if let Some(context_budget) = context_budget {
                                send_context_budget_usage_update(session_id, context_budget)
                                    .await?;
                            }
                        }
                    }
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32003,
                                "BEARS session resume failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                    }
                }
            }
        }
        "session/load" => {
            if let Some(id) = request.id.clone() {
                let Some(config) = runtime.config.as_ref() else {
                    write_response(
                        id,
                        Err(configuration_error(Some(json!({
                            "message": runtime.configuration_error_message(),
                            "problems": runtime.diagnostics,
                        })))),
                    )
                    .await?;
                    return Ok(());
                };
                match handle_session_load(
                    http,
                    config,
                    adapter_state,
                    shared_state,
                    id.clone(),
                    &request.params,
                )
                .await
                {
                    Ok(()) => {}
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32003,
                                "BEARS session load failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                    }
                }
            }
        }
        "session/prompt" => {
            if let Some(id) = request.id {
                if let Some(command) = prompt_text_from_params(&request.params)
                    .ok()
                    .and_then(|prompt| parse_local_slash_command(&prompt))
                {
                    let http = http.clone();
                    let config = runtime.config.clone();
                    let shared_state = shared_state.clone();
                    let mut prompt_state = AdapterState {
                        client_capabilities: shared_state.client_capabilities.lock().await.clone(),
                        session_contexts: shared_state.session_contexts.lock().await.clone(),
                        transport: shared_state.transport.clone(),
                    };
                    tokio::spawn(async move {
                        if let Err(err) = handle_local_slash_prompt(
                            Some(&http),
                            config.as_ref(),
                            &mut prompt_state,
                            &shared_state,
                            id.clone(),
                            request.params,
                            command,
                        )
                        .await
                        {
                            let _ = write_response(
                                id,
                                Err(json_rpc_error(
                                    -32003,
                                    "BEARS local slash command failed",
                                    Some(json!({ "message": format!("{err:#}") })),
                                )),
                            )
                            .await;
                        }
                    });
                    return Ok(());
                }

                let Some(config) = runtime.config.as_ref() else {
                    write_response(
                        id,
                        Err(configuration_error(Some(json!({
                            "message": runtime.configuration_error_message(),
                            "problems": runtime.diagnostics,
                        })))),
                    )
                    .await?;
                    return Ok(());
                };

                if let Err(err) = validate_den_code_token(http, config).await {
                    if let Some(session_id) =
                        request.params.get("sessionId").and_then(Value::as_str)
                    {
                        refresh_slash_commands_for_session(session_id).await;
                    } else {
                        refresh_slash_commands_for_all_sessions(shared_state).await;
                    }
                    write_response(
                        id,
                        Err(auth_check_json_rpc_error(
                            &err,
                            Some("Generate a fresh Den armature token for this bear. Tokens must include armature:chat."),
                        )),
                    )
                    .await?;
                    return Ok(());
                }

                let session_id = match request.params.get("sessionId").and_then(Value::as_str) {
                    Some(value) if !value.trim().is_empty() => value.trim().to_string(),
                    _ => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32602,
                                "Invalid session/prompt params",
                                Some(
                                    json!({ "message": "session/prompt params missing sessionId" }),
                                ),
                            )),
                        )
                        .await?;
                        return Ok(());
                    }
                };
                let turn_token = Uuid::new_v4();
                let conversation_id_for_turn = prompt_conversation_id_from_params(&request.params);
                let response = PromptResponseGuard::new(id.clone());
                let previous = register_prompt_turn_for_session(
                    shared_state,
                    &session_id,
                    turn_token,
                    conversation_id_for_turn.clone(),
                    response.clone(),
                )
                .await;
                if let Some(previous) = previous {
                    if let Some(previous_id) = previous.response.claim() {
                        write_prompt_end_turn_response(previous_id).await?;
                    }
                    let same_conversation = prompt_conversations_overlap(
                        previous.conversation_id.as_deref(),
                        conversation_id_for_turn.as_deref(),
                    );
                    if same_conversation {
                        eprintln!(
                            "bear-armature: steering prompt for same conversation session_id={} previous_turn={} new_turn={} conversation={:?}; cancelling previous turn and gating stale UI text updates",
                            session_id, previous.token, turn_token, conversation_id_for_turn
                        );
                    } else {
                        eprintln!(
                            "bear-armature: overlapping prompt for different conversation session_id={} previous_turn={} new_turn={} previous_conversation={:?} new_conversation={:?}; keeping previous runtime alive and gating stale UI updates",
                            session_id,
                            previous.token,
                            turn_token,
                            previous.conversation_id,
                            conversation_id_for_turn
                        );
                    }
                }

                let http = http.clone();
                let config = config.clone();
                let shared_state = shared_state.clone();
                let mut prompt_state = AdapterState {
                    client_capabilities: shared_state.client_capabilities.lock().await.clone(),
                    session_contexts: shared_state.session_contexts.lock().await.clone(),
                    transport: shared_state.transport.clone(),
                };
                tokio::spawn(async move {
                    match handle_prompt(
                        &http,
                        &config,
                        &mut prompt_state,
                        &shared_state,
                        response.clone(),
                        request.params,
                        turn_token,
                    )
                    .await
                    {
                        Ok(()) => {}
                        Err(err) => {
                            let error_chain = format!("{err:#}");
                            let (failure_kind, user_message) =
                                classify_prompt_failure(&error_chain);
                            let server_version = fetch_server_version(&http, &config).await.ok();
                            tracing::error!(
                                session_id,
                                turn_token = %turn_token,
                                conversation_id = ?conversation_id_for_turn,
                                failure_kind,
                                error = %error_chain,
                                user_message,
                                server_version = ?server_version.as_ref().map(ServerVersion::summary),
                                armature_version = adapter_version(),
                                "session/prompt failed"
                            );
                            eprintln!(
                                "bear-armature: session/prompt failed session_id={} turn_token={} failure_kind={} error={}",
                                session_id, turn_token, failure_kind, error_chain
                            );
                            if let Some(response_id) = response.claim() {
                                if let Err(write_err) = write_response(
                                    response_id,
                                    Err(json_rpc_error(-32003, &user_message, None)),
                                )
                                .await
                                {
                                    tracing::error!(
                                        session_id,
                                        turn_token = %turn_token,
                                        conversation_id = ?conversation_id_for_turn,
                                        error = %format!("{write_err:#}"),
                                        "failed to write terminal session/prompt error response"
                                    );
                                } else {
                                    tracing::debug!(
                                        session_id,
                                        turn_token = %turn_token,
                                        conversation_id = ?conversation_id_for_turn,
                                        "terminal session/prompt error response written"
                                    );
                                }
                            }
                        }
                    }
                });
            }
        }
        "session/close" => {
            let id = request.id;
            let Some(config) = runtime.config.as_ref() else {
                if let Some(id) = id {
                    write_response(
                        id,
                        Err(configuration_error(Some(json!({
                            "message": runtime.configuration_error_message(),
                            "problems": runtime.diagnostics,
                        })))),
                    )
                    .await?;
                } else {
                    eprintln!(
                        "bear-armature: ignoring session/close notification because adapter is not configured"
                    );
                }
                return Ok(());
            };
            match handle_session_close(http, config, shared_state, request.params).await {
                Ok(()) => {
                    if let Some(id) = id {
                        write_response(id, Ok(serde_json::to_value(CloseSessionResponse::new())?))
                            .await?;
                    }
                }
                Err(err) => {
                    if let Some(id) = id {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32003,
                                "Den session close failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                    } else {
                        eprintln!("bear-armature: session/close notification failed error={err:#}");
                    }
                }
            }
        }
        "session/cancel" => {
            let id = request.id;
            let Some(config) = runtime.config.as_ref() else {
                if let Some(id) = id {
                    write_response(
                        id,
                        Err(configuration_error(Some(json!({
                            "message": runtime.configuration_error_message(),
                            "problems": runtime.diagnostics,
                        })))),
                    )
                    .await?;
                } else {
                    eprintln!(
                        "bear-armature: ignoring session/cancel notification because adapter is not configured"
                    );
                }
                return Ok(());
            };
            match handle_session_cancel(http, config, shared_state, request.params).await {
                Ok(()) => {
                    if let Some(id) = id {
                        write_response(id, Ok(serde_json::to_value(CloseSessionResponse::new())?))
                            .await?;
                    }
                }
                Err(err) => {
                    if let Some(id) = id {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32003,
                                "BEARS session cancel failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                    } else {
                        eprintln!(
                            "bear-armature: session/cancel notification failed error={err:#}"
                        );
                    }
                }
            }
        }
        _ => {
            if let Some(id) = request.id {
                write_response(
                    id,
                    Err(json_rpc_error(
                        -32601,
                        "Method not found",
                        Some(json!({ "method": request.method })),
                    )),
                )
                .await?;
            }
        }
    }
    Ok(())
}

fn adapter_contract_context() -> Value {
    json!({
        "name": DEN_ACP_ADAPTER_CONTRACT_NAME,
        "version": DEN_ACP_ADAPTER_CONTRACT_VERSION,
    })
}

fn adapter_capabilities_context() -> Value {
    adapter_capabilities_context_with_client_mcp(false)
}

fn adapter_capabilities_context_with_client_mcp(has_client_mcp_tools: bool) -> Value {
    let chrome_supported = chrome_tools_available() && !has_client_mcp_tools;
    json!({
        "name": "bear-armature",
        "version": adapter_version(),
        "git_sha": env!("DEN_ACP_ADAPTER_GIT_SHA"),
        "built_at_utc": env!("DEN_ACP_ADAPTER_BUILT_AT_UTC"),
        "api_contract": adapter_contract_context(),
        "direct_tools": {
            "fs_read_text_file": direct_tool_descriptor(true, "Read a text file from the workspace.", &["cat", "head", "tail", "sed -n"]),
            "fs_list_directory": direct_tool_descriptor(true, "List files and directories under a workspace path.", &["ls"]),
            "fs_find_paths": direct_tool_descriptor(true, "Find workspace paths by glob pattern.", &["find"]),
            "fs_search_files": direct_tool_descriptor(true, "Search workspace files by text query or pattern. Prefer this over shell search commands for repo text search.", &["rg", "grep"]),
            "fs_stat": direct_tool_descriptor(true, "Inspect a workspace path's type, size, and metadata.", &["stat"]),
            "git_status": direct_tool_descriptor(true, "Show repository status.", &["git status"]),
            "git_diff": direct_tool_descriptor(true, "Show repository diff.", &["git diff"]),
            "git_log": direct_tool_descriptor(true, "Show repository history.", &["git log"]),
            "git_show": direct_tool_descriptor(true, "Show a git object or revision.", &["git show"]),
            "git_add": direct_tool_descriptor(true, "Stage tracked changes in a repository.", &["git add"]),
            "git_restore": direct_tool_descriptor(true, "Restore files in a repository.", &["git restore"]),
            "git_commit": direct_tool_descriptor(true, "Create a git commit.", &["git commit"]),
            "git_stash": direct_tool_descriptor(true, "Stash repository changes.", &["git stash"]),
            "run_command": direct_tool_descriptor(true, "Run a local command in terminal view by default when the ACP client supports terminals; use process_run for short captured output.", &[]),
            "process_run": direct_tool_descriptor(true, "Run a short, bounded local command and capture its result.", &[]),
            "terminal_run_command": direct_tool_descriptor(true, "Run a local terminal command with live output for interactive or long-running work.", &[]),
            "bear_environment": direct_tool_descriptor(true, "Inspect BEARS adapter and environment diagnostics.", &[]),
            "chrome_open": direct_tool_descriptor(chrome_supported, "Open a URL in Chrome.", &[]),
            "chrome_snapshot": direct_tool_descriptor(chrome_supported, "Capture a Chrome page snapshot.", &[]),
            "chrome_console_messages": direct_tool_descriptor(chrome_supported, "Read Chrome console messages.", &[]),
            "chrome_network_requests": direct_tool_descriptor(chrome_supported, "Read Chrome network requests.", &[]),
            "chrome_screenshot": direct_tool_descriptor(chrome_supported, "Capture a Chrome screenshot.", &[]),
            "fs_edit_file": direct_tool_descriptor(true, "Edit a workspace file directly.", &[]),
            "fs_replace_text": direct_tool_descriptor(true, "Replace exact text in a workspace file. Prefer this over `sed` when the edit is a targeted replacement.", &["sed"]),
            "fs_create_text_file": direct_tool_descriptor(true, "Create a new workspace text file.", &[]),
            "fs_create_directory": direct_tool_descriptor(true, "Create a new workspace directory.", &[]),
            "fs_move_path": direct_tool_descriptor(true, "Move or rename a workspace path.", &[]),
            "fs_copy_path": direct_tool_descriptor(true, "Copy a workspace path.", &[]),
            "fs_apply_patch": direct_tool_descriptor(true, "Apply a patch to workspace files.", &[]),
            "fs_delete_path": direct_tool_descriptor(true, "Delete a workspace path.", &[])
        }
    })
}

fn direct_tools_context() -> Value {
    direct_tools_context_with_client_mcp(false)
}

fn direct_tool_descriptor(
    supported: bool,
    description: &'static str,
    prefer_instead_of_shell: &[&'static str],
) -> Value {
    json!({
        "supported": supported,
        "description": description,
        "prefer_instead_of_shell": prefer_instead_of_shell,
    })
}

fn direct_tools_context_with_client_mcp(has_client_mcp_tools: bool) -> Value {
    let chrome_available = chrome_tools_available() && !has_client_mcp_tools;
    json!({
        "fs_read_text_file": direct_tool_descriptor(true, "Read a text file from the workspace.", &["cat", "head", "tail", "sed -n"]),
        "fs_list_directory": direct_tool_descriptor(true, "List files and directories under a workspace path.", &["ls"]),
        "fs_find_paths": direct_tool_descriptor(true, "Find workspace paths by glob pattern.", &["find"]),
        "fs_search_files": direct_tool_descriptor(true, "Search workspace files by text query or pattern. Prefer this over shell search commands for repo text search.", &["rg", "grep"]),
        "fs_stat": direct_tool_descriptor(true, "Inspect a workspace path's type, size, and metadata.", &["stat"]),
        "git_status": direct_tool_descriptor(true, "Show repository status.", &["git status"]),
        "git_diff": direct_tool_descriptor(true, "Show repository diff.", &["git diff"]),
        "git_log": direct_tool_descriptor(true, "Show repository history.", &["git log"]),
        "git_show": direct_tool_descriptor(true, "Show a git object or revision.", &["git show"]),
        "git_add": direct_tool_descriptor(true, "Stage tracked changes in a repository.", &["git add"]),
        "git_restore": direct_tool_descriptor(true, "Restore files in a repository.", &["git restore"]),
        "git_commit": direct_tool_descriptor(true, "Create a git commit.", &["git commit"]),
        "git_stash": direct_tool_descriptor(true, "Stash repository changes.", &["git stash"]),
        "run_command": direct_tool_descriptor(true, "Run a local command in terminal view by default when the ACP client supports terminals; use process_run for short captured output.", &[]),
        "process_run": direct_tool_descriptor(true, "Run a short, bounded local command and capture its result.", &[]),
        "terminal_run_command": direct_tool_descriptor(true, "Run a local terminal command with live output for interactive or long-running work.", &[]),
        "bear_environment": direct_tool_descriptor(true, "Inspect BEARS adapter and environment diagnostics.", &[]),
        "chrome_open": direct_tool_descriptor(chrome_available, "Open a URL in Chrome.", &[]),
        "chrome_snapshot": direct_tool_descriptor(chrome_available, "Capture a Chrome page snapshot.", &[]),
        "chrome_console_messages": direct_tool_descriptor(chrome_available, "Read Chrome console messages.", &[]),
        "chrome_network_requests": direct_tool_descriptor(chrome_available, "Read Chrome network requests.", &[]),
        "chrome_screenshot": direct_tool_descriptor(chrome_available, "Capture a Chrome screenshot.", &[]),
        "client_mcp_tools_present": has_client_mcp_tools,
        "chrome_tools_disabled_reason": if has_client_mcp_tools { "external_browser_mcp_tools_present" } else { "" },
        "fs_edit_file": direct_tool_descriptor(true, "Edit a workspace file directly.", &[]),
        "fs_replace_text": direct_tool_descriptor(true, "Replace exact text in a workspace file. Prefer this over `sed` when the edit is a targeted replacement.", &["sed"]),
        "fs_create_text_file": direct_tool_descriptor(true, "Create a new workspace text file.", &[]),
        "fs_create_directory": direct_tool_descriptor(true, "Create a new workspace directory.", &[]),
        "fs_move_path": direct_tool_descriptor(true, "Move or rename a workspace path.", &[]),
        "fs_copy_path": direct_tool_descriptor(true, "Copy a workspace path.", &[]),
        "fs_apply_patch": direct_tool_descriptor(true, "Apply a patch to workspace files.", &[]),
        "fs_delete_path": direct_tool_descriptor(true, "Delete a workspace path.", &[]),
    })
}

fn workspace_git_remote_origins(roots: &[String]) -> Vec<String> {
    let mut origins = roots
        .iter()
        .filter_map(|root| {
            Command::new("git")
                .args(["config", "--get", "remote.origin.url"])
                .current_dir(root)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|origin| origin.trim().to_string())
                .filter(|origin| !origin.is_empty())
        })
        .collect::<Vec<_>>();
    origins.sort();
    origins.dedup();
    origins
}

fn ensure_session_context_capabilities(context: &mut SessionContext) {
    if !context.raw.is_object() {
        context.raw = json!({});
    }
    let (has_client_mcp_tools, has_host_browser_bridge_tools) = context
        .raw
        .pointer("/mcp/client_tools")
        .and_then(Value::as_array)
        .map(|tools| {
            let has_client = tools.iter().any(|tool| {
                tool.pointer("/x_bears/source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| source == "client_forwarded")
            });
            let has_host_bridge = tools.iter().any(|tool| {
                tool.pointer("/x_bears/source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| source == "host_browser_bridge")
            });
            (has_client, has_host_bridge)
        })
        .unwrap_or((false, false));
    context.raw["adapter_version"] = json!(adapter_version());
    context.raw["adapter"] = adapter_capabilities_context_with_client_mcp(has_client_mcp_tools);
    context.raw["direct_tools"] =
        direct_tools_context_with_client_mcp(has_client_mcp_tools || has_host_browser_bridge_tools);
    let mode = context
        .current_mode
        .as_deref()
        .map(normalize_mode)
        .unwrap_or(MODE_ASK);
    context.current_mode = Some(mode.to_string());
    if context.raw.get("session_mode").is_none() {
        context.raw["session_mode"] = json!({
            "requested_mode": mode,
            "effective_mode": mode,
            "source": "adapter.session_context",
            "pending_den_sync": false,
        });
    }
    if !context.cwd.trim().is_empty() {
        context.raw["cwd"] = json!(context.cwd.clone());
    }
    if !context.roots.is_empty() {
        context.raw["workspace_roots"] = json!(context.roots.clone());
        context.raw["git_remote_origins"] = json!(workspace_git_remote_origins(&context.roots));
    }
}

fn run_command_prefers_terminal(args: &Value) -> bool {
    let command = args.get("command").and_then(Value::as_str).map(str::trim);
    let Some(_command) = command.filter(|value| !value.is_empty()) else {
        return false;
    };
    // ponytail: terminal support is the coarse gate; keep process_run as the explicit
    // simple-process escape hatch instead of maintaining a second command classifier.
    true
}

fn session_context_from_params(params: &Value) -> Result<SessionContext> {
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: session_context_from_params mcp_summary={}",
            summarize_acp_mcp_servers_param(params)
        );
    }
    let mut mcp_sources = parse_acp_mcp_servers(params)?;
    if let Some(host_browser_bridge) = host_browser_bridge_config_from_env() {
        mcp_sources.push(host_browser_bridge);
    }
    let roots = workspace_roots_from_params(params);
    let cwd = explicit_cwd_from_params(params)
        .transpose()?
        .or_else(|| fallback_cwd_from_params(params))
        .or_else(|| roots.first().cloned())
        .ok_or_else(|| {
            anyhow!(
                "ACP session requires an absolute cwd; provide params.cwd as an absolute local path"
            )
        })?;
    if !is_absolute_local_path(&cwd) {
        return Err(anyhow!(
            "ACP session cwd must be an absolute local path; got {cwd:?}"
        ));
    }
    let roots = roots_or_cwd(roots, &cwd);
    let raw = json!({
        "cwd": cwd,
        "workspace_roots": roots,
        "adapter_version": adapter_version(),
        "adapter": adapter_capabilities_context(),
        "direct_tools": direct_tools_context(),
        "mcp_servers": mcp_sources
            .iter()
            .map(McpSourceConfig::safe_summary_for_session_context)
            .collect::<Vec<_>>(),
        "host_browser_bridge": host_browser_bridge_env_summary(),
    });
    let mut context = SessionContext {
        cwd,
        roots,
        raw,
        mcp_sources,
        conversation_id: None,
        resolved_conversation_id: None,
        thread_title: None,
        current_mode: Some(MODE_ASK.to_string()),
    };
    set_context_mode(
        &mut context,
        MODE_ASK,
        "adapter.session_context_default",
        false,
    );
    ensure_session_context_capabilities(&mut context);
    Ok(context)
}

fn explicit_cwd_from_params(params: &Value) -> Option<Result<String>> {
    params.get("cwd").and_then(Value::as_str).map(|raw| {
        let path =
            file_uri_or_path_to_path(raw).ok_or_else(|| anyhow!("params.cwd must not be empty"))?;
        if is_absolute_local_path(&path) {
            Ok(path)
        } else {
            Err(anyhow!(
                "params.cwd must be an absolute local path; got {path:?}"
            ))
        }
    })
}

fn fallback_cwd_from_params(params: &Value) -> Option<String> {
    [
        params.get("workspaceUri"),
        params.pointer("/workspace/currentDirectory"),
        params.pointer("/workspace/cwd"),
        params.pointer("/workspace/root"),
        params.pointer("/workspace/folders/0/path"),
        params.pointer("/workspace/folders/0/uri"),
        params.pointer("/workspaceFolders/0/path"),
        params.pointer("/workspaceFolders/0/uri"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .filter_map(file_uri_or_path_to_path)
    .find(|path| is_absolute_local_path(path))
}

fn validate_optional_cwd_filter(params: &Value) -> Result<()> {
    let Some(cwd) = params
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    let path = file_uri_or_path_to_path(cwd)
        .ok_or_else(|| anyhow!("session/list cwd filter must not be empty"))?;
    if is_absolute_local_path(&path) {
        Ok(())
    } else {
        Err(anyhow!(
            "session/list cwd filter must be an absolute local path; got {path:?}"
        ))
    }
}

async fn handle_authenticate(
    http: &reqwest::Client,
    runtime: &mut RuntimeConfig,
    params: Value,
) -> Result<()> {
    let method_id = params
        .get("methodId")
        .and_then(Value::as_str)
        .unwrap_or("DEN_TOKEN");
    if method_id != "DEN_TOKEN" {
        return Err(anyhow!("unsupported BEARS auth method: {method_id}"));
    }
    let config = runtime_config_from_current_env(runtime)?;
    match validate_den_code_token(http, &config).await {
        Ok(()) => {
            runtime.config = Some(config);
            runtime.diagnostics.clear();
            Ok(())
        }
        Err(err) if looks_like_den_connectivity_error(&err) => {
            runtime.config = Some(config);
            runtime.diagnostics = vec![format!(
                "Den is unreachable ({err:#}). Adapter-local slash commands such as /doctor are still available; normal prompts will fail until connectivity is restored."
            )];
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn runtime_config_from_current_env(runtime: &RuntimeConfig) -> Result<Config> {
    let mut token = env::var("DEN_TOKEN").unwrap_or_default();
    let token_env = runtime.token_env.trim();
    if !token_env.is_empty() {
        token = env::var(token_env).with_context(|| {
            format!(
                "DEN_TOKEN_ENV points at {token_env:?}, but that environment variable is not set"
            )
        })?;
    }
    let api_url = runtime.api_url.trim().trim_end_matches('/').to_string();
    let bear = runtime.bear.trim().to_string();
    let token = token.trim().to_string();
    if api_url.is_empty() {
        return Err(anyhow!(
            "Missing DEN_API_URL / --api-url for BEARS authentication"
        ));
    }
    if bear.is_empty() {
        return Err(anyhow!(
            "Missing BEAR_SLUG / --bear for BEARS authentication"
        ));
    }
    if token.is_empty() {
        return Err(anyhow!(
            "Missing DEN_TOKEN. Paste a Den Code token when prompted, or configure DEN_TOKEN in Zed."
        ));
    }
    Ok(Config {
        api_url,
        bear,
        token,
        client: runtime.client.clone(),
    })
}

async fn validate_den_code_token(http: &reqwest::Client, config: &Config) -> Result<()> {
    if bearwire::enabled() {
        return bearwire::validate_code_token(http, config).await;
    }
    Err(anyhow!(
        "BearWire is disabled in this adapter process; legacy /acp auth-check is retired. Enable BearWire by setting BEARS_BEARWIRE=auto or true."
    ))
}

async fn validate_den_code_token_for_diagnostics(
    http: &reqwest::Client,
    config: &Config,
) -> Result<()> {
    match timeout(
        LOCAL_DEN_INSPECTION_TIMEOUT,
        validate_den_code_token(http, config),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(anyhow!(
            "timed out after {}ms validating BEARS Code token with Den",
            LOCAL_DEN_INSPECTION_TIMEOUT.as_millis()
        )),
    }
}

fn client_supports_read_text_file(adapter_state: &AdapterState) -> bool {
    adapter_state
        .client_capabilities
        .pointer("/fs/readTextFile")
        .and_then(Value::as_bool)
        == Some(true)
}

fn client_supports_terminal(adapter_state: &AdapterState) -> bool {
    adapter_state
        .client_capabilities
        .get("terminal")
        .map(capability_value_bool)
        .unwrap_or(false)
}

fn client_read_text_file_request_path(context: &SessionContext, raw_path: &str) -> Result<PathBuf> {
    let resolved_path = resolve_requested_tool_path(context, raw_path)?;
    ensure_path_allowed_for_session(context, &resolved_path)?;
    Ok(resolved_path)
}

fn read_text_file_requires_client_surface(args: &Value) -> bool {
    args.get("source")
        .or_else(|| args.pointer("/_meta/source"))
        .and_then(Value::as_str)
        .is_some_and(|source| matches!(source, "editor_buffer" | "client_surface"))
        || args
            .get("prefer_client")
            .or_else(|| args.pointer("/_meta/prefer_client"))
            .and_then(Value::as_bool)
            == Some(true)
}

async fn handle_client_read_text_file(
    adapter_state: &mut AdapterState,
    session_id: &str,
    args: &Value,
) -> Result<Value> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_read_text_file args missing path"))?;
    let context = session_context(adapter_state, session_id)?;
    let resolved_path = client_read_text_file_request_path(context, path)?;
    preflight_client_read_text_file_target(&resolved_path).await?;
    let mut request = ReadTextFileRequest::new(session_id.to_string(), resolved_path.clone());
    if let Some(line) = args.get("line").and_then(Value::as_u64) {
        request = request.line(Some(line.clamp(1, u32::MAX as u64) as u32));
    }
    if let Some(limit) = args.get("limit").and_then(Value::as_u64) {
        request = request.limit(Some(limit.clamp(1, u32::MAX as u64) as u32));
    }
    let params = serde_json::to_value(request)?;
    let started = std::time::Instant::now();
    let response = adapter_state
        .transport
        .request(
            "fs/read_text_file",
            params,
            std::time::Duration::from_secs(30),
        )
        .await?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("client fs/read_text_file failed: {error}"));
    }
    let result = response.get("result").cloned().unwrap_or(Value::Null);
    let parsed = serde_json::from_value::<ReadTextFileResponse>(result.clone()).map_err(|err| {
        anyhow!(
            "client fs/read_text_file response did not match ACP schema: {err}; result={}",
            truncate_for_log(&result.to_string(), 240)
        )
    })?;
    let content = parsed.content;
    verify_client_read_text_file_response(&resolved_path, &content).await?;
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: client fs/read_text_file requested_path={} resolved_path={} bytes={} duration_ms={}",
            path,
            resolved_path.display(),
            content.len(),
            started.elapsed().as_millis(),
        );
    }
    Ok(json!({
        "ok": true,
        "path": path,
        "content": content,
        "source": "acp_client",
        "raw_result": result,
        "bytes": content.len(),
    }))
}

async fn preflight_client_read_text_file_target(
    path: &std::path::Path,
) -> Result<std::fs::Metadata> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => Ok(metadata),
        Ok(_) => bail!(
            "client fs/read_text_file target is not a file before ACP request: {}",
            path.display()
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => bail!(
            "client fs/read_text_file target does not exist before ACP request: {}",
            path.display()
        ),
        Err(err) => Err(err).with_context(|| {
            format!(
                "client fs/read_text_file local metadata preflight failed for {}",
                path.display()
            )
        }),
    }
}

async fn verify_client_read_text_file_response(
    path: &std::path::Path,
    content: &str,
) -> Result<()> {
    let metadata = preflight_client_read_text_file_target(path)
        .await
        .with_context(|| {
            format!(
                "client fs/read_text_file local verification failed for {}",
                path.display()
            )
        })?;
    if content.is_empty() && metadata.len() > 0 {
        bail!(
            "client fs/read_text_file returned empty content for non-empty file: {} ({} bytes)",
            path.display(),
            metadata.len()
        );
    }
    Ok(())
}

async fn handle_direct_read_text_file(
    adapter_state: &AdapterState,
    params: Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("bears/read_text_file params missing sessionId"))?
        .to_string();
    let context = session_context(adapter_state, &session_id)?;
    let mut args = params;
    if let Some(object) = args.as_object_mut() {
        object.remove("sessionId");
    }
    handle_read_text_file(context, &session_id, args, policy).await
}

fn policy_from_event(event: &Value) -> ToolPolicy {
    let policy = event
        .get("policy")
        .or_else(|| event.pointer("/data/tool_call/policy"))
        .or_else(|| event.pointer("/data/policy"))
        .unwrap_or(&Value::Null);
    ToolPolicy {
        max_lines: policy
            .get("max_lines")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(1, 2_000) as usize),
        max_entries: policy
            .get("max_entries")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(1, 1_000) as usize),
        max_results: policy
            .get("max_results")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(1, 200) as usize),
        max_bytes: policy
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(1, 5_242_880)),
        recursive_default: policy.get("recursive_default").and_then(Value::as_bool),
        include_hidden_default: policy
            .get("include_hidden_default")
            .and_then(Value::as_bool),
        execution_target: policy
            .get("execution_target")
            .and_then(Value::as_str)
            .map(str::to_string),
        approval_policy: policy
            .get("approval_policy")
            .and_then(Value::as_str)
            .map(str::to_string),
        sensitive_path_policy: policy
            .get("sensitive_path_policy")
            .and_then(Value::as_str)
            .map(str::to_string),
        target_policy: policy.get("target_policy").cloned(),
        max_replacements: policy
            .get("max_replacements")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(1, 100) as usize),
        create_files: policy.get("create_files").and_then(Value::as_bool),
        allow_multiple: policy.get("allow_multiple").and_then(Value::as_bool),
        deny_hidden_paths: policy.get("deny_hidden_paths").and_then(Value::as_bool),
        total_timeout_ms: policy
            .get("total_timeout_ms")
            .or_else(|| policy.get("tool_timeout_ms"))
            .and_then(Value::as_u64),
        permission_timeout_ms: policy.get("permission_timeout_ms").and_then(Value::as_u64),
    }
}

async fn execute_local_tool(
    config: &Config,
    adapter_state: &mut AdapterState,
    mcp_registry: &McpRegistry,
    session_id: &str,
    tool_name: &str,
    args: Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    match tool_name {
        // Den may deliver either the model-facing provider name or the canonical
        // descriptor name. Both must reach the same BearWire diagnostic query.
        name if is_runtime_diagnostics_tool(name) => {
            crate::bearwire::rpc_call(
                &reqwest::Client::new(),
                config,
                "runtime.diagnostics.list",
                runtime_diagnostics_rpc_params(&config.bear, args)?,
            )
            .await
        }
        "fs_read_text_file" | "fs.read_text_file" => {
            if read_text_file_requires_client_surface(&args) {
                if client_supports_read_text_file(adapter_state) {
                    handle_client_read_text_file(adapter_state, session_id, &args).await
                } else {
                    Err(anyhow!(
                        "fs_read_text_file requested ACP client/editor-buffer semantics, but the client did not advertise fs.readTextFile"
                    ))
                }
            } else {
                let mut params = args;
                params["sessionId"] = json!(session_id);
                handle_direct_read_text_file(adapter_state, params, policy).await
            }
        }
        "fs_list_directory" => {
            handle_direct_list_directory(adapter_state, session_id, &args, policy).await
        }
        "fs_find_paths" => handle_direct_find_paths(adapter_state, session_id, &args, policy).await,
        "fs_search_files" => {
            handle_direct_search_files(adapter_state, session_id, &args, policy).await
        }
        "fs_stat" => handle_direct_stat(adapter_state, session_id, &args, policy).await,
        "git_status" => {
            let context = session_context(adapter_state, session_id)?;
            handle_git_status(context, &args, policy).await
        }
        "git_diff" => {
            let context = session_context(adapter_state, session_id)?;
            handle_git_diff(context, &args, policy).await
        }
        "git_log" => {
            let context = session_context(adapter_state, session_id)?;
            handle_git_log(context, &args, policy).await
        }
        "git_show" => {
            let context = session_context(adapter_state, session_id)?;
            handle_git_show(context, &args, policy).await
        }
        "git_add" => {
            let context = session_context(adapter_state, session_id)?;
            handle_git_add(context, &args, policy).await
        }
        "git_restore" => {
            let context = session_context(adapter_state, session_id)?;
            handle_git_restore(context, &args, policy).await
        }
        "git_commit" => {
            let context = session_context(adapter_state, session_id)?;
            handle_git_commit(context, &args, policy).await
        }
        "git_stash" => {
            let context = session_context(adapter_state, session_id)?;
            handle_git_stash(context, &args, policy).await
        }
        "process_run" => {
            let context = session_context(adapter_state, session_id)?;
            handle_process_run(context, session_id, &args, policy).await
        }
        "run_command" => {
            let context = session_context(adapter_state, session_id)?.clone();
            if client_supports_terminal(adapter_state) && run_command_prefers_terminal(&args) {
                handle_terminal_run_command(
                    adapter_state,
                    &context,
                    session_id,
                    None,
                    None,
                    &args,
                    policy,
                    TerminalCommandValidation::Generic,
                )
                .await
            } else {
                handle_process_run(&context, session_id, &args, policy).await
            }
        }
        "terminal_run_command" => {
            let context = session_context(adapter_state, session_id)?.clone();
            handle_terminal_run_command(
                adapter_state,
                &context,
                session_id,
                None,
                None,
                &args,
                policy,
                TerminalCommandValidation::Allowlisted,
            )
            .await
        }
        "bear_environment" => {
            collect_bear_environment(adapter_state, session_id, None, None, &args).await
        }
        "local_web_fetch" => handle_local_web_fetch(session_id, &args, policy).await,
        "chrome_open" => handle_chrome_open(&args, policy).await,
        "chrome_snapshot" => handle_chrome_snapshot(&args, policy).await,
        "chrome_console_messages" => handle_chrome_console_messages(&args, policy).await,
        "chrome_network_requests" => handle_chrome_network_requests(&args, policy).await,
        "chrome_screenshot" => handle_chrome_screenshot(&args, policy).await,
        "fs_edit_file" | "fs_replace_text" => {
            handle_direct_replace_text(adapter_state, session_id, &args, policy).await
        }
        "fs_create_text_file" => {
            handle_direct_create_text_file(adapter_state, session_id, &args, policy).await
        }
        "fs_create_directory" => {
            handle_direct_create_directory(adapter_state, session_id, &args, policy).await
        }
        "fs_move_path" => handle_direct_move_path(adapter_state, session_id, &args, policy).await,
        "fs_copy_path" => handle_direct_copy_path(adapter_state, session_id, &args, policy).await,
        "fs_apply_patch" => {
            handle_direct_apply_patch(adapter_state, session_id, &args, policy).await
        }
        "fs_delete_path" => {
            handle_direct_delete_path(adapter_state, session_id, &args, policy).await
        }
        _ if mcp_registry.has_tool(session_id, tool_name).await => {
            mcp_registry.call_tool(session_id, tool_name, args).await
        }
        _ => Err(anyhow!(
            "unsupported Den tool_request tool_name {tool_name}"
        )),
    }
}

fn is_runtime_diagnostics_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "list_runtime_diagnostics" | "den.runtime_diagnostics.list"
    )
}

fn runtime_diagnostics_rpc_params(bear_slug: &str, args: Value) -> Result<Value> {
    let mut params = args;
    let object = params
        .as_object_mut()
        .ok_or_else(|| anyhow!("list_runtime_diagnostics arguments must be an object"))?;
    object.insert("bear_slug".to_string(), json!(bear_slug));
    Ok(params)
}

async fn handle_direct_list_directory(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?.clone();
    let session_id = session_id.to_string();
    let args = args.clone();
    let policy = policy.clone();
    tokio::task::spawn_blocking(move || {
        handle_list_directory_blocking(&context, &session_id, &args, &policy)
    })
    .await
    .context("fs_list_directory blocking task failed")?
}

async fn handle_direct_find_paths(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?.clone();
    let session_id = session_id.to_string();
    let args = args.clone();
    let policy = policy.clone();
    tokio::task::spawn_blocking(move || {
        handle_find_paths_blocking(&context, &session_id, &args, &policy)
    })
    .await
    .context("fs_find_paths blocking task failed")?
}

async fn handle_direct_search_files(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?.clone();
    let session_id = session_id.to_string();
    let args = args.clone();
    let policy = policy.clone();
    tokio::task::spawn_blocking(move || {
        handle_search_files_blocking(&context, &session_id, &args, &policy)
    })
    .await
    .context("fs_search_files blocking task failed")?
}

async fn handle_direct_stat(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_stat(context, args, policy).await
}

async fn handle_direct_replace_text(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_replace_text(context, session_id, args, policy).await
}

async fn handle_direct_create_text_file(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_create_text_file(context, session_id, args, policy).await
}

async fn handle_direct_create_directory(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_create_directory(context, session_id, args, policy).await
}

async fn handle_direct_move_path(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_move_path(context, session_id, args, policy).await
}

async fn handle_direct_copy_path(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_copy_path(context, session_id, args, policy).await
}

async fn handle_direct_apply_patch(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_apply_patch(context, session_id, args, policy).await
}

async fn handle_direct_delete_path(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_delete_path(context, session_id, args, policy).await
}

fn create_text_file_diff_content(event: &Value) -> Option<ToolCallContent> {
    let path = tool_path(event)?;
    let content = tool_args_from_event(event)
        .and_then(|v| v.get("content"))
        .and_then(Value::as_str)?;
    Some(ToolCallContent::from(Diff::new(
        PathBuf::from(path),
        content.to_string(),
    )))
}

fn replace_text_diff_content(plan: &ReplaceTextPlan) -> ToolCallContent {
    ToolCallContent::from(
        Diff::new(plan.path.clone(), plan.args.new_text.clone())
            .old_text(Some(plan.args.old_text.clone())),
    )
}

fn session_context<'a>(
    adapter_state: &'a AdapterState,
    session_id: &str,
) -> Result<&'a SessionContext> {
    adapter_state
        .session_contexts
        .get(session_id)
        .ok_or_else(|| anyhow!("ACP session {session_id} is not known to this adapter"))
}

fn token_env_for_auth_method() -> String {
    env::var("DEN_TOKEN_ENV")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "DEN_TOKEN".to_string())
}

fn initialize_result(runtime: &RuntimeConfig) -> Result<Value> {
    let capabilities = AgentCapabilities::new()
        .load_session(true)
        .mcp_capabilities(McpCapabilities::new().http(false).sse(false))
        .prompt_capabilities(
            PromptCapabilities::new()
                .image(false)
                .audio(false)
                .embedded_context(true),
        )
        .session_capabilities(
            SessionCapabilities::new()
                .list(Some(SessionListCapabilities::new()))
                .resume(Some(SessionResumeCapabilities::new()))
                .close(Some(SessionCloseCapabilities::new())),
        );
    let info = Implementation::new("bears", adapter_version()).title(Some("BEARS".to_string()));
    let auth_methods = if runtime.should_advertise_auth_method() {
        vec![AuthMethod::EnvVar(
            AuthMethodEnvVar::new(
                "DEN_TOKEN",
                "BEARS Den Code Token",
                vec![AuthEnvVar::new(token_env_for_auth_method())
                    .label(Some("BEARS Den Code Token".to_string()))
                    .secret(true)],
            )
            .description(Some(
                "Bear-scoped Den Code token. Requires DEN_API_URL and BEAR_SLUG to be configured in the ACP agent server environment. This auth flow cannot fix Den server outages or deployment/version mismatches."
                    .to_string(),
            ))
            .link(Some("https://github.com/silarsis/BEARS".to_string())),
        )]
    } else {
        Vec::new()
    };
    Ok(serde_json::to_value(
        InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(capabilities)
            .agent_info(Some(info))
            .auth_methods(auth_methods),
    )?)
}

fn den_session_display_title(session: &Value) -> Option<String> {
    session
        .get("conversation_title")
        .or_else(|| session.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

fn map_den_sessions_list_to_acp(den: &Value) -> Result<Value> {
    let sessions_in = den
        .get("sessions")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let mut sessions_out = Vec::new();
    for s in sessions_in {
        let session_id = s
            .get("acp_session_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let updated_at = s.get("updated_at").and_then(Value::as_str).unwrap_or("");
        let cwd = s
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if session_id.is_empty() || cwd.is_empty() {
            continue;
        }
        let title = den_session_display_title(&s)
            .or_else(|| {
                s.get("resolved_conversation_id")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                s.get("conversation_id")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
            });
        let info = SessionInfo::new(session_id.to_string(), PathBuf::from(cwd))
            .updated_at(Some(updated_at.to_string()))
            .title(title);
        sessions_out.push(info);
    }
    let response = ListSessionsResponse::new(sessions_out).next_cursor(
        den.get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_string),
    );
    Ok(serde_json::to_value(response)?)
}

fn history_conversation_id(den_session: &Value) -> Option<String> {
    den_session
        .get("history_conversation_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn local_session_context_from_params(params: &Value) -> Result<SessionContext> {
    match session_context_from_params(params) {
        Ok(context) => Ok(context),
        Err(err) if session_params_have_cwd_hint(params) => Err(err),
        Err(err) => {
            let cwd = env::current_dir()
                .context("resolve adapter current directory for local ACP session fallback")?
                .display()
                .to_string();
            if !is_absolute_local_path(&cwd) {
                return Err(err).with_context(|| {
                    format!("adapter current directory fallback is not absolute: {cwd:?}")
                });
            }
            eprintln!(
                "bear-armature: using adapter current directory as local session fallback cwd={} reason={err:#}",
                cwd
            );
            let mut mcp_sources = parse_acp_mcp_servers(params)?;
            if let Some(host_browser_bridge) = host_browser_bridge_config_from_env() {
                mcp_sources.push(host_browser_bridge);
            }
            let mut context = SessionContext {
                cwd: cwd.clone(),
                roots: vec![cwd.clone()],
                raw: json!({
                    "cwd": cwd,
                    "workspace_roots": [cwd],
                    "adapter_version": adapter_version(),
                    "adapter": adapter_capabilities_context(),
                    "direct_tools": direct_tools_context(),
                    "mcp_servers": mcp_sources
                        .iter()
                        .map(McpSourceConfig::safe_summary_for_session_context)
                        .collect::<Vec<_>>(),
                    "host_browser_bridge": host_browser_bridge_env_summary(),
                    "local_fallback": {
                        "reason": format!("{err:#}"),
                        "source": "adapter.current_dir"
                    }
                }),
                mcp_sources,
                conversation_id: None,
                resolved_conversation_id: None,
                thread_title: None,
                current_mode: Some(MODE_ASK.to_string()),
            };
            set_context_mode(
                &mut context,
                MODE_ASK,
                "adapter.local_fallback_default",
                false,
            );
            ensure_session_context_capabilities(&mut context);
            Ok(context)
        }
    }
}

fn session_params_have_cwd_hint(params: &Value) -> bool {
    explicit_cwd_from_params(params).is_some()
        || fallback_cwd_from_params(params).is_some()
        || !workspace_roots_from_params(params).is_empty()
}

fn session_context_from_den_session(params: &Value, den_session: &Value) -> Result<SessionContext> {
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: session_context_from_den_session mcp_summary={}",
            summarize_acp_mcp_servers_param(params)
        );
    }
    let mut mcp_sources = parse_acp_mcp_servers(params)?;
    if let Some(host_browser_bridge) = host_browser_bridge_config_from_env() {
        mcp_sources.push(host_browser_bridge);
    }
    let roots = workspace_roots_from_params(params);
    let cwd = explicit_cwd_from_params(params)
        .transpose()?
        .or_else(|| fallback_cwd_from_params(params))
        .or_else(|| roots.first().cloned())
        .or_else(|| {
            den_session
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .ok_or_else(|| anyhow!("ACP session load/resume requires an absolute cwd; Den session row has no cwd and params.cwd was not provided"))?;
    if !is_absolute_local_path(&cwd) {
        return Err(anyhow!(
            "ACP session cwd must be an absolute local path; got {cwd:?}"
        ));
    }
    let roots = roots_or_cwd(roots, &cwd);
    let mut ctx = SessionContext {
        cwd,
        roots,
        raw: Value::Null,
        mcp_sources,
        conversation_id: den_session
            .get("conversation_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        resolved_conversation_id: den_session
            .get("resolved_conversation_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        thread_title: den_session_display_title(den_session),
        current_mode: Some(infer_mode_from_den_session(den_session).to_string()),
    };
    ctx.raw = json!({
        "cwd": ctx.cwd.clone(),
        "workspace_roots": ctx.roots.clone(),
        "adapter_version": adapter_version(),
        "adapter": adapter_capabilities_context(),
        "direct_tools": direct_tools_context(),
        "mcp_servers": ctx
            .mcp_sources
            .iter()
            .map(McpSourceConfig::safe_summary_for_session_context)
            .collect::<Vec<_>>(),
        "host_browser_bridge": host_browser_bridge_env_summary(),
        "den_acp_session": den_session.clone(),
    });
    ensure_session_context_capabilities(&mut ctx);
    Ok(ctx)
}

async fn den_list_acp_sessions(
    http: &reqwest::Client,
    config: &Config,
    params: &Value,
) -> Result<Value> {
    let include_closed = params
        .get("includeClosed")
        .or_else(|| params.get("include_closed"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let limit = params
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(50)
        .clamp(1, 100);
    let value = bearwire::rpc_call(
        http,
        config,
        "session.state",
        json!({
            "bear_slug": config.bear,
            "include_closed": include_closed,
            "limit": limit,
        }),
    )
    .await
    .context("list BearWire sessions via session.state")?;
    Ok(value)
}

#[derive(Debug)]
struct DenHttpError {
    status: reqwest::StatusCode,
    body: String,
}

impl std::fmt::Display for DenHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.body.trim())
    }
}

impl std::error::Error for DenHttpError {}

fn den_session_error_allows_local_fallback(err: &anyhow::Error) -> bool {
    if let Some(http) = err.downcast_ref::<DenHttpError>() {
        return http.status == reqwest::StatusCode::NOT_FOUND
            || http.status == reqwest::StatusCode::REQUEST_TIMEOUT
            || http.status == reqwest::StatusCode::BAD_GATEWAY
            || http.status == reqwest::StatusCode::SERVICE_UNAVAILABLE
            || http.status == reqwest::StatusCode::GATEWAY_TIMEOUT
            || http.status.is_server_error();
    }
    err.chain().any(|cause| cause.is::<reqwest::Error>())
        || format!("{err:#}").contains("timed out after")
}

async fn request_den_session_mode(
    _http: &reqwest::Client,
    config: Option<&Config>,
    session_id: &str,
    requested_mode: &str,
) -> Result<(&'static str, Value)> {
    let pending_mode = normalize_mode(requested_mode);
    let configured = config.is_some();
    Ok((
        pending_mode,
        json!({
            "message": "BearWire has no standalone session mode endpoint; keeping the client-selected mode locally and applying it on the next session.open/run.start.",
            "deferred": true,
            "source": "adapter.bearwire_local_mode_until_next_prompt",
            "pending_mode": pending_mode,
            "session_id": session_id,
            "configured": configured,
        }),
    ))
}

fn infer_mode_from_den_session(den: &Value) -> &'static str {
    if let Some(policy_label) = den
        .get("session_policy")
        .and_then(|policy| policy.get("mode_label"))
        .and_then(Value::as_str)
    {
        return match policy_label {
            "Plan" => MODE_PLAN,
            "Write" => MODE_WRITE,
            _ => MODE_ASK,
        };
    }
    infer_mode_from_plan_mode_state(den.get("plan_mode"))
}

async fn den_get_acp_session_for_lifecycle(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
) -> Result<Value> {
    match timeout(
        LOCAL_DEN_INSPECTION_TIMEOUT,
        den_get_acp_session(http, config, session_id),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(anyhow!(
            "timed out after {}ms getting ACP session from Den",
            LOCAL_DEN_INSPECTION_TIMEOUT.as_millis()
        )),
    }
}

async fn den_get_acp_session(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
) -> Result<Value> {
    let value = bearwire::rpc_call(
        http,
        config,
        "session.state",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
        }),
    )
    .await
    .with_context(|| format!("get BearWire session.state for session {session_id}"))?;
    match value.get("session") {
        Some(Value::Null) | None => Err(anyhow!(DenHttpError {
            status: reqwest::StatusCode::NOT_FOUND,
            body: format!("BearWire session {session_id} not found"),
        })),
        Some(session) => Ok(session.clone()),
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ReloadHistoryMessage {
    id: Option<String>,
    kind: String,
    role: String,
    text: String,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    status: Option<String>,
    arguments: Value,
    raw_output: Value,
    title: Option<String>,
    title_updated_at: Option<String>,
    replay_policy: Option<String>,
}

#[allow(dead_code)]
impl ReloadHistoryMessage {
    fn text(id: &str, role: &str, text: &str) -> Self {
        Self {
            id: Some(id.to_string()),
            kind: "message".to_string(),
            role: role.to_string(),
            text: text.to_string(),
            tool_call_id: None,
            tool_name: None,
            status: None,
            arguments: Value::Null,
            raw_output: Value::Null,
            title: None,
            title_updated_at: None,
            replay_policy: None,
        }
    }
}

fn flatten_history_pages_chronological(
    pages_newest_first: Vec<Vec<ReloadHistoryMessage>>,
) -> Vec<ReloadHistoryMessage> {
    pages_newest_first.into_iter().rev().flatten().collect()
}

fn history_replay_chunks_with_boundaries(
    messages: Vec<ReloadHistoryMessage>,
) -> Vec<ReloadHistoryMessage> {
    let mut previous_role: Option<String> = None;
    messages
        .into_iter()
        .map(|mut message| {
            if message.kind == "message"
                && previous_role.as_deref() == Some(message.role.as_str())
                && matches!(message.role.as_str(), "user" | "assistant")
                && !message.text.starts_with("\n")
            {
                message.text = format!("\n\n{}", message.text);
            }
            previous_role = Some(message.role.clone());
            message
        })
        .collect()
}

fn history_replay_text_update_kind(message: &ReloadHistoryMessage) -> Option<&'static str> {
    if message.kind != "message" {
        return None;
    }
    match message.role.as_str() {
        "user" => Some("user"),
        "assistant" | "agent" => Some("agent"),
        _ => None,
    }
}

fn reload_history_message_from_value(mut m: Value) -> Result<Option<ReloadHistoryMessage>> {
    if m.get("kind").is_none() {
        m["kind"] = json!("message");
    }
    let kind = m
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("message")
        .to_string();
    if matches!(kind.as_str(), "message" | "") {
        let text = m
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| m.get("content").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if text.trim().is_empty() {
            return Ok(None);
        }
        if m.get("text").is_none() {
            m["text"] = json!(text);
        }
    }
    if matches!(kind.as_str(), "reasoning_delta" | "reasoning")
        && m.get("text").is_none()
        && m.get("delta").is_some()
    {
        m["text"] = m.get("delta").cloned().unwrap_or(Value::Null);
    }

    let event: SurfaceHistoryEvent = serde_json::from_value(m)
        .context("decode BearWire surface history record as shared SurfaceHistoryEvent")?;
    event
        .validate_replay_record()
        .map_err(|message| anyhow!("BearWire surface history {message}"))?;

    let message = match event {
        SurfaceHistoryEvent::Message { id, role, text, .. } => ReloadHistoryMessage {
            id,
            kind: "message".to_string(),
            role,
            text,
            tool_call_id: None,
            tool_name: None,
            status: None,
            arguments: Value::Null,
            raw_output: Value::Null,
            title: None,
            title_updated_at: None,
            replay_policy: None,
        },
        SurfaceHistoryEvent::ToolCall {
            id,
            role,
            tool_call_id,
            tool_name,
            status,
            arguments,
            ..
        } => ReloadHistoryMessage {
            id,
            kind: "tool_call".to_string(),
            role: role.unwrap_or_else(|| "assistant".to_string()),
            text: String::new(),
            tool_call_id: Some(tool_call_id),
            tool_name: Some(tool_name),
            status: Some(status),
            arguments,
            raw_output: Value::Null,
            title: None,
            title_updated_at: None,
            replay_policy: None,
        },
        SurfaceHistoryEvent::ToolResult {
            id,
            role,
            tool_call_id,
            tool_name,
            status,
            text,
            raw_output,
            ..
        } => ReloadHistoryMessage {
            id,
            kind: "tool_result".to_string(),
            role: role.unwrap_or_else(|| "tool".to_string()),
            text: text.unwrap_or_default(),
            tool_call_id: Some(tool_call_id),
            tool_name: Some(tool_name),
            status: Some(status),
            arguments: Value::Null,
            raw_output,
            title: None,
            title_updated_at: None,
            replay_policy: None,
        },
        SurfaceHistoryEvent::ReasoningDelta {
            id,
            role,
            text,
            replay_policy,
            ..
        } => ReloadHistoryMessage {
            id,
            kind: "reasoning_delta".to_string(),
            role: role.unwrap_or_else(|| "assistant".to_string()),
            text,
            tool_call_id: None,
            tool_name: None,
            status: None,
            arguments: Value::Null,
            raw_output: Value::Null,
            title: None,
            title_updated_at: None,
            replay_policy,
        },
        SurfaceHistoryEvent::SessionInfoUpdate {
            id,
            role,
            title,
            title_updated_at,
            ..
        } => ReloadHistoryMessage {
            id,
            kind: "session_info_update".to_string(),
            role: role.unwrap_or_else(|| "system".to_string()),
            text: String::new(),
            tool_call_id: None,
            tool_name: None,
            status: None,
            arguments: Value::Null,
            raw_output: Value::Null,
            title,
            title_updated_at,
            replay_policy: None,
        },
    };
    Ok(Some(message))
}

async fn fetch_conversation_surface_history_chronological(
    http: &reqwest::Client,
    config: &Config,
    conversation_id: &str,
) -> Result<Vec<ReloadHistoryMessage>> {
    let mut pages_newest_first: Vec<Vec<ReloadHistoryMessage>> = Vec::new();
    let mut before: Option<String> = None;
    let mut seen_cursors = std::collections::HashSet::new();
    loop {
        let mut params = json!({
            "bear_slug": config.bear,
            "conversation_id": conversation_id,
            "limit": 50,
            // Session metadata, artifacts, and diagnostics are independent of this
            // message cursor. Request them only on the newest page.
            "include_surface_enrichment": before.is_none(),
        });
        if let Some(before) = before.as_deref() {
            params["before"] = json!(before);
        }
        let body = bearwire::rpc_call(http, config, "conversation.surface_history", params)
            .await
            .with_context(|| {
                format!("get BearWire conversation surface history for {conversation_id}")
            })?;
        let records = body
            .get("surface_events")
            .or_else(|| body.get("messages"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut page = Vec::new();
        for m in records {
            if let Some(message) = reload_history_message_from_value(m)? {
                page.push(message);
            }
        }
        pages_newest_first.push(page);
        let has_more = body
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !has_more {
            break;
        }
        let Some(next_before) = body
            .get("next_before")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.is_empty())
        else {
            break;
        };
        if !seen_cursors.insert(next_before.clone()) {
            return Err(anyhow!(
                "BearWire history pagination repeated cursor {next_before:?} for conversation {conversation_id}"
            ));
        }
        before = Some(next_before);
    }
    Ok(flatten_history_pages_chronological(pages_newest_first))
}

fn replay_tool_request(
    requests: &mut std::collections::HashMap<String, ToolRequestPresentation>,
    message: &ReloadHistoryMessage,
    tool_call_id: &str,
    tool_name: &str,
) -> ToolRequestPresentation {
    if message.kind == "tool_call" {
        let request = ToolRequestPresentation {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: (!message.arguments.is_null()).then(|| message.arguments.clone()),
            display: None,
        };
        requests.insert(tool_call_id.to_string(), request.clone());
        request
    } else {
        requests
            .remove(tool_call_id)
            .unwrap_or_else(|| ToolRequestPresentation {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: None,
                display: None,
            })
    }
}

async fn replay_history_for_den_session(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    den: &Value,
    lifecycle_method: &str,
) -> Result<()> {
    if let Some(conv) = history_conversation_id(den) {
        let messages =
            fetch_conversation_surface_history_chronological(http, config, &conv).await?;
        let fetched = messages.len();
        let mut counts = std::collections::BTreeMap::<&str, usize>::new();
        // Tool results intentionally do not duplicate request arguments in canonical history.
        // Keep them only for this replay pass so terminal ACP updates preserve the request card.
        let mut tool_requests = std::collections::HashMap::<String, ToolRequestPresentation>::new();
        for message in history_replay_chunks_with_boundaries(messages) {
            match message.kind.as_str() {
                "tool_call" | "tool_result" => {
                    let category = if message.kind == "tool_call" {
                        "tool_call"
                    } else {
                        "tool_result"
                    };
                    let Some(tool_call_id) =
                        message.tool_call_id.as_deref().or(message.id.as_deref())
                    else {
                        *counts.entry("ignored").or_default() += 1;
                        continue;
                    };
                    let Some(tool_name) = message.tool_name.as_deref() else {
                        *counts.entry("ignored").or_default() += 1;
                        continue;
                    };
                    *counts.entry(category).or_default() += 1;
                    let request =
                        replay_tool_request(&mut tool_requests, &message, tool_call_id, tool_name);
                    send_tool_call_update(
                        session_id,
                        tool_call_id,
                        tool_name,
                        ToolCallUpdatePayload {
                            status: message.status.as_deref().unwrap_or(
                                if message.kind == "tool_call" {
                                    "pending"
                                } else {
                                    "ok"
                                },
                            ),
                            text: &message.text,
                            request: Some(request),
                            raw_output: if message.raw_output.is_null() {
                                None
                            } else {
                                Some(message.raw_output.clone())
                            },
                            extra_content: Vec::new(),
                        },
                    )
                    .await?;
                }
                "session_info_update" => {
                    if message.title.is_some() || message.title_updated_at.is_some() {
                        *counts.entry("session_info").or_default() += 1;
                        send_session_info_update(
                            session_id,
                            message.title.clone(),
                            message.title_updated_at.clone(),
                        )
                        .await?;
                    } else {
                        *counts.entry("ignored").or_default() += 1;
                    }
                }
                "reasoning_delta" | "reasoning" => {
                    if !matches!(message.replay_policy.as_deref(), Some("none")) {
                        *counts.entry("reasoning").or_default() += 1;
                        send_agent_thought_chunk(session_id, &message.text).await?;
                    } else {
                        *counts.entry("ignored").or_default() += 1;
                    }
                }
                _ => match history_replay_text_update_kind(&message) {
                    // ACP session/load replays the visible transcript to the client. This is
                    // client-side rendering only; Den owns model-context replay from canonical
                    // conversation storage, so these chunks are not sent back to the model.
                    Some("user") => {
                        *counts.entry("user").or_default() += 1;
                        send_user_message_chunk(session_id, &message.text).await?;
                    }
                    Some("agent") => {
                        *counts.entry("agent").or_default() += 1;
                        // Same unidentified chunk shape as live agent updates. This client
                        // renders those; agent chunks with messageId do not appear in the
                        // rehydrated thread.
                        send_agent_message_chunk(session_id, &message.text).await?;
                    }
                    _ => *counts.entry("ignored").or_default() += 1,
                },
            }
        }
        eprintln!(
            "bear-armature: {} session_id={} conversation_id={} history fetched={} projected user={} agent={} tool_call={} tool_result={} reasoning={} session_info={} ignored={}",
            lifecycle_method,
            session_id,
            conv,
            fetched,
            counts.get("user").copied().unwrap_or_default(),
            counts.get("agent").copied().unwrap_or_default(),
            counts.get("tool_call").copied().unwrap_or_default(),
            counts.get("tool_result").copied().unwrap_or_default(),
            counts.get("reasoning").copied().unwrap_or_default(),
            counts.get("session_info").copied().unwrap_or_default(),
            counts.get("ignored").copied().unwrap_or_default()
        );
    } else {
        eprintln!(
            "bear-armature: {} session_id={} has no history_conversation_id; projected history counts are all zero",
            lifecycle_method, session_id
        );
    }
    Ok(())
}

async fn restore_session_from_den(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    params: &Value,
) -> Result<(&'static str, Option<Value>, Option<Value>)> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session params missing sessionId"))?;
    let den = match den_get_acp_session_for_lifecycle(http, config, session_id).await {
        Ok(den) => Some(den),
        Err(err) if den_session_error_allows_local_fallback(&err) => {
            eprintln!(
                "bear-armature: session/resume session_id={} could not load Den session ({}); restoring as local pending session",
                session_id,
                truncate_for_log(&format!("{err:#}"), 240)
            );
            None
        }
        Err(err) => return Err(err),
    };
    let context = if let Some(den) = den.as_ref() {
        session_context_from_den_session(params, den)?
    } else {
        local_session_context_from_params(params)?
    };
    let mcp_context = shared_state
        .mcp_registry
        .configure_session(session_id, context.mcp_sources.clone())
        .await?;
    let mut context = context;
    context.raw["mcp"] = mcp_context;
    ensure_session_context_capabilities(&mut context);
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: session/resume session_id={} cwd={} roots={} direct_tools={} mcp={}",
            session_id,
            context.cwd,
            context.roots.join(","),
            context
                .raw
                .get("direct_tools")
                .cloned()
                .unwrap_or(Value::Null),
            summarize_mcp_for_log(context.raw.get("mcp"))
        );
    }
    shared_state
        .session_contexts
        .lock()
        .await
        .insert(session_id.to_string(), context.clone());
    adapter_state
        .session_contexts
        .insert(session_id.to_string(), context);
    spawn_adapter_environment_publish(
        config.clone(),
        session_id.to_string(),
        adapter_state.clone(),
        None,
    );
    Ok((
        den.as_ref()
            .map(infer_mode_from_den_session)
            .unwrap_or(MODE_ASK),
        den.as_ref()
            .and_then(|session| session.get("context_budget").cloned()),
        den,
    ))
}

async fn handle_session_load(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response_id: Value,
    params: &Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/load params missing sessionId"))?;
    let den = match den_get_acp_session_for_lifecycle(http, config, session_id).await {
        Ok(den) => Some(den),
        Err(err) if den_session_error_allows_local_fallback(&err) => {
            eprintln!(
                "bear-armature: session/load session_id={} could not load Den session ({}); loading as local pending session",
                session_id,
                truncate_for_log(&format!("{err:#}"), 240)
            );
            None
        }
        Err(err) => return Err(err),
    };
    let context = if let Some(den) = den.as_ref() {
        session_context_from_den_session(params, den)?
    } else {
        local_session_context_from_params(params)?
    };
    let mcp_context = shared_state
        .mcp_registry
        .configure_session(session_id, context.mcp_sources.clone())
        .await?;
    let mut context = context;
    context.raw["mcp"] = mcp_context;
    ensure_session_context_capabilities(&mut context);
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: session/load session_id={} cwd={} roots={} direct_tools={} mcp={}",
            session_id,
            context.cwd,
            context.roots.join(","),
            context
                .raw
                .get("direct_tools")
                .cloned()
                .unwrap_or(Value::Null),
            summarize_mcp_for_log(context.raw.get("mcp"))
        );
    }
    shared_state
        .session_contexts
        .lock()
        .await
        .insert(session_id.to_string(), context.clone());
    adapter_state
        .session_contexts
        .insert(session_id.to_string(), context);
    spawn_adapter_environment_publish(
        config.clone(),
        session_id.to_string(),
        adapter_state.clone(),
        None,
    );
    let mode = den
        .as_ref()
        .map(infer_mode_from_den_session)
        .unwrap_or(MODE_ASK);
    // ACP session/load is a streaming lifecycle request: replay historical
    // session/update notifications before completing the request.
    if let Some(den) = den.as_ref() {
        replay_history_for_den_session(http, config, session_id, den, "session/load").await?;
        surface_submitted_plan_fallback(session_id, den).await?;
    }
    send_available_commands_update(session_id).await?;
    if let Some(context_budget) = den.and_then(|session| session.get("context_budget").cloned()) {
        send_context_budget_usage_update(session_id, context_budget).await?;
    }
    write_response(response_id, Ok(session_lifecycle_result(mode)?)).await?;
    Ok(())
}

fn session_lifecycle_result(mode: &str) -> Result<Value> {
    Ok(serde_json::to_value(
        LoadSessionResponse::new()
            .config_options(session_config_options_for_mode(mode))
            .modes(session_modes_for_mode(mode)),
    )?)
}

async fn terminalize_active_prompt_for_lifecycle(
    shared_state: &AdapterSharedState,
    session_id: &str,
    origin: CancellationOrigin,
    tool_card_reason: &str,
) -> Result<()> {
    let active_turn = shared_state
        .active_prompts
        .lock()
        .await
        .get(session_id)
        .cloned();
    let terminalization_result = if let Some(turn) = active_turn.as_ref() {
        terminalize_tool_cards_for_turn(shared_state, session_id, turn.token, tool_card_reason)
            .await
    } else {
        Ok(())
    };
    let _ = shared_state.cancellation_tx.send(CancellationNotice {
        session_id: session_id.to_string(),
        turn_token: active_turn.as_ref().map(|turn| turn.token),
        conversation_id: active_turn
            .as_ref()
            .and_then(|turn| turn.conversation_id.clone()),
        scope: if active_turn.is_some() {
            CancellationScope::PromptAndTools
        } else {
            CancellationScope::ToolsOnly
        },
        origin,
    });
    if let Some(turn) = active_turn.as_ref() {
        shared_state
            .tool_tasks
            .cancel_turn(session_id, turn.token)
            .await;
    } else {
        shared_state.tool_tasks.cancel_session(session_id).await;
    }

    let prompt_response_result = if let Some(turn) = active_turn.as_ref() {
        if let Some(response_id) = turn.response.claim() {
            write_prompt_end_turn_response(response_id).await
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };

    if let Some(turn) = active_turn.as_ref() {
        let mut active = shared_state.active_prompts.lock().await;
        if active
            .get(session_id)
            .is_some_and(|current| current.token == turn.token)
        {
            active.remove(session_id);
        }
    }
    clear_surface_tool_statuses_for_session(shared_state, session_id).await;

    terminalization_result?;
    prompt_response_result
}

async fn handle_session_close(
    http: &reqwest::Client,
    config: &Config,
    shared_state: &AdapterSharedState,
    params: Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/close params missing sessionId"))?;
    shared_state.approval_cache.clear_session(session_id).await;
    shared_state
        .last_plan_update_hashes
        .lock()
        .await
        .remove(session_id);
    shared_state
        .session_contexts
        .lock()
        .await
        .remove(session_id);
    let prompt_result = terminalize_active_prompt_for_lifecycle(
        shared_state,
        session_id,
        CancellationOrigin::SessionClose,
        "Session closed; result unavailable.",
    )
    .await;
    let close_result = post_session_lifecycle_action(http, config, session_id, "close").await;
    prompt_result?;
    close_result
}

async fn handle_session_cancel(
    http: &reqwest::Client,
    config: &Config,
    shared_state: &AdapterSharedState,
    params: Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/cancel params missing sessionId"))?;
    shared_state.approval_cache.clear_session(session_id).await;
    shared_state
        .last_plan_update_hashes
        .lock()
        .await
        .remove(session_id);

    let prompt_result = terminalize_active_prompt_for_lifecycle(
        shared_state,
        session_id,
        CancellationOrigin::SessionCancel,
        "Cancelled by user; result unavailable.",
    )
    .await;
    let den_cancel_result = post_session_lifecycle_action(http, config, session_id, "cancel").await;
    prompt_result?;
    den_cancel_result
}

async fn post_session_lifecycle_action(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    action: &str,
) -> Result<()> {
    post_session_lifecycle_action_with_payload(
        http,
        config,
        session_id,
        action,
        json!({ "adapter_contract": adapter_contract_context() }),
    )
    .await
}

async fn post_session_lifecycle_action_with_payload(
    _http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    action: &str,
    _payload: Value,
) -> Result<()> {
    if !bearwire::enabled() {
        return Err(anyhow!(
            "BearWire is disabled in this adapter process, and legacy ACP HTTP is retired. Enable BearWire by setting BEARS_BEARWIRE=auto or true."
        ));
    }
    let result = match action {
        "close" => bearwire::post_session_close(config, session_id).await,
        "cancel" => bearwire::post_run_cancel(config, session_id).await,
        other => {
            return Err(anyhow!(
                "unsupported BearWire session lifecycle action: {other}"
            ))
        }
    }?;
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: posted BearWire session lifecycle action={} session_id={} response={}",
            action,
            session_id,
            truncate_for_log(&result.to_string(), 360)
        );
    }
    Ok(())
}

async fn compact_session_conversation(
    _http: &reqwest::Client,
    config: &Config,
    session_id: &str,
) -> Result<Value> {
    bearwire::post_session_compact(config, session_id).await
}

fn render_compact_recovery_result(result: &Value) -> String {
    let approval_recovery = result.get("approval_recovery");
    let approval_attempted = approval_recovery
        .and_then(|value| value.get("attempted"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let denied_count = approval_recovery
        .and_then(|value| value.get("denied_count"))
        .and_then(Value::as_u64);
    let compacted = result
        .get("compacted")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let approval_sentence = if approval_attempted {
        match denied_count {
            Some(0) => "No stale approval requests needed closing.".to_string(),
            Some(1) => "Closed 1 stale approval request.".to_string(),
            Some(count) => format!("Closed {count} stale approval requests."),
            None => "Checked for stale approval requests.".to_string(),
        }
    } else {
        "No stale approval recovery was attempted; compaction does not repair unresolved approvals."
            .to_string()
    };
    let compact_sentence = if compacted {
        "The conversation was compacted."
    } else {
        "The conversation was checked."
    };

    format!(
        "BEARS ACP recovery completed for this session. {approval_sentence} {compact_sentence} Retry your last prompt."
    )
}

fn prompt_end_turn_response_value() -> Result<Value> {
    Ok(serde_json::to_value(PromptResponse::new(
        StopReason::EndTurn,
    ))?)
}

async fn write_prompt_end_turn_response(response_id: Value) -> Result<()> {
    write_response(response_id, Ok(prompt_end_turn_response_value()?)).await
}

async fn handle_local_slash_prompt(
    http: Option<&reqwest::Client>,
    config: Option<&Config>,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response_id: Value,
    params: Value,
    command: LocalSlashCommand,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/prompt params missing sessionId"))?;
    let prompt = prompt_text_from_params(&params)?;
    let display_prompt = prompt_display_text_from_params(&params).unwrap_or_else(|| prompt.clone());
    send_user_message_chunk(session_id, &display_prompt).await?;
    let mut launched = None;
    let report = if command == LocalSlashCommand::Debug {
        debug_report(debug_argument_from_prompt(&prompt))
    } else if command == LocalSlashCommand::Focus {
        focus_report(
            http,
            config,
            adapter_state,
            shared_state,
            session_id,
            &prompt,
            &mut launched,
        )
        .await
    } else {
        handle_local_slash_command(
            http,
            config,
            adapter_state,
            shared_state,
            session_id,
            command,
        )
        .await
    };
    send_agent_message_chunk(session_id, &report).await?;
    if let (Some(http), Some(config), Some(run_result)) = (http, config, launched) {
        let response = PromptResponseGuard::new(response_id);
        let turn_token = Uuid::new_v4();
        let conversation_id_for_turn = prompt_conversation_id_from_params(&params);
        let previous = register_prompt_turn_for_session(
            shared_state,
            session_id,
            turn_token,
            conversation_id_for_turn.clone(),
            response.clone(),
        )
        .await;
        if let Some(previous) = previous {
            if let Some(previous_id) = previous.response.claim() {
                write_prompt_end_turn_response(previous_id).await?;
            }
            tracing::info!(
                session_id,
                previous_turn = %previous.token,
                turn_token = %turn_token,
                conversation_id = ?conversation_id_for_turn,
                "focused Docket execution superseded the previous ACP projection turn"
            );
        }

        let result = bearwire::follow_run(
            http,
            config,
            adapter_state,
            shared_state,
            response,
            session_id,
            run_result,
            turn_token,
        )
        .await;
        let mut active = shared_state.active_prompts.lock().await;
        if active
            .get(session_id)
            .is_some_and(|turn| turn.token == turn_token)
        {
            active.remove(session_id);
        }
        result
    } else {
        write_prompt_end_turn_response(response_id).await
    }
}

async fn handle_prompt(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response: PromptResponseGuard,
    params: Value,
    turn_token: Uuid,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let result = handle_prompt_with_retry(
        http,
        config,
        adapter_state,
        shared_state,
        PromptRetryContext {
            response,
            params,
            turn_token,
        },
    )
    .await;
    let mut active = shared_state.active_prompts.lock().await;
    if active
        .get(&session_id)
        .is_some_and(|turn| turn.token == turn_token)
    {
        active.remove(&session_id);
    }
    result
}

struct PromptRetryContext {
    response: PromptResponseGuard,
    params: Value,
    turn_token: Uuid,
}

async fn handle_prompt_with_retry(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    retry: PromptRetryContext,
) -> Result<()> {
    let PromptRetryContext {
        response,
        params,
        turn_token,
    } = retry;
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/prompt params missing sessionId"))?;
    let prompt_shape = prompt_block_shape(&params);
    let prompt_context = prompt_context_from_params(&params)?;
    let prompt = require_human_prompt_text(prompt_context.human_message.clone())?;
    let prompt_context_json = bearwire_prompt_context_from_context(&prompt_context);
    let display_prompt = prompt_display_text_from_params(&params).unwrap_or_else(|| prompt.clone());
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: session/prompt session_id={} prompt_len={} display_prompt_len={} prompt_blocks={{text:{}, resource:{}, resource_link:{}, other:{}}} prompt_provenance={{human_text:{}, human_pasted_debug_text:{}, client_resource:{}, client_synthetic_context:{}, unsupported:{}}} prompt_context={{references:{}, synthetic_omitted:{}, resource_bodies_not_in_human_message:{}, sent:{}}} prompt_has_trusted_mode_suffix={} display_has_trusted_mode_suffix={} prompt_has_system_reminder={} display_has_system_reminder={}",
            session_id,
            prompt.len(),
            display_prompt.len(),
            prompt_shape.text,
            prompt_shape.resource,
            prompt_shape.resource_link,
            prompt_shape.other,
            prompt_shape.human_text,
            prompt_shape.human_pasted_debug_text,
            prompt_shape.client_resource,
            prompt_shape.client_synthetic_context,
            prompt_shape.unsupported,
            prompt_context.resource_references.len(),
            prompt_context.diagnostics.synthetic_context_omitted,
            prompt_context
                .diagnostics
                .resource_bodies_not_in_human_message,
            !prompt_context_json.is_null(),
            prompt.contains("Trusted ACP session mode this turn:"),
            display_prompt.contains("Trusted ACP session mode this turn:"),
            prompt.contains("<system-reminder>"),
            display_prompt.contains("<system-reminder>"),
        );
    }
    if let Some(command) = parse_local_slash_command(&prompt) {
        send_user_message_chunk(session_id, &display_prompt).await?;
        let report = handle_local_slash_command(
            Some(http),
            Some(config),
            adapter_state,
            shared_state,
            session_id,
            command,
        )
        .await;
        send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, &report).await?;
        if let Some(response_id) = response.claim() {
            write_prompt_end_turn_response(response_id).await?;
        }
        return Ok(());
    }
    let mut client_context = shared_state
        .session_contexts
        .lock()
        .await
        .get(session_id)
        .cloned()
        .or_else(|| adapter_state.session_contexts.get(session_id).cloned())
        .unwrap_or_else(|| {
            eprintln!(
                "bear-armature: session/prompt session_id={} had no cached session context; using fallback direct tool context",
                session_id
            );
            SessionContext {
                raw: json!({
                    "adapter_version": adapter_version(),
                    "adapter": adapter_capabilities_context(),
                    "direct_tools": direct_tools_context(),
                }),
                ..Default::default()
            }
        });
    ensure_session_context_capabilities(&mut client_context);
    let conversation_id = client_context
        .resolved_conversation_id
        .as_deref()
        .or(client_context.conversation_id.as_deref())
        .map(str::to_string);
    let conversation_log = conversation_id.as_deref().unwrap_or("<den-selected>");
    let prompt_mcp_tool_names = client_context
        .raw
        .pointer("/mcp/client_tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: session/prompt session_id={} bear={} conversation_id={} client={} direct_tools={} mcp_servers={} mcp_tool_count={} mcp_tool_names={:?}",
            session_id,
            config.bear,
            conversation_log,
            config.client,
            client_context
                .raw
                .get("direct_tools")
                .cloned()
                .unwrap_or(Value::Null),
            client_context
                .raw
                .pointer("/mcp/servers")
                .cloned()
                .unwrap_or(Value::Null),
            prompt_mcp_tool_names.len(),
            prompt_mcp_tool_names
        );
    }

    let requested_mode = client_context
        .current_mode
        .as_deref()
        .map(normalize_mode)
        .unwrap_or(MODE_ASK);
    let mut den_payload = json!({
        "message": prompt,
        "prompt_context": prompt_context_json,
        "client": config.client,
        "client_capabilities": shared_state.client_capabilities.lock().await.clone(),
        "client_context": client_context.raw,
        "requested_mode": requested_mode,
        "adapter_contract": adapter_contract_context(),
    });
    if let Some(conversation_id) = conversation_id.as_deref() {
        den_payload["conversation_id"] = json!(conversation_id);
    }

    bearwire::try_handle_prompt(
        http,
        config,
        adapter_state,
        shared_state,
        response,
        session_id,
        &prompt,
        den_payload
            .get("prompt_context")
            .cloned()
            .unwrap_or(Value::Null),
        den_payload
            .get("client_context")
            .cloned()
            .unwrap_or(Value::Null),
        conversation_id.as_deref(),
        requested_mode,
        turn_token,
    )
    .await?;
    return Ok(());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalSlashCommand {
    Doctor,
    Compact,
    Conversation,
    Capabilities,
    Runtime,
    Status,
    Focus,
    Version,
    Debug,
}

#[derive(Debug, Clone, Copy)]
struct LocalSlashCommandDescriptor {
    name: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    command: LocalSlashCommand,
    den_required: bool,
}

const LOCAL_SLASH_COMMANDS: &[LocalSlashCommandDescriptor] = &[
    LocalSlashCommandDescriptor {
        name: "doctor",
        aliases: &[],
        description: "Show BEARS ACP adapter, session, client, and Den configuration diagnostics.",
        command: LocalSlashCommand::Doctor,
        den_required: false,
    },
    LocalSlashCommandDescriptor {
        name: "compact",
        aliases: &["collapse"],
        description: "Ask Den to compact the conversation transcript. Compaction does not repair stale runtime approval state.",
        command: LocalSlashCommand::Compact,
        den_required: true,
    },
    LocalSlashCommandDescriptor {
        name: "conversation",
        aliases: &[],
        description: "Show the current ACP session and runtime conversation binding.",
        command: LocalSlashCommand::Conversation,
        den_required: false,
    },
    LocalSlashCommandDescriptor {
        name: "capabilities",
        aliases: &[],
        description: "Show ACP client capabilities and adapter-local direct tools.",
        command: LocalSlashCommand::Capabilities,
        den_required: false,
    },
    LocalSlashCommandDescriptor {
        name: "runtime",
        aliases: &[],
        description: "Show adapter runtime state, active local tool tasks, and optional Den runtime state.",
        command: LocalSlashCommand::Runtime,
        den_required: false,
    },
    LocalSlashCommandDescriptor {
        name: "status",
        aliases: &[],
        description: "Show concise BEARS status from adapter-local environment plus optional Den health.",
        command: LocalSlashCommand::Status,
        den_required: false,
    },
    LocalSlashCommandDescriptor {
        name: "focus",
        aliases: &[],
        description: "Focus this pair session on a Docket job: /focus [job_id].",
        command: LocalSlashCommand::Focus,
        den_required: true,
    },
    LocalSlashCommandDescriptor {
        name: "version",
        aliases: &[],
        description: "Show BEARS adapter version/build metadata plus optional Den version.",
        command: LocalSlashCommand::Version,
        den_required: false,
    },
    LocalSlashCommandDescriptor {
        name: "debug",
        aliases: &["debug-ui"],
        description: "Show or set BEARS debug thought visibility: /debug off|on|verbose.",
        command: LocalSlashCommand::Debug,
        den_required: false,
    },
];

fn local_slash_available_commands() -> Vec<AvailableCommand> {
    LOCAL_SLASH_COMMANDS
        .iter()
        .flat_map(|descriptor| {
            std::iter::once(descriptor.name)
                .chain(descriptor.aliases.iter().copied())
                .map(move |name| AvailableCommand::new(name, descriptor.description))
        })
        .collect()
}

fn local_slash_descriptor_for_name(name: &str) -> Option<&'static LocalSlashCommandDescriptor> {
    let normalized = name.trim().trim_start_matches('/');
    LOCAL_SLASH_COMMANDS.iter().find(|descriptor| {
        descriptor.name == normalized || descriptor.aliases.contains(&normalized)
    })
}

fn local_slash_descriptor_for_command(
    command: LocalSlashCommand,
) -> Option<&'static LocalSlashCommandDescriptor> {
    LOCAL_SLASH_COMMANDS
        .iter()
        .find(|descriptor| descriptor.command == command)
}

fn parse_local_slash_command(prompt: &str) -> Option<LocalSlashCommand> {
    let token = prompt.split_whitespace().next()?;
    let name = token.strip_prefix('/')?;
    local_slash_descriptor_for_name(name).map(|descriptor| descriptor.command)
}

async fn handle_local_slash_command(
    http: Option<&reqwest::Client>,
    config: Option<&Config>,
    adapter_state: &AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    command: LocalSlashCommand,
) -> String {
    match command {
        LocalSlashCommand::Doctor => {
            acp_doctor_report(
                http,
                config,
                adapter_state,
                &client_context_for_doctor(adapter_state, session_id),
            )
            .await
        }
        LocalSlashCommand::Compact => {
            let (Some(http), Some(config)) = (http, config) else {
                return den_required_slash_command_unavailable(command);
            };
            match compact_session_conversation(http, config, session_id).await {
                Ok(result) => {
                    eprintln!(
                        "bear-armature: manual ACP recovery completed session_id={} result={}",
                        session_id, result
                    );
                    render_compact_recovery_result(&result)
                }
                Err(err) => {
                    eprintln!(
                        "bear-armature: manual ACP recovery failed session_id={} error={err:#}",
                        session_id
                    );
                    "BEARS ACP recovery failed. The session may still be wedged; please start a new ACP session if retrying does not work.".to_string()
                }
            }
        }
        LocalSlashCommand::Conversation => conversation_report(adapter_state, session_id),
        LocalSlashCommand::Capabilities => capabilities_report(adapter_state),
        LocalSlashCommand::Runtime => {
            runtime_report(http, config, adapter_state, shared_state, session_id).await
        }
        LocalSlashCommand::Status => {
            status_report(http, config, adapter_state, shared_state, session_id).await
        }
        LocalSlashCommand::Focus => "Den ACP /focus usage: /focus [job_id]".to_string(),
        LocalSlashCommand::Version => version_report(http, config).await,
        LocalSlashCommand::Debug => debug_report(None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FocusPromptTarget {
    ConversationAssociated,
    JobId(String),
    JobIdPrefix(String),
    Invalid,
}

fn is_job_id_prefix(candidate: &str) -> bool {
    const UUID_HYPHEN_INDICES: [usize; 4] = [8, 13, 18, 23];

    candidate.len() >= 8
        && candidate.len() < Uuid::nil().hyphenated().to_string().len()
        && candidate.bytes().enumerate().all(|(index, byte)| {
            if UUID_HYPHEN_INDICES.contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn focus_prompt_target(prompt: &str) -> FocusPromptTarget {
    let mut parts = prompt.split_whitespace();
    let _command = parts.next();
    let Some(candidate) = parts.next() else {
        return FocusPromptTarget::ConversationAssociated;
    };
    if parts.next().is_some() {
        return FocusPromptTarget::Invalid;
    }
    if Uuid::parse_str(candidate).is_ok() {
        FocusPromptTarget::JobId(candidate.to_string())
    } else if is_job_id_prefix(candidate) {
        FocusPromptTarget::JobIdPrefix(candidate.to_ascii_lowercase())
    } else {
        FocusPromptTarget::Invalid
    }
}

#[cfg(test)]
fn focus_job_id_from_prompt(prompt: &str) -> Option<String> {
    match focus_prompt_target(prompt) {
        FocusPromptTarget::JobId(job_id) => Some(job_id),
        FocusPromptTarget::JobIdPrefix(_)
        | FocusPromptTarget::ConversationAssociated
        | FocusPromptTarget::Invalid => None,
    }
}

fn collect_docket_job_refs(value: &Value, jobs: &mut std::collections::BTreeSet<String>) {
    match value {
        Value::String(text) => {
            if let Some(job_id) = text.strip_prefix("docket_job:") {
                if job_id != "<none>" && Uuid::parse_str(job_id).is_ok() {
                    jobs.insert(job_id.to_string());
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_docket_job_refs(value, jobs);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_docket_job_refs(value, jobs);
            }
        }
        Value::Bool(_) | Value::Number(_) | Value::Null => {}
    }
}

fn docket_job_ids_from_task_list_status(value: &Value) -> Vec<String> {
    let mut jobs = std::collections::BTreeSet::new();
    collect_docket_job_refs(value, &mut jobs);
    jobs.into_iter().collect()
}

fn docket_job_ids_from_den_session_state(value: &Value) -> Vec<String> {
    let mut jobs = std::collections::BTreeSet::new();
    if let Some(plan) = value.pointer("/diagnostics/active_activity_plan") {
        collect_docket_job_refs(plan, &mut jobs);
    }
    jobs.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocketJobListEntry {
    id: String,
    goal: String,
    status: String,
}

impl DocketJobListEntry {
    fn is_completed(&self) -> bool {
        self.status.eq_ignore_ascii_case("completed")
    }
}

async fn den_list_docket_jobs_for_session(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
) -> Result<Vec<DocketJobListEntry>> {
    let value = bearwire::rpc_call(
        http,
        config,
        "docket.jobs.list",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "limit": 50,
        }),
    )
    .await
    .with_context(|| format!("list BearWire Docket jobs for session {session_id}"))?;
    let jobs = value
        .get("jobs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|job| {
            Some(DocketJobListEntry {
                id: job.get("id")?.as_str()?.to_string(),
                goal: job
                    .get("goal")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                status: job
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .trim()
                    .to_string(),
            })
        })
        .filter(|job| Uuid::parse_str(&job.id).is_ok())
        .collect();
    Ok(jobs)
}

fn truncate_for_focus_list(text: &str) -> String {
    const LIMIT: usize = 120;
    let trimmed = text.trim();
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_string();
    }
    format!("{}...", trimmed.chars().take(LIMIT - 3).collect::<String>())
}

fn focus_job_choice_lines(jobs: &[DocketJobListEntry]) -> String {
    jobs.iter()
        .map(|job| {
            let goal = truncate_for_focus_list(&job.goal);
            if goal.is_empty() {
                format!("- /focus {}\n  {}", job.id, job.status)
            } else {
                format!("- /focus {}\n  {} — {}", job.id, job.status, goal)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn focus_noncompleted_jobs(jobs: &[DocketJobListEntry]) -> Vec<DocketJobListEntry> {
    jobs.iter()
        .filter(|job| !job.is_completed())
        .cloned()
        .collect()
}

fn focus_choice_jobs(jobs: &[DocketJobListEntry]) -> Vec<DocketJobListEntry> {
    const MAX_CHOICES: usize = 10;

    let mut choices = jobs
        .iter()
        .filter(|job| !job.is_completed())
        .take(MAX_CHOICES)
        .cloned()
        .collect::<Vec<_>>();
    let remaining = MAX_CHOICES.saturating_sub(choices.len());
    if remaining > 0 {
        choices.extend(
            jobs.iter()
                .filter(|job| job.is_completed())
                .take(remaining)
                .cloned(),
        );
    }
    choices
}

fn session_title_from_adapter_state(
    adapter_state: &AdapterState,
    session_id: &str,
) -> Option<String> {
    adapter_state
        .session_contexts
        .get(session_id)
        .and_then(|context| context.thread_title.as_deref())
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

async fn session_title_from_shared_state(
    shared_state: &AdapterSharedState,
    session_id: &str,
) -> Option<String> {
    shared_state
        .session_contexts
        .lock()
        .await
        .get(session_id)
        .and_then(|context| context.thread_title.as_deref())
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

async fn publish_focus_title_update(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
) -> Result<()> {
    let den_session = den_get_acp_session(http, config, session_id).await.ok();
    let mut title = den_session
        .as_ref()
        .and_then(den_session_display_title)
        .or_else(|| session_title_from_adapter_state(adapter_state, session_id));
    if title.is_none() {
        title = session_title_from_shared_state(shared_state, session_id).await;
    }
    let Some(title) = project_focused_acp_title(title) else {
        return Ok(());
    };
    let updated_at = den_session.as_ref().and_then(|session| {
        session
            .get("conversation_title_updated_at")
            .or_else(|| session.get("title_updated_at"))
            .or_else(|| session.get("updated_at"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    if let Some(context) = shared_state
        .session_contexts
        .lock()
        .await
        .get_mut(session_id)
    {
        context.thread_title = Some(title.clone());
    }
    send_session_info_update(session_id, Some(title), updated_at).await
}

fn confirmed_focus_run_id(result: &Value) -> Result<&str> {
    let binding = result
        .get("pair_binding")
        .ok_or_else(|| anyhow!("Docket response omitted Pair execution result"))?;
    let control = binding
        .get("control")
        .ok_or_else(|| anyhow!("Docket response omitted canonical attempt state"))?;
    let launch_state = control.get("launch_state").and_then(Value::as_str);
    if control.get("kind").and_then(Value::as_str) != Some("docket")
        || control.get("state").and_then(Value::as_str) != Some("running")
        || control.get("attempt_state").and_then(Value::as_str) != Some("running")
        || !matches!(launch_state, Some("started" | "already_running"))
    {
        return Err(anyhow!(
            "Docket did not start a running canonical attempt and native Pair run: {}",
            compact_json_for_status(binding)
        ));
    }
    control
        .get("attempt_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| anyhow!("Docket execution result omitted its canonical attempt id"))?;
    binding
        .get("run")
        .and_then(|run| run.get("id"))
        .and_then(Value::as_str)
        .filter(|run_id| !run_id.trim().is_empty())
        .ok_or_else(|| anyhow!("Docket execution result omitted its Pair run id"))
}

async fn focus_job_report(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    job_id: String,
    launched: &mut Option<Value>,
) -> String {
    match bearwire::rpc_call(
        http,
        config,
        "docket.jobs.execute",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "job_id": job_id.clone(),
        }),
    )
    .await
    {
        Ok(result) => {
            let run_id = match confirmed_focus_run_id(&result) {
                Ok(run_id) => run_id.to_string(),
                Err(err) => {
                    return format!(
                        "Den ACP /focus did not start Docket execution for job {job_id}: {err:#}\n\nThe task may still be selected, but no running attempt was confirmed. Retry /focus; an ordinary prompt is not a substitute."
                    );
                }
            };
            if let Err(err) = publish_focus_title_update(
                http,
                config,
                adapter_state,
                shared_state,
                session_id,
            )
            .await
            {
                if bear_debug_verbose() {
                    eprintln!(
                        "bear-armature: failed to publish /focus title update session_id={} error={err:#}",
                        session_id
                    );
                }
            }
            let report = format!(
                "Docket execution started\n\n- Job: {job_id}\n- Pair run: {run_id}"
            );
            *launched = Some(result);
            report
        },
        Err(err) => format!(
            "Den ACP /focus could not start focus for job {job_id}: {err:#}\n\nRetry after reconnecting if this session was opened before the latest Den/armature deploy."
        ),
    }
}

async fn focus_report(
    http: Option<&reqwest::Client>,
    config: Option<&Config>,
    adapter_state: &AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    prompt: &str,
    launched: &mut Option<Value>,
) -> String {
    match focus_prompt_target(prompt) {
        FocusPromptTarget::JobId(job_id) => {
            let (Some(http), Some(config)) = (http, config) else {
                return den_required_slash_command_unavailable(LocalSlashCommand::Focus);
            };
            focus_job_report(
                http,
                config,
                adapter_state,
                shared_state,
                session_id,
                job_id,
                launched,
            )
            .await
        }
        FocusPromptTarget::JobIdPrefix(prefix) => {
            let (Some(http), Some(config)) = (http, config) else {
                return den_required_slash_command_unavailable(LocalSlashCommand::Focus);
            };
            match den_list_docket_jobs_for_session(http, config, session_id).await {
                Ok(jobs) => match jobs
                    .iter()
                    .filter(|job| job.id.starts_with(&prefix))
                    .collect::<Vec<_>>()
                    .as_slice()
                {
                    [job] => focus_job_report(
                        http,
                        config,
                        adapter_state,
                        shared_state,
                        session_id,
                        job.id.clone(),
                        launched,
                    )
                    .await,
                    [] => format!(
                        "Den ACP /focus found no Docket Job matching prefix {prefix}. Use /focus <job_id> to provide a full job UUID."
                    ),
                    _ => format!(
                        "Den ACP /focus found multiple Docket Jobs matching prefix {prefix}. Use a longer prefix."
                    ),
                },
                Err(err) => format!(
                    "Den ACP /focus could not list this conversation's Docket Jobs to resolve prefix {prefix}: {err:#}\n\nUse a full job UUID, or reconnect this ACP session and retry."
                ),
            }
        }
        FocusPromptTarget::Invalid => {
            "Den ACP /focus needs zero arguments or exactly one Docket job UUID (or an 8+ character UUID prefix): /focus [job_id]"
                .to_string()
        }
        FocusPromptTarget::ConversationAssociated => {
            let (Some(http), Some(config)) = (http, config) else {
                return den_required_slash_command_unavailable(LocalSlashCommand::Focus);
            };
            match den_list_docket_jobs_for_session(http, config, session_id).await {
                Ok(jobs) => match focus_noncompleted_jobs(&jobs).as_slice() {
                    [job] => focus_job_report(
                        http,
                        config,
                        adapter_state,
                        shared_state,
                        session_id,
                        job.id.clone(),
                        launched,
                    )
                    .await,
                    [] => "Den ACP /focus found no non-completed Job-backed task list associated with this conversation. Use /focus <job_id>, or create a durable Job before focusing."
                        .to_string(),
                    many => {
                        let choices = focus_choice_jobs(&jobs);
                        format!(
                            "Den ACP /focus found multiple non-completed Jobs associated with this conversation. Choose one explicitly:\n\n{}",
                            focus_job_choice_lines(if choices.is_empty() { many } else { &choices })
                        )
                    }
                },
                Err(err) => {
                    let session_state = match den_get_acp_session(http, config, session_id).await {
                        Ok(session_state) => session_state,
                        Err(state_err) => {
                            return format!(
                                "Den ACP /focus could not list this conversation's Docket Jobs: {err:#}\n\nIt also could not inspect this conversation's Den session state: {state_err:#}\n\nBare /focus uses Den's recorded Docket/session projection, not ACP MCP tool registration. Reconnect this ACP session after deploying Den/armature, then retry /focus."
                            );
                        }
                    };
                    let mut job_ids = docket_job_ids_from_den_session_state(&session_state);
                    if job_ids.is_empty() {
                        if let Ok(context) = session_context(adapter_state, session_id) {
                            job_ids = docket_job_ids_from_task_list_status(&context.raw);
                        }
                    }
                    match job_ids.as_slice() {
                        [job_id] => focus_job_report(
                            http,
                            config,
                            adapter_state,
                            shared_state,
                            session_id,
                            job_id.clone(),
                            launched,
                        )
                        .await,
                        [] => format!(
                            "Den ACP /focus could not list this conversation's Docket Jobs: {err:#}\n\nIt found no Job-backed task list associated with this conversation. Use /focus <job_id>, or create a durable Job before focusing."
                        ),
                        many => format!(
                            "Den ACP /focus could not list Docket Job descriptions: {err:#}\n\nIt found multiple Job-backed task lists associated with this conversation. Choose one explicitly:\n\n{}",
                            many.iter()
                                .map(|job_id| format!("- /focus {job_id}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        ),
                    }
                }
            }
        }
    }
}

fn den_required_slash_command_unavailable(command: LocalSlashCommand) -> String {
    let descriptor = local_slash_descriptor_for_command(command);
    let name = descriptor
        .map(|descriptor| descriptor.name)
        .unwrap_or("command");
    let requirement = if descriptor.is_some_and(|descriptor| descriptor.den_required) {
        "requires Den"
    } else {
        "needs unavailable Den context"
    };
    format!(
        "BEARS ACP /{name} {requirement}, but the adapter is not configured for Den right now. Use /status for adapter-local diagnostics."
    )
}

fn client_context_for_doctor(adapter_state: &AdapterState, session_id: &str) -> SessionContext {
    adapter_state
        .session_contexts
        .get(session_id)
        .cloned()
        .unwrap_or_default()
}

fn conversation_report(adapter_state: &AdapterState, session_id: &str) -> String {
    let context = client_context_for_doctor(adapter_state, session_id);
    format!(
        "BEARS ACP conversation\n\n- ACP session: {session_id}\n- cwd: {}\n- roots: {}\n- conversation_id: {}\n- resolved_conversation_id: {}",
        context.cwd,
        if context.roots.is_empty() {
            "<none>".to_string()
        } else {
            context.roots.join(", ")
        },
        context.conversation_id.as_deref().unwrap_or("<none>"),
        context
            .resolved_conversation_id
            .as_deref()
            .unwrap_or("<none>"),
    )
}

fn descriptor_source_counts(descriptors: &[Value]) -> Value {
    let mut counts = std::collections::BTreeMap::new();
    for descriptor in descriptors {
        let source = descriptor
            .pointer("/x_bears/source")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        *counts.entry(source).or_insert(0usize) += 1;
    }
    json!(counts)
}

fn browser_tool_source_summary(context: &SessionContext) -> Value {
    let client_tools = context
        .raw
        .pointer("/mcp/client_tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_client_forwarded = client_tools.iter().any(|tool| {
        tool.pointer("/x_bears/source")
            .and_then(Value::as_str)
            .is_some_and(|source| source == "client_forwarded")
    });
    let has_host_bridge = client_tools.iter().any(|tool| {
        tool.pointer("/x_bears/source")
            .and_then(Value::as_str)
            .is_some_and(|source| source == "host_browser_bridge")
    });
    let chrome_available = chrome_tools_available();
    let active_source = if has_client_forwarded {
        "client_forwarded_mcp"
    } else if has_host_bridge {
        "host_browser_bridge"
    } else if chrome_available {
        "local_chrome_fallback"
    } else {
        "none"
    };
    let unavailable_reason = if active_source == "none" {
        Some(chrome_capability_status_line())
    } else {
        None
    };
    json!({
        "active_source": active_source,
        "total_client_tools": client_tools.len(),
        "source_counts": descriptor_source_counts(&client_tools),
        "client_forwarded_mcp_tools": has_client_forwarded,
        "host_browser_bridge_tools": has_host_bridge,
        "local_chrome_fallback_available": chrome_available,
        "chrome_capability": chrome_capability_status_line(),
        "host_browser_bridge_env": host_browser_bridge_env_summary(),
        "unavailable_reason": unavailable_reason,
    })
}

fn capabilities_report(adapter_state: &AdapterState) -> String {
    let context = SessionContext {
        raw: json!({}),
        ..Default::default()
    };
    let adapter = adapter_capabilities_context();
    let direct_tools = direct_tools_context();
    let browser_source = browser_tool_source_summary(&context);
    let host_bridge_env = host_browser_bridge_env_summary();
    format!(
        "BEARS ACP capabilities\n\nAdapter:\n{}\n\nClient capabilities:\n{}\n\nAdapter direct tools:\n{}\n\nBrowser tool source:\n{}\n\nHost browser bridge env:\n{}",
        serde_json::to_string_pretty(&adapter).unwrap_or_else(|_| adapter.to_string()),
        serde_json::to_string_pretty(&adapter_state.client_capabilities)
            .unwrap_or_else(|_| adapter_state.client_capabilities.to_string()),
        serde_json::to_string_pretty(&direct_tools).unwrap_or_else(|_| direct_tools.to_string()),
        serde_json::to_string_pretty(&browser_source)
            .unwrap_or_else(|_| browser_source.to_string()),
        serde_json::to_string_pretty(&host_bridge_env)
            .unwrap_or_else(|_| host_bridge_env.to_string()),
    )
}

fn render_status_report(environment: &Value, tasks: &[tool_tasks::ToolTaskRecord]) -> String {
    let mut lines = vec!["BEARS ACP status".to_string(), String::new()];
    lines.push(format!(
        "- Overall: {}",
        environment
            .pointer("/diagnostics/status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    ));
    lines.push(format!(
        "- Runtime: {} {}",
        environment
            .pointer("/runtime/kind")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        environment
            .pointer("/runtime/version")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>")
    ));
    lines.push(format!(
        "- ACP session: {}",
        environment
            .pointer("/session/id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>")
    ));
    lines.push(format!(
        "- Conversation: {}",
        environment
            .pointer("/session/resolved_conversation_id")
            .and_then(Value::as_str)
            .or_else(|| environment
                .pointer("/session/conversation_id")
                .and_then(Value::as_str))
            .unwrap_or("<den-selected>")
    ));
    let den = environment.pointer("/services/den").unwrap_or(&Value::Null);
    lines.push(format!("- Den: {}", compact_json_for_status(den)));
    if tasks.is_empty() {
        lines.push("- Adapter-local tools: none active".to_string());
    } else {
        lines.push(format!("- Adapter-local tools: {} active", tasks.len()));
        for task in tasks.iter().take(5) {
            lines.push(format!(
                "  - {} {} phase={} elapsed_ms={}",
                task.tool_name,
                task.tool_call_id,
                task.phase.as_str(),
                task.started_at.elapsed().as_millis(),
            ));
        }
    }
    lines.push(format!(
        "- Browser: {}",
        compact_json_for_status(environment.pointer("/browser").unwrap_or(&Value::Null))
    ));
    lines.push(format!(
        "- MCP: {}",
        compact_json_for_status(
            environment
                .pointer("/environment_variants/acp_adapter/session_mcp")
                .unwrap_or(&Value::Null)
        )
    ));
    if let Some(warnings) = environment
        .pointer("/diagnostics/warnings")
        .and_then(Value::as_array)
    {
        for warning in warnings.iter().take(3) {
            if let Some(text) = warning.as_str() {
                lines.push(format!("- Warning: {text}"));
            }
        }
    }
    lines.join("\n")
}

async fn status_report(
    http: Option<&reqwest::Client>,
    config: Option<&Config>,
    adapter_state: &AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
) -> String {
    let environment = match collect_bear_environment(
        adapter_state,
        session_id,
        config,
        http,
        &json!({
            "include_session_mcp": true,
            "inspect_den": true
        }),
    )
    .await
    {
        Ok(environment) => environment,
        Err(err) => json!({
            "runtime": { "kind": "acp_adapter", "version": adapter_version() },
            "session": { "id": session_id },
            "services": { "den": { "status": "unavailable", "error": format!("{err:#}") } },
            "browser": Value::Null,
            "environment_variants": { "acp_adapter": { "session_mcp": Value::Null } },
            "diagnostics": {
                "status": "degraded",
                "warnings": [format!("Could not collect full bear environment: {err:#}")],
                "errors": [format!("{err:#}")]
            }
        }),
    };
    let tasks = shared_state.tool_tasks.list_for_session(session_id).await;
    let mut report = render_status_report(&environment, &tasks);
    report.push_str("\n- Debug: ");
    report.push_str(bear_debug_mode().as_str());
    if let (Some(http), Some(config)) = (http, config) {
        match timeout(
            LOCAL_DEN_INSPECTION_TIMEOUT,
            fetch_den_runtime_state(http, config, session_id),
        )
        .await
        {
            Ok(Ok(runtime_state)) => {
                for line in render_den_runtime_status(&runtime_state) {
                    report.push('\n');
                    report.push_str(&line);
                }
            }
            Ok(Err(err)) => {
                report.push_str(&format!("\n- Run: unavailable ({err:#})"));
            }
            Err(_) => {
                report.push_str(&format!(
                    "\n- Run: unavailable (timed out after {}ms)",
                    LOCAL_DEN_INSPECTION_TIMEOUT.as_millis()
                ));
            }
        }
    } else {
        report.push_str("\n- Run: unavailable (adapter is not configured for Den)");
    }
    let bearwire_status = if let (Some(http), Some(config)) = (http, config) {
        bearwire::protocol_status(http, config).await
    } else {
        format!("not configured; {}", bearwire::mode_summary())
    };
    report.push_str("\n- BearWire: ");
    report.push_str(&bearwire_status);
    report
}

fn compact_json_for_status(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    truncate_for_log(&text, 600)
}

fn status_scalar(value: &Value, path: &str) -> Option<String> {
    match value.pointer(path)? {
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Null => None,
        other => Some(compact_json_for_status(other)),
    }
}

fn render_den_runtime_status(runtime_state_response: &Value) -> Vec<String> {
    let Some(session) = runtime_state_response.pointer("/session") else {
        return vec!["- Run: unavailable (no BearWire session state)".to_string()];
    };
    let live = status_scalar(session, "/diagnostics/runtime_session_live")
        .unwrap_or_else(|| "unknown".to_string());
    let Some(runtime) = session.pointer("/diagnostics/runtime_state") else {
        return vec![format!("- Run: live={live} runtime_state=<none>")];
    };
    let run_id = status_scalar(runtime, "/run/run_id").unwrap_or_else(|| "<none>".to_string());
    let stance = status_scalar(runtime, "/run/stance").unwrap_or_else(|| "unknown".to_string());
    let governance =
        status_scalar(runtime, "/run/governance").unwrap_or_else(|| "unknown".to_string());
    let orientation = status_scalar(runtime, "/run/objective_orientation_kind")
        .unwrap_or_else(|| "unknown".to_string());
    let focused_job =
        status_scalar(runtime, "/run/focused_job_id").unwrap_or_else(|| "<none>".to_string());
    let loop_level = status_scalar(runtime, "/agent_loop_control/level")
        .unwrap_or_else(|| "unknown".to_string());
    let active_execution = session.pointer("/diagnostics/active_docket_execution");
    let execution_job = active_execution.and_then(|execution| status_scalar(execution, "/job_id"));
    let execution_task =
        active_execution.and_then(|execution| status_scalar(execution, "/task_id"));
    let task_active = status_scalar(runtime, "/task_focus/active")
        .or_else(|| execution_job.as_ref().map(|_| "true".to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    let next_task = status_scalar(runtime, "/task_focus/next_incomplete_task_title")
        .or_else(|| {
            execution_task
                .as_ref()
                .map(|task_id| format!("task {task_id}"))
        })
        .unwrap_or_else(|| "<none>".to_string());
    let docket_job = status_scalar(runtime, "/docket/active_job_id")
        .or(execution_job)
        .unwrap_or_else(|| "<none>".to_string());
    let docket_task = status_scalar(runtime, "/docket/active_task_id")
        .or(execution_task)
        .unwrap_or_else(|| "<none>".to_string());
    let docket_source = status_scalar(runtime, "/docket/source")
        .or_else(|| active_execution.map(|_| "docket_execution_session".to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    vec![
        format!(
            "- Run: live={live} id={run_id} stance={stance} governance={governance} orientation={orientation} focused_job={focused_job} loop={loop_level}"
        ),
        format!("- Focus: active={task_active} next={next_task}"),
        format!("- Docket: job={docket_job} task={docket_task} source={docket_source}"),
    ]
}

async fn runtime_report(
    http: Option<&reqwest::Client>,
    config: Option<&Config>,
    adapter_state: &AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
) -> String {
    let context = client_context_for_doctor(adapter_state, session_id);
    let browser_source = browser_tool_source_summary(&context);
    let host_bridge_env = host_browser_bridge_env_summary();
    let mut lines = vec!["BEARS ACP runtime".to_string(), String::new()];
    lines.push("Browser tools:".to_string());
    lines.push(
        serde_json::to_string_pretty(&browser_source)
            .unwrap_or_else(|_| browser_source.to_string()),
    );
    lines.push(String::new());
    lines.push("Host browser bridge env:".to_string());
    lines.push(
        serde_json::to_string_pretty(&host_bridge_env)
            .unwrap_or_else(|_| host_bridge_env.to_string()),
    );
    lines.push(String::new());
    lines.push("Session MCP state:".to_string());
    lines.push(
        serde_json::to_string_pretty(context.raw.get("mcp").unwrap_or(&Value::Null))
            .unwrap_or_else(|_| context.raw.get("mcp").unwrap_or(&Value::Null).to_string()),
    );
    lines.push(String::new());
    if let (Some(http), Some(config)) = (http, config) {
        match timeout(
            LOCAL_DEN_INSPECTION_TIMEOUT,
            fetch_den_runtime_state(http, config, session_id),
        )
        .await
        {
            Ok(Ok(value)) => {
                lines.push("Den runtime state:".to_string());
                lines.push(
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
                );
            }
            Ok(Err(err)) => {
                lines.push(format!("Den runtime state unavailable: {err:#}"));
            }
            Err(_) => {
                lines.push(format!(
                    "Den runtime state unavailable: timed out after {}ms",
                    LOCAL_DEN_INSPECTION_TIMEOUT.as_millis()
                ));
            }
        }
    } else {
        lines.push("Den runtime state unavailable: adapter is not configured for Den.".to_string());
    }
    lines.push(String::new());
    let tasks = shared_state.tool_tasks.list_for_session(session_id).await;
    if tasks.is_empty() {
        lines.push("No active adapter-local tool tasks for this session.".to_string());
    } else {
        lines.push("Adapter-local tool tasks:".to_string());
        for task in tasks {
            lines.push(format!(
                "- {} {} phase={} elapsed_ms={}",
                task.tool_name,
                task.tool_call_id,
                task.phase.as_str(),
                task.started_at.elapsed().as_millis(),
            ));
        }
    }
    lines.join("\n")
}

async fn version_report(http: Option<&reqwest::Client>, config: Option<&Config>) -> String {
    let den = if let (Some(http), Some(config)) = (http, config) {
        match fetch_server_version_for_diagnostics(http, config).await {
            Ok(version) => version.summary(),
            Err(err) => format!("Den server unreachable: {err:#}"),
        }
    } else {
        "Den not configured in this adapter process".to_string()
    };
    let adapter = adapter_capabilities_context();
    let host_bridge_env = host_browser_bridge_env_summary();
    format!(
        "BEARS ACP version\n\nAdapter: version={} git_sha={} built_at_utc={} contract={} v{}\nAdapter metadata:\n{}\n\nHost browser bridge env:\n{}\n\nDen: {}",
        adapter_version(),
        env!("DEN_ACP_ADAPTER_GIT_SHA"),
        env!("DEN_ACP_ADAPTER_BUILT_AT_UTC"),
        DEN_ACP_ADAPTER_CONTRACT_NAME,
        DEN_ACP_ADAPTER_CONTRACT_VERSION,
        serde_json::to_string_pretty(&adapter).unwrap_or_else(|_| adapter.to_string()),
        serde_json::to_string_pretty(&host_bridge_env)
            .unwrap_or_else(|_| host_bridge_env.to_string()),
        den,
    )
}

fn debug_argument_from_prompt(prompt: &str) -> Option<&str> {
    prompt.split_whitespace().nth(1)
}

fn debug_report(arg: Option<&str>) -> String {
    let previous = bear_debug_mode();
    let mut message = String::new();
    if let Some(arg) = arg.map(str::trim).filter(|value| !value.is_empty()) {
        match BearDebugMode::parse(arg) {
            Some(mode) => {
                set_bear_debug_mode(mode);
                message = format!(
                    "Updated BEARS debug mode: {} → {}\n\n",
                    previous.as_str(),
                    mode.as_str()
                );
            }
            None => {
                message = format!(
                    "Unsupported debug mode `{arg}`. Use `/debug off`, `/debug on`, or `/debug verbose`.\n\n"
                );
            }
        }
    }
    let current = bear_debug_mode();
    format!(
        "{message}BEARS debug\n\n- BEAR_DEBUG env default: {}\n- current mode: {}\n- thought messages: {}\n- verbose adapter logs: {}\n\nUse `/debug off`, `/debug on`, or `/debug verbose`.",
        env::var("BEAR_DEBUG").unwrap_or_else(|_| "<unset>".to_string()),
        current.as_str(),
        if current.shows_thoughts() {
            "shown"
        } else {
            "hidden"
        },
        if current.is_verbose() {
            "enabled"
        } else {
            "disabled"
        },
    )
}

async fn acp_doctor_report(
    http: Option<&reqwest::Client>,
    config: Option<&Config>,
    adapter_state: &AdapterState,
    context: &SessionContext,
) -> String {
    let (api_url, bear, den_status, token_status, bearwire_status) =
        if let (Some(http), Some(config)) = (http, config) {
            let den_status = match fetch_server_version_for_diagnostics(http, config).await {
                Ok(version) => version.summary(),
                Err(err) => format!("Den server unreachable: {err:#}"),
            };
            let token_status = match validate_den_code_token_for_diagnostics(http, config).await {
                Ok(()) => "valid for this Bear".to_string(),
                Err(err) => format!("not validated: {err:#}"),
            };
            let bearwire_status = bearwire::protocol_status(http, config).await;
            (
                config.api_url.clone(),
                config.bear.clone(),
                den_status,
                token_status,
                bearwire_status,
            )
        } else {
            (
                "<not configured>".to_string(),
                "<not configured>".to_string(),
                "Den not configured in this adapter process".to_string(),
                "not validated: Den not configured".to_string(),
                format!("not configured; {}", bearwire::mode_summary()),
            )
        };
    format!(
        "BEARS ACP doctor\n\nAdapter:\n- version: {}\n- git_sha: {}\n- built_at_utc: {}\n- contract: {} v{}\n\nDen:\n- api_url: {}\n- bear: {}\n- server: {}\n- token: {}\n- BearWire: {}\n\nClient capabilities:\n{}\n\nSession:\n- cwd: {}\n- roots: {}\n- resolved_conversation_id: {}\n\nDirect tools: {}\n\nBrowser tool source:\n{}\n\nHost browser bridge env:\n{}\n\nSession MCP state:\n{}",
        adapter_version(),
        env!("DEN_ACP_ADAPTER_GIT_SHA"),
        env!("DEN_ACP_ADAPTER_BUILT_AT_UTC"),
        DEN_ACP_ADAPTER_CONTRACT_NAME,
        DEN_ACP_ADAPTER_CONTRACT_VERSION,
        api_url,
        bear,
        den_status,
        token_status,
        bearwire_status,
        serde_json::to_string_pretty(&adapter_state.client_capabilities)
            .unwrap_or_else(|_| adapter_state.client_capabilities.to_string()),
        context.cwd,
        if context.roots.is_empty() {
            "<none>".to_string()
        } else {
            context.roots.join(", ")
        },
        context
            .resolved_conversation_id
            .as_deref()
            .unwrap_or("<none>"),
        serde_json::to_string_pretty(&direct_tools_context())
            .unwrap_or_else(|_| direct_tools_context().to_string()),
        serde_json::to_string_pretty(&browser_tool_source_summary(context))
            .unwrap_or_else(|_| browser_tool_source_summary(context).to_string()),
        serde_json::to_string_pretty(&host_browser_bridge_env_summary())
            .unwrap_or_else(|_| host_browser_bridge_env_summary().to_string()),
        serde_json::to_string_pretty(context.raw.get("mcp").unwrap_or(&Value::Null))
            .unwrap_or_else(|_| context.raw.get("mcp").unwrap_or(&Value::Null).to_string()),
    )
}

async fn check_server_version(http: &reqwest::Client, config: &Config) -> Result<()> {
    let server_version = fetch_server_version(http, config).await?;
    eprintln!(
        "Den server version:\n  service: {}\n  version: {}\n  git_sha: {}\n  built_at_utc: {}",
        server_version.service,
        server_version.version,
        server_version.git_sha,
        server_version.built_at_utc,
    );
    Ok(())
}

async fn run_doctor(http: &reqwest::Client, runtime: &RuntimeConfig) -> Result<()> {
    let mut failed = false;
    eprintln!("BEARS ACP Adapter Doctor\n");
    eprintln!("✓ Adapter binary runs");
    eprintln!("  version: {}", adapter_version());
    eprintln!("  build_git_sha: {}", env!("DEN_ACP_ADAPTER_GIT_SHA"));
    eprintln!("  built_at_utc: {}", env!("DEN_ACP_ADAPTER_BUILT_AT_UTC"));
    eprintln!("  local_head_sha: {}", local_head_sha());
    eprintln!("  os_arch: {} {}", env::consts::OS, env::consts::ARCH);
    if let Ok(exe) = env::current_exe() {
        eprintln!("  executable: {}", exe.display());
    }
    eprintln!("  direct_tools: {}", direct_tools_context());
    eprintln!("  chrome_tools: {}", chrome_capability_status_line());
    eprintln!(
        "  host_browser_bridge_env: {}",
        host_browser_bridge_env_summary()
    );
    eprintln!();
    eprintln!("{}", update_doctor_line(http).await);
    eprintln!();

    if runtime.api_url.trim().is_empty() {
        failed = true;
        eprintln!("✗ DEN_API_URL is missing");
    } else {
        eprintln!("✓ DEN_API_URL is set");
        eprintln!("  {}", runtime.api_url);
    }

    if runtime.bear.trim().is_empty() {
        failed = true;
        eprintln!("✗ BEAR_SLUG is missing");
    } else {
        eprintln!("✓ BEAR_SLUG is set");
        eprintln!("  {}", runtime.bear);
    }

    if runtime.token_env.trim().is_empty() {
        eprintln!("• DEN_TOKEN_ENV is not set; checking DEN_TOKEN/--token directly");
    } else {
        eprintln!("✓ DEN_TOKEN_ENV is set");
        eprintln!("  {}", runtime.token_env);
    }

    if runtime
        .config
        .as_ref()
        .is_some_and(|config| !config.token.is_empty())
    {
        eprintln!("✓ Den bearer token is available");
    } else {
        failed = true;
        eprintln!("✗ Den bearer token is missing");
    }

    if runtime.client.trim().is_empty() {
        eprintln!(
            "• Client label is empty; ACP protocol still works, but set DEN_ACP_CLIENT if you want labeled requests"
        );
    } else {
        eprintln!("✓ Client label: {}", runtime.client);
    }

    if runtime.diagnostics.is_empty() {
        eprintln!("✓ Configuration values are valid");
    } else {
        failed = true;
        eprintln!("✗ Configuration has problems:");
        for diagnostic in &runtime.diagnostics {
            eprintln!("  - {diagnostic}");
        }
    }
    eprintln!();

    if let Some(config) = runtime.config.as_ref() {
        match fetch_server_version(http, config).await {
            Ok(server_version) => {
                eprintln!("✓ Reached BEARS Den server");
                eprintln!("  service: {}", server_version.service);
                eprintln!("  version: {}", server_version.version);
                eprintln!("  git_sha: {}", server_version.git_sha);
                eprintln!("  built_at_utc: {}", server_version.built_at_utc);
            }
            Err(err) => {
                failed = true;
                eprintln!("✗ Could not reach BEARS Den server");
                eprintln!("  {err:#}");
            }
        }
        eprintln!(
            "  BearWire: {}",
            bearwire::protocol_status(http, config).await
        );
    } else {
        eprintln!("• Skipping server reachability check until configuration is fixed");
        eprintln!("  BearWire: not configured; {}", bearwire::mode_summary());
    }
    eprintln!();

    eprintln!("ACP client command:");
    eprintln!("  {}", installed_or_current_command_hint());
    eprintln!();
    eprintln!("Required ACP client environment:");
    let api_url_hint = if runtime.api_url.is_empty() {
        "https://api.bears.example"
    } else {
        &runtime.api_url
    };
    let bear_hint = if runtime.bear.is_empty() {
        "my-bear"
    } else {
        &runtime.bear
    };
    eprintln!("  DEN_API_URL={api_url_hint}");
    eprintln!("  BEAR_SLUG={bear_hint}");
    if runtime.token_env.is_empty() {
        eprintln!("  DEN_TOKEN=...");
    } else {
        eprintln!("  {}=...", runtime.token_env);
        eprintln!("  DEN_TOKEN_ENV={}", runtime.token_env);
    }
    eprintln!();

    if failed {
        eprintln!(
            "Doctor found setup problems. Fix the items marked ✗ above, then run `bear-armature doctor` again."
        );
        std::process::exit(2);
    }

    eprintln!("Setup looks good.");
    Ok(())
}

fn installed_or_current_command_hint() -> String {
    for candidate in [
        "/usr/local/bin/bear-armature",
        "/usr/local/bin/bears-acp-adapter",
    ] {
        let installed = Path::new(candidate);
        if installed.exists() {
            return format!("{candidate} acp");
        }
    }
    env::current_exe()
        .map(|path| format!("{} acp", path.display()))
        .unwrap_or_else(|_| "bear-armature acp".to_string())
}

async fn fetch_server_version(http: &reqwest::Client, config: &Config) -> Result<ServerVersion> {
    let url = format!("{}/version", config.api_url);
    let response = http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("could not fetch Den server version from {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "Den server version check failed with HTTP {status}: {}",
            body.trim()
        ));
    }

    let value: Value = serde_json::from_str(&body).with_context(|| {
        format!(
            "Den server version response from {url} was not JSON: {}",
            body.trim()
        )
    })?;
    Ok(server_version_from_json(&value))
}

async fn fetch_server_version_for_diagnostics(
    http: &reqwest::Client,
    config: &Config,
) -> Result<ServerVersion> {
    match timeout(
        LOCAL_DEN_INSPECTION_TIMEOUT,
        fetch_server_version(http, config),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(anyhow!(
            "timed out after {}ms fetching Den server version",
            LOCAL_DEN_INSPECTION_TIMEOUT.as_millis()
        )),
    }
}

fn server_version_from_json(value: &Value) -> ServerVersion {
    ServerVersion {
        service: value
            .get("service")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        version: value
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        git_sha: value
            .get("git_sha")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        built_at_utc: value
            .get("built_at_utc")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    }
}

fn den_request_context(url: &str) -> String {
    format!(
        "could not connect to the BEARS Den API at {url}. Check that DEN_API_URL is the Den API origin reachable from this editor process, that the API service is running with BearWire routes enabled, and that the network/VPN/firewall permits the connection"
    )
}

#[allow(dead_code)]
fn den_status_error_message(status: reqwest::StatusCode, body: &str) -> String {
    if let Some(message) = den_compatibility_status_message(body) {
        return message;
    }
    let hint = match status.as_u16() {
        401 => {
            "The bearer token was rejected. Check DEN_TOKEN or --token-env and make sure the token is an active Den Code token."
        }
        403 => {
            "The token authenticated but is not allowed to use this bear or armature access. Check bear membership and token scopes."
        }
        404 => {
            "The BearWire endpoint was not found. Check DEN_API_URL, BEAR_SLUG, and that Den is running with RUN_API=true."
        }
        405 => {
            "The server exists but did not accept the BearWire request. Check that DEN_API_URL points to the Den API origin, not the web UI origin or a proxy route with method restrictions."
        }
        429 => "The Den API rate limited this request. Wait and retry, or check service limits.",
        500..=599 => {
            "The Den API returned a server error. Check Den service logs for the request failure."
        }
        _ => {
            "The Den API rejected the prompt request. Check the response body and Den logs for details."
        }
    };

    if body.is_empty() {
        format!("Den API returned HTTP {status}. {hint}")
    } else {
        format!("Den API returned HTTP {status}: {body}. {hint}")
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PromptBlockShape {
    text: usize,
    resource: usize,
    resource_link: usize,
    other: usize,
    human_text: usize,
    human_pasted_debug_text: usize,
    client_resource: usize,
    client_synthetic_context: usize,
    unsupported: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpPromptBlockType {
    Text,
    Resource,
    ResourceLink,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpPromptBlockProvenance {
    HumanText,
    HumanPastedDebugText,
    ClientResource,
    ClientSyntheticContext,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpPromptContextDeliveryPolicy {
    ReferenceOnly,
    DiagnosticOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcpPromptResourceReference {
    block_type: AcpPromptBlockType,
    provenance: AcpPromptBlockProvenance,
    uri: Option<String>,
    name: Option<String>,
    mime_type: Option<String>,
    text_bytes: Option<usize>,
    delivery_policy: AcpPromptContextDeliveryPolicy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AcpPromptContextDiagnostics {
    synthetic_context_omitted: usize,
    unsupported_blocks: usize,
    resource_bodies_not_in_human_message: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AcpPromptContextBundle {
    human_message: String,
    resource_references: Vec<AcpPromptResourceReference>,
    diagnostics: AcpPromptContextDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcpPromptBlockClassification {
    block_type: AcpPromptBlockType,
    provenance: AcpPromptBlockProvenance,
    diagnostic_flags: Vec<&'static str>,
}

impl AcpPromptBlockClassification {
    fn new(block_type: AcpPromptBlockType, provenance: AcpPromptBlockProvenance) -> Self {
        Self {
            block_type,
            provenance,
            diagnostic_flags: Vec::new(),
        }
    }

    fn include_in_human_message(&self) -> bool {
        matches!(
            self.provenance,
            AcpPromptBlockProvenance::HumanText | AcpPromptBlockProvenance::HumanPastedDebugText
        )
    }

    #[cfg(test)]
    fn include_in_display(&self) -> bool {
        self.include_in_human_message()
    }
}

fn prompt_block_shape(params: &Value) -> PromptBlockShape {
    let mut shape = PromptBlockShape::default();
    let Some(prompt) = params.get("prompt").and_then(Value::as_array) else {
        return shape;
    };
    for block in prompt {
        let classification = classify_prompt_block(block);
        match classification.block_type {
            AcpPromptBlockType::Text => shape.text += 1,
            AcpPromptBlockType::Resource => shape.resource += 1,
            AcpPromptBlockType::ResourceLink => shape.resource_link += 1,
            AcpPromptBlockType::Other => shape.other += 1,
        }
        match classification.provenance {
            AcpPromptBlockProvenance::HumanText => shape.human_text += 1,
            AcpPromptBlockProvenance::HumanPastedDebugText => shape.human_pasted_debug_text += 1,
            AcpPromptBlockProvenance::ClientResource => shape.client_resource += 1,
            AcpPromptBlockProvenance::ClientSyntheticContext => shape.client_synthetic_context += 1,
            AcpPromptBlockProvenance::Unsupported => shape.unsupported += 1,
        }
    }
    shape
}

fn classify_prompt_block(block: &Value) -> AcpPromptBlockClassification {
    match block.get("type").and_then(Value::as_str).unwrap_or("") {
        "text" => {
            let provenance = if block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(looks_like_pasted_debug_payload)
            {
                AcpPromptBlockProvenance::HumanPastedDebugText
            } else {
                AcpPromptBlockProvenance::HumanText
            };
            AcpPromptBlockClassification::new(AcpPromptBlockType::Text, provenance)
        }
        "resource" => {
            let mut classification = AcpPromptBlockClassification::new(
                AcpPromptBlockType::Resource,
                AcpPromptBlockProvenance::ClientResource,
            );
            if block
                .get("resource")
                .and_then(|resource| resource.get("text"))
                .and_then(Value::as_str)
                .is_some_and(looks_like_client_synthetic_context)
            {
                classification.provenance = AcpPromptBlockProvenance::ClientSyntheticContext;
                classification
                    .diagnostic_flags
                    .push("likely_client_synthetic_context");
            }
            classification
        }
        "resource_link" => AcpPromptBlockClassification::new(
            AcpPromptBlockType::ResourceLink,
            AcpPromptBlockProvenance::ClientResource,
        ),
        _ => AcpPromptBlockClassification::new(
            AcpPromptBlockType::Other,
            AcpPromptBlockProvenance::Unsupported,
        ),
    }
}

fn looks_like_pasted_debug_payload(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() > 4096 {
        return false;
    }
    trimmed.to_ascii_lowercase().contains("system_alert")
}

fn looks_like_client_synthetic_context(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() > 4096 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.contains("system_alert")
        && (lower.contains("client")
            || lower.contains("summary")
            || lower.contains("synthetic")
            || lower.contains("zed"))
}

fn prompt_context_from_params(params: &Value) -> Result<AcpPromptContextBundle> {
    let prompt = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("session/prompt params missing prompt array"))?;

    let mut human_parts = Vec::new();
    let mut bundle = AcpPromptContextBundle::default();
    for block in prompt {
        let classification = classify_prompt_block(block);
        if classification.include_in_human_message() {
            if let Some(text) = prompt_block_text_for_human_message(block) {
                human_parts.push(text.to_string());
            }
            continue;
        }
        match classification.block_type {
            AcpPromptBlockType::Resource | AcpPromptBlockType::ResourceLink => {
                if classification.provenance == AcpPromptBlockProvenance::ClientSyntheticContext {
                    bundle.diagnostics.synthetic_context_omitted += 1;
                    if let Some(reference) = prompt_resource_reference_from_block(
                        block,
                        &classification,
                        AcpPromptContextDeliveryPolicy::DiagnosticOnly,
                    ) {
                        bundle.resource_references.push(reference);
                    }
                    continue;
                }
                if let Some(reference) = prompt_resource_reference_from_block(
                    block,
                    &classification,
                    AcpPromptContextDeliveryPolicy::ReferenceOnly,
                ) {
                    if reference.text_bytes.is_some() {
                        bundle.diagnostics.resource_bodies_not_in_human_message += 1;
                    }
                    bundle.resource_references.push(reference);
                }
            }
            AcpPromptBlockType::Other => bundle.diagnostics.unsupported_blocks += 1,
            AcpPromptBlockType::Text => {}
        }
    }
    bundle.human_message = human_parts.join("\n\n").trim().to_string();
    Ok(bundle)
}

fn prompt_text_from_params(params: &Value) -> Result<String> {
    require_human_prompt_text(prompt_context_from_params(params)?.human_message)
}

fn require_human_prompt_text(text: String) -> Result<String> {
    if text.is_empty() {
        Err(anyhow!(
            "prompt did not contain supported human-authored text content"
        ))
    } else {
        Ok(text)
    }
}

fn bearwire_prompt_context_from_context(context: &AcpPromptContextBundle) -> Value {
    let references = context
        .resource_references
        .iter()
        .filter(|reference| {
            reference.delivery_policy == AcpPromptContextDeliveryPolicy::ReferenceOnly
        })
        .map(|reference| {
            let label = reference
                .name
                .as_deref()
                .or(reference.uri.as_deref())
                .unwrap_or("unnamed resource");
            json!({
                "label": label,
                "uri": reference.uri,
                "name": reference.name,
                "mime_type": reference.mime_type,
                "embedded_text_bytes": reference.text_bytes,
                "block_type": match reference.block_type {
                    AcpPromptBlockType::Text => "text",
                    AcpPromptBlockType::Resource => "resource",
                    AcpPromptBlockType::ResourceLink => "resource_link",
                    AcpPromptBlockType::Other => "other",
                },
                "provenance": match reference.provenance {
                    AcpPromptBlockProvenance::HumanText => "human_text",
                    AcpPromptBlockProvenance::HumanPastedDebugText => "human_pasted_debug_text",
                    AcpPromptBlockProvenance::ClientResource => "client_resource",
                    AcpPromptBlockProvenance::ClientSyntheticContext => "client_synthetic_context",
                    AcpPromptBlockProvenance::Unsupported => "unsupported",
                },
            })
        })
        .collect::<Vec<_>>();
    if references.is_empty() {
        return Value::Null;
    }
    json!({
        "format": "acp_prompt_context.v1",
        "host_context": {
            "kind": "referenced_resources",
            "delivery": "reference_only",
            "persistence": "not_human_message",
            "resources": references,
        }
    })
}

fn prompt_block_text_for_human_message(block: &Value) -> Option<&str> {
    match block.get("type").and_then(Value::as_str).unwrap_or("") {
        "text" => block.get("text").and_then(Value::as_str),
        _ => None,
    }
}

fn prompt_resource_reference_from_block(
    block: &Value,
    classification: &AcpPromptBlockClassification,
    delivery_policy: AcpPromptContextDeliveryPolicy,
) -> Option<AcpPromptResourceReference> {
    match classification.block_type {
        AcpPromptBlockType::Resource => {
            let resource = block.get("resource")?;
            let text = resource.get("text").and_then(Value::as_str);
            Some(AcpPromptResourceReference {
                block_type: classification.block_type,
                provenance: classification.provenance,
                uri: prompt_string_field(resource, &["uri", "url"])
                    .or_else(|| prompt_string_field(block, &["uri", "url"])),
                name: prompt_string_field(resource, &["name", "title"])
                    .or_else(|| prompt_string_field(block, &["name", "title"])),
                mime_type: prompt_string_field(
                    resource,
                    &["mime_type", "mimeType", "media_type", "mediaType"],
                )
                .or_else(|| {
                    prompt_string_field(
                        block,
                        &["mime_type", "mimeType", "media_type", "mediaType"],
                    )
                }),
                text_bytes: text.map(str::len),
                delivery_policy,
            })
        }
        AcpPromptBlockType::ResourceLink => Some(AcpPromptResourceReference {
            block_type: classification.block_type,
            provenance: classification.provenance,
            uri: prompt_string_field(block, &["uri", "url"]),
            name: prompt_string_field(block, &["name", "title"]),
            mime_type: prompt_string_field(
                block,
                &["mime_type", "mimeType", "media_type", "mediaType"],
            ),
            text_bytes: None,
            delivery_policy,
        }),
        _ => None,
    }
}

fn prompt_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

fn prompt_conversation_id_from_params(params: &Value) -> Option<String> {
    params
        .get("conversation_id")
        .or_else(|| params.get("conversationId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn prompt_conversations_overlap(previous: Option<&str>, next: Option<&str>) -> bool {
    match (previous, next) {
        (Some(previous), Some(next)) => previous == next,
        (None, None) => true,
        // If either side lacks a conversation id, be conservative: the Den session binding may
        // resolve both to the same runtime conversation.
        _ => true,
    }
}

async fn register_prompt_turn_for_session(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    conversation_id_for_turn: Option<String>,
    response: PromptResponseGuard,
) -> Option<ActivePromptTurn> {
    let previous = {
        let mut active = shared_state.active_prompts.lock().await;
        active.insert(
            session_id.to_string(),
            ActivePromptTurn {
                token: turn_token,
                conversation_id: conversation_id_for_turn.clone(),
                response,
            },
        )
    };
    if let Some(previous) = previous.as_ref() {
        if prompt_conversations_overlap(
            previous.conversation_id.as_deref(),
            conversation_id_for_turn.as_deref(),
        ) {
            let _ = shared_state.cancellation_tx.send(CancellationNotice {
                session_id: session_id.to_string(),
                turn_token: Some(previous.token),
                conversation_id: previous.conversation_id.clone(),
                scope: CancellationScope::PromptAndTools,
                origin: CancellationOrigin::Steering,
            });
        }
    }
    previous
}

fn normalize_client_capabilities(mut capabilities: Value) -> Value {
    if !capabilities.is_object() {
        return capabilities;
    }
    let read_text_file = capability_bool(
        &capabilities,
        &[
            "/fs/readTextFile",
            "/fs/read_text_file",
            "/filesystem/readTextFile",
            "/filesystem/read_text_file",
            "/fs/read_text_file/supported",
            "/filesystem/read_text_file/supported",
        ],
    );
    let write_text_file = capability_bool(
        &capabilities,
        &[
            "/fs/writeTextFile",
            "/fs/write_text_file",
            "/filesystem/writeTextFile",
            "/filesystem/write_text_file",
            "/fs/write_text_file/supported",
            "/filesystem/write_text_file/supported",
        ],
    );
    let terminal = capability_bool(
        &capabilities,
        &[
            "/terminal",
            "/terminal/supported",
            "/client/terminal",
            "/client/terminal/supported",
        ],
    );
    if read_text_file || write_text_file {
        if capabilities.get("fs").is_none() {
            capabilities["fs"] = json!({});
        }
        if read_text_file {
            capabilities["fs"]["readTextFile"] = json!(true);
        }
        if write_text_file {
            capabilities["fs"]["writeTextFile"] = json!(true);
        }
    }
    if terminal {
        capabilities["terminal"] = json!(true);
    }
    capabilities
}

fn capability_bool(capabilities: &Value, pointers: &[&str]) -> bool {
    pointers.iter().any(|pointer| {
        capabilities
            .pointer(pointer)
            .map(capability_value_bool)
            .unwrap_or(false)
    })
}

fn capability_value_bool(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Object(map) => map
            .get("supported")
            .or_else(|| map.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        _ => false,
    }
}

fn workspace_roots_from_params(params: &Value) -> Vec<String> {
    let mut roots = Vec::new();
    push_path_value(&mut roots, params.get("workspaceUri"));
    push_path_value(&mut roots, params.get("rootUri"));
    push_path_value(&mut roots, params.get("root_uri"));
    push_path_value(&mut roots, params.get("workspaceRoot"));
    push_path_value(&mut roots, params.get("workspace_root"));
    push_path_value(&mut roots, params.pointer("/workspace/currentDirectory"));
    push_path_value(&mut roots, params.pointer("/workspace/cwd"));
    push_path_value(&mut roots, params.pointer("/workspace/root"));
    push_path_value(&mut roots, params.pointer("/workspace/rootUri"));
    push_path_value(&mut roots, params.pointer("/workspace/root_uri"));
    push_folder_array(&mut roots, params.get("workspaceFolders"));
    push_folder_array(&mut roots, params.get("workspace_folders"));
    push_folder_array(&mut roots, params.get("workspaceRoots"));
    push_folder_array(&mut roots, params.get("workspace_roots"));
    push_folder_array(&mut roots, params.pointer("/workspace/folders"));
    push_folder_array(&mut roots, params.pointer("/workspace/workspaceFolders"));
    push_folder_array(&mut roots, params.pointer("/workspace/workspace_folders"));
    push_folder_array(&mut roots, params.pointer("/workspace/roots"));
    push_folder_array(&mut roots, params.pointer("/workspace/workspaceRoots"));
    push_folder_array(&mut roots, params.pointer("/workspace/workspace_roots"));
    roots.sort();
    roots.dedup();
    roots
}

fn roots_or_cwd(mut roots: Vec<String>, cwd: &str) -> Vec<String> {
    if roots.is_empty() && is_absolute_local_path(cwd) {
        roots.push(cwd.to_string());
    }
    roots
}

fn push_folder_array(roots: &mut Vec<String>, value: Option<&Value>) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        if let Some(object) = item.as_object() {
            for key in ["path", "uri", "rootUri", "root_uri", "root", "cwd"] {
                push_path_value(roots, object.get(key));
            }
        }
        if item.as_str().is_some() {
            push_path_value(roots, Some(item));
        }
    }
}

fn push_path_value(roots: &mut Vec<String>, value: Option<&Value>) {
    if let Some(path) = value
        .and_then(Value::as_str)
        .and_then(file_uri_or_path_to_path)
        .filter(|s| is_absolute_local_path(s))
    {
        roots.push(path);
    }
}

fn prompt_display_text_from_params(params: &Value) -> Option<String> {
    prompt_text_for_display_from_params(params)
        .ok()
        .map(|text| strip_prompt_scaffolding_for_display(&text))
        .filter(|text| !text.trim().is_empty())
}

fn strip_prompt_scaffolding_for_display(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    loop {
        let Some(start) = find_ascii_case_insensitive(rest, "<system-reminder") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_start = &rest[start..];
        let Some(close_start) = find_ascii_case_insensitive(after_start, "</system-reminder>")
        else {
            break;
        };
        let close_len = "</system-reminder>".len();
        rest = &after_start[close_start + close_len..];
    }
    out.trim().to_string()
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let hn = needle.len();
    if hn == 0 || haystack.len() < hn {
        return None;
    }
    let nb = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(hn)
        .position(|w| w.eq_ignore_ascii_case(nb))
}

fn prompt_text_for_display_from_params(params: &Value) -> Result<String> {
    let text = prompt_context_from_params(params)?.human_message;
    if text.is_empty() {
        Err(anyhow!("prompt did not contain displayable text content"))
    } else {
        Ok(text)
    }
}

fn cancellation_matches_turn(
    notice: &CancellationNotice,
    session_id: &str,
    turn_token: Uuid,
    conversation_id: Option<&str>,
) -> bool {
    if notice.session_id != session_id {
        return false;
    }
    if let Some(token) = notice.turn_token {
        if token != turn_token {
            return false;
        }
    }
    if let (Some(expected), Some(actual)) = (notice.conversation_id.as_deref(), conversation_id) {
        if expected != actual {
            return false;
        }
    }
    true
}

enum LeasedToolTaskWaitOutcome<T> {
    ToolFinished(T),
    Cancelled(CancellationNotice),
    LeaseLost(anyhow::Error),
}

enum ToolTaskWaitOutcome<T> {
    ToolFinished(T),
    Cancelled(CancellationNotice),
}

#[derive(Debug)]
struct ToolExecutionLease {
    attempt_token: String,
    renew_after: Duration,
}

fn is_tool_execution_claim_rejection(response: &Value) -> bool {
    response.get("status").and_then(|value| value.as_str()) == Some("claim_rejected")
}

fn parse_tool_execution_lease(response: &Value) -> Result<ToolExecutionLease> {
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!(
            "tool execution claim was rejected: {}",
            response
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ));
    }
    let attempt_token = response
        .get("attempt_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("tool execution claim omitted attempt_token"))?
        .to_string();
    let renew_after_ms = response
        .get("renew_after_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("tool execution claim omitted renew_after_ms"))?;
    Ok(ToolExecutionLease {
        attempt_token,
        renew_after: Duration::from_millis(renew_after_ms),
    })
}

async fn wait_for_leased_tool_future_or_matching_cancellation<F>(
    mut cancellation_rx: broadcast::Receiver<CancellationNotice>,
    session_id: &str,
    turn_token: Uuid,
    conversation_id: Option<&str>,
    config: &Config,
    run_id: &str,
    obligation_id: &str,
    tool_call_id: &str,
    lease: &ToolExecutionLease,
    tool_future: F,
) -> LeasedToolTaskWaitOutcome<F::Output>
where
    F: std::future::Future,
{
    let mut cancellation_closed = false;
    let mut renewal = tokio::time::interval(lease.renew_after);
    renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    renewal.tick().await;
    tokio::pin!(tool_future);
    loop {
        tokio::select! {
            result = &mut tool_future => return LeasedToolTaskWaitOutcome::ToolFinished(result),
            _ = renewal.tick() => {
                match crate::bearwire::renew_tool_execution(
                    config,
                    session_id,
                    run_id,
                    obligation_id,
                    tool_call_id,
                    &lease.attempt_token,
                ).await {
                    Ok(response) if response.get("ok").and_then(Value::as_bool) == Some(true) => {}
                    Ok(response) => return LeasedToolTaskWaitOutcome::LeaseLost(anyhow!(
                        "tool execution lease was lost: {}",
                        response.get("status").and_then(Value::as_str).unwrap_or("unknown")
                    )),
                    Err(err) => {
                        // ponytail: retry transient renewal failures at Den's fixed interval; a prolonged
                        // outage can still expire the lease, at which point renewal/result settlement is
                        // rejected. Upgrade by scheduling against lease_expires_at if tighter timing is needed.
                        tracing::warn!(
                            target: "bear_armature::lifecycle",
                            session_id,
                            run_id,
                            obligation_id,
                            tool_call_id,
                            error = %err,
                            "tool execution lease renewal failed transiently; continuing local wait"
                        );
                    }
                }
            }
            cancelled = cancellation_rx.recv(), if !cancellation_closed => {
                match cancelled {
                    Ok(notice) if cancellation_matches_turn(&notice, session_id, turn_token, conversation_id) => {
                        return LeasedToolTaskWaitOutcome::Cancelled(notice);
                    }
                    Ok(notice) => {
                        eprintln!(
                            "bear-armature: ignored unrelated cancellation notice while local tool was running session_id={} turn_token={} notice_session_id={} notice_turn_token={:?}",
                            session_id,
                            turn_token,
                            notice.session_id,
                            notice.turn_token,
                        );
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        eprintln!(
                            "bear-armature: local tool cancellation receiver lagged session_id={} turn_token={} skipped={}",
                            session_id,
                            turn_token,
                            skipped,
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        cancellation_closed = true;
                    }
                }
            }
        }
    }
}

async fn wait_for_tool_future_or_matching_cancellation<F>(
    mut cancellation_rx: broadcast::Receiver<CancellationNotice>,
    session_id: &str,
    turn_token: Uuid,
    conversation_id: Option<&str>,
    tool_future: F,
) -> ToolTaskWaitOutcome<F::Output>
where
    F: std::future::Future,
{
    let mut cancellation_closed = false;
    tokio::pin!(tool_future);
    loop {
        tokio::select! {
            result = &mut tool_future => return ToolTaskWaitOutcome::ToolFinished(result),
            cancelled = cancellation_rx.recv(), if !cancellation_closed => {
                match cancelled {
                    Ok(notice) if cancellation_matches_turn(&notice, session_id, turn_token, conversation_id) => {
                        return ToolTaskWaitOutcome::Cancelled(notice);
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => cancellation_closed = true,
                }
            }
        }
    }
}

pub(crate) fn spawn_tool_request_task(
    config: Config,
    shared_state: AdapterSharedState,
    session_id: String,
    event: Value,
    turn_token: Uuid,
) {
    // Subscribe before spawning or registering the task so cancellation cannot be lost
    // between task publication and the first poll of its execution future.
    let cancellation_rx = shared_state.cancellation_tx.subscribe();
    tokio::spawn(async move {
        let canonical = match BearWireToolCallRequestData::parse(&event) {
            Ok(canonical) => canonical,
            Err(err) => {
                eprintln!(
                    "bear-armature: malformed canonical tool request session_id={} error={err:#} event={}",
                    session_id,
                    truncate_for_log(&event.to_string(), 400)
                );
                let run_id = event
                    .get("run_id")
                    .or_else(|| event.pointer("/data/run_id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let tool_call_id = event
                    .pointer("/data/tool_call/id")
                    .or_else(|| event.pointer("/data/tool_call_id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if let (Some(run_id), Some(tool_call_id)) = (run_id, tool_call_id) {
                    let tool_name = event
                        .pointer("/data/tool_call/name")
                        .or_else(|| event.pointer("/data/tool_name"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("malformed_tool_request");
                    let payload = json!({
                        "status": "error",
                        "tool_name": tool_name,
                        "content": "Malformed BearWire tool request could not be executed by the armature.",
                        "diagnostic": {
                            "category": "malformed_tool_request",
                            "message": err.to_string(),
                            "event_sample": truncate_for_log(&event.to_string(), 1000),
                        }
                    });
                    if let Err(post_err) = crate::bearwire::post_tool_result(
                        &config,
                        &session_id,
                        run_id,
                        tool_call_id,
                        payload,
                        None,
                    )
                    .await
                    {
                        eprintln!(
                            "bear-armature: failed to post malformed tool request error result session_id={} run_id={} tool_call_id={} error={post_err:#}",
                            session_id,
                            run_id,
                            tool_call_id,
                        );
                    }
                }
                return;
            }
        };
        let tool_call_id = canonical.tool_call.id.clone();
        let tool_name = canonical.tool_call.name.clone();
        let den_owned_display_only = is_den_server_tool_request(&event);
        let execution_ownership = if den_owned_display_only {
            ToolExecutionOwnership::DenDisplayOnly
        } else {
            ToolExecutionOwnership::ArmatureLocal
        };
        if !shared_state
            .tool_tasks
            .try_register(
                &session_id,
                &tool_call_id,
                &tool_name,
                Some(turn_token),
                execution_ownership,
            )
            .await
        {
            tracing::trace!(
                target: "bear_armature::lifecycle",
                session_id = session_id.as_str(),
                tool_call_id = tool_call_id.as_str(),
                tool_name = tool_name.as_str(),
                run_id = event.get("run_id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                "duplicate local tool task ignored"
            );
            return;
        }
        if !den_owned_display_only && canonical.client_obligation_id().is_err() {
            tracing::warn!(
                target: "bear_armature::lifecycle",
                session_id = session_id.as_str(),
                tool_call_id = tool_call_id.as_str(),
                tool_name = tool_name.as_str(),
                "armature-local tool request missing obligation_id; local execution suppressed"
            );
            let _ = shared_state
                .tool_tasks
                .remove(&session_id, &tool_call_id)
                .await;
            return;
        }
        let mut event = event;
        let lease = if den_owned_display_only {
            None
        } else {
            let run_id = event
                .get("run_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("canonical tool request omitted run_id"));
            let claim = match run_id {
                Ok(run_id) => {
                    crate::bearwire::claim_tool_execution(
                        &config,
                        &session_id,
                        run_id,
                        canonical
                            .client_obligation_id()
                            .expect("armature-local tool request was validated above"),
                        &tool_call_id,
                    )
                    .await
                }
                Err(err) => Err(err),
            };
            match claim {
                Ok(response) if response.get("ok").and_then(Value::as_bool) == Some(true) => {
                    match parse_tool_execution_lease(&response) {
                        Ok(lease) => {
                            event["data"]["attempt_token"] = json!(lease.attempt_token);
                            Some(lease)
                        }
                        Err(err) => {
                            tracing::warn!(
                                target: "bear_armature::lifecycle",
                                session_id = session_id.as_str(),
                                tool_call_id = tool_call_id.as_str(),
                                tool_name = tool_name.as_str(),
                                error = %err,
                                "tool execution claim response was invalid; local execution suppressed"
                            );
                            let _ = shared_state
                                .tool_tasks
                                .remove(&session_id, &tool_call_id)
                                .await;
                            return;
                        }
                    }
                }
                Ok(response) if is_tool_execution_claim_rejection(&response) => {
                    tracing::debug!(
                        target: "bear_armature::lifecycle",
                        session_id = session_id.as_str(),
                        tool_call_id = tool_call_id.as_str(),
                        tool_name = tool_name.as_str(),
                        claim_status = "claim_rejected",
                        "tool execution claim rejected; local execution suppressed"
                    );
                    let _ = shared_state
                        .tool_tasks
                        .remove(&session_id, &tool_call_id)
                        .await;
                    return;
                }
                Ok(response) => {
                    tracing::warn!(
                        target: "bear_armature::lifecycle",
                        session_id = session_id.as_str(),
                        tool_call_id = tool_call_id.as_str(),
                        tool_name = tool_name.as_str(),
                        claim_status = response
                            .get("status")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown"),
                        "tool execution claim returned an unexpected rejection; local execution suppressed"
                    );
                    let _ = shared_state
                        .tool_tasks
                        .remove(&session_id, &tool_call_id)
                        .await;
                    return;
                }
                Err(err) => {
                    tracing::warn!(
                        target: "bear_armature::lifecycle",
                        session_id = session_id.as_str(),
                        tool_call_id = tool_call_id.as_str(),
                        tool_name = tool_name.as_str(),
                        error = %err,
                        "tool execution claim failed; local execution suppressed"
                    );
                    let _ = shared_state
                        .tool_tasks
                        .remove(&session_id, &tool_call_id)
                        .await;
                    return;
                }
            }
        };
        tracing::trace!(
            target: "bear_armature::lifecycle",
            session_id = session_id.as_str(),
            tool_call_id = tool_call_id.as_str(),
            tool_name = tool_name.as_str(),
            run_id = event.get("run_id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
            den_owned_display_only,
            "local tool task spawned"
        );
        let mut task_state = AdapterState {
            client_capabilities: shared_state.client_capabilities.lock().await.clone(),
            session_contexts: shared_state.session_contexts.lock().await.clone(),
            transport: shared_state.transport.clone(),
        };
        let tool_future = handle_tool_request_event(
            &config,
            &mut task_state,
            &shared_state,
            &shared_state.tool_tasks,
            &shared_state.mcp_registry,
            &shared_state.approval_cache,
            &session_id,
            &event,
            turn_token,
        );
        let result = if let Some(lease) = lease.as_ref() {
            let run_id = event
                .get("run_id")
                .and_then(Value::as_str)
                .expect("claimed canonical tool request has run_id");
            wait_for_leased_tool_future_or_matching_cancellation(
                cancellation_rx,
                &session_id,
                turn_token,
                None,
                &config,
                run_id,
                canonical
                    .client_obligation_id()
                    .expect("armature-local tool request was validated above"),
                &tool_call_id,
                lease,
                tool_future,
            )
            .await
        } else {
            LeasedToolTaskWaitOutcome::ToolFinished(tool_future.await)
        };
        let result = match result {
            LeasedToolTaskWaitOutcome::ToolFinished(result) => result,
            LeasedToolTaskWaitOutcome::LeaseLost(err) => {
                tracing::warn!(
                    target: "bear_armature::lifecycle",
                    session_id = session_id.as_str(),
                    tool_call_id = tool_call_id.as_str(),
                    tool_name = tool_name.as_str(),
                    error = %err,
                    "stopped waiting for local tool after losing execution lease"
                );
                let _ = shared_state
                    .tool_tasks
                    .remove(&session_id, &tool_call_id)
                    .await;
                return;
            }
            LeasedToolTaskWaitOutcome::Cancelled(_notice) => {
                shared_state
                    .tool_tasks
                    .set_phase(
                        &session_id,
                        &tool_call_id,
                        &tool_name,
                        ToolTaskPhase::Cancelled,
                    )
                    .await;
                log_tool_task_phase(
                    &session_id,
                    &tool_call_id,
                    &tool_name,
                    ToolTaskPhase::Cancelled,
                );
                let local_err = LocalToolError::cancelled(
                    "ACP session was cancelled before local tool completed",
                );
                let _ = post_local_tool_error_result(
                    &config,
                    &shared_state,
                    turn_token,
                    &session_id,
                    &tool_call_id,
                    &tool_name,
                    &event,
                    local_err,
                    std::time::Instant::now(),
                )
                .await;
                tracing::trace!(
                    target: "bear_armature::lifecycle",
                    session_id = session_id.as_str(),
                    tool_call_id = tool_call_id.as_str(),
                    tool_name = tool_name.as_str(),
                    "local tool task cancelled"
                );
                let _ = shared_state
                    .tool_tasks
                    .remove(&session_id, &tool_call_id)
                    .await;
                return;
            }
        };
        if let Err(err) = result {
            eprintln!(
                "bear-armature: local tool task failed session_id={} tool_call_id={} tool_name={} error={err:#}",
                session_id, tool_call_id, tool_name
            );
            tracing::warn!(
                target: "bear_armature::lifecycle",
                session_id = session_id.as_str(),
                tool_call_id = tool_call_id.as_str(),
                tool_name = tool_name.as_str(),
                error = %err,
                "local tool task failed"
            );
            let local_err = LocalToolError::error(format!("local tool task failed: {err:#}"));
            let _ = post_local_tool_error_result(
                &config,
                &shared_state,
                turn_token,
                &session_id,
                &tool_call_id,
                &tool_name,
                &event,
                local_err,
                std::time::Instant::now(),
            )
            .await;
            let _ = shared_state
                .tool_tasks
                .remove(&session_id, &tool_call_id)
                .await;
        } else if den_owned_display_only {
            tracing::trace!(
                target: "bear_armature::lifecycle",
                session_id = session_id.as_str(),
                tool_call_id = tool_call_id.as_str(),
                tool_name = tool_name.as_str(),
                "den-owned display-only tool task finished without posting client result"
            );
            shared_state
                .tool_tasks
                .set_phase(
                    &session_id,
                    &tool_call_id,
                    &tool_name,
                    ToolTaskPhase::ResultPosted,
                )
                .await;
            let _ = shared_state
                .tool_tasks
                .remove(&session_id, &tool_call_id)
                .await;
        } else {
            tracing::trace!(
                target: "bear_armature::lifecycle",
                session_id = session_id.as_str(),
                tool_call_id = tool_call_id.as_str(),
                tool_name = tool_name.as_str(),
                "local tool task finished; retaining request presentation for canonical terminal projection"
            );
        }
    });
}

pub(crate) async fn project_den_owned_tool_request(
    shared_state: &AdapterSharedState,
    session_id: &str,
    event: &Value,
    turn_token: Uuid,
) -> Result<()> {
    let canonical = BearWireToolCallRequestData::parse(event)?;
    let tool_call_id = canonical.tool_call.id.as_str();
    let tool_name = canonical.tool_call.name.as_str();
    if current_surface_tool_status(shared_state, session_id, tool_call_id)
        .await
        .is_some_and(SurfaceToolStatus::is_terminal)
    {
        tracing::trace!(
            target: "bear_armature::lifecycle",
            session_id,
            tool_call_id,
            tool_name,
            "skipping Den-owned display-only tool projection because surface is already terminal"
        );
        return Ok(());
    }
    if !is_current_prompt_turn(
        shared_state,
        session_id,
        turn_token,
        "den_owned_tool_projection",
    )
    .await
    {
        return Ok(());
    }
    if shared_state
        .tool_tasks
        .try_register(
            session_id,
            tool_call_id,
            tool_name,
            Some(turn_token),
            ToolExecutionOwnership::DenDisplayOnly,
        )
        .await
    {
        shared_state
            .tool_tasks
            .set_phase(session_id, tool_call_id, tool_name, ToolTaskPhase::Received)
            .await;
        log_tool_task_phase(session_id, tool_call_id, tool_name, ToolTaskPhase::Received);
        shared_state
            .tool_tasks
            .remember_presentation(
                session_id,
                tool_call_id,
                tool_name,
                canonical.tool_call.arguments.clone(),
                canonical.tool_call.display.clone(),
            )
            .await;
    }
    let preparing = friendly_tool_status(tool_name, event, "preparing");
    send_tool_call_update_for_turn(
        shared_state,
        session_id,
        turn_token,
        tool_call_id,
        tool_name,
        ToolCallUpdatePayload {
            status: "pending",
            text: &preparing,
            request: Some(ToolRequestPresentation::from_event(
                tool_call_id,
                tool_name,
                event,
            )),
            raw_output: None,
            extra_content: Vec::new(),
        },
    )
    .await?;
    let running = friendly_tool_status(tool_name, event, "running");
    send_tool_call_update_for_turn(
        shared_state,
        session_id,
        turn_token,
        tool_call_id,
        tool_name,
        ToolCallUpdatePayload {
            status: "in_progress",
            text: &running,
            request: Some(ToolRequestPresentation::from_event(
                tool_call_id,
                tool_name,
                event,
            )),
            raw_output: None,
            extra_content: Vec::new(),
        },
    )
    .await?;
    shared_state
        .tool_tasks
        .set_phase(
            session_id,
            tool_call_id,
            tool_name,
            ToolTaskPhase::ExecutionStarted,
        )
        .await;
    log_tool_task_phase(
        session_id,
        tool_call_id,
        tool_name,
        ToolTaskPhase::ExecutionStarted,
    );
    Ok(())
}

async fn handle_tool_request_event(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    task_registry: &ToolTaskRegistry,
    mcp_registry: &McpRegistry,
    approval_cache: &ApprovalCache,
    session_id: &str,
    event: &Value,
    turn_token: Uuid,
) -> Result<()> {
    let canonical = BearWireToolCallRequestData::parse(event)?;
    let tool_call_id = canonical.tool_call.id.as_str();
    let tool_name = canonical.tool_call.name.as_str();
    task_registry
        .set_phase(session_id, tool_call_id, tool_name, ToolTaskPhase::Received)
        .await;
    log_tool_task_phase(session_id, tool_call_id, tool_name, ToolTaskPhase::Received);
    let args = canonical.tool_call.arguments.clone();
    task_registry
        .remember_presentation(
            session_id,
            tool_call_id,
            tool_name,
            args.clone(),
            canonical.tool_call.display.clone(),
        )
        .await;
    if is_den_server_tool_request(event) {
        project_den_owned_tool_request(shared_state, session_id, event, turn_token).await?;
        task_registry
            .set_phase(
                session_id,
                tool_call_id,
                tool_name,
                ToolTaskPhase::ResultPosted,
            )
            .await;
        return Ok(());
    }
    let preparing = friendly_tool_status(tool_name, event, "preparing");
    send_detached_tool_call_update_for_turn(
        shared_state,
        session_id,
        turn_token,
        tool_call_id,
        tool_name,
        ToolCallUpdatePayload {
            status: "pending",
            text: &preparing,
            request: Some(ToolRequestPresentation::from_event(
                tool_call_id,
                tool_name,
                event,
            )),
            raw_output: None,
            extra_content: Vec::new(),
        },
    )
    .await?;
    let policy = policy_from_event(event);
    let replace_plan = if tool_name == "fs_edit_file" || tool_name == "fs_replace_text" {
        let context = session_context(adapter_state, session_id)?;
        Some(ReplaceTextPlan::preflight(
            context,
            ReplaceTextArgs::from_value(&args, &policy)?,
            &policy,
        )?)
    } else {
        None
    };
    let context_for_approval = session_context(adapter_state, session_id).ok().cloned();
    let target_path_for_approval = context_for_approval
        .as_ref()
        .and_then(|context| policy_target_path(context, &args, &policy))
        .or_else(|| {
            tool_path(event).and_then(|path| {
                context_for_approval
                    .as_ref()
                    .and_then(|context| resolve_requested_tool_path(context, path).ok())
                    .or_else(|| normalize_requested_tool_path(path).ok())
            })
        });
    let target_url_for_approval = tool_url(event).map(str::to_string);
    let target_command_for_approval = tool_command(event).map(str::to_string);
    let approval_reused = if let Some(context) = context_for_approval.as_ref() {
        approval_cache
            .is_allowed_for_target(
                context,
                tool_name,
                target_path_for_approval.as_deref(),
                target_url_for_approval.as_deref(),
                target_command_for_approval.as_deref(),
            )
            .await
    } else {
        false
    };
    if approval_reused && bear_debug_verbose() {
        let target_label = target_path_for_approval
            .as_ref()
            .map(|path| path.display().to_string())
            .or_else(|| target_url_for_approval.clone())
            .or_else(|| target_command_for_approval.clone())
            .or_else(|| tool_path(event).map(str::to_string))
            .unwrap_or_else(|| "<unknown>".to_string());
        eprintln!(
            "bear-armature: approval_reused session_id={} tool_name={} target={}",
            session_id, tool_name, target_label
        );
    }
    if !approval_reused && approval_required_from_event(event) {
        task_registry
            .set_phase(
                session_id,
                tool_call_id,
                tool_name,
                ToolTaskPhase::PermissionRequested,
            )
            .await;
        log_tool_task_phase(
            session_id,
            tool_call_id,
            tool_name,
            ToolTaskPhase::PermissionRequested,
        );
        let permission = friendly_tool_status(tool_name, event, "permission");
        send_detached_tool_call_update_for_turn(
            shared_state,
            session_id,
            turn_token,
            tool_call_id,
            tool_name,
            ToolCallUpdatePayload {
                status: "pending",
                text: &permission,
                request: Some(ToolRequestPresentation::from_event(
                    tool_call_id,
                    tool_name,
                    event,
                )),
                raw_output: None,
                extra_content: replace_plan
                    .as_ref()
                    .map(|plan| vec![replace_text_diff_content(plan)])
                    .unwrap_or_default(),
            },
        )
        .await?;
        let replace_plan_ref = replace_plan.as_ref();
        let permission_decision = request_tool_permission(
            adapter_state,
            session_id,
            PermissionRequestContext {
                tool_call_id,
                tool_name,
                event,
                replace_plan: replace_plan_ref,
                policy: &policy,
                context: context_for_approval.as_ref(),
                target_path: target_path_for_approval.as_deref(),
                target_url: target_url_for_approval.as_deref(),
                target_command: target_command_for_approval.as_deref(),
            },
        )
        .await;
        if let Err(err) = permission_decision {
            let message = format!("{err:#}");
            let local_err = if message.contains("timed out waiting for client response") {
                task_registry
                    .set_phase(
                        session_id,
                        tool_call_id,
                        tool_name,
                        ToolTaskPhase::PermissionTimeout,
                    )
                    .await;
                log_tool_task_phase(
                    session_id,
                    tool_call_id,
                    tool_name,
                    ToolTaskPhase::PermissionTimeout,
                );
                LocalToolError::timeout(message)
            } else {
                task_registry
                    .set_phase(
                        session_id,
                        tool_call_id,
                        tool_name,
                        ToolTaskPhase::PermissionDenied,
                    )
                    .await;
                log_tool_task_phase(
                    session_id,
                    tool_call_id,
                    tool_name,
                    ToolTaskPhase::PermissionDenied,
                );
                LocalToolError::permission_denied(message)
            };
            post_local_tool_error_result(
                config,
                shared_state,
                turn_token,
                session_id,
                tool_call_id,
                tool_name,
                event,
                local_err,
                std::time::Instant::now(),
            )
            .await?;
            return Ok(());
        }
        task_registry
            .set_phase(
                session_id,
                tool_call_id,
                tool_name,
                ToolTaskPhase::PermissionGranted,
            )
            .await;
        if permission_decision
            .as_ref()
            .is_ok_and(|decision| decision.remember)
        {
            if let Some(context) = context_for_approval.as_ref() {
                let scope = permission_decision
                    .as_ref()
                    .map(|decision| decision.scope)
                    .unwrap_or(ApprovalScope::Workspace);
                approval_cache
                    .remember_for_target(
                        context,
                        tool_name,
                        policy.risk(),
                        scope,
                        ApprovalTarget {
                            path: target_path_for_approval.as_deref(),
                            url: target_url_for_approval.as_deref(),
                            command: target_command_for_approval.as_deref(),
                        },
                    )
                    .await;
                eprintln!(
                    "bear-armature: approval_remembered session_id={} tool_name={} scope={}",
                    session_id,
                    tool_name,
                    scope.as_str()
                );
            }
        }
        log_tool_task_phase(
            session_id,
            tool_call_id,
            tool_name,
            ToolTaskPhase::PermissionGranted,
        );
    }
    let running = friendly_tool_status(tool_name, event, "running");
    send_detached_tool_call_update_for_turn(
        shared_state,
        session_id,
        turn_token,
        tool_call_id,
        tool_name,
        ToolCallUpdatePayload {
            status: "pending",
            text: &running,
            request: Some(ToolRequestPresentation::from_event(
                tool_call_id,
                tool_name,
                event,
            )),
            raw_output: None,
            extra_content: Vec::new(),
        },
    )
    .await?;
    let started = std::time::Instant::now();
    task_registry
        .set_phase(
            session_id,
            tool_call_id,
            tool_name,
            ToolTaskPhase::ExecutionStarted,
        )
        .await;
    log_tool_task_phase(
        session_id,
        tool_call_id,
        tool_name,
        ToolTaskPhase::ExecutionStarted,
    );
    if tool_name == "session_info" {
        let local_err = LocalToolError::error(
            "Den routed server-side tool `session_info` to the ACP adapter unexpectedly; this tool must be executed inside Den.".to_string(),
        );
        task_registry
            .set_phase(
                session_id,
                tool_call_id,
                tool_name,
                ToolTaskPhase::ExecutionFailed,
            )
            .await;
        log_tool_task_phase(
            session_id,
            tool_call_id,
            tool_name,
            ToolTaskPhase::ExecutionFailed,
        );
        let mut payload = json!({
            "turn_id": event.get("turn_id").and_then(Value::as_str),
            "request_id": event.get("request_id").and_then(Value::as_str),
            "tool_call_id": tool_call_id,
            "run_id": event.get("run_id").and_then(Value::as_str),
            "approval_request_id": event.get("approval_request_id").and_then(Value::as_str),
            "tool_name": tool_name,
            "status": local_err.status_str(),
            "content": local_err.message,
            "structured_content": {},
            "diagnostic": {
                "component": "bear-armature",
                "adapter_version": adapter_version(),
                "phase": "unexpected_den_server_tool_routed_to_adapter",
                "session_id": session_id,
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "duration_ms": started.elapsed().as_millis(),
            }
        });
        merge_diagnostic(&mut payload["diagnostic"], local_err.diagnostic);
        if let Err(err) = post_tool_result(config, session_id, tool_call_id, payload).await {
            if is_turn_missing_error(&err) {
                eprintln!(
                    "bear-armature: late unexpected server-tool result ignored because Den turn is gone session_id={} tool_call_id={} tool_name={} error={:#}",
                    session_id, tool_call_id, tool_name, err
                );
                return Ok(());
            }
            return Err(err);
        }
        task_registry
            .set_phase(
                session_id,
                tool_call_id,
                tool_name,
                ToolTaskPhase::ResultPosted,
            )
            .await;
        log_tool_task_phase(
            session_id,
            tool_call_id,
            tool_name,
            ToolTaskPhase::ResultPosted,
        );
        return Ok(());
    }

    let result = if let Some(ref plan) = replace_plan {
        let context = session_context(adapter_state, session_id)?;
        plan.apply(context, &policy)
    } else if tool_name == "terminal_run_command" {
        let context = session_context(adapter_state, session_id)?.clone();
        handle_terminal_run_command(
            adapter_state,
            &context,
            session_id,
            Some(tool_call_id),
            Some(tool_call_title(tool_name, event)),
            &args,
            &policy,
            TerminalCommandValidation::Allowlisted,
        )
        .await
    } else if tool_name == "run_command" {
        let context = session_context(adapter_state, session_id)?.clone();
        if client_supports_terminal(adapter_state) && run_command_prefers_terminal(&args) {
            handle_terminal_run_command(
                adapter_state,
                &context,
                session_id,
                Some(tool_call_id),
                Some(tool_call_title(tool_name, event)),
                &args,
                &policy,
                TerminalCommandValidation::Generic,
            )
            .await
        } else {
            handle_process_run(&context, session_id, &args, &policy).await
        }
    } else {
        execute_local_tool(
            config,
            adapter_state,
            mcp_registry,
            session_id,
            tool_name,
            args,
            &policy,
        )
        .await
    };
    let status;
    let deferred_ui_update: Option<(String, String, Option<Value>, Vec<ToolCallContent>)>;
    let mut payload = json!({
        "turn_id": event.get("turn_id").and_then(Value::as_str),
        "request_id": event.get("request_id").and_then(Value::as_str),
        "tool_call_id": tool_call_id,
        "run_id": event.get("run_id").and_then(Value::as_str),
        "approval_request_id": event.get("approval_request_id").and_then(Value::as_str),
        "tool_name": tool_name,
        "attempt_token": event.pointer("/data/attempt_token").and_then(Value::as_str),
        "diagnostic": {
            "component": "bear-armature",
            "adapter_version": adapter_version(),
            "phase": "adapter_execution_started",
            "session_id": session_id,
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "duration_ms": started.elapsed().as_millis(),
        }
    });
    match result {
        Ok(value) => {
            status = "ok";
            if matches!(tool_name, "update_task_list" | "update_plan") {
                let entries = value
                    .get("plan")
                    .map(plan_entries_from_work_plan_args)
                    .unwrap_or_default();
                let _projection = shared_state
                    .projection_dispatcher
                    .begin_detached_projection(session_id, turn_token)
                    .await;
                send_plan_update(session_id, entries).await?;
            }
            if let Some(mode) = value.get("mode_update").and_then(Value::as_str) {
                if matches!(mode, MODE_ASK | MODE_PLAN | MODE_WRITE) {
                    let _projection = shared_state
                        .projection_dispatcher
                        .begin_detached_projection(session_id, turn_token)
                        .await;
                    notify_mode_state(session_id, mode).await?;
                }
            }
            task_registry
                .set_phase(
                    session_id,
                    tool_call_id,
                    tool_name,
                    ToolTaskPhase::ExecutionSucceeded,
                )
                .await;
            log_tool_task_phase(
                session_id,
                tool_call_id,
                tool_name,
                ToolTaskPhase::ExecutionSucceeded,
            );
            payload["status"] = json!(status);
            let preview = tool_completion_preview(tool_name, &value);
            payload["content"] = value.get("content").cloned().unwrap_or_else(|| json!(""));
            let raw_output = value.clone();
            let extra_content = if let Some(plan) = replace_plan.as_ref() {
                vec![replace_text_diff_content(plan)]
            } else if tool_name == "fs_create_text_file" {
                create_text_file_diff_content(event).into_iter().collect()
            } else {
                Vec::new()
            };
            payload["structured_content"] = value;
            deferred_ui_update = Some((
                "completed".to_string(),
                preview,
                Some(raw_output),
                extra_content,
            ));
        }
        Err(err) => {
            let local_err = LocalToolError::from(err);
            status = local_err.status_str();
            task_registry
                .set_phase(
                    session_id,
                    tool_call_id,
                    tool_name,
                    ToolTaskPhase::ExecutionFailed,
                )
                .await;
            log_tool_task_phase(
                session_id,
                tool_call_id,
                tool_name,
                ToolTaskPhase::ExecutionFailed,
            );
            let message = local_err.message.clone();
            payload["status"] = json!(status);
            payload["content"] = json!(message.clone());
            payload["diagnostic"]["phase"] = json!("adapter_execution_failed");
            merge_diagnostic(&mut payload["diagnostic"], local_err.diagnostic);
            deferred_ui_update = Some(("failed".to_string(), message, None, Vec::new()));
        }
    }
    if let Err(err) = post_tool_result(config, session_id, tool_call_id, payload).await {
        if is_turn_missing_error(&err) {
            eprintln!(
                "bear-armature: late local tool result ignored because Den turn is gone session_id={} tool_call_id={} tool_name={} error={:#}",
                session_id, tool_call_id, tool_name, err
            );
            return Ok(());
        }
        task_registry
            .set_phase(
                session_id,
                tool_call_id,
                tool_name,
                ToolTaskPhase::ResultPostFailed,
            )
            .await;
        log_tool_task_phase(
            session_id,
            tool_call_id,
            tool_name,
            ToolTaskPhase::ResultPostFailed,
        );
        let message = format!("Could not deliver local tool result to Den.\n\n{err:#}");
        let _ = send_detached_tool_call_update_for_turn(
            shared_state,
            session_id,
            turn_token,
            tool_call_id,
            tool_name,
            ToolCallUpdatePayload {
                status: "failed",
                text: &message,
                request: Some(ToolRequestPresentation::from_event(
                    tool_call_id,
                    tool_name,
                    event,
                )),
                raw_output: Some(json!({
                    "component": "bear-armature",
                    "phase": "result_post_failed",
                    "error": format!("{err:#}"),
                })),
                extra_content: Vec::new(),
            },
        )
        .await;
        return Err(err);
    }
    task_registry
        .set_phase(
            session_id,
            tool_call_id,
            tool_name,
            ToolTaskPhase::ResultPosted,
        )
        .await;
    log_tool_task_phase(
        session_id,
        tool_call_id,
        tool_name,
        ToolTaskPhase::ResultPosted,
    );
    if let Some((status, text, raw_output, extra_content)) = deferred_ui_update {
        if let Err(err) = send_detached_tool_call_update_for_turn(
            shared_state,
            session_id,
            turn_token,
            tool_call_id,
            tool_name,
            ToolCallUpdatePayload {
                status: &status,
                text: &text,
                request: Some(ToolRequestPresentation::from_event(
                    tool_call_id,
                    tool_name,
                    event,
                )),
                raw_output,
                extra_content,
            },
        )
        .await
        {
            eprintln!(
                "bear-armature: ACP tool-card update failed after Den accepted result session_id={} tool_call_id={} tool_name={} error={:#}",
                session_id,
                tool_call_id,
                tool_name,
                err
            );
        }
    }
    Ok(())
}

fn command_line_from_value(value: &Value) -> Option<String> {
    let command = value.get("command")?.as_str()?.trim();
    if command.is_empty() {
        return None;
    }
    let args = value
        .get("args")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    Some(if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    })
}

#[derive(Debug, Clone, Deserialize)]
struct BearWireToolCallRequestCard {
    id: String,
    name: String,
    arguments: Value,
    #[serde(default)]
    display: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct BearWireToolCallRequestData {
    #[serde(default)]
    obligation_id: Option<String>,
    tool_call: BearWireToolCallRequestCard,
}

impl BearWireToolCallRequestData {
    fn client_obligation_id(&self) -> Result<&str> {
        self.obligation_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow!("armature-local tool request missing obligation_id"))
    }

    fn parse(event: &Value) -> Result<Self> {
        let data = event
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("BearWire tool request missing data"))?;
        serde_json::from_value(data).context("parse canonical BearWire tool request data")
    }
}

#[derive(Debug, Clone, Deserialize)]
struct BearWirePermissionInfo {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    target: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct BearWireClientWaitingData {
    expected_client_method: String,
    obligation_id: String,
    permission: BearWirePermissionInfo,
    tool_call: BearWireToolCallRequestCard,
}

impl BearWireClientWaitingData {
    fn parse(event: &Value) -> Result<Self> {
        let data = event
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("BearWire client.waiting missing data"))?;
        let parsed: Self =
            serde_json::from_value(data).context("parse canonical BearWire client.waiting data")?;
        if parsed.expected_client_method.trim() != "client.permission.result" {
            bail!("BearWire client.waiting has unsupported expected_client_method");
        }
        if parsed.obligation_id.trim().is_empty() {
            bail!("BearWire client.waiting missing obligation_id");
        }
        if parsed.permission.id.trim().is_empty() {
            bail!("BearWire client.waiting missing permission.id");
        }
        if parsed.tool_call.id.trim().is_empty() {
            bail!("BearWire client.waiting missing tool_call.id");
        }
        if parsed.tool_call.name.trim().is_empty() {
            bail!("BearWire client.waiting missing tool_call.name");
        }
        Ok(parsed)
    }
}

fn tool_args_from_event(event: &Value) -> Option<&Value> {
    event
        .pointer("/data/tool_call/arguments")
        .or_else(|| event.pointer("/data/tool_call/input"))
        .or_else(|| event.pointer("/data/tool_call/raw_input"))
        .or_else(|| event.get("args"))
        .or_else(|| event.get("arguments"))
        .or_else(|| event.get("input"))
        .or_else(|| event.get("raw_input"))
        .or_else(|| event.pointer("/data/arguments"))
        .or_else(|| event.pointer("/data/input"))
        .or_else(|| event.pointer("/data/raw_input"))
}

fn event_display_from_event(event: &Value) -> Option<&Value> {
    event
        .pointer("/data/tool_call/display")
        .or_else(|| event.get("display"))
        .or_else(|| event.pointer("/data/display"))
}

fn approval_required_from_event(event: &Value) -> bool {
    event
        .get("approval")
        .and_then(|approval| approval.get("required"))
        .or_else(|| event.pointer("/data/approval/required"))
        .or_else(|| event.pointer("/data/approval_required"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn policy_target_path(
    context: &SessionContext,
    args: &Value,
    policy: &ToolPolicy,
) -> Option<PathBuf> {
    let target_policy = policy.target_policy.as_ref()?;
    let kind = target_policy.get("kind").and_then(Value::as_str)?;
    let raw_path = match kind {
        "workspace_path" => target_policy
            .get("arg")
            .and_then(Value::as_str)
            .and_then(|arg| args.get(arg))
            .and_then(Value::as_str),
        "workspace_root_or_path" => target_policy
            .get("arg")
            .and_then(Value::as_str)
            .and_then(|arg| args.get(arg))
            .and_then(Value::as_str)
            .or_else(|| {
                target_policy
                    .get("default_to_workspace_root")
                    .and_then(Value::as_bool)
                    .filter(|default| *default)
                    .and_then(|_| {
                        context
                            .roots
                            .first()
                            .map(String::as_str)
                            .or(Some(context.cwd.as_str()))
                    })
            }),
        "source_destination" => target_policy
            .get("source_arg")
            .and_then(Value::as_str)
            .and_then(|arg| args.get(arg))
            .and_then(Value::as_str),
        "command" => target_policy
            .get("cwd_arg")
            .and_then(Value::as_str)
            .and_then(|arg| args.get(arg))
            .and_then(Value::as_str),
        _ => None,
    }?;
    resolve_requested_tool_path(context, raw_path)
        .ok()
        .filter(|path| ensure_path_allowed_for_session(context, path).is_ok())
}

fn approval_reason_from_event(event: &Value) -> Option<&str> {
    event
        .pointer("/data/permission/reason")
        .or_else(|| event.get("reason"))
        .or_else(|| event.pointer("/data/reason"))
        .or_else(|| event.pointer("/data/approval/reason"))
        .or_else(|| event.pointer("/approval/reason"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

const ACP_TOOL_CARD_RAW_OUTPUT_PREVIEW_CHARS: usize = 4 * 1024;
const TOOL_RESULT_POST_SLOW_MS: u128 = 2_000;
const TOOL_RESULT_POST_TIMEOUT_SECS: u64 = 120;

fn compact_tool_card_json_value(value: Value) -> Value {
    Value::String(compact_tool_json_detail(
        &value,
        ACP_TOOL_CARD_RAW_OUTPUT_PREVIEW_CHARS,
    ))
}

pub(crate) fn compact_tool_json_detail(value: &Value, max_chars: usize) -> String {
    let mut text = serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string());
    if text.chars().count() > max_chars {
        text = text.chars().take(max_chars).collect::<String>();
        text.push_str("... truncated");
    }
    text
}

pub(crate) fn tool_completion_preview(tool_name: &str, value: &Value) -> String {
    if matches!(tool_name, "fs_read_text_file" | "fs.read_text_file") {
        return read_text_file_completion_preview(value);
    }
    if matches!(tool_name, "set_conversation_title") {
        if let Some(title) = value
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            return format!("Conversation title set to {}.", markdown_inline_code(title));
        }
    }
    if matches!(
        tool_name,
        "run_command" | "process_run" | "terminal_run_command"
    ) {
        let command = command_line_from_value(value).unwrap_or_else(|| "command".to_string());
        let cwd = value
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("workspace");
        let elapsed = value.get("elapsed_ms").and_then(Value::as_u64);
        let timed_out = value
            .get("timed_out")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let status = if timed_out {
            "timed out".to_string()
        } else if let Some(code) = value.get("exit_code").and_then(Value::as_i64) {
            format!("exit code {code}")
        } else if let Some(signal) = value.get("signal").and_then(Value::as_str) {
            format!("signal {signal}")
        } else {
            "completed".to_string()
        };
        let mut text = format!("`{command}` in `{cwd}` finished with {status}.");
        if let Some(elapsed) = elapsed {
            text.push_str(&format!(" elapsed_ms={elapsed}."));
        }
        if value
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            text.push_str(" Output was truncated.");
        }
        return text;
    }

    if matches!(
        tool_name,
        "request_task_list_handoff" | "request_work_handoff"
    ) {
        return String::new();
    }
    if matches!(tool_name, "update_task_list" | "update_plan") {
        let entries = value
            .get("plan")
            .map(|plan| {
                plan.get("items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| {
                                plan_entry_from_acp_plan_item(item)
                                    .or_else(|| plan_entry_from_work_plan_item(item))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        if entries.is_empty() {
            return "Task list updated.".to_string();
        }
        let completed = entries
            .iter()
            .filter(|entry| entry.status == PlanEntryStatus::Completed)
            .count();
        let in_progress = entries
            .iter()
            .filter(|entry| entry.status == PlanEntryStatus::InProgress)
            .count();
        let pending = entries
            .iter()
            .filter(|entry| entry.status == PlanEntryStatus::Pending)
            .count();
        return format!(
            "Task list updated: {} total, {} in progress, {} pending, {} completed.",
            entries.len(),
            in_progress,
            pending,
            completed
        );
    }

    let content = value
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let mut text = if content.is_empty() {
        if is_placeholder_tool_name(tool_name) && !value.is_null() {
            format!(
                "Tool call completed. Result: `{}`",
                compact_tool_json_detail(value, 1_200)
            )
        } else {
            String::new()
        }
    } else {
        content.to_string()
    };
    let max_chars = 4_000;
    if text.chars().count() > max_chars {
        text = text.chars().take(max_chars).collect::<String>();
        text.push_str("\n... truncated");
    }
    text
}

fn read_text_file_completion_preview(value: &Value) -> String {
    let content = value.get("content").and_then(Value::as_str).unwrap_or("");
    if content.is_empty() {
        return "Read text file completed with empty content.".to_string();
    }
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty());
    let max_chars = 4_000;
    let truncated = content.chars().count() > max_chars;
    let mut display_content = if truncated {
        let mut value = content.chars().take(max_chars).collect::<String>();
        value.push_str("\n... truncated");
        value
    } else {
        content.to_string()
    };
    if !display_content.ends_with('\n') {
        display_content.push('\n');
    }
    let fence = markdown_fence_for_content(&display_content);
    let mut text = String::new();
    if let Some(path) = path {
        text.push_str(&format!("Read {}:\n\n", markdown_inline_code(path)));
    }
    text.push_str(&fence);
    text.push('\n');
    text.push_str(&display_content);
    text.push_str(&fence);
    if value
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !truncated
    {
        text.push_str("\n\nFile output was truncated by the read operation.");
    }
    text
}

fn markdown_inline_code(value: &str) -> String {
    let tick_count = value.chars().filter(|ch| *ch == '`').count();
    if tick_count == 0 {
        format!("`{value}`")
    } else {
        let fence = "`".repeat(tick_count + 1);
        format!("{fence} {value} {fence}")
    }
}

fn markdown_fence_for_content(content: &str) -> String {
    let mut max_run = 0usize;
    let mut current = 0usize;
    for ch in content.chars() {
        if ch == '`' {
            current += 1;
            max_run = max_run.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(3.max(max_run + 1))
}

async fn post_local_tool_error_result(
    config: &Config,
    shared_state: &AdapterSharedState,
    turn_token: Uuid,
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    event: &Value,
    local_err: LocalToolError,
    started: std::time::Instant,
) -> Result<()> {
    let mut payload = json!({
        "turn_id": event.get("turn_id").and_then(Value::as_str),
        "request_id": event.get("request_id").and_then(Value::as_str),
        "tool_call_id": tool_call_id,
        "run_id": event.get("run_id").and_then(Value::as_str),
        "approval_request_id": event.get("approval_request_id").and_then(Value::as_str),
        "tool_name": tool_name,
        "attempt_token": event.pointer("/data/attempt_token").and_then(Value::as_str),
        "status": local_err.status_str(),
        "content": local_err.message,
        "structured_content": {},
        "diagnostic": {
            "component": "bear-armature",
            "adapter_version": adapter_version(),
            "phase": "adapter_execution_failed",
            "session_id": session_id,
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "duration_ms": started.elapsed().as_millis(),
        }
    });
    merge_diagnostic(&mut payload["diagnostic"], local_err.diagnostic);
    send_detached_tool_call_update_for_turn(
        shared_state,
        session_id,
        turn_token,
        tool_call_id,
        tool_name,
        ToolCallUpdatePayload {
            status: "failed",
            text: payload["content"].as_str().unwrap_or("Local tool failed"),
            request: Some(ToolRequestPresentation::from_event(
                tool_call_id,
                tool_name,
                event,
            )),
            raw_output: None,
            extra_content: Vec::new(),
        },
    )
    .await?;
    post_tool_result(config, session_id, tool_call_id, payload).await
}

fn merge_diagnostic(target: &mut Value, extra: Value) {
    let Some(target_obj) = target.as_object_mut() else {
        *target = extra;
        return;
    };
    if let Some(extra_obj) = extra.as_object() {
        for (key, value) in extra_obj {
            target_obj.insert(key.clone(), value.clone());
        }
    }
}

struct PermissionRequestContext<'a> {
    tool_call_id: &'a str,
    tool_name: &'a str,
    event: &'a Value,
    replace_plan: Option<&'a ReplaceTextPlan>,
    policy: &'a ToolPolicy,
    context: Option<&'a SessionContext>,
    target_path: Option<&'a Path>,
    target_url: Option<&'a str>,
    target_command: Option<&'a str>,
}

async fn request_tool_permission(
    adapter_state: &mut AdapterState,
    session_id: &str,
    request_context: PermissionRequestContext<'_>,
) -> Result<PermissionDecision> {
    let PermissionRequestContext {
        tool_call_id,
        tool_name,
        event,
        replace_plan,
        policy,
        context,
        target_path,
        target_url,
        target_command,
    } = request_context;
    let path = tool_args_from_event(event)
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
        .or(target_url)
        .or(target_command)
        .unwrap_or("the requested target");
    let display = ToolDisplay::from_event(tool_name, event);
    let title = display.title.clone();
    let reason = approval_reason_from_event(event)
        .unwrap_or("Runtime requested approval before running this local ACP tool.");
    eprintln!(
        "bear-armature: requesting permission session_id={} tool_call_id={} tool_name={} path={}",
        session_id, tool_call_id, tool_name, path
    );
    let permission_content = replace_plan
        .map(|plan| plan.permission_summary(tool_name, reason))
        .unwrap_or_else(|| format!("{reason}\n\nTool: {tool_name}\nPath: {path}"));
    let mut content = vec![ToolCallContent::from(permission_content)];
    if let Some(plan) = replace_plan {
        content.push(replace_text_diff_content(plan));
    }
    let mut fields = ToolCallUpdateFields::new()
        .kind(Some(display.kind))
        .status(Some(ToolCallStatus::Pending))
        .title(Some(title))
        .content(Some(content));
    if let Some(locations) = tool_locations_from_event(tool_name, event) {
        fields = fields.locations(Some(locations));
    }
    if let Some(args) = tool_args_from_event(event) {
        fields = fields.raw_input(Some(args.clone()));
    }
    let mut meta = serde_json::Map::new();
    meta.insert("toolName".to_string(), json!(tool_name));
    meta.insert("toolKind".to_string(), json!(tool_kind_str(display.kind)));
    meta.insert("targetKind".to_string(), json!(tool_target_kind(tool_name)));
    meta.insert("targetPath".to_string(), json!(path));
    if let Some(url) = target_url {
        meta.insert("targetUrl".to_string(), json!(url));
        if let Some(host) = approval_url_host_scope(url) {
            meta.insert("targetHost".to_string(), json!(host));
        }
    }
    if let Some(command) = target_command {
        meta.insert("targetCommand".to_string(), json!(command));
    }
    meta.insert(
        "permissionClass".to_string(),
        json!(permission_class_for_tool(tool_name)),
    );
    meta.insert("risk".to_string(), json!(policy.risk()));
    meta.insert("operation".to_string(), json!(display.permission_operation));
    if let Some(category) = display.category.as_ref() {
        meta.insert("category".to_string(), json!(category));
    }
    if let Some(arguments_summary) = display.arguments_summary.as_ref() {
        meta.insert("argumentsSummary".to_string(), arguments_summary.clone());
    }
    let tool_call = ToolCallUpdate::new(tool_call_id.to_string(), fields).meta(Some(meta.clone()));
    let options = permission_options_for_context(
        context,
        target_path,
        target_url,
        target_command,
        permission_family_label(tool_name),
    );
    let request =
        RequestPermissionRequest::new(session_id.to_string(), tool_call, options).meta(Some(meta));
    let permission_timeout_ms = policy.permission_timeout_ms.unwrap_or(120_000);
    tracing::trace!(
        target: "bear_armature::lifecycle",
        session_id,
        tool_call_id,
        tool_name,
        timeout_ms = permission_timeout_ms,
        "ACP permission request sending"
    );
    let decision = send_permission_request(
        adapter_state,
        request,
        std::time::Duration::from_millis(permission_timeout_ms),
    )
    .await?;
    tracing::trace!(
        target: "bear_armature::lifecycle",
        session_id,
        tool_call_id,
        tool_name,
        approved = decision.approved,
        remember = decision.remember,
        scope = decision.scope.as_str(),
        "ACP permission response received"
    );
    if decision.approved {
        Ok(decision)
    } else {
        Err(anyhow!("permission denied for {tool_name} on {path}"))
    }
}

async fn send_permission_request(
    adapter_state: &mut AdapterState,
    request: RequestPermissionRequest,
    timeout: std::time::Duration,
) -> Result<PermissionDecision> {
    if headless_mode() {
        return headless::decide_permission_headless(&request);
    }
    let response = adapter_state
        .transport
        .request(
            "session/request_permission",
            serde_json::to_value(request)?,
            timeout,
        )
        .await?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("permission request failed: {error}"));
    }
    let result = response.get("result").cloned().unwrap_or(Value::Null);
    Ok(parse_permission_decision(&result))
}

async fn post_permission_result(
    config: &Config,
    session_id: &str,
    permission_id: &str,
    payload: Value,
) -> Result<Value> {
    if let Some(run_id) = payload.get("run_id").and_then(Value::as_str) {
        tracing::trace!(
            target: "bear_armature::lifecycle",
            session_id,
            run_id,
            permission_id,
            "posting BearWire permission result"
        );
        let result = bearwire::post_permission_result(
            config,
            session_id,
            run_id,
            permission_id,
            payload.clone(),
        )
        .await?;
        tracing::trace!(
            target: "bear_armature::lifecycle",
            session_id,
            run_id,
            permission_id,
            response = %truncate_for_log(&result.to_string(), 360),
            "posted BearWire permission result"
        );
        if bear_debug_verbose() {
            eprintln!(
                "bear-armature: posted BearWire permission result session_id={} run_id={} permission_id={} response={}",
                session_id,
                run_id,
                permission_id,
                truncate_for_log(&result.to_string(), 360)
            );
        }
        return Ok(result);
    }

    Err(anyhow!(
        "BearWire permission result payload missing run_id; legacy ACP permission endpoint is retired"
    ))
}

fn is_turn_missing_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}");
    message.contains("turn_missing") || message.contains("tool_result_missing")
}

async fn post_adapter_environment(
    config: &Config,
    session_id: &str,
    environment: Value,
    conversation_title: Option<&str>,
) -> Result<()> {
    let resource = adapter_environment_resource(environment, conversation_title);
    let value = bearwire::post_resource_update(config, session_id, resource).await?;
    if bear_debug_verbose() {
        eprintln!(
            "bear-armature: posted BearWire resource.update session_id={} response={}",
            session_id,
            truncate_for_log(&value.to_string(), 360)
        );
    }
    Ok(())
}

fn adapter_environment_resource(environment: Value, conversation_title: Option<&str>) -> Value {
    let title = conversation_title
        .map(str::trim)
        .filter(|value| !value.is_empty());
    json!({
        "kind": "acp_adapter",
        "environment": environment,
        "conversation_title": title,
    })
}

async fn post_tool_result(
    config: &Config,
    session_id: &str,
    tool_call_id: &str,
    payload: Value,
) -> Result<()> {
    if let Some(run_id) = payload.get("run_id").and_then(Value::as_str) {
        let started = std::time::Instant::now();
        tracing::trace!(
            target: "bear_armature::lifecycle",
            session_id,
            run_id,
            tool_call_id,
            payload_bytes = payload.to_string().len(),
            "posting BearWire tool result"
        );
        if bear_debug_verbose() {
            eprintln!(
                "bear-armature: posting BearWire tool result session_id={} run_id={} tool_call_id={} payload_bytes={}",
                session_id,
                run_id,
                tool_call_id,
                payload.to_string().len()
            );
        }
        let result = timeout(
            Duration::from_secs(TOOL_RESULT_POST_TIMEOUT_SECS),
            bearwire::post_tool_result(
                config,
                session_id,
                run_id,
                tool_call_id,
                payload.clone(),
                payload.get("attempt_token").and_then(Value::as_str),
            ),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "timed out after {}s posting BearWire tool result session_id={} run_id={} tool_call_id={}",
                TOOL_RESULT_POST_TIMEOUT_SECS,
                session_id,
                run_id,
                tool_call_id
            )
        })??;
        let duration_ms = started.elapsed().as_millis();
        if duration_ms >= TOOL_RESULT_POST_SLOW_MS {
            eprintln!(
                "bear-armature: slow BearWire tool result post session_id={} run_id={} tool_call_id={} duration_ms={}",
                session_id,
                run_id,
                tool_call_id,
                duration_ms
            );
        }
        tracing::trace!(
            target: "bear_armature::lifecycle",
            session_id,
            run_id,
            tool_call_id,
            duration_ms,
            response = %truncate_for_log(&result.to_string(), 720),
            "posted BearWire tool result"
        );
        log_bearwire_tool_result_response(session_id, run_id, tool_call_id, &result, duration_ms);
        return Ok(());
    }

    Err(anyhow!(
        "BearWire tool result payload missing run_id for tool_call_id={tool_call_id}; legacy ACP tool-results endpoint is retired"
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BearWireToolResultResponseClass {
    Continued,
    WaitingForMore,
    Duplicate,
    LateIgnored,
    ContinuationUnavailable,
    Error,
    Unknown,
}

impl BearWireToolResultResponseClass {
    fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::LateIgnored | Self::ContinuationUnavailable | Self::Error | Self::Unknown
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Continued => "continued",
            Self::WaitingForMore => "waiting_for_more",
            Self::Duplicate => "duplicate",
            Self::LateIgnored => "late_ignored",
            Self::ContinuationUnavailable => "continuation_unavailable",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

fn classify_bearwire_tool_result_response(result: &Value) -> BearWireToolResultResponseClass {
    if result.get("ok").and_then(Value::as_bool) == Some(false) {
        return match result.get("status").and_then(Value::as_str) {
            Some("late_result_ignored") => BearWireToolResultResponseClass::LateIgnored,
            Some("continuation_unavailable") => {
                BearWireToolResultResponseClass::ContinuationUnavailable
            }
            _ => BearWireToolResultResponseClass::Error,
        };
    }
    if result.get("duplicate").and_then(Value::as_bool) == Some(true) {
        return BearWireToolResultResponseClass::Duplicate;
    }
    match result.get("continuation").and_then(Value::as_str) {
        Some("started") => BearWireToolResultResponseClass::Continued,
        Some("waiting_for_more_client_results") => BearWireToolResultResponseClass::WaitingForMore,
        Some("continuation_unavailable") => {
            BearWireToolResultResponseClass::ContinuationUnavailable
        }
        Some(_) => BearWireToolResultResponseClass::Unknown,
        None => BearWireToolResultResponseClass::Unknown,
    }
}

fn log_bearwire_tool_result_response(
    session_id: &str,
    run_id: &str,
    tool_call_id: &str,
    result: &Value,
    duration_ms: u128,
) {
    let class = classify_bearwire_tool_result_response(result);
    if bear_debug_verbose() || class.needs_attention() {
        let level = if class.needs_attention() {
            "warning"
        } else {
            "debug"
        };
        eprintln!(
            "bear-armature: BearWire tool result response {level} class={} session_id={} run_id={} tool_call_id={} duration_ms={} response={}",
            class.as_str(),
            session_id,
            run_id,
            tool_call_id,
            duration_ms,
            truncate_for_log(&result.to_string(), 720)
        );
    }
}

pub(crate) async fn handle_status_text_for_turn(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    text: &str,
) -> Result<()> {
    if !text.is_empty() && bear_debug_mode().shows_thoughts() {
        send_agent_thought_chunk_for_turn(
            shared_state,
            session_id,
            turn_token,
            normalize_thought_chunk_text(text).as_ref(),
        )
        .await?;
    } else if !text.is_empty() && bear_debug_verbose() {
        eprintln!(
            "bear-armature: suppressed thought chunk session_id={} text={}",
            session_id,
            truncate_for_log(text, 240)
        );
    }
    Ok(())
}

pub(crate) async fn handle_session_info_projection(
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    title: Option<String>,
    updated_at: Option<String>,
    context_budget: Option<Value>,
    runtime: Option<Value>,
) -> Result<()> {
    if !is_current_prompt_turn(
        shared_state,
        session_id,
        turn_token,
        "session_info_projection",
    )
    .await
    {
        return Ok(());
    }
    apply_session_title_projection_state(adapter_state, shared_state, session_id, title.clone())
        .await;
    send_session_info_update(session_id, title, updated_at).await?;
    if let Some(context_budget) = context_budget.clone() {
        send_context_budget_usage_update(session_id, context_budget).await?;
    }
    if env_bool("BEAR_ARMATURE_SEND_RUNTIME_SESSION_META") {
        send_den_runtime_session_info_update(session_id, runtime, context_budget).await?;
    }
    Ok(())
}

async fn apply_session_title_projection_state(
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    title: Option<String>,
) {
    if let Some(context) = adapter_state.session_contexts.get_mut(session_id) {
        context.thread_title = title.clone();
    }
    if let Some(context) = shared_state
        .session_contexts
        .lock()
        .await
        .get_mut(session_id)
    {
        context.thread_title = title;
    }
}

pub(crate) async fn handle_plan_update_projection(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    entries: Vec<PlanEntry>,
    approval_fallback: Option<&Value>,
) -> Result<()> {
    if let Some(fallback) = approval_fallback {
        if let Some(message) = plan_approval_fallback_message(fallback) {
            send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, &message)
                .await?;
        }
    }
    if entries.is_empty() {
        if bear_debug_verbose() {
            eprintln!(
                "bear-armature: received empty plan update for session_id={}; not sending ACP plan UI update",
                session_id
            );
        }
    } else if should_send_plan_update(shared_state, session_id, &entries).await? {
        if is_current_prompt_turn(shared_state, session_id, turn_token, "plan_update").await {
            send_plan_update(session_id, entries).await?;
        }
    } else if bear_debug_verbose() {
        eprintln!(
            "bear-armature: skipped unchanged plan update for session_id={}",
            session_id
        );
    }
    Ok(())
}

pub(crate) async fn handle_conversation_resolved_projection(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    conversation_id: &str,
) -> Result<()> {
    if !is_current_prompt_turn(
        shared_state,
        session_id,
        turn_token,
        "conversation_resolved_projection",
    )
    .await
    {
        return Ok(());
    }
    let conversation_id = conversation_id.trim();
    if !conversation_id.starts_with("conv-") {
        return Ok(());
    }
    let context = adapter_state
        .session_contexts
        .entry(session_id.to_string())
        .or_default();
    context.resolved_conversation_id = Some(conversation_id.to_string());
    let thread_title = context.thread_title.clone();
    {
        let mut shared_contexts = shared_state.session_contexts.lock().await;
        let shared = shared_contexts.entry(session_id.to_string()).or_default();
        shared.resolved_conversation_id = Some(conversation_id.to_string());
        if thread_title.is_some() {
            shared.thread_title = thread_title.clone();
        }
    }
    if let Some(title) = thread_title.as_deref() {
        if let Ok(snapshot) = collect_bear_environment(
            adapter_state,
            session_id,
            Some(config),
            None,
            &json!({
                "include_session_mcp": true,
                "include_client_capabilities": true,
                "include_raw_context": true,
                "inspect_den": false,
            }),
        )
        .await
        {
            if let Err(err) =
                post_adapter_environment(config, session_id, snapshot, Some(title)).await
            {
                eprintln!(
                    "bear-armature: failed to publish adapter environment after conversation_resolved session_id={} error={err:#}",
                    session_id
                );
            }
        }
    }
    eprintln!(
        "bear-armature: session_id={} resolved conversation_id={}",
        session_id, conversation_id
    );
    Ok(())
}

fn approved_local_tool_request_event(
    permission_event: &Value,
    local_tool: &Value,
) -> Result<Value> {
    let run_id = permission_event
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("approved local tool request missing run_id"))?;
    let obligation_id = local_tool
        .get("obligation_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("approved local tool request missing obligation_id"))?;
    let tool_call_id = local_tool
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("approved local tool request missing tool_call_id"))?;
    let tool_name = local_tool
        .get("tool_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("approved local tool request missing tool_name"))?;

    Ok(json!({
        "type": "tool_call.requested",
        "run_id": run_id,
        "data": {
            "expected_responder_action": "tool_result",
            "obligation_id": obligation_id,
            "tool_call": {
                "id": tool_call_id,
                "name": tool_name,
                "kind": "function",
                "arguments": local_tool.get("args").cloned().unwrap_or_else(|| json!({})),
            },
            "approval_required": false,
            "approval_request_id": local_tool.get("permission_id").cloned().unwrap_or(Value::Null),
            "execution_target": "armature_local",
            "policy": local_tool.get("policy").cloned().unwrap_or(Value::Null),
        }
    }))
}

pub(crate) async fn handle_permission_request_event(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    event: &Value,
    turn_token: Uuid,
) -> Result<()> {
    let canonical = BearWireClientWaitingData::parse(event)?;
    let permission_id = canonical.permission.id.trim();
    let obligation_id = Some(canonical.obligation_id.trim());
    let tool_call_id = canonical.tool_call.id.trim();
    let tool_name = canonical.tool_call.name.trim();
    let run_id = event
        .get("run_id")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    tracing::debug!(
        target: "bear_armature::lifecycle",
        session_id,
        run_id,
        obligation_id = canonical.obligation_id.trim(),
        permission_id,
        tool_call_id,
        tool_name,
        "permission obligation accepted for ACP projection"
    );
    if tool_request_execution_target(event) == Some("den") {
        eprintln!(
            "bear-armature: BearWire invariant violation: Den-owned tool arrived as client.waiting session_id={} tool_call_id={} tool_name={} permission_id={}",
            session_id,
            tool_call_id,
            tool_name,
            permission_id
        );
    }
    let title = canonical
        .permission
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Permission request");
    let reason = canonical
        .permission
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or("Den requests permission.");
    let target = canonical
        .permission
        .target
        .clone()
        .unwrap_or_else(|| canonical.tool_call.arguments.clone());
    let url = target.get("url").and_then(Value::as_str);
    let host = target.get("host").and_then(Value::as_str);
    let plan_mode_id = target.get("plan_mode_id").and_then(Value::as_str);
    let target_kind = target.get("kind").and_then(Value::as_str);
    let is_plan_mode = target_kind == Some("acp_plan_mode") || plan_mode_id.is_some();
    let command = target.get("command").and_then(Value::as_str);
    let command_args = target
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let cwd = target.get("cwd").and_then(Value::as_str);
    let timeout_ms = target.get("timeout_ms").and_then(Value::as_u64);
    let max_output_bytes = target.get("max_output_bytes").and_then(Value::as_u64);
    let is_command_permission =
        matches!(tool_name, "process_run" | "terminal_run_command") || command.is_some();
    let mut display = tool_display(tool_name);
    if is_plan_mode {
        display.title = "Approve implementation plan".to_string();
        display.kind = ToolKind::SwitchMode;
        display.verb = "Reviewing plan".to_string();
        display.permission_operation = "approve this implementation plan".to_string();
    } else if is_command_permission {
        display.title = "Approve command".to_string();
        display.kind = ToolKind::Execute;
        display.verb = "Reviewing command".to_string();
        display.permission_operation = "run this command".to_string();
    }
    let plan_body = target.get("body").and_then(Value::as_str);
    let artifact_path = target.get("artifact_path").and_then(Value::as_str);
    let command_line = command.map(|command| {
        if command_args.is_empty() {
            command.to_string()
        } else {
            format!("{command} {command_args}")
        }
    });
    let site_account = url.and_then(crate::approvals::approval_url_site_account_scope);
    let source_path = target.get("source_path").and_then(Value::as_str);
    let destination_path = target.get("destination_path").and_then(Value::as_str);
    let path = target
        .get("path")
        .or_else(|| target.get("file_path"))
        .and_then(Value::as_str);
    let policy = policy_from_event(event);
    let context_for_approval = adapter_state
        .session_contexts
        .get(session_id)
        .cloned()
        .or_else(|| {
            shared_state
                .session_contexts
                .try_lock()
                .ok()?
                .get(session_id)
                .cloned()
        });
    let target_path_for_approval = context_for_approval
        .as_ref()
        .and_then(|context| policy_target_path(context, &canonical.tool_call.arguments, &policy))
        .or_else(|| {
            path.and_then(|path| {
                context_for_approval
                    .as_ref()
                    .and_then(|context| resolve_requested_tool_path(context, path).ok())
                    .or_else(|| normalize_requested_tool_path(path).ok())
            })
        });
    let target_label = if is_plan_mode {
        artifact_path
            .or(plan_mode_id)
            .unwrap_or("submitted plan artifact")
    } else if let Some(command_line) = command_line.as_deref() {
        command_line
    } else if let Some(source_path) = source_path {
        source_path
    } else if let Some(path) = path {
        path
    } else {
        url.or(host).unwrap_or("the requested target")
    };
    let permission_body = if is_plan_mode {
        format!(
            "{reason}\n\nTool: {tool_name}\nTarget: {target_label}\nPlan ID: {}\n\n{}",
            plan_mode_id.unwrap_or("unknown"),
            plan_body
                .unwrap_or("Plan body is unavailable; use the artifact path for audit context.")
        )
    } else if is_command_permission {
        format!(
            "{reason}\n\nTool: {tool_name}\nCommand: {}\nWorking directory: {}\nTimeout: {}\nMax output bytes: {}\n\nApprove only if this command matches the user's requested task.",
            command_line.as_deref().unwrap_or("<missing command>"),
            cwd.unwrap_or("<missing cwd>"),
            timeout_ms.map(|value| format!("{value}ms")).unwrap_or_else(|| "default".to_string()),
            max_output_bytes.map(|value| value.to_string()).unwrap_or_else(|| "default".to_string()),
        )
    } else if matches!(tool_name, "chrome_open" | "web_fetch" | "local_web_fetch") {
        let mut body = format!(
            "{reason}\n\nAction: {}\nURL: {}",
            display.permission_operation,
            url.unwrap_or("<missing url>")
        );
        if let Some(site_account) = site_account.as_deref() {
            body.push_str(&format!("\nKnown site account: {site_account}"));
        }
        if let Some(host) = host {
            body.push_str(&format!("\nHost: {host}"));
        }
        body
    } else if matches!(
        tool_name,
        "chrome_snapshot"
            | "chrome_console_messages"
            | "chrome_network_requests"
            | "chrome_screenshot"
    ) {
        let mut body = format!("{reason}\n\nAction: {}", display.permission_operation);
        if let Some(url) = url {
            body.push_str(&format!("\nURL: {url}"));
        }
        if let Some(site_account) = site_account.as_deref() {
            body.push_str(&format!("\nKnown site account: {site_account}"));
        }
        if let Some(host) = host {
            body.push_str(&format!("\nHost: {host}"));
        }
        body
    } else if matches!(tool_name, "fs_delete_path") {
        format!(
            "{reason}\n\nAction: {}\nPath: {}",
            display.permission_operation,
            path.unwrap_or("<missing path>")
        )
    } else if matches!(tool_name, "fs_move_path" | "fs_copy_path") {
        format!(
            "{reason}\n\nAction: {}\nSource: {}\nDestination: {}",
            display.permission_operation,
            source_path.unwrap_or("<missing source>"),
            destination_path.unwrap_or("<missing destination>")
        )
    } else if let Some(path) = path {
        format!(
            "{reason}\n\nAction: {}\nPath: {}",
            display.permission_operation, path
        )
    } else {
        format!(
            "{reason}\n\nAction: {}\nTarget: {target_label}",
            display.permission_operation
        )
    };
    let request_title = if is_command_permission {
        command_line
            .as_deref()
            .map(|command| format!("Run command: {command}"))
            .unwrap_or_else(|| "Run command".to_string())
    } else if matches!(tool_name, "chrome_open") {
        url.map(|url| format!("Open: {}", truncate_title(url)))
            .unwrap_or_else(|| display.title.clone())
    } else if matches!(tool_name, "web_fetch" | "local_web_fetch") {
        url.map(|url| format!("Fetch: {}", truncate_title(url)))
            .unwrap_or_else(|| display.title.clone())
    } else if matches!(
        tool_name,
        "chrome_snapshot"
            | "chrome_console_messages"
            | "chrome_network_requests"
            | "chrome_screenshot"
    ) {
        if let Some(url) = url {
            format!("{}: {}", display.title, truncate_title(url))
        } else {
            display.title.clone()
        }
    } else if matches!(tool_name, "fs_delete_path") {
        path.map(|path| format!("Delete: {}", truncate_title(path)))
            .unwrap_or_else(|| display.title.clone())
    } else if matches!(tool_name, "fs_move_path" | "fs_copy_path") {
        match (source_path, destination_path) {
            (Some(source), Some(destination)) => format!(
                "{}: {} -> {}",
                display.title,
                truncate_title(source),
                truncate_title(destination)
            ),
            _ => display.title.clone(),
        }
    } else if let Some(path) = path {
        format!("{}: {}", display.title, truncate_title(path))
    } else {
        title.to_string()
    };
    let mut content = vec![ToolCallContent::from(permission_body)];
    let fields = ToolCallUpdateFields::new()
        .kind(Some(display.kind))
        .status(Some(ToolCallStatus::Pending))
        .title(Some(request_title))
        .content(Some(std::mem::take(&mut content)))
        .raw_input(Some(target.clone()));
    let tool_call = ToolCallUpdate::new(tool_call_id.to_string(), fields).meta(Some({
        let mut meta = serde_json::Map::new();
        meta.insert("toolName".to_string(), json!(tool_name));
        meta.insert("permissionId".to_string(), json!(permission_id));
        if let Some(obligation_id) = obligation_id {
            meta.insert("obligationId".to_string(), json!(obligation_id));
        }
        if let Some(url) = url {
            meta.insert("targetUrl".to_string(), json!(url));
        }
        if let Some(host) = host {
            meta.insert("targetHost".to_string(), json!(host));
        }
        if let Some(site_account) = site_account.as_ref() {
            meta.insert("siteAccount".to_string(), json!(site_account));
        }
        if let Some(command) = command {
            meta.insert("targetCommand".to_string(), json!(command));
        }
        if !command_args.is_empty() {
            meta.insert("targetCommandArgs".to_string(), json!(command_args));
        }
        if let Some(cwd) = cwd {
            meta.insert("targetCwd".to_string(), json!(cwd));
        }
        if let Some(plan_mode_id) = plan_mode_id {
            meta.insert("planModeId".to_string(), json!(plan_mode_id));
        }
        if let Some(artifact_path) = artifact_path {
            meta.insert("artifactPath".to_string(), json!(artifact_path));
        }
        meta
    }));
    let options = if is_plan_mode {
        vec![
            agent_client_protocol::schema::PermissionOption::new(
                "approve",
                "Approve this plan and allow implementation",
                agent_client_protocol::schema::PermissionOptionKind::AllowOnce,
            ),
            agent_client_protocol::schema::PermissionOption::new(
                "reject",
                "Reject this plan and keep implementation blocked",
                agent_client_protocol::schema::PermissionOptionKind::RejectOnce,
            ),
        ]
    } else if is_command_permission {
        permission_options_for_context(
            adapter_state.session_contexts.get(session_id),
            None,
            None,
            command_line.as_deref(),
            "commands",
        )
    } else {
        permission_options_for_context(
            context_for_approval.as_ref(),
            target_path_for_approval.as_deref(),
            url,
            None,
            permission_family_label(tool_name),
        )
    };
    let request = RequestPermissionRequest::new(session_id.to_string(), tool_call, options);
    let auto_allowed = if let Some(context) = context_for_approval.as_ref() {
        shared_state
            .approval_cache
            .is_allowed_for_target(
                context,
                tool_name,
                target_path_for_approval.as_deref(),
                url,
                command_line.as_deref(),
            )
            .await
    } else {
        false
    };
    let decision = if auto_allowed {
        tracing::debug!(
            target: "bear_armature::lifecycle",
            session_id,
            run_id,
            obligation_id = canonical.obligation_id.trim(),
            permission_id,
            tool_call_id,
            tool_name,
            "permission obligation auto-approved; ACP request not dispatched"
        );
        PermissionDecision {
            approved: true,
            remember: false,
            scope: ApprovalScope::Workspace,
        }
    } else {
        tracing::debug!(
            target: "bear_armature::lifecycle",
            session_id,
            run_id,
            obligation_id = canonical.obligation_id.trim(),
            permission_id,
            tool_call_id,
            tool_name,
            "dispatching ACP permission request"
        );
        match send_permission_request(adapter_state, request, std::time::Duration::from_secs(120))
            .await
        {
            Ok(decision) => {
                tracing::debug!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    obligation_id = canonical.obligation_id.trim(),
                    permission_id,
                    tool_call_id,
                    tool_name,
                    approved = decision.approved,
                    remember = decision.remember,
                    "ACP permission request completed"
                );
                decision
            }
            Err(err) => {
                tracing::warn!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    obligation_id = canonical.obligation_id.trim(),
                    permission_id,
                    tool_call_id,
                    tool_name,
                    error = %format!("{err:#}"),
                    "ACP permission request failed before a decision"
                );
                let message = format!("Permission request timed out or failed: {err:#}");
                let _ = post_permission_result(
                    config,
                    session_id,
                    permission_id,
                    json!({
                        "decision": "timeout",
                        "obligation_id": obligation_id,
                        "plan_mode_id": plan_mode_id,
                        "run_id": event.get("run_id").and_then(Value::as_str),
                    }),
                )
                .await;
                if is_plan_mode {
                    let _ = notify_mode_state(session_id, MODE_PLAN).await;
                }
                let _ = send_tool_call_update(
                    session_id,
                    tool_call_id,
                    tool_name,
                    ToolCallUpdatePayload {
                        status: "failed",
                        text: &message,
                        request: Some(ToolRequestPresentation::from_event(
                            tool_call_id,
                            tool_name,
                            event,
                        )),
                        raw_output: Some(json!({
                            "component": "bear-armature",
                            "phase": "permission_request_failed",
                            "permission_id": permission_id,
                            "error": format!("{err:#}"),
                        })),
                        extra_content: Vec::new(),
                    },
                )
                .await;
                return Ok(());
            }
        }
    };
    if decision.approved && decision.remember {
        if let Some(context) = context_for_approval.as_ref() {
            shared_state
                .approval_cache
                .remember_for_target(
                    context,
                    tool_name,
                    policy.risk(),
                    decision.scope,
                    ApprovalTarget {
                        path: target_path_for_approval.as_deref(),
                        url,
                        command: command_line.as_deref(),
                    },
                )
                .await;
        }
    }
    let decision_str = if is_plan_mode {
        if decision.approved {
            "approve"
        } else {
            "reject"
        }
    } else {
        match decision.scope {
            ApprovalScope::SiteAccount if decision.approved => "allow_site_account",
            ApprovalScope::Host if decision.approved => "allow_host",
            ApprovalScope::Workspace
            | ApprovalScope::Directory
            | ApprovalScope::Command
            | ApprovalScope::CommandExactWorkspace
            | ApprovalScope::CommandFamilyWorkspace
            | ApprovalScope::Global
                if decision.approved =>
            {
                "allow_once"
            }
            _ => "reject_once",
        }
    };
    let response = post_permission_result(
        config,
        session_id,
        permission_id,
        json!({
            "decision": decision_str,
            "obligation_id": obligation_id,
            "plan_mode_id": plan_mode_id,
            "run_id": event.get("run_id").and_then(Value::as_str),
        }),
    )
    .await?;
    if is_plan_mode {
        let mode = response
            .get("effective_mode")
            .and_then(Value::as_str)
            .and_then(|mode| match mode {
                MODE_ASK => Some(MODE_ASK),
                MODE_PLAN => Some(MODE_PLAN),
                MODE_WRITE => Some(MODE_WRITE),
                _ => None,
            })
            .unwrap_or(if decision_str == "approve" {
                MODE_WRITE
            } else {
                MODE_PLAN
            });
        notify_mode_state(session_id, mode).await?;
        if let Some(fallback) = response.get("approval_fallback") {
            if let Some(message) = plan_approval_fallback_message(fallback) {
                send_agent_message_chunk(session_id, &message).await?;
            }
            let entries = plan_entries_from_den_session(&json!({ "approval_fallback": fallback }));
            if !entries.is_empty() {
                send_plan_update(session_id, entries).await?;
            }
        }
    }
    if let Some(local_tool) = response.get("local_tool_request") {
        let request_event = approved_local_tool_request_event(event, local_tool)?;
        spawn_tool_request_task(
            config.clone(),
            shared_state.clone(),
            session_id.to_string(),
            request_event,
            turn_token,
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ToolDisplay {
    title: String,
    kind: ToolKind,
    verb: String,
    permission_operation: String,
    subtitle: Option<String>,
    category: Option<String>,
    arguments_summary: Option<Value>,
}

impl ToolDisplay {
    fn builtin(
        title: &'static str,
        kind: ToolKind,
        verb: &'static str,
        permission_operation: &'static str,
    ) -> Self {
        Self {
            title: title.to_string(),
            kind,
            verb: verb.to_string(),
            permission_operation: permission_operation.to_string(),
            subtitle: None,
            category: None,
            arguments_summary: None,
        }
    }

    fn from_event(tool_name: &str, event: &Value) -> Self {
        let mut display = tool_display(tool_name);
        let Some(event_display) = event_display_from_event(event) else {
            if is_placeholder_tool_name(tool_name) {
                display.title = fallback_tool_title_from_event(event);
                display.permission_operation = display.title.to_ascii_lowercase();
            }
            return display;
        };
        if let Some(title) = event_display
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            display.title = title.to_string();
        } else if let Some(label) = event_display
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            display.title = label.to_string();
        }
        if let Some(progress) = event_display
            .get("progress")
            .or_else(|| event_display.get("progress_verb"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            display.verb = progress.to_string();
        }
        if let Some(approval) = event_display
            .get("approval_summary")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            display.permission_operation = approval.trim_end_matches('.').to_string();
        }
        display.subtitle = event_display
            .get("subtitle")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        display.category = event_display
            .get("category")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        display.arguments_summary = event_display.get("arguments_summary").cloned();
        if is_placeholder_tool_name(tool_name) && display.title == "Tool call" {
            display.title = fallback_tool_title_from_event(event);
            display.permission_operation = display.title.to_ascii_lowercase();
        }
        if tool_name == "git_commit" {
            if let Some(message) = tool_args_from_event(event)
                .and_then(|args| args.get("message"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|message| !message.is_empty())
            {
                display.title = format!("Git commit: {}", truncate_title(message));
                display.subtitle = Some(message.to_string());
            }
        }
        display
    }
}

fn fallback_tool_title_from_event(event: &Value) -> String {
    let args = tool_args_from_event(event);
    for key in ["path", "command", "url", "query", "glob", "cwd"] {
        if let Some(value) = args
            .and_then(|args| args.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return format!("Tool call: {}", truncate_title(value));
        }
    }
    if let Some(keys) = args
        .and_then(Value::as_object)
        .map(|map| map.keys().take(4).cloned().collect::<Vec<_>>().join(", "))
        .filter(|keys| !keys.is_empty())
    {
        return format!("Tool call with {keys}");
    }
    if let Some(tool_call_id) = event
        .get("tool_call_id")
        .or_else(|| event.pointer("/data/tool_call_id"))
        .or_else(|| event.pointer("/data/tool_call/id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return format!("Tool call: {}", truncate_title(tool_call_id));
    }
    "Tool call".to_string()
}

pub(crate) fn is_placeholder_tool_name(tool_name: &str) -> bool {
    let trimmed = tool_name.trim();
    trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "tool" | "local_tool" | "local tool" | "unknown" | "unknown_tool"
        )
}

fn fallback_tool_title(tool_name: &str) -> String {
    let trimmed = tool_name.trim();
    if is_placeholder_tool_name(trimmed) {
        return "Tool call".to_string();
    }
    let words = trimmed
        .split(|ch: char| matches!(ch, '_' | '-' | '.' | '/' | ':'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return "Tool call".to_string();
    }
    let mut out = String::new();
    for (index, word) in words.into_iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn mcp_tool_title(provider_name: &str) -> String {
    let Some(rest) = provider_name.strip_prefix("mcp__") else {
        return fallback_tool_title(provider_name);
    };
    let mut parts = rest.split("__").collect::<Vec<_>>();
    let tool = parts.pop().unwrap_or(rest);
    let server = parts.join(" ");
    let server_title = if server.contains("chrome") && server.contains("devtools") {
        "Chrome DevTools".to_string()
    } else if server.trim().is_empty() {
        "MCP".to_string()
    } else {
        fallback_tool_title(&server)
    };
    format!("{server_title}: {}", fallback_tool_title(tool))
}

fn tool_request_execution_target(event: &Value) -> Option<&str> {
    event
        .get("execution_target")
        .or_else(|| event.pointer("/data/execution_target"))
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("policy")
                .or_else(|| event.pointer("/data/policy"))
                .or_else(|| event.pointer("/data/tool_call/policy"))
                .and_then(|policy| policy.get("execution_target"))
                .and_then(Value::as_str)
        })
}

pub(crate) fn is_den_server_tool_request(event: &Value) -> bool {
    tool_request_execution_target(event) == Some("den")
}

pub(crate) fn friendly_tool_title(tool_name: &str) -> String {
    tool_display(tool_name).title
}

fn tool_display(tool_name: &str) -> ToolDisplay {
    match tool_name {
        "fs_read_text_file" | "fs.read_text_file" => {
            ToolDisplay::builtin("Read file", ToolKind::Read, "Reading", "read this file")
        }
        "fs_list_directory" => ToolDisplay::builtin(
            "List directory",
            ToolKind::Read,
            "Listing",
            "list this directory",
        ),
        "fs_find_paths" => ToolDisplay::builtin(
            "Find paths",
            ToolKind::Search,
            "Finding paths under",
            "find paths",
        ),
        "fs_search_files" => ToolDisplay::builtin(
            "Search files",
            ToolKind::Search,
            "Searching",
            "search files",
        ),
        "fs_stat" => ToolDisplay::builtin(
            "Stat path",
            ToolKind::Read,
            "Inspecting",
            "inspect this path",
        ),
        "git_status" => ToolDisplay::builtin(
            "Git status",
            ToolKind::Read,
            "Checking git status for",
            "read git status",
        ),
        "git_diff" => ToolDisplay::builtin(
            "Git diff",
            ToolKind::Read,
            "Reading git diff for",
            "read git diff",
        ),
        "git_log" => ToolDisplay::builtin(
            "Git log",
            ToolKind::Read,
            "Reading git log for",
            "read git log",
        ),
        "git_show" => ToolDisplay::builtin(
            "Git show",
            ToolKind::Read,
            "Reading git revision for",
            "read git revision",
        ),
        "git_add" => ToolDisplay::builtin(
            "Git add",
            ToolKind::Edit,
            "Staging git paths in",
            "stage git paths",
        ),
        "git_restore" => ToolDisplay::builtin(
            "Git restore",
            ToolKind::Edit,
            "Restoring git paths in",
            "restore git paths",
        ),
        "git_commit" => ToolDisplay::builtin(
            "Git commit",
            ToolKind::Edit,
            "Creating git commit in",
            "create git commit",
        ),
        "git_stash" => ToolDisplay::builtin(
            "Git stash",
            ToolKind::Edit,
            "Creating git stash in",
            "create git stash",
        ),
        "web_fetch" | "local_web_fetch" => {
            ToolDisplay::builtin("Fetch URL", ToolKind::Fetch, "Fetching", "fetch this URL")
        }
        "web_search" => ToolDisplay::builtin(
            "Search web",
            ToolKind::Search,
            "Searching web",
            "search the web",
        ),
        "session_info" | "bear_environment" => ToolDisplay::builtin(
            "Inspect session",
            ToolKind::Read,
            "Inspecting session",
            "inspect session context",
        ),
        "memory_browse" => ToolDisplay::builtin(
            "Browse memory",
            ToolKind::Read,
            "Browsing memory",
            "browse memory",
        ),
        "memory_read" => ToolDisplay::builtin(
            "Read memory",
            ToolKind::Read,
            "Reading memory",
            "read memory",
        ),
        "memory_search" => ToolDisplay::builtin(
            "Search memory",
            ToolKind::Search,
            "Searching memory",
            "search memory",
        ),
        "memory_write_entry" => ToolDisplay::builtin(
            "Write memory entry",
            ToolKind::Edit,
            "Writing memory",
            "write memory",
        ),
        "memory_request_review" => ToolDisplay::builtin(
            "Request memory review",
            ToolKind::Think,
            "Requesting memory review",
            "request memory review",
        ),
        "list_task_lists" => ToolDisplay::builtin(
            "List task lists",
            ToolKind::Read,
            "Listing task lists",
            "list task lists",
        ),
        "get_task_list_status" => ToolDisplay::builtin(
            "Get task list status",
            ToolKind::Read,
            "Reading task list status",
            "read task list status",
        ),
        "update_task" => ToolDisplay::builtin(
            "Update task",
            ToolKind::Edit,
            "Updating task",
            "update task",
        ),
        "update_task_list" | "update_plan" => ToolDisplay::builtin(
            "Update task list",
            ToolKind::Edit,
            "Updating task list",
            "update task list",
        ),
        "request_task_list_handoff" | "request_work_handoff" => ToolDisplay::builtin(
            "Request work handoff",
            ToolKind::Think,
            "Requesting work handoff",
            "request work handoff",
        ),
        "process_run" => ToolDisplay::builtin(
            "Run process",
            ToolKind::Execute,
            "Running process in",
            "run this command",
        ),
        "terminal_run_command" => ToolDisplay::builtin(
            "Run terminal command",
            ToolKind::Execute,
            "Running terminal command in",
            "run this terminal command",
        ),

        "chrome_open" => ToolDisplay::builtin(
            "Chrome open",
            ToolKind::Fetch,
            "Opening Chrome URL",
            "open this Chrome URL",
        ),
        "chrome_snapshot" => ToolDisplay::builtin(
            "Chrome snapshot",
            ToolKind::Read,
            "Reading Chrome snapshot",
            "read Chrome snapshot",
        ),
        "chrome_console_messages" => ToolDisplay::builtin(
            "Chrome console",
            ToolKind::Read,
            "Reading Chrome console",
            "read Chrome console messages",
        ),
        "chrome_network_requests" => ToolDisplay::builtin(
            "Chrome network",
            ToolKind::Read,
            "Reading Chrome network",
            "read Chrome network requests",
        ),
        "chrome_screenshot" => ToolDisplay::builtin(
            "Chrome screenshot",
            ToolKind::Read,
            "Capturing Chrome screenshot",
            "capture Chrome screenshot",
        ),
        "fs_edit_file" | "fs_replace_text" => {
            ToolDisplay::builtin("Edit file", ToolKind::Edit, "Editing", "modify this file")
        }
        "fs_create_text_file" => ToolDisplay::builtin(
            "Create file",
            ToolKind::Edit,
            "Creating",
            "create this file",
        ),
        "fs_create_directory" => ToolDisplay::builtin(
            "Create directory",
            ToolKind::Edit,
            "Creating directory",
            "create this directory",
        ),
        "fs_move_path" => {
            ToolDisplay::builtin("Move path", ToolKind::Move, "Moving", "move this path")
        }
        "fs_copy_path" => {
            ToolDisplay::builtin("Copy path", ToolKind::Edit, "Copying", "copy this path")
        }
        "fs_apply_patch" => ToolDisplay::builtin(
            "Apply patch",
            ToolKind::Edit,
            "Applying patch to",
            "apply this patch",
        ),
        "fs_delete_path" => ToolDisplay::builtin(
            "Delete path",
            ToolKind::Delete,
            "Deleting",
            "delete this path",
        ),
        _ if tool_name.starts_with("mcp__") => {
            let title = mcp_tool_title(tool_name);
            ToolDisplay {
                title: title.clone(),
                kind: ToolKind::Other,
                verb: "Running MCP tool".to_string(),
                permission_operation: title.to_ascii_lowercase(),
                subtitle: Some("MCP tool".to_string()),
                category: Some("mcp".to_string()),
                arguments_summary: None,
            }
        }
        _ => ToolDisplay {
            title: fallback_tool_title(tool_name),
            kind: ToolKind::Other,
            verb: "Running".to_string(),
            permission_operation: format!("run `{tool_name}`"),
            subtitle: None,
            category: None,
            arguments_summary: None,
        },
    }
}

fn permission_family_label(tool_name: &str) -> &'static str {
    match permission_class_for_tool(tool_name) {
        "read_files" => "reading files",
        "edit_files" => "editing files",
        "delete_files" => "deleting files",
        "git_read" => "reading git status",
        "git_write" => "modifying git status",
        "command_run" => "running commands",
        "network" | "browser" => "network access",
        _ => "similar local actions",
    }
}

fn tool_call_title(tool_name: &str, event: &Value) -> String {
    if matches!(tool_name, "set_conversation_title") {
        if let Some(title) = tool_args_from_event(event)
            .and_then(|args| args.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            return format!("Set conversation title: {}", truncate_title(title));
        }
        // ponytail: if a future transport shape hides the title somewhere else, prefer a
        // plain static label over pretending the requested title was `conversation`.
        return "Set conversation title".to_string();
    }
    if matches!(tool_name, "create_job") {
        if let Some(goal) = tool_args_from_event(event)
            .and_then(|args| args.get("goal"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|goal| !goal.is_empty())
        {
            return format!("Create job: {}", truncate_title(goal));
        }
        return "Create job".to_string();
    }
    if matches!(
        tool_name,
        "run_command" | "process_run" | "terminal_run_command"
    ) {
        let command_args = tool_args_from_event(event);
        let command = command_args
            .and_then(|args| args.get("command"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let args = command_args
            .and_then(|args| args.get("args"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .take(4)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !command.is_empty() {
            let suffix = if args.is_empty() {
                String::new()
            } else {
                format!(" {}", args.join(" "))
            };
            let rendered = format!("{command}{suffix}");
            let rendered = if rendered.chars().count() > 80 {
                format!("{}…", rendered.chars().take(79).collect::<String>())
            } else {
                rendered
            };
            return if tool_name == "terminal_run_command" {
                format!("Run terminal command: {rendered}")
            } else if tool_name == "run_command" {
                format!("Run command: {rendered}")
            } else {
                format!("Run process: {rendered}")
            };
        }
    }
    if matches!(tool_name, "update_task") {
        let args = tool_args_from_event(event);
        let subject = args
            .and_then(|args| args.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                args.and_then(|args| args.get("task_id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            });
        let status = args
            .and_then(|args| args.get("status"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        return match (subject, status) {
            (Some(subject), Some(status)) => {
                format!("Update task: {} → {}", truncate_title(subject), status)
            }
            (Some(subject), None) => format!("Update task: {}", truncate_title(subject)),
            (None, Some(status)) => format!("Update task: {status}"),
            (None, None) => "Update task".to_string(),
        };
    }
    if matches!(tool_name, "fs_search_files") {
        let query = tool_args_from_event(event)
            .and_then(|args| args.get("query"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("paths");
        return format!("Search files: {}", truncate_title(query));
    }
    if matches!(tool_name, "fs_find_paths") {
        let glob = tool_args_from_event(event)
            .and_then(|args| args.get("glob"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("paths");
        return format!("Find paths: {}", truncate_title(glob));
    }
    if matches!(
        tool_name,
        "git_status" | "git_diff" | "git_log" | "git_show"
    ) {
        let repo = tool_path(event).unwrap_or("repository");
        return format!(
            "{}: {}",
            tool_display(tool_name).title,
            truncate_title(repo)
        );
    }
    if matches!(tool_name, "fs_move_path" | "fs_copy_path") {
        let args = tool_args_from_event(event);
        let source = args
            .and_then(|a| a.get("source_path"))
            .and_then(Value::as_str)
            .unwrap_or("source");
        let destination = args
            .and_then(|a| a.get("destination_path"))
            .and_then(Value::as_str)
            .unwrap_or("destination");
        return format!(
            "{}: {} → {}",
            tool_display(tool_name).title,
            truncate_title(source),
            truncate_title(destination)
        );
    }
    if matches!(tool_name, "fs_delete_path") {
        let path = tool_path(event).unwrap_or("path");
        return format!("Delete path: {}", truncate_title(path));
    }
    if matches!(tool_name, "fs_read_text_file" | "fs.read_text_file") {
        let path = tool_path(event).unwrap_or("file");
        return format!("Read file: {}", truncate_title(path));
    }
    if matches!(tool_name, "fs_list_directory") {
        let path = tool_path(event).unwrap_or("directory");
        return format!("List directory: {}", truncate_title(path));
    }
    if matches!(tool_name, "fs_stat") {
        let path = tool_path(event).unwrap_or("path");
        return format!("Stat path: {}", truncate_title(path));
    }
    if matches!(tool_name, "fs_edit_file" | "fs_replace_text") {
        let path = tool_path(event).unwrap_or("file");
        return format!("Edit file: {}", truncate_title(path));
    }
    if matches!(tool_name, "fs_create_text_file") {
        let path = tool_path(event).unwrap_or("file");
        return format!("Create file: {}", truncate_title(path));
    }
    if matches!(tool_name, "fs_create_directory") {
        let path = tool_path(event).unwrap_or("directory");
        return format!("Create directory: {}", truncate_title(path));
    }
    if matches!(tool_name, "fs_apply_patch") {
        let path = tool_path(event).unwrap_or("patch target");
        return format!("Apply patch: {}", truncate_title(path));
    }
    if matches!(tool_name, "chrome_open") {
        if let Some(url) = tool_url(event) {
            return format!("Open page: {}", truncate_title(url));
        }
    }
    tool_display(tool_name).title
}

fn truncate_title(value: &str) -> String {
    if value.chars().count() > 60 {
        format!("{}…", value.chars().take(59).collect::<String>())
    } else {
        value.to_string()
    }
}

fn tool_kind_str(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        ToolKind::Other => "other",
        _ => "other",
    }
}

fn tool_target_kind(tool_name: &str) -> &'static str {
    match tool_name {
        "fs_list_directory" => "directory",
        "fs_find_paths" => "directory",
        "fs_search_files" => "directory",
        "fs_stat" => "path",
        "fs_create_directory" => "directory",
        "fs_move_path" | "fs_copy_path" => "path",
        "fs_apply_patch" => "patch",
        "git_status" | "git_diff" | "git_log" | "git_show" | "git_add" | "git_restore"
        | "git_commit" | "git_stash" => "repository",
        "web_fetch" | "local_web_fetch" => "url",
        "process_run" | "terminal_run_command" => "command",
        "chrome_open" => "url",
        "chrome_snapshot"
        | "chrome_console_messages"
        | "chrome_network_requests"
        | "chrome_screenshot" => "chrome",
        "fs_delete_path" => "path",
        _ => "file",
    }
}

fn tool_path(event: &Value) -> Option<&str> {
    tool_args_from_event(event)
        .and_then(|v| {
            v.get("path")
                .or_else(|| v.get("source_path"))
                .or_else(|| v.get("destination_path"))
                .or_else(|| v.get("base_path"))
                .or_else(|| v.get("cwd"))
                .or_else(|| v.get("url"))
        })
        .and_then(Value::as_str)
}

fn tool_url(event: &Value) -> Option<&str> {
    tool_args_from_event(event)
        .and_then(|v| v.get("url"))
        .and_then(Value::as_str)
}

fn tool_command(event: &Value) -> Option<&str> {
    tool_args_from_event(event)
        .and_then(|v| v.get("command"))
        .and_then(Value::as_str)
}

fn tool_locations_from_event(tool_name: &str, event: &Value) -> Option<Vec<ToolCallLocation>> {
    if !tool_supports_input_location(tool_name, event) {
        return None;
    }
    let path = tool_path(event)?;
    let path_buf = PathBuf::from(path);
    if path_buf.is_dir() {
        return None;
    }
    let mut location = ToolCallLocation::new(path_buf);
    if let Some(line) = tool_args_from_event(event)
        .and_then(|v| v.get("line"))
        .and_then(Value::as_u64)
        .filter(|line| *line > 0)
    {
        location = location.line(Some(line.min(u32::MAX as u64) as u32));
    }
    Some(vec![location])
}

fn tool_supports_input_location(tool_name: &str, event: &Value) -> bool {
    match tool_name {
        "fs_read_text_file"
        | "fs.read_text_file"
        | "fs_edit_file"
        | "fs_replace_text"
        | "fs_create_text_file" => true,
        "fs_delete_path" => tool_args_from_event(event)
            .and_then(|v| v.get("expected_kind"))
            .and_then(Value::as_str)
            .map(|kind| kind == "file")
            .unwrap_or(false),
        _ => false,
    }
}

fn friendly_tool_status(tool_name: &str, event: &Value, phase: &str) -> String {
    let display = ToolDisplay::from_event(tool_name, event);
    let target = display
        .subtitle
        .as_deref()
        .or_else(|| tool_path(event))
        .unwrap_or("the selected workspace target");
    match phase {
        "preparing" => format!("Preparing: {}.", display.title),
        "permission" => format!(
            "Waiting for approval: {}. Target: `{target}`.",
            display.permission_operation
        ),
        "running" => format!("{} `{target}`…", display.verb),
        _ => format!("{} `{target}`…", display.verb),
    }
}

fn is_generic_completion_text(text: &str) -> bool {
    let normalized = text.trim().trim_end_matches('.').to_ascii_lowercase();
    normalized == "completed" || normalized.ends_with(" completed")
}

fn is_meaningful_terminal_tool_text(tool_name: &str, text: &str) -> bool {
    let normalized = text.trim().trim_end_matches('.').to_ascii_lowercase();
    if normalized.is_empty() || is_generic_completion_text(&normalized) {
        return false;
    }

    // ACP replaces a tool card rather than patching it. These bare result-kind labels carry no
    // outcome, and must not replace the useful request/running summary retained for the card.
    let tool_label = tool_name.replace(['_', '.'], " ").to_ascii_lowercase();
    !matches!(
        normalized.as_str(),
        "file" | "files" | "directory" | "directories" | "result" | "results"
    ) && normalized != tool_label
}
fn tool_status_from_str(status: &str) -> ToolCallStatus {
    match status {
        "pending" => ToolCallStatus::Pending,
        "running" | "in_progress" => ToolCallStatus::InProgress,
        "completed" | "complete" | "ok" | "success" => ToolCallStatus::Completed,
        "failed" | "error" => ToolCallStatus::Failed,
        "incomplete" => ToolCallStatus::InProgress,
        _ => ToolCallStatus::Pending,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl SurfaceToolStatus {
    fn from_str(status: &str) -> Self {
        match status {
            "running" | "in_progress" | "incomplete" => Self::InProgress,
            "completed" | "complete" | "ok" | "success" => Self::Completed,
            "failed" | "error" => Self::Failed,
            _ => Self::Pending,
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    fn rank(self) -> u8 {
        match self {
            Self::Pending => 0,
            Self::InProgress => 1,
            Self::Completed | Self::Failed => 2,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

fn should_emit_surface_tool_status(
    previous: Option<SurfaceToolStatus>,
    next: SurfaceToolStatus,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    if previous.is_terminal() {
        return false;
    }
    next.rank() >= previous.rank()
}

async fn current_surface_tool_status(
    shared_state: &AdapterSharedState,
    session_id: &str,
    tool_call_id: &str,
) -> Option<SurfaceToolStatus> {
    let key = format!("{session_id}\n{tool_call_id}");
    shared_state
        .surface_tool_statuses
        .lock()
        .await
        .get(&key)
        .copied()
}

async fn clear_surface_tool_statuses_for_session(
    shared_state: &AdapterSharedState,
    session_id: &str,
) {
    let prefix = format!("{session_id}\n");
    shared_state
        .surface_tool_statuses
        .lock()
        .await
        .retain(|key, _| !key.starts_with(&prefix));
}

async fn terminalize_tool_cards_for_turn(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    reason: &str,
) -> Result<()> {
    let reason = reason.trim();
    let reason = if reason.is_empty() {
        "Tool result unavailable.".to_string()
    } else {
        truncate_for_log(reason, 240)
    };
    let records = shared_state.tool_tasks.list_for_session(session_id).await;
    for record in records
        .into_iter()
        .filter(|record| record.turn_token == Some(turn_token))
    {
        let displayed_nonterminal =
            current_surface_tool_status(shared_state, session_id, &record.tool_call_id)
                .await
                .is_some_and(|status| !status.is_terminal());
        if !displayed_nonterminal {
            continue;
        }
        send_tool_call_update_for_turn(
            shared_state,
            session_id,
            turn_token,
            &record.tool_call_id,
            &record.tool_name,
            ToolCallUpdatePayload {
                status: "failed",
                text: &reason,
                request: Some(ToolRequestPresentation {
                    tool_call_id: record.tool_call_id.clone(),
                    tool_name: record.tool_name.clone(),
                    arguments: record.input_args,
                    display: record.display,
                }),
                raw_output: None,
                extra_content: Vec::new(),
            },
        )
        .await?;
    }
    Ok(())
}

async fn record_surface_tool_status(
    shared_state: &AdapterSharedState,
    session_id: &str,
    tool_call_id: &str,
    next: SurfaceToolStatus,
) -> bool {
    let key = format!("{session_id}\n{tool_call_id}");
    let mut statuses = shared_state.surface_tool_statuses.lock().await;
    let previous = statuses.get(&key).copied();
    if !should_emit_surface_tool_status(previous, next) {
        if bear_debug_verbose()
            && (previous.is_some_and(SurfaceToolStatus::is_terminal) || next.is_terminal())
        {
            eprintln!(
                "bear-armature: suppressing duplicate/non-monotonic tool surface update session_id={} tool_call_id={} previous={} next={}",
                session_id,
                tool_call_id,
                previous.map(SurfaceToolStatus::as_str).unwrap_or("none"),
                next.as_str()
            );
        }
        return false;
    }
    statuses.insert(key, next);
    true
}

fn tool_card_title(tool_name: &str, event: Option<&Value>, display: &ToolDisplay) -> String {
    match event {
        Some(event) if event_display_from_event(event).is_none() => {
            tool_call_title(tool_name, event)
        }
        _ => display.title.clone(),
    }
}

pub(crate) async fn send_terminal_tool_call_update(
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    title: String,
    summary: String,
    terminal_id: String,
) -> Result<()> {
    let display = tool_display(tool_name);
    let tool_call = ToolCall::new(tool_call_id.to_string(), title)
        .kind(display.kind)
        .status(ToolCallStatus::InProgress)
        .content(vec![
            ToolCallContent::from(summary),
            ToolCallContent::Terminal(Terminal::new(terminal_id)),
        ]);
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": serde_json::to_value(SessionUpdate::ToolCall(tool_call))?,
        }),
    )
    .await
}

#[derive(Clone, Debug)]
pub(crate) struct ToolRequestPresentation {
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) arguments: Option<Value>,
    pub(crate) display: Option<Value>,
}

impl ToolRequestPresentation {
    fn from_event(tool_call_id: &str, tool_name: &str, event: &Value) -> Self {
        Self {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: tool_args_from_event(event).cloned(),
            display: event_display_from_event(event).cloned(),
        }
    }

    pub(crate) fn projection_event(&self) -> Value {
        let mut event = json!({
            "data": {
                "tool_call": {
                    "id": self.tool_call_id,
                    "name": self.tool_name,
                }
            }
        });
        if let Some(arguments) = self.arguments.as_ref() {
            event["data"]["tool_call"]["arguments"] = arguments.clone();
        }
        if let Some(display) = self.display.as_ref() {
            event["data"]["tool_call"]["display"] = display.clone();
        }
        event
    }
}

struct ToolCallUpdatePayload<'a> {
    status: &'a str,
    text: &'a str,
    request: Option<ToolRequestPresentation>,
    raw_output: Option<Value>,
    extra_content: Vec<ToolCallContent>,
}

/// Project a tool update emitted from a detached local-tool task. The task may
/// execute concurrently, but its ACP write is serialized behind any live
/// BearWire event-page projection for this session.
async fn send_detached_tool_call_update_for_turn(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    tool_call_id: &str,
    tool_name: &str,
    payload: ToolCallUpdatePayload<'_>,
) -> Result<()> {
    let _projection = shared_state
        .projection_dispatcher
        .begin_detached_projection(session_id, turn_token)
        .await;
    send_tool_call_update_for_turn(
        shared_state,
        session_id,
        turn_token,
        tool_call_id,
        tool_name,
        payload,
    )
    .await
}

pub(crate) async fn send_tool_call_update_for_turn(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    tool_call_id: &str,
    tool_name: &str,
    payload: ToolCallUpdatePayload<'_>,
) -> Result<()> {
    if is_current_prompt_turn(shared_state, session_id, turn_token, "tool_call_update").await {
        let surface_status = SurfaceToolStatus::from_str(payload.status);
        if !record_surface_tool_status(shared_state, session_id, tool_call_id, surface_status).await
        {
            return Ok(());
        }
        let text = if surface_status.is_terminal()
            && !is_meaningful_terminal_tool_text(tool_name, payload.text)
        {
            shared_state
                .tool_tasks
                .get(session_id, tool_call_id)
                .await
                .and_then(|record| record.visible_summary)
                .unwrap_or(payload.text.to_string())
        } else {
            payload.text.to_string()
        };
        if !surface_status.is_terminal() {
            shared_state
                .tool_tasks
                .remember_visible_summary(session_id, tool_call_id, &text)
                .await;
        }
        let payload = ToolCallUpdatePayload {
            text: &text,
            ..payload
        };
        send_tool_call_update(session_id, tool_call_id, tool_name, payload).await
    } else {
        Ok(())
    }
}

#[derive(Debug)]
struct ToolOutcome {
    status: ToolCallStatus,
    text: String,
    raw_output: Option<Value>,
    extra_content: Vec<ToolCallContent>,
}

impl ToolOutcome {
    fn new(
        status: &str,
        text: &str,
        raw_output: Option<Value>,
        extra_content: Vec<ToolCallContent>,
    ) -> Self {
        Self {
            status: tool_status_from_str(status),
            text: text.to_string(),
            raw_output,
            extra_content,
        }
    }
}

fn project_tool_call(request: &ToolRequestPresentation, outcome: ToolOutcome) -> ToolCall {
    let event = request.projection_event();
    let display = ToolDisplay::from_event(&request.tool_name, &event);
    let mut content = Vec::new();
    let trimmed_text = outcome.text.trim();
    if !trimmed_text.is_empty() && trimmed_text != "Completed." {
        content.push(ToolCallContent::from(trimmed_text.to_string()));
    }
    content.extend(outcome.extra_content);
    let title = tool_card_title(&request.tool_name, Some(&event), &display);
    let mut tool_call = ToolCall::new(request.tool_call_id.clone(), title)
        .kind(display.kind)
        .status(outcome.status)
        .content(content);
    if let Some(locations) = tool_locations_from_event(&request.tool_name, &event) {
        tool_call = tool_call.locations(locations);
    }
    if let Some(args) = request.arguments.as_ref() {
        tool_call = tool_call.raw_input(Some(compact_tool_card_json_value(args.clone())));
    }
    if let Some(raw_output) = outcome.raw_output {
        tool_call = tool_call.raw_output(Some(compact_tool_card_json_value(raw_output)));
    }
    tool_call
}

async fn send_tool_call_update(
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    payload: ToolCallUpdatePayload<'_>,
) -> Result<()> {
    let ToolCallUpdatePayload {
        status,
        text,
        request,
        raw_output,
        extra_content,
    } = payload;
    let request = request.unwrap_or_else(|| ToolRequestPresentation {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        arguments: None,
        display: None,
    });
    let outcome = ToolOutcome::new(status, text, raw_output, extra_content);
    let tool_call = project_tool_call(&request, outcome);
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": serde_json::to_value(SessionUpdate::ToolCall(tool_call))?,
        }),
    )
    .await
}

async fn is_current_prompt_turn(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    update_kind: &str,
) -> bool {
    let active = shared_state.active_prompts.lock().await;
    let ok = active
        .get(session_id)
        .is_some_and(|turn| turn.token == turn_token);
    if !ok {
        eprintln!(
            "bear-armature: dropped stale turn update session_id={} turn_token={} update_kind={}",
            session_id, turn_token, update_kind
        );
    }
    ok
}

fn text_chunk_update(kind: &str, text: &str) -> Result<Value> {
    let chunk = ContentChunk::new(ContentBlock::from(text.to_string()));
    let update = match kind {
        "user" => SessionUpdate::UserMessageChunk(chunk),
        "agent" => SessionUpdate::AgentMessageChunk(chunk),
        "thought" => SessionUpdate::AgentThoughtChunk(chunk),
        _ => return Err(anyhow!("unknown chunk kind {kind}")),
    };
    Ok(serde_json::to_value(update)?)
}

async fn send_user_message_chunk(session_id: &str, text: &str) -> Result<()> {
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": text_chunk_update("user", text)?,
        }),
    )
    .await
}

async fn send_agent_message_chunk(session_id: &str, text: &str) -> Result<()> {
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": text_chunk_update("agent", text)?,
        }),
    )
    .await
}

async fn send_agent_message_chunk_for_turn(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    text: &str,
) -> Result<()> {
    if is_current_prompt_turn(shared_state, session_id, turn_token, "agent_message_chunk").await {
        send_agent_message_chunk(session_id, text).await
    } else {
        Ok(())
    }
}

async fn send_agent_thought_chunk(session_id: &str, text: &str) -> Result<()> {
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": text_chunk_update("thought", text)?,
        }),
    )
    .await
}

async fn send_agent_thought_chunk_for_turn(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    text: &str,
) -> Result<()> {
    if is_current_prompt_turn(shared_state, session_id, turn_token, "agent_thought_chunk").await {
        send_agent_thought_chunk(session_id, text).await
    } else {
        Ok(())
    }
}

/// Adapter-local mirror of Den runtime status text normalization.
///
/// Keep this aligned with Den's shared chat display helper. It is intentionally limited to
/// adapter-/Den-owned operational status units before they become ACP thought chunks; never apply
/// it to assistant text deltas or raw model reasoning deltas.
fn normalize_thought_chunk_text(text: &str) -> std::borrow::Cow<'_, str> {
    if text.ends_with(char::is_whitespace) {
        return std::borrow::Cow::Borrowed(text);
    }
    if text.ends_with('.') || text.ends_with('!') || text.ends_with('?') || text.ends_with(':') {
        return std::borrow::Cow::Owned(format!("{text}\n"));
    }
    std::borrow::Cow::Owned(format!("{text}.\n"))
}

fn should_emit_notification(is_headless: bool, method: &str) -> bool {
    !(is_headless && method == "session/update")
}

async fn write_notification(method: &str, params: Value) -> Result<()> {
    if !should_emit_notification(headless_mode(), method) {
        return Ok(());
    }
    JsonRpcTransport::default().notify(method, params).await
}

async fn write_response(id: impl Into<Option<Value>>, result: Result<Value, Value>) -> Result<()> {
    let id = id.into();
    let message = match result {
        Ok(result) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
        Err(error) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": error,
        }),
    };
    write_json(message).await
}

#[allow(dead_code)]
fn with_adapter_contract(mut payload: Value) -> Value {
    if !payload.is_object() {
        payload = json!({ "value": payload });
    }
    payload["adapter_contract"] = adapter_contract_context();
    payload
}

fn authenticate_json_rpc_error(err: &anyhow::Error, runtime: &RuntimeConfig) -> Value {
    let message = format!("{err:#}");
    if runtime.config.is_none() || looks_like_configuration_error(err) {
        return configuration_error(Some(json!({
            "message": message,
            "problems": runtime.diagnostics,
            "hint": "Configure DEN_API_URL, BEAR_SLUG, and DEN_TOKEN/DEN_TOKEN_ENV in the ACP agent server environment, then restart the agent server.",
        })));
    }
    auth_check_json_rpc_error(err, None)
}

fn auth_check_json_rpc_error(err: &anyhow::Error, token_hint: Option<&str>) -> Value {
    let message = format!("{err:#}");
    if looks_like_den_connectivity_error(err) {
        return den_connectivity_error(Some(json!({
            "message": format!("Could not reach the BEARS Den server while checking the Code token: {message}"),
            "hint": "Check that DEN_API_URL is correct and that the Den API server is online/reachable. This does not necessarily mean your token is invalid. If a session is open, /doctor still works for adapter-local diagnostics.",
        })));
    }
    let mut data = json!({
        "message": format!("BEARS Code token authentication failed: {message}"),
    });
    if message.contains("diagnostics:") {
        data["hint"] = json!(
            "Den returned token diagnostics. Inspect token_found, bear_found, token_bound_to_bear, token_owner_is_bear_member, and required_scope_present to identify whether DEN_API_URL, BEAR_SLUG, token value, Bear grant, or membership is wrong."
        );
    } else if let Some(hint) = token_hint {
        data["hint"] = json!(hint);
    }
    token_validation_error(Some(data))
}

fn looks_like_configuration_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("Missing DEN_TOKEN")
            || message.contains("Missing DEN_API_URL")
            || message.contains("Missing BEAR_SLUG")
            || message.contains("DEN_TOKEN_ENV points at")
    })
}

fn looks_like_den_connectivity_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        if let Some(reqwest_err) = cause.downcast_ref::<reqwest::Error>() {
            return reqwest_err.is_connect()
                || reqwest_err.is_timeout()
                || reqwest_err.is_request()
                || reqwest_err.is_body();
        }
        if let Some(http_err) = cause.downcast_ref::<DenHttpError>() {
            return matches!(
                http_err.status,
                reqwest::StatusCode::BAD_GATEWAY
                    | reqwest::StatusCode::SERVICE_UNAVAILABLE
                    | reqwest::StatusCode::GATEWAY_TIMEOUT
            ) || http_err.status.is_server_error();
        }
        let message = cause.to_string();
        (message.contains("BearWire RPC")
            || message.contains("ACP auth-check")
            || message.contains("Den server"))
            && (message.contains("HTTP 502")
                || message.contains("HTTP 503")
                || message.contains("HTTP 504")
                || message.contains("Gateway Timeout")
                || message.contains("Bad Gateway")
                || message.contains("Service Unavailable"))
    })
}

fn configuration_error(data: Option<Value>) -> Value {
    json_rpc_error(-32010, "BEARS configuration incomplete", data)
}

fn token_validation_error(data: Option<Value>) -> Value {
    json_rpc_error(-32011, "BEARS Code token validation failed", data)
}

fn den_connectivity_error(data: Option<Value>) -> Value {
    json_rpc_error(-32012, "BEARS Den server unreachable", data)
}

#[allow(dead_code)]
fn den_compatibility_status_message(body: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(body).ok()?;
    match value.get("error_code").and_then(Value::as_str)? {
        "adapter_out_of_date" => {
            let message = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("The BEARS ACP adapter is older than this Den server.");
            let action = value
                .get("suggested_action")
                .and_then(Value::as_str)
                .unwrap_or("Update bear-armature and restart your ACP client.");
            Some(format!("{message}\n\n{action}"))
        }
        "den_out_of_date" => {
            let message = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("This BEARS Den server is older than the ACP adapter.");
            let action = value
                .get("suggested_action")
                .and_then(Value::as_str)
                .unwrap_or("Deploy the matching BEARS Den server or use an older adapter.");
            Some(format!("{message}\n\n{action}"))
        }
        _ => None,
    }
}

fn json_rpc_error(code: i64, message: &str, data: Option<Value>) -> Value {
    match data {
        Some(data) => json!({ "code": code, "message": message, "data": data }),
        None => json!({ "code": code, "message": message }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        extract::State,
        http::Request,
        response::{IntoResponse, Response},
        routing::any,
        Json, Router,
    };
    use std::net::SocketAddr;
    use std::sync::Mutex as StdMutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn terminal_tool_card_text_keeps_prior_summary_when_completion_is_a_bare_result_kind() {
        assert!(!is_meaningful_terminal_tool_text(
            "fs_search_files",
            "files"
        ));
        assert!(!is_meaningful_terminal_tool_text(
            "fs_search_files",
            "Completed."
        ));
        assert!(is_meaningful_terminal_tool_text(
            "fs_search_files",
            "Found 3 matches in 2 files."
        ));
    }

    #[test]
    fn workspace_git_remote_origins_reads_and_deduplicates_origins() {
        let root = std::env::temp_dir().join(format!("bear-armature-origin-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .status()
            .unwrap();
        Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/bears-ai/bear-den.git",
            ])
            .current_dir(&root)
            .status()
            .unwrap();
        let root = root.to_string_lossy().to_string();
        assert_eq!(
            workspace_git_remote_origins(&[root.clone(), root.clone()]),
            vec!["https://github.com/bears-ai/bear-den.git"]
        );
        fs::remove_dir_all(std::path::Path::new(&root)).unwrap();
    }

    #[test]
    fn truncate_for_log_preserves_utf8_boundaries() {
        let input = format!("{}§tail", "x".repeat(239));

        assert_eq!(
            truncate_for_log(&input, 240),
            format!("{}...", "x".repeat(239))
        );
    }

    #[test]
    fn headless_suppresses_only_acp_session_updates() {
        assert!(!should_emit_notification(true, "session/update"));
        assert!(should_emit_notification(true, "session/request_permission"));
        assert!(should_emit_notification(false, "session/update"));
    }

    #[derive(Clone)]
    struct BearWireTestServerState {
        fail_bearwire: bool,
        paths: Arc<TokioMutex<Vec<String>>>,
        rpc_methods: Arc<TokioMutex<Vec<String>>>,
        events: Arc<TokioMutex<Vec<Value>>>,
        history_messages: Arc<TokioMutex<Vec<Value>>>,
        conversation_title: Arc<TokioMutex<Option<String>>>,
    }

    // ponytail: this is a narrow BearWire mock for ACP boundary tests, not a fake server.
    // Keep these mocked methods in sync with `services/den/crates/den-bearwire/src/methods`;
    // if a BearWire method contract changes, update this mock in the same change.
    // Do not make tests generous by emitting downstream success projections directly; script
    // the upstream BearWire cause first (for example tool_call.requested/completed before a
    // session_info_update) so ACP tests fail when the bridge stops exposing the causal event.
    async fn bearwire_test_handler(
        State(state): State<BearWireTestServerState>,
        request: Request<Body>,
    ) -> Response {
        let path = request.uri().path().to_string();
        state.paths.lock().await.push(path.clone());
        if path.starts_with("/bearwire/v1/sessions/") && path.ends_with("/events/page") {
            let events = state.events.lock().await.clone();
            let events = events
                .into_iter()
                .enumerate()
                .map(|(index, event)| {
                    json!({
                        "sequence": index + 1,
                        "event": event,
                    })
                })
                .collect::<Vec<_>>();
            let next_after = events.len();
            return Json(json!({
                "ok": true,
                "events": events,
                "next_after": next_after,
                "has_more": false,
            }))
            .into_response();
        }
        if path != "/bearwire/v1/rpc" {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({ "error": "not found" })),
            )
                .into_response();
        }
        if state.fail_bearwire {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "bearwire unavailable" })),
            )
                .into_response();
        }

        let body = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            state.rpc_methods.lock().await.push(method.to_string());
        }
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        let conversation_title = state.conversation_title.lock().await.clone();
        let result = match value.get("method").and_then(Value::as_str) {
            Some("initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocol": "bearwire", "version": 1 }
            }),
            Some("session.open") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "ok": true,
                    "session": {
                        "client_session_id": value.pointer("/params/session_id").and_then(Value::as_str).unwrap_or("acp-test-session"),
                        "conversation_id": value.pointer("/params/conversation_id").and_then(Value::as_str).unwrap_or("default"),
                        "resolved_conversation_id": null,
                        "cwd": value.pointer("/params/cwd").and_then(Value::as_str).unwrap_or("/workspace"),
                        "current_mode": value.pointer("/params/mode").and_then(Value::as_str).unwrap_or("ask")
                    }
                }
            }),
            Some("session.state") => {
                if let Some(session_id) =
                    value.pointer("/params/session_id").and_then(Value::as_str)
                {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "kind": "single",
                            "session": {
                                "client_session_id": session_id,
                                "conversation_id": "default",
                                "resolved_conversation_id": "den-conv-test",
                                "history_conversation_id": "den-conv-test",
                                "cwd": "/workspace",
                                "current_mode": "ask",
                                "conversation_title": conversation_title.clone(),
                                "conversation_title_updated_at": conversation_title.as_ref().map(|_| "2026-07-07T00:00:00Z"),
                                "context_budget": {
                                    "model": "openai/test-model",
                                    "context_window": 200000,
                                    "max_output_tokens": 4096,
                                    "reserved_output_tokens": 4096,
                                    "estimated_input_tokens": 48904,
                                    "estimated_total_tokens": 53000,
                                    "estimate_precision": "approximate",
                                    "near_budget": false,
                                    "over_budget": false,
                                    "components": []
                                }
                            }
                        }
                    })
                } else {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "kind": "session_state",
                            "sessions": [{
                                "acp_session_id": "session-1",
                                "conversation_id": "default",
                                "resolved_conversation_id": "den-conv-test",
                                "history_conversation_id": "den-conv-test",
                                "cwd": "/workspace",
                                "updated_at": "2026-07-07T00:00:00Z",
                                "current_mode": "ask",
                                "conversation_title": conversation_title.clone(),
                                "conversation_title_updated_at": conversation_title.as_ref().map(|_| "2026-07-07T00:00:00Z")
                            }]
                        }
                    })
                }
            }
            Some("session.model.get") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "selection_mode": "auto",
                    "model": null,
                    "effective_model": "openai/test-model"
                }
            }),
            Some("conversation.history") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "kind": "conversation_history",
                    "conversation_id": value.pointer("/params/conversation_id").and_then(Value::as_str).unwrap_or("default"),
                    "messages": state.history_messages.lock().await.clone(),
                    "has_more": false,
                    "next_before": null
                }
            }),
            Some("conversation.surface_history") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "kind": "conversation_surface_history",
                    "conversation_id": value.pointer("/params/conversation_id").and_then(Value::as_str).unwrap_or("default"),
                    "surface_events": state.history_messages.lock().await.clone(),
                    "has_more": false,
                    "next_before": null
                }
            }),
            Some("run.start") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "run_id": "run-test-title",
                    "event_sequence": 1
                }
            }),
            Some("resource.update") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "ok": true }
            }),
            other => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("unknown method: {other:?}") }
            }),
        };
        (axum::http::StatusCode::OK, Json(result)).into_response()
    }

    async fn start_bearwire_test_server(
        fail_bearwire: bool,
    ) -> (String, Arc<TokioMutex<Vec<String>>>) {
        start_bearwire_test_server_with_events(fail_bearwire, Vec::new()).await
    }

    async fn start_bearwire_test_server_with_events(
        fail_bearwire: bool,
        events: Vec<Value>,
    ) -> (String, Arc<TokioMutex<Vec<String>>>) {
        let (api_url, paths, _rpc_methods) =
            start_bearwire_test_server_with_events_and_methods(fail_bearwire, events).await;
        (api_url, paths)
    }

    async fn start_bearwire_test_server_with_events_and_methods(
        fail_bearwire: bool,
        events: Vec<Value>,
    ) -> (
        String,
        Arc<TokioMutex<Vec<String>>>,
        Arc<TokioMutex<Vec<String>>>,
    ) {
        let paths = Arc::new(TokioMutex::new(Vec::new()));
        let rpc_methods = Arc::new(TokioMutex::new(Vec::new()));
        let state = BearWireTestServerState {
            fail_bearwire,
            paths: paths.clone(),
            rpc_methods: rpc_methods.clone(),
            events: Arc::new(TokioMutex::new(events)),
            history_messages: Arc::new(TokioMutex::new(Vec::new())),
            conversation_title: Arc::new(TokioMutex::new(None)),
        };
        let app = Router::new()
            .route("/bearwire/v1/rpc", any(bearwire_test_handler))
            .fallback(any(bearwire_test_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), paths, rpc_methods)
    }

    async fn start_bearwire_test_server_with_history(
        history_messages: Vec<Value>,
    ) -> (String, Arc<TokioMutex<Vec<String>>>) {
        start_bearwire_test_server_with_history_and_title(history_messages, None).await
    }

    async fn start_bearwire_test_server_with_history_and_title(
        history_messages: Vec<Value>,
        conversation_title: Option<&str>,
    ) -> (String, Arc<TokioMutex<Vec<String>>>) {
        start_bearwire_test_server_with_events_history_and_title(
            Vec::new(),
            history_messages,
            conversation_title,
        )
        .await
    }

    async fn start_bearwire_test_server_with_events_history_and_title(
        events: Vec<Value>,
        history_messages: Vec<Value>,
        conversation_title: Option<&str>,
    ) -> (String, Arc<TokioMutex<Vec<String>>>) {
        let paths = Arc::new(TokioMutex::new(Vec::new()));
        let state = BearWireTestServerState {
            fail_bearwire: false,
            paths: paths.clone(),
            rpc_methods: Arc::new(TokioMutex::new(Vec::new())),
            events: Arc::new(TokioMutex::new(events)),
            history_messages: Arc::new(TokioMutex::new(history_messages)),
            conversation_title: Arc::new(TokioMutex::new(conversation_title.map(str::to_string))),
        };
        let app = Router::new()
            .route("/bearwire/v1/rpc", any(bearwire_test_handler))
            .fallback(any(bearwire_test_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), paths)
    }

    fn test_config(api_url: String) -> Config {
        Config {
            api_url,
            bear: "test-bear".to_string(),
            token: "bear_arm_test_token".to_string(),
            client: "zed".to_string(),
        }
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("bear-armature-{name}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_adapter_state(session_id: &str, root: &Path) -> AdapterState {
        let mut state = AdapterState::default();
        state.session_contexts.insert(
            session_id.to_string(),
            SessionContext {
                cwd: root.to_string_lossy().to_string(),
                roots: vec![root.to_string_lossy().to_string()],
                ..Default::default()
            },
        );
        state
    }

    fn test_runtime_config(api_url: String) -> RuntimeConfig {
        RuntimeConfig {
            config: Some(test_config(api_url.clone())),
            diagnostics: Vec::new(),
            check_server: false,
            doctor: false,
            headless: false,
            update_command: None,
            browser_bridge: None,
            api_url,
            bear: "test-bear".to_string(),
            token_env: "DEN_TOKEN".to_string(),
            client: "zed".to_string(),
        }
    }

    async fn run_acp_request_for_test(
        http: &reqwest::Client,
        runtime: &mut RuntimeConfig,
        adapter_state: &mut AdapterState,
        shared_state: &AdapterSharedState,
        value: Value,
    ) -> Result<()> {
        let request = request_from_value(value)?;
        handle_request(http, runtime, adapter_state, shared_state, request).await
    }

    fn test_shared_state() -> AdapterSharedState {
        let (cancellation_tx, _) = broadcast::channel(8);
        AdapterSharedState {
            transport: JsonRpcTransport::default(),
            client_capabilities: Arc::new(TokioMutex::new(Value::Null)),
            session_contexts: Arc::new(TokioMutex::new(HashMap::new())),
            last_plan_update_hashes: Arc::new(TokioMutex::new(HashMap::new())),
            surface_tool_statuses: Arc::new(TokioMutex::new(HashMap::new())),
            tool_tasks: ToolTaskRegistry::default(),
            mcp_registry: McpRegistry::default(),
            approval_cache: ApprovalCache::default(),
            cancellation_tx,
            active_prompts: Arc::new(TokioMutex::new(HashMap::new())),
            projection_dispatcher: AcpProjectionDispatcher::default(),
        }
    }

    #[tokio::test]
    async fn validate_den_code_token_uses_bearwire() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("BEARS_LEGACY_ACP_HTTP");
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, paths) = start_bearwire_test_server(false).await;
        let http = reqwest::Client::new();

        validate_den_code_token(&http, &test_config(api_url))
            .await
            .expect("bearwire token validation");

        let paths = paths.lock().await.clone();
        assert_eq!(paths, vec!["/bearwire/v1/rpc", "/bearwire/v1/rpc"]);
    }

    #[tokio::test]
    async fn den_get_session_uses_bearwire_session_state() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, paths) = start_bearwire_test_server(false).await;
        let http = reqwest::Client::new();

        let session = den_get_acp_session(&http, &test_config(api_url), "acp-test-session")
            .await
            .expect("load BearWire session state");

        assert_eq!(session["client_session_id"], "acp-test-session");
        assert_eq!(session["cwd"], "/workspace");
        assert_eq!(session["resolved_conversation_id"], "den-conv-test");
        assert_eq!(session["context_budget"]["estimated_total_tokens"], 53000);
        let paths = paths.lock().await.clone();
        assert_eq!(paths, vec!["/bearwire/v1/rpc"]);
    }

    #[tokio::test]
    async fn validate_den_code_token_does_not_fallback_to_acp_auth_check() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("BEARS_LEGACY_ACP_HTTP");
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, paths) = start_bearwire_test_server(true).await;
        let http = reqwest::Client::new();

        let err = validate_den_code_token(&http, &test_config(api_url))
            .await
            .expect_err("bearwire validation should fail");
        assert!(format!("{err:#}").contains("BearWire RPC initialize HTTP 502"));

        let paths = paths.lock().await.clone();
        assert_eq!(paths, vec!["/bearwire/v1/rpc"]);
        assert!(!paths.iter().any(|path| path.contains("/acp/")));
    }

    #[test]
    fn render_den_runtime_status_includes_orientation_and_governance() {
        let runtime_state = json!({
            "session": {
                "diagnostics": {
                    "runtime_session_live": true,
                    "runtime_state": {
                        "run": {
                            "run_id": "run-123",
                            "stance": "pair",
                            "governance": "interactive",
                            "objective_orientation_kind": "focused",
                            "focused_job_id": "job-123"
                        },
                        "agent_loop_control": { "level": "careful" },
                        "task_focus": {
                            "active": true,
                            "next_incomplete_task_title": "Ship status command"
                        },
                        "docket": {
                            "active_job_id": "job-123",
                            "active_task_id": "task-456",
                            "source": "objective_orientation"
                        }
                    }
                }
            }
        });

        let lines = render_den_runtime_status(&runtime_state);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("live=true"));
        assert!(lines[0].contains("governance=interactive"));
        assert!(lines[0].contains("orientation=focused"));
        assert!(lines[0].contains("focused_job=job-123"));
        assert!(lines[0].contains("loop=careful"));
        assert!(lines[1].contains("active=true"));
        assert!(lines[1].contains("next=Ship status command"));
        assert!(lines[2].contains("job=job-123"));
        assert!(lines[2].contains("task=task-456"));
        assert!(lines[2].contains("source=objective_orientation"));
    }

    #[test]
    fn auth_check_error_hint_mentions_armature_scope() {
        let err = anyhow!("token failed");
        let value = auth_check_json_rpc_error(
            &err,
            Some("Generate a fresh Den armature token for this bear. Tokens must include armature:chat."),
        );

        let hint = value
            .pointer("/data/hint")
            .and_then(Value::as_str)
            .expect("hint");
        assert!(hint.contains("armature:chat"));
        assert!(!hint.contains("acp:chat"));
    }

    #[test]
    fn bearwire_initialize_gateway_timeout_is_connectivity_error_not_token_error() {
        let err = anyhow!("BearWire RPC initialize HTTP 504 Gateway Timeout: Gateway Timeout");
        let value = auth_check_json_rpc_error(
            &err,
            Some("Generate a fresh Den armature token for this bear. Tokens must include armature:chat."),
        );

        assert_eq!(value["message"], "BEARS Den server unreachable");
        let data = value.get("data").expect("data");
        let message = data
            .get("message")
            .and_then(Value::as_str)
            .expect("message");
        let hint = data.get("hint").and_then(Value::as_str).expect("hint");
        assert!(message.contains("Could not reach the BEARS Den server"));
        assert!(message.contains("BearWire RPC initialize HTTP 504"));
        assert!(hint.contains("does not necessarily mean your token is invalid"));
        assert!(!hint.contains("armature:chat"));
    }

    #[test]
    fn prompt_block_shape_counts_blocks_and_provenance_without_content() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "hello"},
                {"type": "resource", "resource": {"uri": "file:///tmp/a", "text": "{\"system_alert\":\"client synthetic summary\"}"}},
                {"type": "resource_link", "uri": "file:///tmp/b"},
                {"type": "image", "data": "..."}
            ]
        });
        let shape = prompt_block_shape(&params);

        assert_eq!(shape.text, 1);
        assert_eq!(shape.resource, 1);
        assert_eq!(shape.resource_link, 1);
        assert_eq!(shape.other, 1);
        assert_eq!(shape.human_text, 1);
        assert_eq!(shape.client_synthetic_context, 1);
        assert_eq!(shape.client_resource, 1);
        assert_eq!(shape.unsupported, 1);
    }

    #[test]
    fn text_block_is_human_message() {
        let params = json!({
            "prompt": [{"type": "text", "text": "Please continue."}]
        });
        let classification = classify_prompt_block(&params["prompt"][0]);
        let prompt = prompt_text_from_params(&params).unwrap();
        let display_prompt = prompt_display_text_from_params(&params).unwrap();

        assert_eq!(
            classification.provenance,
            AcpPromptBlockProvenance::HumanText
        );
        assert!(classification.include_in_human_message());
        assert!(classification.include_in_display());
        assert_eq!(prompt, "Please continue.");
        assert_eq!(display_prompt, "Please continue.");
    }

    #[test]
    fn resource_block_is_client_context_not_human_message() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "Please inspect this."},
                {"type": "resource", "resource": {"uri": "file:///tmp/a", "name": "a.txt", "text": "file contents"}}
            ]
        });
        let classification = classify_prompt_block(&params["prompt"][1]);
        let prompt = prompt_text_from_params(&params).unwrap();
        let display_prompt = prompt_display_text_from_params(&params).unwrap();

        assert_eq!(
            classification.provenance,
            AcpPromptBlockProvenance::ClientResource
        );
        assert!(!classification.include_in_human_message());
        assert!(!classification.include_in_display());
        assert_eq!(prompt, "Please inspect this.");
        assert_eq!(display_prompt, "Please inspect this.");
        assert!(!prompt.contains("file contents"));
        assert!(!display_prompt.contains("file contents"));
    }

    #[test]
    fn synthetic_resource_is_client_synthetic_context() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "Please continue."},
                {"type": "resource", "resource": {"uri": "zed://system", "text": "{\"system_alert\":\"client synthetic summary from zed\"}"}}
            ]
        });
        let classification = classify_prompt_block(&params["prompt"][1]);
        let prompt = prompt_text_from_params(&params).unwrap();

        assert_eq!(
            classification.provenance,
            AcpPromptBlockProvenance::ClientSyntheticContext
        );
        assert_eq!(
            classification.diagnostic_flags,
            vec!["likely_client_synthetic_context"]
        );
        assert!(!classification.include_in_human_message());
        assert_eq!(prompt, "Please continue.");
        assert!(!prompt.contains("system_alert"));
        assert!(!prompt.contains("synthetic summary"));
    }

    #[test]
    fn user_pasted_system_alert_text_remains_human_text() {
        let params = json!({
            "prompt": [{
                "type": "text",
                "text": "I pasted this debugging payload intentionally: {\"system_alert\":\"raw fixture\"}"
            }]
        });
        let classification = classify_prompt_block(&params["prompt"][0]);
        let shape = prompt_block_shape(&params);
        let prompt = prompt_text_from_params(&params).unwrap();

        assert_eq!(
            classification.provenance,
            AcpPromptBlockProvenance::HumanPastedDebugText
        );
        assert!(classification.include_in_human_message());
        assert_eq!(shape.text, 1);
        assert_eq!(shape.human_pasted_debug_text, 1);
        assert!(prompt.contains("system_alert"));
    }

    #[test]
    fn history_replay_boundaries_separate_adjacent_user_messages() {
        let messages = history_replay_chunks_with_boundaries(vec![
            ReloadHistoryMessage::text("1", "user", "first prompt"),
            ReloadHistoryMessage::text("2", "user", "second prompt after failed turn"),
        ]);

        assert_eq!(messages[0].text, "first prompt");
        assert_eq!(messages[1].text, "\n\nsecond prompt after failed turn");
    }

    #[test]
    fn history_replay_boundaries_do_not_modify_alternating_roles() {
        let messages = history_replay_chunks_with_boundaries(vec![
            ReloadHistoryMessage::text("1", "user", "prompt"),
            ReloadHistoryMessage::text("2", "assistant", "reply"),
            ReloadHistoryMessage::text("3", "user", "follow-up"),
        ]);

        assert_eq!(
            messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["prompt", "reply", "follow-up"]
        );
    }

    #[test]
    fn history_replay_includes_user_and_assistant_text_chunks() {
        assert_eq!(
            history_replay_text_update_kind(&ReloadHistoryMessage::text("1", "user", "prompt")),
            Some("user")
        );
        assert_eq!(
            history_replay_text_update_kind(&ReloadHistoryMessage::text("2", "assistant", "reply")),
            Some("agent")
        );
        assert_eq!(
            history_replay_text_update_kind(&ReloadHistoryMessage::text("3", "agent", "reply")),
            Some("agent")
        );
    }

    #[test]
    fn reload_history_accepts_content_alias_for_message_text() {
        let message = reload_history_message_from_value(json!({
            "kind": "message",
            "role": "assistant",
            "content": "reply stored as content"
        }))
        .expect("content alias should parse")
        .expect("content alias should not be dropped");
        assert_eq!(message.text, "reply stored as content");
        assert_eq!(history_replay_text_update_kind(&message), Some("agent"));
    }

    #[test]
    fn sparse_surface_tool_history_is_rejected_before_replay() {
        let err = reload_history_message_from_value(json!({
            "kind": "tool_result",
            "tool_call_id": "call-1",
            "status": "ok"
        }))
        .expect_err("structured tool history missing name should fail");

        let message = format!("{err:#}");
        assert!(message.contains("tool_name"), "unexpected error: {message}");
    }

    #[test]
    fn structured_surface_tool_history_parses_when_complete() {
        let message = reload_history_message_from_value(json!({
            "kind": "tool_result",
            "tool_call_id": "call-1",
            "tool_name": "run_command",
            "status": "ok",
            "raw_output": {"content": "done"}
        }))
        .expect("complete structured tool history should parse")
        .expect("tool history should not be filtered");

        assert_eq!(message.kind, "tool_result");
        assert_eq!(message.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(message.tool_name.as_deref(), Some("run_command"));
        assert_eq!(message.status.as_deref(), Some("ok"));
    }

    #[test]
    fn structured_surface_session_info_history_parses_title_update() {
        let message = reload_history_message_from_value(json!({
            "kind": "session_info_update",
            "session_id": "session-1",
            "title": "Loaded title",
            "title_updated_at": "2026-01-02T03:04:05Z",
            "current_mode": "write"
        }))
        .expect("session info surface event should parse")
        .expect("session info surface event should not be filtered");

        assert_eq!(message.kind, "session_info_update");
        assert_eq!(message.title.as_deref(), Some("Loaded title"));
        assert_eq!(
            message.title_updated_at.as_deref(),
            Some("2026-01-02T03:04:05Z")
        );
    }

    #[test]
    fn structured_surface_session_info_history_accepts_current_mode_only() {
        let message = reload_history_message_from_value(json!({
            "kind": "session_info_update",
            "session_id": "session-1",
            "current_mode": "ask"
        }))
        .expect("mode-only session info surface event should parse")
        .expect("session info surface event should not be filtered");

        assert_eq!(message.kind, "session_info_update");
        assert_eq!(message.title, None);
        assert_eq!(message.title_updated_at, None);
    }

    #[test]
    fn structured_surface_reasoning_history_parses_delta_not_as_message() {
        let message = reload_history_message_from_value(json!({
            "kind": "reasoning_delta",
            "delta": "private reasoning",
            "source": "provider_reasoning",
            "replay_policy": "thought"
        }))
        .expect("reasoning surface event should parse")
        .expect("reasoning surface event should not be filtered");

        assert_eq!(message.kind, "reasoning_delta");
        assert_eq!(message.text, "private reasoning");
        assert_eq!(message.replay_policy.as_deref(), Some("thought"));
    }

    #[test]
    fn sparse_surface_reasoning_history_is_rejected_before_replay() {
        let err = reload_history_message_from_value(json!({
            "kind": "reasoning_delta",
            "source": "provider_reasoning",
            "replay_policy": "thought"
        }))
        .expect_err("structured reasoning history missing text should fail");

        let message = format!("{err:#}");
        assert!(message.contains("text"), "unexpected error: {message}");
    }

    #[test]
    fn prompt_den_message_without_resources_is_plain_human_message() {
        let params = json!({
            "prompt": [{"type": "text", "text": "Please continue."}]
        });
        let bundle = prompt_context_from_params(&params).unwrap();
        let prompt_context = bearwire_prompt_context_from_context(&bundle);

        assert!(prompt_context.is_null());
    }

    #[test]
    fn prompt_den_message_includes_reference_host_context_without_resource_body() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "Please inspect this."},
                {"type": "resource", "resource": {
                    "uri": "file:///tmp/a",
                    "name": "a.txt",
                    "mimeType": "text/plain",
                    "text": "file contents"
                }}
            ]
        });
        let bundle = prompt_context_from_params(&params).unwrap();
        let prompt_context = bearwire_prompt_context_from_context(&bundle);

        assert_eq!(prompt_context["format"], "acp_prompt_context.v1");
        assert_eq!(
            prompt_context["host_context"]["kind"],
            "referenced_resources"
        );
        assert_eq!(prompt_context["host_context"]["delivery"], "reference_only");
        assert_eq!(
            prompt_context["host_context"]["persistence"],
            "not_human_message"
        );
        assert_eq!(
            prompt_context["host_context"]["resources"][0]["uri"],
            "file:///tmp/a"
        );
        assert_eq!(
            prompt_context["host_context"]["resources"][0]["name"],
            "a.txt"
        );
        assert_eq!(
            prompt_context["host_context"]["resources"][0]["embedded_text_bytes"],
            13
        );
        assert_eq!(
            prompt_context["host_context"]["resources"][0]["label"],
            "a.txt"
        );
        assert_eq!(bundle.human_message, "Please inspect this.");
    }

    #[test]
    fn prompt_den_message_omits_diagnostic_only_synthetic_resource_context() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "Please continue."},
                {"type": "resource", "resource": {
                    "uri": "zed://system",
                    "text": "{\"system_alert\":\"client synthetic summary from zed\"}"
                }}
            ]
        });
        let bundle = prompt_context_from_params(&params).unwrap();
        let prompt_context = bearwire_prompt_context_from_context(&bundle);

        assert!(prompt_context.is_null());
    }

    #[test]
    fn prompt_context_extracts_resource_reference_without_body_in_human_message() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "Please inspect this."},
                {"type": "resource", "resource": {
                    "uri": "file:///tmp/a",
                    "name": "a.txt",
                    "mimeType": "text/plain",
                    "text": "file contents"
                }}
            ]
        });
        let bundle = prompt_context_from_params(&params).unwrap();

        assert_eq!(bundle.human_message, "Please inspect this.");
        assert_eq!(bundle.resource_references.len(), 1);
        let reference = &bundle.resource_references[0];
        assert_eq!(reference.block_type, AcpPromptBlockType::Resource);
        assert_eq!(
            reference.provenance,
            AcpPromptBlockProvenance::ClientResource
        );
        assert_eq!(reference.uri.as_deref(), Some("file:///tmp/a"));
        assert_eq!(reference.name.as_deref(), Some("a.txt"));
        assert_eq!(reference.mime_type.as_deref(), Some("text/plain"));
        assert_eq!(reference.text_bytes, Some("file contents".len()));
        assert_eq!(
            reference.delivery_policy,
            AcpPromptContextDeliveryPolicy::ReferenceOnly
        );
        assert_eq!(bundle.diagnostics.resource_bodies_not_in_human_message, 1);
        assert!(!bundle.human_message.contains("file contents"));
    }

    #[test]
    fn synthetic_resource_is_diagnostic_only_context() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "Please continue."},
                {"type": "resource", "resource": {
                    "uri": "zed://system",
                    "text": "{\"system_alert\":\"client synthetic summary from zed\"}"
                }}
            ]
        });
        let bundle = prompt_context_from_params(&params).unwrap();

        assert_eq!(bundle.human_message, "Please continue.");
        assert_eq!(bundle.diagnostics.synthetic_context_omitted, 1);
        assert_eq!(bundle.resource_references.len(), 1);
        assert_eq!(
            bundle.resource_references[0].provenance,
            AcpPromptBlockProvenance::ClientSyntheticContext
        );
        assert_eq!(
            bundle.resource_references[0].delivery_policy,
            AcpPromptContextDeliveryPolicy::DiagnosticOnly
        );
        assert!(!bundle.human_message.contains("system_alert"));
    }

    #[test]
    fn resource_link_is_reference_not_human_message() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "Please consider this reference."},
                {"type": "resource_link", "uri": "file:///tmp/b", "name": "b.txt"}
            ]
        });
        let classification = classify_prompt_block(&params["prompt"][1]);
        let prompt = prompt_text_from_params(&params).unwrap();
        let display_prompt = prompt_display_text_from_params(&params).unwrap();
        let bundle = prompt_context_from_params(&params).unwrap();

        assert_eq!(
            classification.provenance,
            AcpPromptBlockProvenance::ClientResource
        );
        assert!(!classification.include_in_human_message());
        assert!(!classification.include_in_display());
        assert_eq!(prompt, "Please consider this reference.");
        assert_eq!(display_prompt, "Please consider this reference.");
        assert_eq!(bundle.resource_references.len(), 1);
        assert_eq!(
            bundle.resource_references[0].block_type,
            AcpPromptBlockType::ResourceLink
        );
        assert_eq!(
            bundle.resource_references[0].uri.as_deref(),
            Some("file:///tmp/b")
        );
        assert_eq!(bundle.resource_references[0].name.as_deref(), Some("b.txt"));
        assert_eq!(
            bundle.resource_references[0].delivery_policy,
            AcpPromptContextDeliveryPolicy::ReferenceOnly
        );
        assert!(!prompt.contains("Referenced resource"));
        assert!(!display_prompt.contains("Referenced resource"));
    }

    #[tokio::test]
    async fn adapter_explicit_session_cancel_cancels_active_turn_and_tools() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await.unwrap();
                if n == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
            }
            stream = reader.into_inner();
            use tokio::io::AsyncWriteExt;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}",
                )
                .await
                .unwrap();
        });
        let config = Config {
            api_url: format!("http://{addr}"),
            bear: "test-bear".to_string(),
            token: "token-test".to_string(),
            client: "zed".to_string(),
        };
        let shared = test_shared_state();
        let turn_token = Uuid::new_v4();
        shared.active_prompts.lock().await.insert(
            "acp-session".to_string(),
            ActivePromptTurn {
                token: turn_token,
                response: PromptResponseGuard::new(json!("cancel-prompt-exactly-once")),
                conversation_id: Some("conv-1".to_string()),
            },
        );
        assert!(
            shared
                .tool_tasks
                .try_register(
                    "acp-session",
                    "call-local",
                    "fs_read_text_file",
                    Some(turn_token),
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        assert!(
            record_surface_tool_status(
                &shared,
                "acp-session",
                "call-local",
                SurfaceToolStatus::InProgress,
            )
            .await
        );
        assert!(
            shared
                .tool_tasks
                .try_register(
                    "acp-session",
                    "call-den-display",
                    "memory_search",
                    Some(turn_token),
                    ToolExecutionOwnership::DenDisplayOnly,
                )
                .await
        );
        shared
            .tool_tasks
            .remember_presentation(
                "acp-session",
                "call-den-display",
                "memory_search",
                json!({ "query": "liveness" }),
                None,
            )
            .await;
        assert!(
            record_surface_tool_status(
                &shared,
                "acp-session",
                "call-den-display",
                SurfaceToolStatus::InProgress,
            )
            .await
        );
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let http = reqwest::Client::new();

        let (result, output) = capture_json_output_for_test(|| async {
            handle_session_cancel(
                &http,
                &config,
                &shared,
                json!({ "sessionId": "acp-session" }),
            )
            .await
        })
        .await;
        result.unwrap();

        assert!(shared
            .active_prompts
            .lock()
            .await
            .get("acp-session")
            .is_none());
        assert!(shared
            .tool_tasks
            .list_for_session("acp-session")
            .await
            .is_empty());
        let prompt_responses = output
            .iter()
            .filter(|frame| frame.get("id") == Some(&json!("cancel-prompt-exactly-once")))
            .collect::<Vec<_>>();
        assert_eq!(
            prompt_responses.len(),
            1,
            "cancellation must answer the original prompt exactly once: {output:#?}"
        );
        for tool_call_id in ["call-local", "call-den-display"] {
            assert!(
                output.iter().any(|frame| {
                    frame.get("method").and_then(Value::as_str) == Some("session/update")
                        && frame.to_string().contains(tool_call_id)
                        && frame.to_string().contains("\"status\":\"failed\"")
                        && frame.to_string().contains("Cancelled by user")
                }),
                "displayed card {tool_call_id} must be terminalized before prompt cleanup: {output:#?}"
            );
        }
        let notice = cancel_rx.recv().await.expect("cancellation notice");
        assert_eq!(notice.session_id, "acp-session");
        assert_eq!(notice.turn_token, Some(turn_token));
        assert_eq!(notice.conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(notice.scope, CancellationScope::PromptAndTools);
        assert_eq!(notice.origin, CancellationOrigin::SessionCancel);
    }

    #[tokio::test]
    async fn adapter_session_close_notification_posts_den_archive_and_cancels_local_state() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let request_line = Arc::new(TokioMutex::new(None::<String>));
        let request_line_for_server = request_line.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            let mut first_line = None;
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await.unwrap();
                if first_line.is_none() && n > 0 {
                    first_line = Some(line.trim().to_string());
                }
                if n == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
            }
            *request_line_for_server.lock().await = first_line;
            stream = reader.into_inner();
            use tokio::io::AsyncWriteExt;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}",
                )
                .await
                .unwrap();
        });
        let config = Config {
            api_url: format!("http://{addr}"),
            bear: "test-bear".to_string(),
            token: "token-test".to_string(),
            client: "zed".to_string(),
        };
        let mut runtime = RuntimeConfig {
            config: Some(config),
            diagnostics: Vec::new(),
            check_server: false,
            doctor: false,
            headless: false,
            update_command: None,
            browser_bridge: None,
            api_url: String::new(),
            bear: String::new(),
            token_env: String::new(),
            client: "zed".to_string(),
        };
        let mut adapter_state = AdapterState::default();
        let shared = test_shared_state();
        let turn_token = Uuid::new_v4();
        shared.active_prompts.lock().await.insert(
            "acp-session".to_string(),
            ActivePromptTurn {
                token: turn_token,
                response: PromptResponseGuard::new(json!("close-prompt-exactly-once")),
                conversation_id: Some("conv-1".to_string()),
            },
        );
        assert!(
            shared
                .tool_tasks
                .try_register(
                    "acp-session",
                    "call-1",
                    "fs_read_text_file",
                    Some(turn_token),
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let http = reqwest::Client::new();

        let (result, output) = capture_json_output_for_test(|| async {
            handle_request(
                &http,
                &mut runtime,
                &mut adapter_state,
                &shared,
                JsonRpcRequest {
                    id: None,
                    method: "session/close".to_string(),
                    params: json!({ "sessionId": "acp-session" }),
                },
            )
            .await
        })
        .await;
        result.unwrap();

        let request_line = request_line.lock().await.clone().unwrap_or_default();
        assert!(
            request_line.starts_with("POST /bearwire/v1/rpc "),
            "request_line={request_line:?}"
        );
        assert!(shared
            .active_prompts
            .lock()
            .await
            .get("acp-session")
            .is_none());
        assert!(shared
            .tool_tasks
            .list_for_session("acp-session")
            .await
            .is_empty());
        assert_eq!(
            output
                .iter()
                .filter(|frame| frame.get("id") == Some(&json!("close-prompt-exactly-once")))
                .count(),
            1,
            "session close must settle the active prompt exactly once: {output:#?}"
        );
        let notice = cancel_rx.recv().await.expect("close cancellation notice");
        assert_eq!(notice.session_id, "acp-session");
        assert_eq!(notice.turn_token, Some(turn_token));
        assert_eq!(notice.conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(notice.scope, CancellationScope::PromptAndTools);
        assert_eq!(notice.origin, CancellationOrigin::SessionClose);
    }

    #[tokio::test]
    async fn adapter_session_cancel_notification_cancels_active_turn_and_tools() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await.unwrap();
                if n == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
            }
            stream = reader.into_inner();
            use tokio::io::AsyncWriteExt;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}",
                )
                .await
                .unwrap();
        });
        let config = Config {
            api_url: format!("http://{addr}"),
            bear: "test-bear".to_string(),
            token: "token-test".to_string(),
            client: "zed".to_string(),
        };
        let mut runtime = RuntimeConfig {
            config: Some(config),
            diagnostics: Vec::new(),
            check_server: false,
            doctor: false,
            headless: false,
            update_command: None,
            browser_bridge: None,
            api_url: String::new(),
            bear: String::new(),
            token_env: String::new(),
            client: "zed".to_string(),
        };
        let mut adapter_state = AdapterState::default();
        let shared = test_shared_state();
        let turn_token = Uuid::new_v4();
        shared.active_prompts.lock().await.insert(
            "acp-session".to_string(),
            ActivePromptTurn {
                token: turn_token,
                response: PromptResponseGuard::new(json!("test")),
                conversation_id: Some("conv-1".to_string()),
            },
        );
        assert!(
            shared
                .tool_tasks
                .try_register(
                    "acp-session",
                    "call-1",
                    "fs_read_text_file",
                    Some(turn_token),
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let http = reqwest::Client::new();

        handle_request(
            &http,
            &mut runtime,
            &mut adapter_state,
            &shared,
            JsonRpcRequest {
                id: None,
                method: "session/cancel".to_string(),
                params: json!({ "sessionId": "acp-session" }),
            },
        )
        .await
        .unwrap();

        assert!(shared
            .active_prompts
            .lock()
            .await
            .get("acp-session")
            .is_none());
        assert!(shared
            .tool_tasks
            .list_for_session("acp-session")
            .await
            .is_empty());
        let notice = cancel_rx.recv().await.expect("cancellation notice");
        assert_eq!(notice.session_id, "acp-session");
        assert_eq!(notice.turn_token, Some(turn_token));
        assert_eq!(notice.conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(notice.scope, CancellationScope::PromptAndTools);
        assert_eq!(notice.origin, CancellationOrigin::SessionCancel);
    }

    #[tokio::test]
    async fn adapter_same_conversation_overlap_sends_cancellation_for_previous_turn() {
        let shared = test_shared_state();
        let previous_token = Uuid::new_v4();
        let next_token = Uuid::new_v4();
        register_prompt_turn_for_session(
            &shared,
            "acp-session",
            previous_token,
            Some("conv-1".to_string()),
            PromptResponseGuard::new(json!(1)),
        )
        .await;
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let previous = register_prompt_turn_for_session(
            &shared,
            "acp-session",
            next_token,
            Some("conv-1".to_string()),
            PromptResponseGuard::new(json!(2)),
        )
        .await
        .expect("previous turn returned");
        let notice = cancel_rx.recv().await.expect("cancellation notice");

        assert_eq!(previous.token, previous_token);
        assert_eq!(previous.response.claim(), Some(json!(1)));
        assert_eq!(
            previous.response.claim(),
            None,
            "superseded prompt handler must not send a second response"
        );
        assert_eq!(notice.session_id, "acp-session");
        assert_eq!(notice.turn_token, Some(previous_token));
        assert_eq!(notice.conversation_id.as_deref(), Some("conv-1"));
        assert!(cancellation_matches_turn(
            &notice,
            "acp-session",
            previous_token,
            Some("conv-1")
        ));
    }

    #[tokio::test]
    async fn adapter_different_conversation_overlap_does_not_cancel_previous_turn() {
        let shared = test_shared_state();
        let previous_token = Uuid::new_v4();
        let next_token = Uuid::new_v4();
        register_prompt_turn_for_session(
            &shared,
            "acp-session",
            previous_token,
            Some("conv-1".to_string()),
            PromptResponseGuard::new(json!(1)),
        )
        .await;
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let previous = register_prompt_turn_for_session(
            &shared,
            "acp-session",
            next_token,
            Some("conv-2".to_string()),
            PromptResponseGuard::new(json!(2)),
        )
        .await
        .expect("previous turn returned");

        assert_eq!(previous.token, previous_token);
        assert_eq!(previous.response.claim(), Some(json!(1)));
        assert_eq!(
            previous.response.claim(),
            None,
            "superseded prompt handler must not send a second response"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), cancel_rx.recv())
                .await
                .is_err(),
            "different-conversation overlap should not send cancellation"
        );
    }

    #[test]
    fn adapter_identifies_only_expected_tool_execution_claim_rejections() {
        assert!(is_tool_execution_claim_rejection(&json!({
            "ok": false,
            "status": "claim_rejected"
        })));
        assert!(!is_tool_execution_claim_rejection(&json!({
            "ok": false,
            "status": "invalid_obligation"
        })));
        assert!(!is_tool_execution_claim_rejection(&json!({ "ok": true })));
    }

    #[test]
    fn adapter_parses_tool_execution_lease() {
        let lease = parse_tool_execution_lease(&json!({
            "ok": true,
            "attempt_token": "attempt-1",
            "renew_after_ms": 10_000,
        }))
        .expect("valid lease");

        assert_eq!(lease.attempt_token, "attempt-1");
        assert_eq!(lease.renew_after, Duration::from_secs(10));
    }

    #[test]
    fn adapter_rejects_incomplete_or_rejected_tool_execution_lease() {
        for response in [
            json!({"ok": false, "status": "claim_rejected"}),
            json!({"ok": true, "renew_after_ms": 10_000}),
            json!({"ok": true, "attempt_token": "attempt-1"}),
            json!({"ok": true, "attempt_token": "attempt-1", "renew_after_ms": 0}),
        ] {
            assert!(parse_tool_execution_lease(&response).is_err(), "{response}");
        }
    }

    #[tokio::test]
    async fn adapter_tool_wait_ignores_unrelated_cancellation_notice() {
        let shared = test_shared_state();
        let turn_token = Uuid::new_v4();
        let cancellation_rx = shared.cancellation_tx.subscribe();
        let sender = shared.cancellation_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let _ = sender.send(CancellationNotice {
                session_id: "other-session".to_string(),
                turn_token: None,
                conversation_id: None,
                scope: CancellationScope::ToolsOnly,
                origin: CancellationOrigin::RunTerminal,
            });
        });

        let outcome = wait_for_tool_future_or_matching_cancellation(
            cancellation_rx,
            "acp-session",
            turn_token,
            None,
            async {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                42
            },
        )
        .await;

        match outcome {
            ToolTaskWaitOutcome::ToolFinished(value) => assert_eq!(value, 42),
            ToolTaskWaitOutcome::Cancelled(notice) => {
                panic!("unrelated cancellation should have been ignored: {notice:?}")
            }
        }
    }

    #[tokio::test]
    async fn adapter_tool_wait_observes_cancellation_sent_before_wait_begins() {
        let shared = test_shared_state();
        let turn_token = Uuid::new_v4();
        let cancellation_rx = shared.cancellation_tx.subscribe();
        let side_effect_reached = Arc::new(TokioMutex::new(false));
        let side_effect_for_future = side_effect_reached.clone();
        shared
            .cancellation_tx
            .send(CancellationNotice {
                session_id: "acp-session".to_string(),
                turn_token: Some(turn_token),
                conversation_id: None,
                scope: CancellationScope::ToolsOnly,
                origin: CancellationOrigin::RunTerminal,
            })
            .expect("send cancellation before wait");

        let outcome = wait_for_tool_future_or_matching_cancellation(
            cancellation_rx,
            "acp-session",
            turn_token,
            None,
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                *side_effect_for_future.lock().await = true;
            },
        )
        .await;

        assert!(matches!(outcome, ToolTaskWaitOutcome::Cancelled(_)));
        assert!(!*side_effect_reached.lock().await);
    }

    #[tokio::test]
    async fn adapter_tool_wait_stops_on_matching_cancellation_notice() {
        let shared = test_shared_state();
        let turn_token = Uuid::new_v4();
        let cancellation_rx = shared.cancellation_tx.subscribe();
        let sender = shared.cancellation_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let _ = sender.send(CancellationNotice {
                session_id: "acp-session".to_string(),
                turn_token: Some(turn_token),
                conversation_id: None,
                scope: CancellationScope::PromptAndTools,
                origin: CancellationOrigin::SessionCancel,
            });
        });

        let outcome = wait_for_tool_future_or_matching_cancellation(
            cancellation_rx,
            "acp-session",
            turn_token,
            None,
            async {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                42
            },
        )
        .await;

        match outcome {
            ToolTaskWaitOutcome::Cancelled(notice) => {
                assert_eq!(notice.session_id, "acp-session");
                assert_eq!(notice.turn_token, Some(turn_token));
            }
            ToolTaskWaitOutcome::ToolFinished(value) => {
                panic!("matching cancellation should have won before tool result {value}")
            }
        }
    }

    #[test]
    fn parse_status_slash_command() {
        assert_eq!(
            parse_local_slash_command("/status"),
            Some(LocalSlashCommand::Status)
        );
    }

    #[test]
    fn parse_focus_job_id_from_prompt() {
        let job_id = "11111111-1111-1111-1111-111111111111";
        assert_eq!(
            focus_job_id_from_prompt(&format!("/focus {job_id}")),
            Some(job_id.to_string())
        );
        assert_eq!(
            focus_prompt_target("/focus 11111111"),
            FocusPromptTarget::JobIdPrefix("11111111".to_string())
        );
        assert_eq!(
            focus_prompt_target("/focus 11111111-1111"),
            FocusPromptTarget::JobIdPrefix("11111111-1111".to_string())
        );
        assert_eq!(
            focus_prompt_target("/focus 1111111"),
            FocusPromptTarget::Invalid
        );
        assert_eq!(
            focus_prompt_target("/focus 11111111z"),
            FocusPromptTarget::Invalid
        );
        assert_eq!(
            focus_prompt_target("/focus"),
            FocusPromptTarget::ConversationAssociated
        );
        assert_eq!(focus_job_id_from_prompt("/focus"), None);
        assert_eq!(
            focus_prompt_target("/focus nope"),
            FocusPromptTarget::Invalid
        );
        assert_eq!(focus_job_id_from_prompt("/focus nope"), None);
        assert_eq!(
            focus_prompt_target(&format!("/focus {job_id} extra")),
            FocusPromptTarget::Invalid
        );
        assert_eq!(
            focus_job_id_from_prompt(&format!("/focus {job_id} extra")),
            None
        );
    }

    #[test]
    fn focus_requires_running_canonical_attempt_and_started_native_run() {
        let started = json!({
            "pair_binding": {
                "control": {
                    "kind": "docket",
                    "state": "running",
                    "attempt_id": "11111111-1111-1111-1111-111111111111",
                    "attempt_state": "running",
                    "launch_state": "started"
                },
                "run": { "id": "run_123" }
            }
        });
        assert_eq!(confirmed_focus_run_id(&started).unwrap(), "run_123");

        let already_running = json!({
            "pair_binding": {
                "control": {
                    "kind": "docket",
                    "state": "running",
                    "attempt_id": "11111111-1111-1111-1111-111111111111",
                    "attempt_state": "running",
                    "launch_state": "already_running"
                },
                "run": { "id": "run_123" }
            }
        });
        assert_eq!(confirmed_focus_run_id(&already_running).unwrap(), "run_123");

        for invalid in [
            json!({ "pair_binding": { "run": { "id": "run_123" } } }),
            json!({ "pair_binding": {
                "control": {
                    "kind": "docket", "state": "running",
                    "attempt_id": "11111111-1111-1111-1111-111111111111",
                    "attempt_state": "authorized", "launch_state": "started"
                },
                "run": { "id": "run_123" }
            }}),
            json!({ "pair_binding": {
                "control": {
                    "kind": "docket", "state": "running",
                    "attempt_id": "11111111-1111-1111-1111-111111111111",
                    "attempt_state": "running", "launch_state": "missing"
                },
                "run": { "id": "run_123" }
            }}),
            json!({ "pair_binding": {
                "control": {
                    "kind": "docket", "state": "running",
                    "attempt_id": "11111111-1111-1111-1111-111111111111",
                    "attempt_state": "running", "launch_state": "started"
                },
                "run": { "id": null }
            }}),
        ] {
            assert!(confirmed_focus_run_id(&invalid).is_err());
        }
    }

    #[test]
    fn project_focused_acp_title_is_idempotent() {
        assert_eq!(
            project_focused_acp_title(Some("Roadmap".to_string())).as_deref(),
            Some("⌖ Roadmap")
        );
        assert_eq!(
            project_focused_acp_title(Some("⌖ Roadmap".to_string())).as_deref(),
            Some("⌖ Roadmap")
        );
    }

    #[test]
    fn task_list_status_docket_jobs_ignore_session_only_refs() {
        let job_id = "11111111-1111-1111-1111-111111111111";
        let other_job_id = "22222222-2222-2222-2222-222222222222";
        let status = json!({
            "task_list": {
                "items": [
                    {"source_ref": {"refs": ["docket_job:<none>", "docket_task:task-only"]}},
                    {"source_ref": {"refs": [format!("docket_job:{job_id}")]}},
                    {"source_ref": {"refs": [format!("docket_job:{job_id}")]}},
                    {"source_ref": {"refs": [format!("docket_job:{other_job_id}")]}}
                ]
            }
        });

        assert_eq!(
            docket_job_ids_from_task_list_status(&status),
            vec![job_id.to_string(), other_job_id.to_string()]
        );
    }

    #[test]
    fn den_session_state_docket_jobs_use_active_activity_plan() {
        let job_id = "11111111-1111-1111-1111-111111111111";
        let session_state = json!({
            "diagnostics": {
                "active_activity_plan": {
                    "items": [
                        {"source_ref": {"refs": ["docket_job:<none>"]}},
                        {"source_ref": {"refs": [format!("docket_job:{job_id}")]}}
                    ]
                }
            },
            "adapter_environment": {
                "stale": {"refs": ["docket_job:22222222-2222-2222-2222-222222222222"]}
            }
        });

        assert_eq!(
            docket_job_ids_from_den_session_state(&session_state),
            vec![job_id.to_string()]
        );
    }

    #[test]
    fn focus_job_choice_lines_include_status_and_goal() {
        let lines = focus_job_choice_lines(&[DocketJobListEntry {
            id: "11111111-1111-1111-1111-111111111111".to_string(),
            status: "ready".to_string(),
            goal: "Advance the roadmap with evidence-backed slices".to_string(),
        }]);

        assert!(lines.contains("/focus 11111111-1111-1111-1111-111111111111"));
        assert!(lines.contains("ready — Advance the roadmap with evidence-backed slices"));
    }

    #[test]
    fn focus_noncompleted_jobs_ignore_completed_candidates() {
        let jobs = vec![
            DocketJobListEntry {
                id: "11111111-1111-1111-1111-111111111111".to_string(),
                status: "completed".to_string(),
                goal: "Done work".to_string(),
            },
            DocketJobListEntry {
                id: "22222222-2222-2222-2222-222222222222".to_string(),
                status: "ready".to_string(),
                goal: "Current work".to_string(),
            },
        ];

        assert_eq!(
            focus_noncompleted_jobs(&jobs),
            vec![DocketJobListEntry {
                id: "22222222-2222-2222-2222-222222222222".to_string(),
                status: "ready".to_string(),
                goal: "Current work".to_string(),
            }]
        );
    }

    #[test]
    fn focus_choice_jobs_append_completed_only_when_under_cap() {
        let mut jobs = (0..11)
            .map(|index| DocketJobListEntry {
                id: format!("00000000-0000-0000-0000-{index:012}"),
                status: "ready".to_string(),
                goal: format!("Ready job {index}"),
            })
            .collect::<Vec<_>>();
        jobs.push(DocketJobListEntry {
            id: "99999999-9999-9999-9999-999999999999".to_string(),
            status: "completed".to_string(),
            goal: "Completed overflow".to_string(),
        });

        let choices = focus_choice_jobs(&jobs);
        assert_eq!(choices.len(), 10);
        assert!(choices.iter().all(|job| !job.is_completed()));

        let choices = focus_choice_jobs(&jobs[0..2]);
        assert_eq!(choices.len(), 2);

        let choices = focus_choice_jobs(&jobs[10..]);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[1].status, "completed");
    }

    #[test]
    fn prompt_end_turn_response_uses_end_turn_stop_reason() {
        let value = prompt_end_turn_response_value().unwrap();
        assert_eq!(value["stopReason"], json!("end_turn"));
    }

    #[test]
    fn status_report_renders_session_health_summary() {
        let environment = json!({
            "runtime": { "kind": "acp_adapter", "version": "0.1.0" },
            "session": {
                "id": "acp-test",
                "conversation_id": "conv-selected",
                "resolved_conversation_id": "conv-resolved"
            },
            "services": {
                "den": {
                    "status": "ok",
                    "runtime": {
                        "runtime": {
                            "state": "requires_action",
                            "active_turn": {"pending_obligations": 1},
                            "source": "acp_active_turn_registry"
                        },
                        "context_budget": {
                            "status": "unavailable",
                            "source": "den.acp"
                        }
                    }
                }
            },
            "browser": {
                "active_source": "host_browser_bridge",
                "source_counts": {"host_browser_bridge": 1}
            },
            "environment_variants": {
                "acp_adapter": {
                    "session_mcp": {
                        "servers": [{"name": "chrome-devtools-custom", "status": "ok", "transport": "stdio", "tool_count": 29}],
                        "client_tools": [{"name": "mcp__chrome_devtools_custom__take_snapshot"}]
                    }
                }
            },
            "diagnostics": { "status": "ok", "warnings": [] }
        });
        let report = render_status_report(&environment, &[]);

        assert!(report.contains("BEARS ACP status"));
        assert!(report.contains("ACP session: acp-test"));
        assert!(report.contains("Conversation: conv-resolved"));
        assert!(report.contains("Adapter-local tools: none active"));
        assert!(report.contains("chrome-devtools-custom"));
        assert!(report.contains("host_browser_bridge"));
        assert!(report.contains("Den:"));
    }

    #[tokio::test]
    async fn adapter_keeps_pending_mode_until_den_session_exists() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await.unwrap();
                if n == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
            }
            stream = reader.into_inner();
            use tokio::io::AsyncWriteExt;
            let body = r#"{"error":"ACP session not found","error_code":"not_found"}"#;
            let response = format!(
                "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let config = Config {
            api_url: format!("http://{addr}"),
            token: "token-test".to_string(),
            bear: "meta".to_string(),
            client: "zed".to_string(),
        };
        let http = reqwest::Client::new();
        let (mode, response) =
            request_den_session_mode(&http, Some(&config), "acp-missing", MODE_WRITE)
                .await
                .unwrap();

        assert_eq!(mode, MODE_WRITE);
        assert_eq!(response["deferred"], true);
        assert_eq!(
            response["source"],
            "adapter.bearwire_local_mode_until_next_prompt"
        );
        assert_eq!(response["pending_mode"], MODE_WRITE);
    }

    #[test]
    fn adapter_summarizes_mcp_context_in_session_logs() {
        let mcp = json!({
            "servers": [{
                "name": "chrome-devtools-custom",
                "status": "ok",
                "transport": "stdio",
                "tool_count": 29,
                "command": "docker"
            }],
            "client_tools": [{
                "name": "mcp__chrome_devtools_custom__take_snapshot",
                "description": "large schema should not be dumped",
                "input_schema": {"properties": {"huge": {"type": "string"}}}
            }]
        });
        let summary = summarize_mcp_for_log(Some(&mcp));

        assert_eq!(summary["server_count"], 1);
        assert_eq!(summary["tool_count"], 1);
        assert_eq!(summary["servers"][0]["name"], "chrome-devtools-custom");
        assert_eq!(
            summary["tool_names"][0],
            "mcp__chrome_devtools_custom__take_snapshot"
        );
        let rendered = summary.to_string();
        assert!(!rendered.contains("large schema should not be dumped"));
        assert!(!rendered.contains("input_schema"));
        assert!(!rendered.contains("docker"));
    }

    #[test]
    fn recovery_user_messages_do_not_expose_raw_debug_payloads() {
        let result = json!({
            "ok": true,
            "compacted": true,
            "approval_recovery": {
                "attempted": false,
                "reason": "compaction_only",
                "denied_tool_call_ids": ["tool-call-secret"],
                "denied_source_message_ids": ["message-secret"],
            },
            "compact_result": {
                "debug": "raw upstream compaction response",
            },
        });

        let rendered = render_compact_recovery_result(&result);
        assert!(rendered.contains("No stale approval recovery was attempted"));
        assert!(rendered.contains("The conversation was compacted."));
        assert!(!rendered.contains("approval_recovery"));
        assert!(!rendered.contains("compact_result"));
        assert!(!rendered.contains("tool-call-secret"));
        assert!(!rendered.contains("message-secret"));
        assert!(!rendered.contains("raw upstream compaction response"));
    }

    #[test]
    fn infer_mode_prefers_den_session_policy_over_plan_mode_state() {
        let den = json!({
            "session_policy": {
                "mode_label": "Ask",
                "mutation_gate": { "state": "closed", "allows_workspace_mutation": false }
                },
            "plan_mode": { "state": "approved" }
        });
        assert_eq!(infer_mode_from_den_session(&den), MODE_ASK);

        let write = json!({
            "session_policy": {
                "mode_label": "Write",
                "mutation_gate": { "state": "open", "allows_workspace_mutation": true }
            }
        });
        assert_eq!(infer_mode_from_den_session(&write), MODE_WRITE);
    }

    #[test]
    fn session_lifecycle_result_includes_mode_metadata() {
        assert_eq!(normalize_mode("Write"), MODE_WRITE);
        assert_eq!(normalize_mode("Ask"), MODE_ASK);
        assert_eq!(normalize_mode(""), MODE_ASK);
        let value = session_lifecycle_result(MODE_PLAN).expect("load response");
        assert_eq!(value["configOptions"][0]["id"].as_str(), Some("mode"));
        assert_eq!(
            value["configOptions"][0]["currentValue"].as_str(),
            Some(MODE_PLAN)
        );
        assert_eq!(value["modes"]["currentModeId"].as_str(), Some(MODE_PLAN));
        let option_values = value["configOptions"][0]["options"]
            .as_array()
            .expect("mode options")
            .iter()
            .filter_map(|option| option.get("value").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(option_values, vec![MODE_ASK, MODE_PLAN, MODE_WRITE]);
    }

    #[test]
    fn fetch_history_chronological_shape_supports_ids_for_reload_debugging() {
        let body = json!({
            "messages": [
                {
                    "id": "msg-1",
                    "role": "user",
                    "text": "hello"
                },
                {
                    "id": "msg-2",
                    "role": "assistant",
                    "text": "world"
                }
            ],
            "has_more": false
        });
        let messages = body["messages"].as_array().unwrap();
        let page = messages
            .iter()
            .map(|m| ReloadHistoryMessage {
                id: m.get("id").and_then(Value::as_str).map(str::to_string),
                kind: m
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("message")
                    .to_string(),
                role: m
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                text: m
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                tool_call_id: None,
                tool_name: None,
                status: None,
                arguments: Value::Null,
                raw_output: Value::Null,
                title: None,
                title_updated_at: None,
                replay_policy: None,
            })
            .collect::<Vec<_>>();
        assert_eq!(page[0].id.as_deref(), Some("msg-1"));
        assert_eq!(page[1].id.as_deref(), Some("msg-2"));
    }

    #[test]
    fn history_pages_flatten_oldest_to_newest_across_desc_pagination() {
        let pages = vec![
            vec![
                ReloadHistoryMessage::text("m3", "user", "ask 2"),
                ReloadHistoryMessage::text("m4", "assistant", "reply 2"),
            ],
            vec![
                ReloadHistoryMessage::text("m1", "user", "ask 1"),
                ReloadHistoryMessage::text("m2", "assistant", "reply 1"),
            ],
        ];
        let messages = flatten_history_pages_chronological(pages);
        let ids = messages
            .iter()
            .map(|message| message.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["m1", "m2", "m3", "m4"]);
    }

    #[test]
    fn prompt_display_text_strips_system_reminder_blocks() {
        let params = json!({
            "prompt": [{
                "type": "text",
                "text": "Please fix this.\n\n<system-reminder>hidden workflow state</system-reminder>"
            }]
        });
        let display = prompt_display_text_from_params(&params).expect("display text");
        assert_eq!(display, "Please fix this.");
    }

    #[test]
    fn submitted_plan_fallback_creates_visible_plan_entry() {
        let den = json!({
            "approval_fallback": {
                "kind": "submitted_plan_approval",
                "plan_id": "00000000-0000-0000-0000-000000000000",
                "title": "Example plan",
                "body": "Do the thing carefully",
                "artifact_path": "pair/plans/example.md",
                "state": "submitted",
                "approval_status": "awaiting_human_approval"
            }
        });
        let entries = plan_entries_from_den_session(&den);
        assert_eq!(entries.len(), 1);
        let payload = acp_plan_update_payload("sess", entries).expect("payload");
        assert_eq!(
            payload["update"]["entries"][0]["content"],
            "Review submitted implementation plan: Example plan"
        );
        assert_eq!(payload["update"]["entries"][0]["priority"], "high");
        assert_eq!(payload["update"]["entries"][0]["status"], "in_progress");
        let message =
            plan_approval_fallback_message(&den["approval_fallback"]).expect("fallback message");
        assert!(message.contains("pair/plans/example.md"));
        assert!(message.contains("Do the thing carefully"));
    }

    #[test]
    fn plan_update_event_parser_accepts_acp_plan_entries() {
        let entries = plan_entries_from_plan_update_event(&json!({
            "type": "plan_update",
            "entries": [
                {
                    "content": "Tell the user the first animal",
                    "priority": "high",
                    "status": "completed"
                },
                {
                    "content": "Tell the user the second animal",
                    "priority": "medium",
                    "status": "in_progress"
                }
            ]
        }));
        assert_eq!(entries.len(), 2);
        let payload = acp_plan_update_payload("sess", entries).expect("payload");
        assert_eq!(
            payload["update"]["entries"][0]["content"],
            "Tell the user the first animal"
        );
        assert_eq!(payload["update"]["entries"][0]["priority"], "high");
        assert_eq!(payload["update"]["entries"][0]["status"], "completed");
        assert_eq!(payload["update"]["entries"][1]["status"], "in_progress");
    }

    #[test]
    fn acp_usage_update_payload_matches_prompt_turn_spec() {
        let payload = acp_usage_update_payload(
            "sess_abc123def456",
            json!({
                "model": "openai/test-model",
                "context_window": 200000,
                "estimated_input_tokens": 48904,
                "estimated_total_tokens": 53000,
                "estimate_precision": "approximate",
                "near_budget": false,
                "over_budget": false,
                "components": []
            }),
        )
        .expect("usage update payload");
        assert_eq!(payload["sessionId"], "sess_abc123def456");
        assert_eq!(payload["update"]["sessionUpdate"], "usage_update");
        assert_eq!(payload["update"]["used"], 53000);
        assert_eq!(payload["update"]["size"], 200000);
        assert_eq!(
            payload["update"]["_meta"]["bears"]["context_budget"]["estimate_precision"],
            "approximate"
        );
    }

    #[test]
    fn acp_usage_update_payload_requires_context_window() {
        let payload = acp_usage_update_payload(
            "sess",
            json!({
                "estimated_total_tokens": 53000,
                "components": []
            }),
        );
        assert!(payload.is_none());
    }

    #[tokio::test]
    async fn suppresses_duplicate_plan_updates_per_session() {
        let shared = test_shared_state();
        let entries = vec![PlanEntry::new(
            "Tell the user an otter",
            PlanEntryPriority::Medium,
            PlanEntryStatus::Completed,
        )];
        assert!(should_send_plan_update(&shared, "session-a", &entries)
            .await
            .expect("first update check"));
        assert!(!should_send_plan_update(&shared, "session-a", &entries)
            .await
            .expect("duplicate update check"));
        assert!(should_send_plan_update(&shared, "session-b", &entries)
            .await
            .expect("different session update check"));
    }

    #[test]
    fn acp_plan_update_payload_matches_agent_plan_spec() {
        let payload = acp_plan_update_payload(
            "sess_abc123def456",
            vec![
                PlanEntry::new(
                    "Analyze the existing codebase structure",
                    PlanEntryPriority::High,
                    PlanEntryStatus::Pending,
                ),
                PlanEntry::new(
                    "Create unit tests for critical functions",
                    PlanEntryPriority::Medium,
                    PlanEntryStatus::InProgress,
                ),
            ],
        )
        .expect("plan payload");
        assert_eq!(payload["sessionId"], "sess_abc123def456");
        assert_eq!(payload["update"]["sessionUpdate"], "plan");
        assert_eq!(
            payload["update"]["entries"][0]["content"],
            "Analyze the existing codebase structure"
        );
        assert_eq!(payload["update"]["entries"][0]["priority"], "high");
        assert_eq!(payload["update"]["entries"][0]["status"], "pending");
        assert_eq!(payload["update"]["entries"][1]["priority"], "medium");
        assert_eq!(payload["update"]["entries"][1]["status"], "in_progress");
    }

    #[test]
    fn run_command_defaults_to_terminal_when_command_is_present() {
        assert!(run_command_prefers_terminal(
            &json!({ "command": "cargo", "args": ["check"] })
        ));
        assert!(run_command_prefers_terminal(
            &json!({ "command": "docker", "args": ["build", "."] })
        ));
        assert!(run_command_prefers_terminal(&json!({ "command": "pwd" })));
        assert!(!run_command_prefers_terminal(&json!({ "args": ["check"] })));
    }

    #[test]
    fn command_tool_titles_include_command_details() {
        let event = json!({ "args": { "command": "cargo", "args": ["test", "--manifest-path", "tools/bear-armature/Cargo.toml"] } });
        assert_eq!(
            tool_call_title("terminal_run_command", &event),
            "Run terminal command: cargo test --manifest-path tools/bear-armature/Cargo.toml"
        );
        assert_eq!(
            tool_call_title("process_run", &event),
            "Run process: cargo test --manifest-path tools/bear-armature/Cargo.toml"
        );
    }

    #[test]
    fn canonical_bearwire_tool_request_parser_requires_nested_payload() {
        let event = json!({
            "type": "tool_call.requested",
            "run_id": "run-1",
            "data": {
                "obligation_id": "obl-call-1",
                "tool_call": {
                    "id": "call-1",
                    "name": "fs_read_text_file",
                    "arguments": { "path": "/workspace/README.md", "line": 4 },
                    "display": { "title": "Read workspace README", "progress": "Reading file" }
                }
            }
        });

        let parsed = BearWireToolCallRequestData::parse(&event).unwrap();
        assert_eq!(parsed.tool_call.id, "call-1");
        assert_eq!(parsed.tool_call.name, "fs_read_text_file");
        assert_eq!(parsed.tool_call.arguments["path"], "/workspace/README.md");
        assert_eq!(tool_path(&event), Some("/workspace/README.md"));
        assert_eq!(
            tool_call_title("fs_read_text_file", &event),
            "Read file: /workspace/README.md"
        );
        assert_eq!(
            ToolDisplay::from_event("fs_read_text_file", &event).title,
            "Read workspace README"
        );
    }

    #[test]
    fn den_owned_checkpoint_request_can_omit_client_obligation() {
        let event = json!({
            "type": "tool_call.requested",
            "run_id": "run-1",
            "data": {
                "execution_target": "den",
                "policy": { "execution_target": "den" },
                "tool_call": {
                    "id": "call-checkpoint-1",
                    "name": "checkpoint",
                    "arguments": { "checkpoint_id": "ckpt-1" }
                }
            }
        });

        let parsed = BearWireToolCallRequestData::parse(&event).expect("parse Den-owned request");
        assert_eq!(parsed.obligation_id, None);
        assert_eq!(parsed.tool_call.name, "checkpoint");
        assert!(parsed.client_obligation_id().is_err());
        assert!(is_den_server_tool_request(&event));
    }

    #[test]
    fn canonical_bearwire_client_waiting_parser_reads_permission_obligation() {
        let event = json!({
            "type": "client.waiting",
            "run_id": "run-web-1",
            "data": {
                "expected_client_method": "client.permission.result",
                "obligation_id": "obl-web-1",
                "tool_call": {
                    "id": "call-web-1",
                    "name": "web_fetch",
                    "title": "Fetch URL",
                    "arguments": { "kind": "url", "url": "https://example.com/", "host": "example.com" }
                },
                "permission": {
                    "id": "perm-web-1",
                    "title": "Fetch URL",
                    "reason": "BEARS wants to fetch https://example.com/.",
                    "target": { "kind": "url", "url": "https://example.com/", "host": "example.com" }
                }
            }
        });

        let parsed = BearWireClientWaitingData::parse(&event).unwrap();
        assert_eq!(parsed.permission.id, "perm-web-1");
        assert_eq!(parsed.obligation_id, "obl-web-1");
        assert_eq!(parsed.tool_call.id, "call-web-1");
        assert_eq!(parsed.tool_call.name, "web_fetch");
        assert_eq!(parsed.permission.title.as_deref(), Some("Fetch URL"));
        assert_eq!(
            parsed.permission.reason.as_deref(),
            Some("BEARS wants to fetch https://example.com/.")
        );
        assert_eq!(
            parsed.permission.target.unwrap()["url"],
            "https://example.com/"
        );
    }

    #[test]
    fn approved_local_tool_response_becomes_claimable_tool_request() {
        let permission_event = json!({
            "type": "client.waiting",
            "run_id": "run-git-1",
        });
        let local_tool = json!({
            "tool_call_id": "call-git-1",
            "tool_name": "git_status",
            "result_tool_name": "git_status",
            "args": { "path": "." },
            "permission_id": "perm-git-1",
            "obligation_id": "obl-git-1",
            "policy": {
                "execution_target": "armature_local",
                "total_timeout_ms": 150000,
            },
        });

        let event = approved_local_tool_request_event(&permission_event, &local_tool)
            .expect("canonical approved tool request");
        let parsed = BearWireToolCallRequestData::parse(&event).expect("parse tool request");

        assert_eq!(event["type"], "tool_call.requested");
        assert_eq!(event["run_id"], "run-git-1");
        assert_eq!(event["data"]["expected_responder_action"], "tool_result");
        assert_eq!(event["data"]["approval_required"], false);
        assert_eq!(event["data"]["approval_request_id"], "perm-git-1");
        assert_eq!(event["data"]["policy"]["total_timeout_ms"], 150000);
        assert_eq!(parsed.obligation_id.as_deref(), Some("obl-git-1"));
        assert_eq!(parsed.tool_call.id, "call-git-1");
        assert_eq!(parsed.tool_call.name, "git_status");
        assert_eq!(parsed.tool_call.arguments["path"], ".");
    }

    #[test]
    fn tool_display_uses_specific_titles() {
        assert_eq!(tool_display("fs_read_text_file").title, "Read file");
        assert_eq!(tool_display("fs_list_directory").title, "List directory");
        assert_eq!(tool_display("fs_search_files").title, "Search files");
        assert_eq!(tool_display("fs_edit_file").title, "Edit file");
    }

    #[test]
    fn all_advertised_direct_tools_have_specific_titles() {
        let direct = direct_tools_context_with_client_mcp(false);
        let map = direct.as_object().expect("direct tools object");
        for (tool, value) in map {
            if !value
                .get("supported")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || tool.ends_with("_present")
                || tool.ends_with("_reason")
            {
                continue;
            }
            let title = tool_display(tool).title;
            assert_ne!(title, "Tool call", "{tool} should have a specific title");
            assert_ne!(title, "Unknown tool", "{tool} should have a specific title");
        }
    }

    #[test]
    fn direct_tools_context_includes_search_and_replace_affordance_hints() {
        let direct = direct_tools_context_with_client_mcp(false);
        assert_eq!(
            direct["fs_search_files"]["prefer_instead_of_shell"],
            json!(["rg", "grep"])
        );
        assert!(direct["fs_search_files"]["description"]
            .as_str()
            .unwrap_or("")
            .contains("Prefer this over shell search commands"));
        assert_eq!(
            direct["fs_replace_text"]["prefer_instead_of_shell"],
            json!(["sed"])
        );
    }

    #[test]
    fn adapter_capabilities_context_direct_tools_match_affordance_shape() {
        let direct = adapter_capabilities_context_with_client_mcp(false)["direct_tools"].clone();
        assert_eq!(
            direct["fs_search_files"]["prefer_instead_of_shell"],
            json!(["rg", "grep"])
        );
        assert_eq!(
            direct["fs_replace_text"]["prefer_instead_of_shell"],
            json!(["sed"])
        );
        assert!(direct["run_command"]["supported"]
            .as_bool()
            .unwrap_or(false));
    }

    #[test]
    fn known_den_hosted_tools_have_specific_titles() {
        for (tool, expected) in [
            ("session_info", "Inspect session"),
            ("memory_browse", "Browse memory"),
            ("memory_read", "Read memory"),
            ("memory_search", "Search memory"),
            ("memory_write_entry", "Write memory entry"),
            ("memory_request_review", "Request memory review"),
            ("web_fetch", "Fetch URL"),
            ("web_search", "Search web"),
            ("list_task_lists", "List task lists"),
            ("get_task_list_status", "Get task list status"),
            ("update_task", "Update task"),
            ("update_task_list", "Update task list"),
            ("request_task_list_handoff", "Request work handoff"),
        ] {
            assert_eq!(tool_display(tool).title, expected, "{tool}");
        }
    }

    #[test]
    fn tool_call_title_includes_conversation_title_and_file_targets() {
        assert_eq!(
            tool_call_title(
                "set_conversation_title",
                &json!({ "args": { "title": "Runtime investigation" } })
            ),
            "Set conversation title: Runtime investigation"
        );
        assert_eq!(
            tool_call_title(
                "set_conversation_title",
                &json!({ "arguments": { "title": "Direct ACP card title" } })
            ),
            "Set conversation title: Direct ACP card title"
        );
        assert_eq!(
            tool_call_title(
                "set_conversation_title",
                &json!({ "data": { "tool_call": { "arguments": { "title": "Nested ACP card title" } } } })
            ),
            "Set conversation title: Nested ACP card title"
        );
        assert_eq!(
            tool_call_title(
                "set_conversation_title",
                &json!({ "data": { "tool_call": { "input": { "title": "Nested ACP input title" } } } })
            ),
            "Set conversation title: Nested ACP input title"
        );
        assert_eq!(
            tool_call_title("set_conversation_title", &json!({ "args": {} })),
            "Set conversation title"
        );
        let stale_display_event = json!({
            "display": { "title": "Set conversation title: conversation" },
            "arguments": { "title": "Actual ACP card title" }
        });
        let display = ToolDisplay::from_event("set_conversation_title", &stale_display_event);
        assert_eq!(
            tool_card_title(
                "set_conversation_title",
                Some(&stale_display_event),
                &display
            ),
            "Set conversation title: Actual ACP card title"
        );
        assert_eq!(
            tool_args_from_event(&stale_display_event)
                .and_then(|args| args.get("title"))
                .and_then(Value::as_str),
            Some("Actual ACP card title")
        );
        let stale_file_display_event = json!({
            "display": { "title": "Read file: file" },
            "arguments": { "path": "/workspace/README.md" }
        });
        let display = ToolDisplay::from_event("fs_read_text_file", &stale_file_display_event);
        assert_eq!(
            tool_card_title(
                "fs_read_text_file",
                Some(&stale_file_display_event),
                &display
            ),
            "Read file: /workspace/README.md"
        );
        assert_eq!(
            tool_call_title(
                "create_job",
                &json!({ "args": { "goal": "Ship ACP card title tests" } })
            ),
            "Create job: Ship ACP card title tests"
        );
        assert_eq!(
            tool_call_title(
                "create_job",
                &json!({ "data": { "tool_call": { "arguments": { "goal": "Avoid prominent Docket branding" } } } })
            ),
            "Create job: Avoid prominent Docket branding"
        );
        assert_eq!(
            tool_call_title("create_job", &json!({ "args": {} })),
            "Create job"
        );
        assert!(
            !tool_call_title("create_job", &json!({ "args": { "goal": "Plan release" } }))
                .contains("Docket")
        );
        assert_eq!(
            tool_call_title(
                "fs_replace_text",
                &json!({ "args": { "path": "/workspace/src/main.rs" } })
            ),
            "Edit file: /workspace/src/main.rs"
        );
    }

    #[test]
    fn mcp_tools_have_human_friendly_titles() {
        assert_eq!(
            tool_display("mcp__chrome_devtools_custom__take_snapshot").title,
            "Chrome DevTools: Take Snapshot"
        );
        assert_eq!(
            tool_display("mcp__github__list_issues").title,
            "Github: List Issues"
        );
    }

    #[test]
    fn tool_display_humanizes_unknown_tool_names() {
        assert_eq!(
            tool_display("custom_memory_read").title,
            "Custom Memory Read"
        );
        assert_eq!(tool_display("custom.call_tool").title, "Custom Call Tool");
    }

    #[test]
    fn tool_display_does_not_render_placeholder_tool_name_as_tool() {
        assert_eq!(tool_display("tool").title, "Tool call");
        assert_eq!(tool_display("").title, "Tool call");
    }

    #[test]
    fn placeholder_tool_display_derives_title_from_event_details() {
        let event = json!({
            "tool_call_id": "call-1",
            "args": { "path": "/workspace/README.md" }
        });
        assert_eq!(
            ToolDisplay::from_event("tool", &event).title,
            "Tool call: /workspace/README.md"
        );

        let only_id = json!({ "tool_call_id": "call-opaque" });
        assert_eq!(
            ToolDisplay::from_event("local_tool", &only_id).title,
            "Tool call: call-opaque"
        );
    }

    #[test]
    fn den_memory_tool_display_uses_den_labels() {
        let event = json!({
            "display": {
                "label": "Read memory file",
                "title": "Reading memory pair/notes/example.md",
                "progress": "Reading memory",
                "category": "memory"
            },
            "args": { "path": "pair/notes/example.md" }
        });
        let display = ToolDisplay::from_event("memory_read", &event);
        assert_eq!(display.title, "Reading memory pair/notes/example.md");
        assert_eq!(display.verb, "Reading memory");
        assert_eq!(display.category.as_deref(), Some("memory"));
    }

    #[test]
    fn den_server_tool_requests_are_detected() {
        let legacy_event = json!({
            "policy": { "execution_target": "den" },
            "tool_name": "memory_read"
        });
        assert!(is_den_server_tool_request(&legacy_event));

        let canonical_event = json!({
            "type": "tool_call.requested",
            "data": {
                "obligation_id": "obl-call-den",
                "policy": { "execution_target": "den" },
                "tool_call": {
                    "id": "call-den",
                    "name": "set_conversation_title",
                    "arguments": { "title": "Loaded title" }
                }
            }
        });
        assert!(is_den_server_tool_request(&canonical_event));

        let local = json!({ "data": { "policy": { "execution_target": "adapter" } } });
        assert!(!is_den_server_tool_request(&local));
    }

    #[test]
    fn doctor_slash_command_is_always_advertised() {
        let commands = local_slash_available_commands();
        assert!(commands.iter().any(|command| command.name == "doctor"));
        let descriptor = local_slash_descriptor_for_name("doctor").expect("doctor descriptor");
        assert!(!descriptor.den_required);
    }

    #[test]
    fn tool_locations_are_only_emitted_for_file_targets() {
        let search_event = json!({ "args": { "path": "/workspace", "query": "needle" } });
        assert!(tool_locations_from_event("fs_search_files", &search_event).is_none());
        assert!(tool_locations_from_event("fs_find_paths", &search_event).is_none());
        assert!(tool_locations_from_event("fs_list_directory", &search_event).is_none());

        let read_event = json!({ "args": { "path": "/workspace/README.md", "line": 3 } });
        let locations = tool_locations_from_event("fs_read_text_file", &read_event)
            .expect("read file has a file location");
        assert_eq!(locations.len(), 1);

        let delete_dir_event =
            json!({ "args": { "path": "/workspace/docs", "expected_kind": "directory" } });
        assert!(tool_locations_from_event("fs_delete_path", &delete_dir_event).is_none());
        let delete_file_event =
            json!({ "args": { "path": "/workspace/README.md", "expected_kind": "file" } });
        assert!(tool_locations_from_event("fs_delete_path", &delete_file_event).is_some());
    }

    #[test]
    fn compact_tool_card_json_value_keeps_nested_payloads_out_of_acp_cards() {
        let compacted = compact_tool_card_json_value(json!({
            "entries": (0..500).map(|idx| json!({
                "path": format!("/workspace/file-{idx}.txt"),
                "kind": "file"
            })).collect::<Vec<_>>()
        }));
        let preview = compacted.as_str().expect("ACP raw values are JSON strings");
        assert!(preview.contains("entries"));
        assert!(preview.contains("... truncated"));
        assert!(
            preview.chars().count()
                <= ACP_TOOL_CARD_RAW_OUTPUT_PREVIEW_CHARS + "... truncated".len()
        );
        assert!(compacted.get("entries").is_none());
    }

    #[test]
    fn tool_completion_preview_includes_content_and_truncates() {
        let value = json!({ "content": "abc" });
        assert_eq!(tool_completion_preview("fs_list_directory", &value), "abc");
        let long = json!({ "content": "x".repeat(4_100) });
        let preview = tool_completion_preview("fs_read_text_file", &long);
        assert!(preview.starts_with("```\n"));
        assert!(preview.contains("... truncated"));
        assert!(preview.chars().count() < 4_050);
    }

    #[test]
    fn client_read_text_file_request_path_resolves_relative_path_before_acp_call() {
        let context = SessionContext {
            cwd: "/workspace".to_string(),
            roots: vec!["/workspace".to_string()],
            ..Default::default()
        };
        let resolved = client_read_text_file_request_path(&context, "docs/roadmap/PLAN.md")
            .expect("relative path resolves under workspace");
        assert_eq!(resolved, PathBuf::from("/workspace/docs/roadmap/PLAN.md"));

        let escaped = client_read_text_file_request_path(&context, "../etc/passwd")
            .expect_err("path escaping workspace is denied");
        assert!(format!("{escaped:#}").contains("outside the ACP session workspace roots"));
    }

    #[test]
    fn read_text_file_uses_armature_local_execution_by_default() {
        assert!(!read_text_file_requires_client_surface(&json!({
            "path": "/workspace/README.md"
        })));
        assert!(read_text_file_requires_client_surface(&json!({
            "path": "/workspace/README.md",
            "source": "editor_buffer"
        })));
        assert!(read_text_file_requires_client_surface(&json!({
            "path": "/workspace/README.md",
            "_meta": { "prefer_client": true }
        })));
    }

    #[test]
    fn read_text_file_completion_preview_wraps_content_in_escaping_code_fence() {
        let value = json!({
            "path": "/workspace/README.md",
            "content": "before\n```\nnot a real fence break\n```\nafter"
        });
        let preview = tool_completion_preview("fs_read_text_file", &value);

        assert!(
            preview.starts_with("Read `/workspace/README.md`:"),
            "{preview}"
        );
        assert!(preview.contains("````\nbefore"), "{preview}");
        assert!(preview.contains("```\nnot a real fence break"), "{preview}");
        assert!(preview.ends_with("````"), "{preview}");
    }

    #[test]
    fn read_text_file_completion_preview_escapes_backticks_in_path() {
        let value = json!({
            "path": "/workspace/`odd`.md",
            "content": "hello"
        });
        let preview = tool_completion_preview("fs_read_text_file", &value);

        assert!(
            preview.starts_with("Read ``` /workspace/`odd`.md ```:"),
            "{preview}"
        );
        assert!(preview.contains("```\nhello\n```"), "{preview}");
    }

    #[test]
    fn command_tool_completion_preview_shows_command() {
        let value = json!({
            "command": "cargo",
            "args": ["test", "--all"],
            "cwd": "/workspace/tools/bear-armature",
            "exit_code": 0,
            "timed_out": false,
            "elapsed_ms": 1234,
            "truncated": false
        });
        let preview = tool_completion_preview("terminal_run_command", &value);
        assert!(preview.contains("`cargo test --all`"));
        assert!(preview.contains("exit code 0"));
        assert!(!preview.contains("Local tool"));
    }

    #[test]
    fn run_command_title_and_preview_show_command() {
        let event = json!({
            "args": {
                "command": "cargo",
                "args": ["test", "--all"]
            }
        });
        assert_eq!(
            tool_call_title("run_command", &event),
            "Run command: cargo test --all"
        );
        let display = ToolDisplay::from_event(
            "run_command",
            &json!({
                "args": { "command": "cargo", "args": ["test", "--all"] },
                "display": { "title": "Run Command" }
            }),
        );
        assert_eq!(
            tool_card_title(
                "run_command",
                Some(&json!({
                    "args": { "command": "cargo", "args": ["test", "--all"] },
                    "display": { "title": "Run Command" }
                })),
                &display
            ),
            "Run command: cargo test --all"
        );

        let value = json!({
            "command": "cargo",
            "args": ["test", "--all"],
            "cwd": "/workspace/tools/bear-armature",
            "exit_code": 0,
            "timed_out": false,
            "elapsed_ms": 1234,
            "truncated": false
        });
        let preview = tool_completion_preview("run_command", &value);
        assert!(preview.contains("`cargo test --all`"), "{preview}");
        assert!(preview.contains("exit code 0"), "{preview}");
    }

    #[test]
    fn run_command_title_reads_bearwire_argument_shape() {
        let event = json!({
            "data": {
                "arguments": {
                    "command": "git",
                    "args": ["status", "--short"]
                }
            },
            "display": { "title": "Run Command" }
        });

        assert_eq!(
            tool_call_title("run_command", &event),
            "Run command: git status --short"
        );
    }

    #[test]
    fn update_task_title_includes_useful_target_fields() {
        let event = json!({
            "data": {
                "tool_call": {
                    "input": {
                        "task_id": "task-123",
                        "title": "Patch live ACP card titles",
                        "status": "done"
                    }
                }
            },
            "display": { "title": "Update Task" }
        });
        assert_eq!(
            tool_call_title("update_task", &event),
            "Update task: Patch live ACP card titles → done"
        );
        let display = ToolDisplay::from_event("update_task", &event);
        assert_eq!(
            tool_card_title("update_task", Some(&event), &display),
            "Update task: Patch live ACP card titles → done"
        );

        let id_only = json!({ "arguments": { "task_id": "task-456" } });
        assert_eq!(
            tool_call_title("update_task", &id_only),
            "Update task: task-456"
        );
    }

    #[test]
    fn update_task_list_completion_preview_summarizes_entries() {
        let value = json!({
            "plan": {
                "items": [
                    { "content": "Inspect logs", "status": "completed", "priority": "high" },
                    { "content": "Patch parser", "status": "in_progress", "priority": "high" },
                    { "content": "Run tests", "status": "pending", "priority": "medium" }
                ]
            }
        });
        let preview = tool_completion_preview("update_task_list", &value);
        assert!(preview.contains("Task list updated: 3 total"), "{preview}");
        assert!(preview.contains("1 in progress"), "{preview}");
        assert!(preview.contains("1 pending"), "{preview}");
        assert!(preview.contains("1 completed"), "{preview}");
    }

    #[test]
    fn set_conversation_title_completion_preview_shows_new_title() {
        let value = json!({ "title": "Investigate Bifrost stream framing" });
        let preview = tool_completion_preview("set_conversation_title", &value);
        assert!(preview.contains("Conversation title set to"), "{preview}");
        assert!(
            preview.contains("Investigate Bifrost stream framing"),
            "{preview}"
        );
    }

    fn set_conversation_title_tool_events(
        call_id: &str,
        title: &str,
        updated_at: Option<&str>,
    ) -> Vec<Value> {
        let tool_call = json!({
            "id": call_id,
            "name": "set_conversation_title",
            "arguments": { "title": title },
            "display": { "title": format!("Set conversation title: {title}") }
        });
        let mut title_data = json!({ "title": title });
        if let Some(updated_at) = updated_at {
            title_data["updated_at"] = json!(updated_at);
        }
        vec![
            json!({
                "type": "tool_call.requested",
                "run_id": "run-test-title",
                "data": {
                    "obligation_id": format!("obl-{call_id}"),
                    "tool_call": tool_call.clone()
                },
                "policy": { "execution_target": "den" }
            }),
            json!({
                "type": "tool_call.completed",
                "run_id": "run-test-title",
                "data": {
                    "tool_call": tool_call,
                    "summary": format!("Conversation title set to {title}.")
                }
            }),
            json!({
                "type": "session_info_update",
                "run_id": "run-test-title",
                "data": title_data
            }),
        ]
    }

    #[tokio::test]
    async fn set_conversation_title_acp_prompt_roundtrip_emits_update_before_result() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let title = "Retire legacy BearWire tool payload aliases";
        let mut events =
            set_conversation_title_tool_events("call-title-1", title, Some("2026-01-02T03:04:05Z"));
        events.push(json!({
            "type": "message.delta",
            "run_id": "run-test-title",
            "data": { "delta": "Done." }
        }));
        events.push(json!({ "type": "run.completed", "run_id": "run-test-title", "data": {} }));
        let (api_url, paths) = start_bearwire_test_server_with_events(false, events).await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("title-acp-roundtrip-immediate");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();
        shared_state.session_contexts.lock().await.insert(
            "session-1".to_string(),
            state.session_contexts["session-1"].clone(),
        );

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "prompt-1",
                    "method": "session/prompt",
                    "params": {
                        "sessionId": "session-1",
                        "prompt": [{ "type": "text", "text": "please set the conversation title" }]
                    }
                }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let tool_index = output
            .iter()
            .position(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("tool_call")
                    && frame.to_string().contains("call-title-1")
            })
            .expect("set_conversation_title tool_call frame");
        let update_index = output
            .iter()
            .position(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("session_info_update")
            })
            .expect("session_info_update frame");
        let result_index = output
            .iter()
            .position(|frame| frame.get("id").and_then(Value::as_str) == Some("prompt-1"))
            .expect("prompt result frame");
        assert!(tool_index < update_index, "{output:#?}");
        assert!(update_index < result_index, "{output:#?}");
        assert_eq!(output[update_index]["jsonrpc"], "2.0");
        assert_eq!(output[update_index]["params"]["sessionId"], "session-1");
        assert_eq!(
            output[update_index]["params"]["update"]["sessionUpdate"],
            "session_info_update"
        );
        assert_eq!(output[update_index]["params"]["update"]["title"], title);
        assert_eq!(
            output[update_index]["params"]["update"]["updatedAt"],
            "2026-01-02T03:04:05Z"
        );
        assert!(output[result_index].get("result").is_some(), "{output:#?}");
        assert_eq!(
            shared_state
                .session_contexts
                .lock()
                .await
                .get("session-1")
                .and_then(|context| context.thread_title.as_deref()),
            Some(title)
        );
        let paths = paths.lock().await.clone();
        assert!(paths.iter().any(|path| path == "/bearwire/v1/rpc"));
        assert!(paths
            .iter()
            .any(|path| path.starts_with("/bearwire/v1/sessions/session-1/events")));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn set_conversation_title_acp_roundtrip_title_sticks_for_later_update_output() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let mut events = set_conversation_title_tool_events("call-title-old", "Old title", None);
        events.extend(set_conversation_title_tool_events(
            "call-title-new",
            "New sticky title",
            None,
        ));
        events.push(json!({
            "type": "message.delta",
            "run_id": "run-test-title",
            "data": { "delta": "Done." }
        }));
        events.push(json!({ "type": "run.completed", "run_id": "run-test-title", "data": {} }));
        let (api_url, _paths) = start_bearwire_test_server_with_events(false, events).await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("title-acp-roundtrip-sticky");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();
        shared_state.session_contexts.lock().await.insert(
            "session-1".to_string(),
            state.session_contexts["session-1"].clone(),
        );

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "prompt-1",
                    "method": "session/prompt",
                    "params": {
                        "sessionId": "session-1",
                        "prompt": [{ "type": "text", "text": "please set the conversation title twice" }]
                    }
                }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let sticky_title = shared_state
                .session_contexts
                .lock()
                .await
                .get("session-1")
                .and_then(|context| context.thread_title.clone());
            send_session_info_update("session-1", sticky_title, None).await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let tool_frames = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("tool_call")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>();
        assert!(
            tool_frames
                .iter()
                .any(|frame| frame.contains("call-title-old")),
            "{output:#?}"
        );
        assert!(
            tool_frames
                .iter()
                .any(|frame| frame.contains("call-title-new")),
            "{output:#?}"
        );
        let titles = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("session_info_update")
            })
            .filter_map(|frame| {
                frame
                    .pointer("/params/update/title")
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            vec!["Old title", "New sticky title", "New sticky title"]
        );
        assert!(
            output.iter().any(
                |frame| frame.get("id").and_then(Value::as_str) == Some("prompt-1")
                    && frame.get("result").is_some()
            ),
            "{output:#?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn set_conversation_title_lifecycle_surfaces_live_list_and_load() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let title = "Full lifecycle conversation title";
        let updated_at = "2026-07-07T00:00:00Z";
        let mut events =
            set_conversation_title_tool_events("call-title-lifecycle", title, Some(updated_at));
        events.push(json!({
            "type": "message.delta",
            "run_id": "run-test-title",
            "data": { "delta": "Done." }
        }));
        events.push(json!({ "type": "run.completed", "run_id": "run-test-title", "data": {} }));
        let history = vec![
            json!({
                "kind": "tool_call",
                "id": "call-title-lifecycle",
                "role": "assistant",
                "tool_call_id": "call-title-lifecycle",
                "tool_name": "set_conversation_title",
                "status": "pending",
                "arguments": { "title": title },
                "created_at": "2026-07-07T00:00:00Z"
            }),
            json!({
                "kind": "tool_result",
                "id": "call-title-lifecycle",
                "role": "tool",
                "tool_call_id": "call-title-lifecycle",
                "tool_name": "set_conversation_title",
                "status": "ok",
                "text": "Conversation title set.",
                "raw_output": { "title": title },
                "created_at": "2026-07-07T00:00:01Z"
            }),
            json!({
                "kind": "session_info_update",
                "id": "session-info-title",
                "role": "system",
                "session_id": "session-1",
                "title": title,
                "title_updated_at": updated_at,
                "current_mode": "ask",
                "created_at": updated_at
            }),
        ];
        let (api_url, _paths) =
            start_bearwire_test_server_with_events_history_and_title(events, history, Some(title))
                .await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("title-lifecycle");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();
        shared_state.session_contexts.lock().await.insert(
            "session-1".to_string(),
            state.session_contexts["session-1"].clone(),
        );

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "prompt-title-lifecycle",
                    "method": "session/prompt",
                    "params": {
                        "sessionId": "session-1",
                        "prompt": [{ "type": "text", "text": "set the title" }]
                    }
                }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "list-title-lifecycle",
                    "method": "session/list",
                    "params": {}
                }),
            )
            .await?;
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "load-title-lifecycle",
                    "method": "session/load",
                    "params": { "sessionId": "session-1" }
                }),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        assert!(
            output.iter().any(|frame| {
                frame
                    .pointer("/params/update/sessionUpdate")
                    .and_then(Value::as_str)
                    == Some("session_info_update")
                    && frame
                        .pointer("/params/update/title")
                        .and_then(Value::as_str)
                        == Some(title)
            }),
            "live/load session_info_update title missing: {output:#?}"
        );
        assert!(
            output.iter().any(|frame| {
                frame.get("id").and_then(Value::as_str) == Some("list-title-lifecycle")
                    && frame
                        .pointer("/result/sessions/0/title")
                        .and_then(Value::as_str)
                        == Some(title)
            }),
            "session/list did not expose conversation_title: {output:#?}"
        );
        assert!(
            output.iter().any(|frame| {
                frame
                    .pointer("/params/update/sessionUpdate")
                    .and_then(Value::as_str)
                    == Some("tool_call")
                    && frame.to_string().contains("call-title-lifecycle")
            }),
            "load/prompt should surface title tool cards: {output:#?}"
        );
        assert_eq!(
            shared_state
                .session_contexts
                .lock()
                .await
                .get("session-1")
                .and_then(|context| context.thread_title.as_deref()),
            Some(title)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn den_owned_tool_start_renders_without_local_result_post() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, _paths, rpc_methods) = start_bearwire_test_server_with_events_and_methods(
            false,
            vec![
                json!({
                    "type": "tool_call.requested",
                    "run_id": "run-test-title",
                    "data": {
                        "obligation_id": "obl-den-owned-title",
                        "policy": { "execution_target": "den" },
                        "tool_call": {
                            "id": "call-den-owned-title",
                            "name": "set_conversation_title",
                            "arguments": { "title": "Den owned title" },
                            "display": { "title": "Set conversation title: Den owned title" }
                        }
                    }
                }),
                json!({
                    "type": "tool_call.completed",
                    "run_id": "run-test-title",
                    "data": {
                        "tool_call": {
                            "id": "call-den-owned-title",
                            "name": "set_conversation_title"
                        },
                        "summary": "Conversation title set."
                    }
                }),
                json!({
                    "type": "message.delta",
                    "run_id": "run-test-title",
                    "data": { "delta": "Done." }
                }),
                json!({ "type": "run.completed", "run_id": "run-test-title", "data": {} }),
            ],
        )
        .await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("den-owned-tool-start");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();
        shared_state.session_contexts.lock().await.insert(
            "session-1".to_string(),
            state.session_contexts["session-1"].clone(),
        );

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "prompt-den-owned",
                    "method": "session/prompt",
                    "params": {
                        "sessionId": "session-1",
                        "prompt": [{ "type": "text", "text": "set title" }]
                    }
                }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let tool_frames = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("tool_call")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>();
        assert!(
            tool_frames
                .iter()
                .any(|frame| frame.contains("call-den-owned-title")),
            "{output:#?}"
        );
        assert!(
            tool_frames
                .iter()
                .any(|frame| frame.contains("Set conversation title: Den owned title")),
            "sparse Den-owned completion should use cached start arguments for the card title: {output:#?}"
        );
        let methods = rpc_methods.lock().await.clone();
        assert!(
            !methods.iter().any(|method| method == "client.tool.result"),
            "Den-owned surfaced tool starts must not be executed locally: {methods:?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn reasoning_bearwire_delta_roundtrips_to_acp_thought_not_agent_message() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, _paths) = start_bearwire_test_server_with_events(
            false,
            vec![
                json!({
                    "type": "message.reasoning.delta",
                    "run_id": "run-test-title",
                    "data": { "delta": "private reasoning" }
                }),
                json!({
                    "type": "message.delta",
                    "run_id": "run-test-title",
                    "data": { "delta": "visible answer" }
                }),
                json!({ "type": "run.completed", "run_id": "run-test-title", "data": {} }),
            ],
        )
        .await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("reasoning-acp-roundtrip");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();
        shared_state.session_contexts.lock().await.insert(
            "session-1".to_string(),
            state.session_contexts["session-1"].clone(),
        );

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "prompt-1",
                    "method": "session/prompt",
                    "params": {
                        "sessionId": "session-1",
                        "prompt": [{ "type": "text", "text": "think then answer" }]
                    }
                }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let reasoning_frames = output
            .iter()
            .filter(|frame| {
                frame
                    .pointer("/params/update/sessionUpdate")
                    .and_then(Value::as_str)
                    == Some("agent_thought_chunk")
            })
            .collect::<Vec<_>>();
        assert_eq!(reasoning_frames.len(), 1, "{output:#?}");
        assert!(
            reasoning_frames[0]
                .to_string()
                .contains("private reasoning"),
            "{output:#?}"
        );

        let agent_frames = output
            .iter()
            .filter(|frame| {
                frame
                    .pointer("/params/update/sessionUpdate")
                    .and_then(Value::as_str)
                    == Some("agent_message_chunk")
            })
            .collect::<Vec<_>>();
        assert_eq!(agent_frames.len(), 1, "{output:#?}");
        assert!(
            agent_frames[0].to_string().contains("visible answer"),
            "{output:#?}"
        );
        assert!(
            !agent_frames
                .iter()
                .any(|frame| frame.to_string().contains("private reasoning")),
            "reasoning leaked as agent message: {output:#?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_load_sends_bearwire_assistant_message_as_acp_agent_chunk() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, _paths) = start_bearwire_test_server_with_history(vec![json!({
            "id": "persisted-agent-message",
            "kind": "message",
            "role": "assistant",
            "text": "agent message received over BearWire"
        })])
        .await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("load-bearwire-agent-message");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "load-agent-history",
                    "method": "session/load",
                    "params": { "sessionId": "session-1" }
                }),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let load_response_index = output
            .iter()
            .position(|frame| frame.get("id") == Some(&json!("load-agent-history")))
            .expect("session/load response");
        let agent_update_index = output
            .iter()
            .position(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("agent_message_chunk")
                    && frame
                        .to_string()
                        .contains("agent message received over BearWire")
            })
            .unwrap_or_else(|| panic!("missing ACP agent message chunk: {output:#?}"));
        assert!(
            output[agent_update_index]
                .pointer("/params/update/messageId")
                .is_none(),
            "replayed ACP agent chunks must match the live/diagnostic chunk shape: {output:#?}"
        );
        assert!(
            agent_update_index < load_response_index,
            "ACP session/load must replay the BearWire agent message before completing the request: {output:#?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_load_replays_user_history_as_acp_user_chunks() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, _paths) = start_bearwire_test_server_with_history(vec![
            json!({
                "id": "old-user-prompt",
                "kind": "message",
                "role": "user",
                "text": "old instruction that must not be replayed as fresh input"
            }),
            json!({
                "id": "old-assistant-answer",
                "kind": "message",
                "role": "assistant",
                "text": "old answer can be replayed as agent history"
            }),
        ])
        .await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("history-user-not-fresh-input");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "load-1",
                    "method": "session/load",
                    "params": { "sessionId": "session-1" }
                }),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let load_response_index = output
            .iter()
            .position(|frame| frame.get("id") == Some(&json!("load-1")))
            .expect("session/load response");
        let first_agent_update_index = output
            .iter()
            .position(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("agent_message_chunk")
            })
            .expect("replayed agent history update");
        assert!(
            first_agent_update_index < load_response_index,
            "ACP session/load must replay history before completing the request: {output:#?}"
        );

        let user_chunks = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("user_message_chunk")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>();
        assert!(
            user_chunks
                .iter()
                .any(|frame| frame
                    .contains("old instruction that must not be replayed as fresh input")),
            "session/load should replay historical user messages to the ACP client: {output:#?}"
        );
        let agent_chunks = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("agent_message_chunk")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>();
        assert!(
            agent_chunks
                .iter()
                .any(|chunk| chunk.contains("old answer can be replayed as agent history")),
            "assistant history should still be replayed: {output:#?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_load_replays_history_tool_records_as_acp_tool_updates_not_text() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, _paths) = start_bearwire_test_server_with_history(vec![
            json!({
                "id": "call-history",
                "kind": "tool_call",
                "role": "assistant",
                "tool_call_id": "call-history",
                "tool_name": "run_command",
                "status": "pending",
                "arguments": { "command": "true" }
            }),
            json!({
                "id": "call-history",
                "kind": "tool_result",
                "role": "assistant",
                "tool_call_id": "call-history",
                "tool_name": "run_command",
                "status": "ok",
                "text": "Used run_command (ok)",
                "raw_output": { "exit_code": 0 }
            }),
        ])
        .await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("history-tool-replay");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "load-1",
                    "method": "session/load",
                    "params": { "sessionId": "session-1" }
                }),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let tool_frames = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("tool_call")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>();
        assert_eq!(tool_frames.len(), 2, "{output:#?}");
        assert!(
            tool_frames
                .iter()
                .all(|frame| frame.contains("call-history")),
            "{output:#?}"
        );
        assert!(
            tool_frames.iter().any(|frame| frame.contains("completed")),
            "{output:#?}"
        );
        assert!(
            tool_frames
                .iter()
                .all(|frame| !frame.contains("incomplete")),
            "{output:#?}"
        );
        let agent_text = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("agent_message_chunk")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!agent_text.contains("Used run_command"), "{output:#?}");
        assert!(!agent_text.contains("incomplete"), "{output:#?}");
        assert!(
            output.iter().any(
                |frame| frame.get("id").and_then(Value::as_str) == Some("load-1")
                    && frame.get("result").is_some()
            ),
            "{output:#?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_resume_does_not_replay_history_updates() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, _paths) = start_bearwire_test_server_with_history(vec![
            json!({
                "id": "answer-resume-history",
                "kind": "message",
                "role": "assistant",
                "text": "A historical agent answer"
            }),
            json!({
                "id": "call-resume-history",
                "kind": "tool_call",
                "role": "assistant",
                "tool_call_id": "call-resume-history",
                "tool_name": "run_command",
                "status": "pending",
                "arguments": { "command": "true" }
            }),
            json!({
                "id": "call-resume-history",
                "kind": "tool_result",
                "role": "assistant",
                "tool_call_id": "call-resume-history",
                "tool_name": "run_command",
                "status": "ok",
                "text": "Used run_command (ok)",
                "raw_output": { "exit_code": 0 }
            }),
        ])
        .await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("resume-history-no-replay");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "resume-1",
                    "method": "session/resume",
                    "params": { "sessionId": "session-1" }
                }),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let tool_frames = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("tool_call")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>();
        assert!(
            tool_frames.is_empty(),
            "session/resume must not replay historical tool updates: {output:#?}"
        );

        let agent_text = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("agent_message_chunk")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !agent_text.contains("A historical agent answer"),
            "{output:#?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_load_replays_surface_reasoning_as_thought_not_agent_message() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("BEARS_BEARWIRE", "true");
        }
        let (api_url, _paths) = start_bearwire_test_server_with_history(vec![
            json!({
                "id": "reasoning-1",
                "kind": "reasoning_delta",
                "delta": "private reasoning",
                "source": "provider_reasoning",
                "replay_policy": "thought"
            }),
            json!({
                "id": "answer-1",
                "kind": "message",
                "role": "assistant",
                "text": "visible answer"
            }),
        ])
        .await;
        let http = reqwest::Client::new();
        let root = unique_test_dir("history-reasoning-replay");
        let mut runtime = test_runtime_config(api_url);
        let mut state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();

        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &http,
                &mut runtime,
                &mut state,
                &shared_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": "load-1",
                    "method": "session/load",
                    "params": { "sessionId": "session-1" }
                }),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();

        let thought_frames = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("agent_thought_chunk")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>();
        assert_eq!(thought_frames.len(), 1, "{output:#?}");
        assert!(
            thought_frames[0].contains("private reasoning"),
            "{output:#?}"
        );

        let agent_frames = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("agent_message_chunk")
            })
            .map(Value::to_string)
            .collect::<Vec<_>>();
        assert_eq!(agent_frames.len(), 1, "{output:#?}");
        assert!(
            agent_frames
                .iter()
                .any(|frame| frame.contains("visible answer")),
            "{output:#?}"
        );
        assert!(
            !agent_frames
                .iter()
                .any(|frame| frame.contains("private reasoning")),
            "reasoning leaked as agent message: {output:#?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generic_empty_completion_preview_is_suppressed() {
        let value = json!({ "content": "" });
        assert_eq!(tool_completion_preview("fs_stat", &value), "");
    }

    #[test]
    fn placeholder_tool_completion_preview_exposes_structured_result() {
        let value = json!({ "content": "", "result": { "changed": true } });
        let preview = tool_completion_preview("tool", &value);
        assert!(preview.contains("Tool call completed"), "{preview}");
        assert!(preview.contains("\"changed\":true"), "{preview}");
        assert_ne!(preview, "Tool completed.");
    }

    #[test]
    fn permission_local_tool_completion_payload_omits_noisy_content() {
        let value = json!({ "content": "Local tool terminal_run_command completed." });
        let payload = json!({
            "tool_call_id": "call-1",
            "tool_name": "terminal_run_command",
            "status": "ok",
            "content": "",
            "structured_content": value,
            "diagnostic": { "phase": "permission_local_tool_completed" }
        });
        assert_eq!(payload["content"], "");
        assert_eq!(
            payload["structured_content"]["content"],
            "Local tool terminal_run_command completed."
        );
    }

    #[test]
    fn friendly_tool_status_mentions_path_and_action() {
        let event = json!({ "args": { "path": "/workspace/README.md" } });
        assert_eq!(
            friendly_tool_status("fs_replace_text", &event, "permission"),
            "Waiting for approval: modify this file. Target: `/workspace/README.md`."
        );
        assert_eq!(
            friendly_tool_status("fs_list_directory", &event, "running"),
            "Listing `/workspace/README.md`…"
        );
    }

    #[test]
    fn local_tool_status_strings_are_protocol_stable() {
        assert_eq!(LocalToolStatus::Ok.as_str(), "ok");
        assert_eq!(LocalToolStatus::Error.as_str(), "error");
        assert_eq!(
            LocalToolStatus::PermissionDenied.as_str(),
            "permission_denied"
        );
        assert_eq!(LocalToolStatus::Timeout.as_str(), "timeout");
        assert_eq!(LocalToolStatus::Cancelled.as_str(), "cancelled");
        assert_eq!(LocalToolStatus::Unsupported.as_str(), "unsupported");
    }

    #[tokio::test]
    async fn read_text_file_denies_sensitive_paths_when_policy_requires() {
        let root = unique_test_dir("read-sensitive");
        let secret = root.join("api-token.txt");
        fs::write(&secret, "secret").unwrap();
        let state = test_adapter_state("session-1", &root);
        let denied = handle_direct_read_text_file(
            &state,
            json!({ "sessionId": "session-1", "path": secret.to_string_lossy() }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("denied sensitive path"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn list_directory_enforces_root_containment() {
        let root = unique_test_dir("list-root");
        let outside = unique_test_dir("list-outside");
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_list_directory(
            &state,
            "session-1",
            &json!({ "path": outside.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", result.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn list_directory_reports_truncation() {
        let root = unique_test_dir("list-truncated");
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_list_directory(
            &state,
            "session-1",
            &json!({ "path": root.to_string_lossy(), "limit": 1 }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["returned_entries"], 1);
        assert_eq!(result["truncated"], true);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn find_paths_matches_glob_and_hides_dotfiles_by_default() {
        let root = unique_test_dir("find-paths");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join("src").join("lib.rs"), "").unwrap();
        fs::write(root.join("src").join("main.ts"), "").unwrap();
        fs::write(root.join(".hidden").join("secret.rs"), "").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_find_paths(
            &state,
            "session-1",
            &json!({ "root": root.to_string_lossy(), "glob": "src/*.rs" }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["returned_matches"], 1);
        assert_eq!(result["matches"][0]["relative_path"], "src/lib.rs");
        let hidden = handle_direct_find_paths(
            &state,
            "session-1",
            &json!({ "root": root.to_string_lossy(), "glob": ".hidden/*.rs" }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(hidden["returned_matches"], 0);
        fs::write(root.join("secret-token.txt"), "secret").unwrap();
        let sensitive = handle_direct_find_paths(
            &state,
            "session-1",
            &json!({ "root": root.to_string_lossy(), "glob": "*secret*", "include_hidden": true }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                include_hidden_default: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(sensitive["returned_matches"], 0);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn find_paths_enforces_root_containment_and_policy_limit() {
        let root = unique_test_dir("find-root");
        let outside = unique_test_dir("find-outside");
        fs::write(root.join("a.txt"), "").unwrap();
        fs::write(root.join("b.txt"), "").unwrap();
        let state = test_adapter_state("session-1", &root);
        let denied = handle_direct_find_paths(
            &state,
            "session-1",
            &json!({ "root": outside.to_string_lossy(), "glob": "*.txt" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let limited = handle_direct_find_paths(
            &state,
            "session-1",
            &json!({ "root": root.to_string_lossy(), "glob": "*.txt", "limit": 99 }),
            &ToolPolicy {
                max_results: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(limited["returned_matches"], 1);
        assert_eq!(limited["truncated"], true);
        assert_eq!(limited["policy"]["applied_limit"], 1);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn stat_reports_file_directory_and_denies_outside_root() {
        let root = unique_test_dir("stat-root");
        let outside = unique_test_dir("stat-outside");
        let file = root.join("file.txt");
        fs::write(&file, "hello").unwrap();
        let state = test_adapter_state("session-1", &root);
        let file_stat = handle_direct_stat(
            &state,
            "session-1",
            &json!({ "path": file.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(file_stat["kind"], "file");
        assert_eq!(file_stat["size_bytes"], 5);
        let dir_stat = handle_direct_stat(
            &state,
            "session-1",
            &json!({ "path": root.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(dir_stat["kind"], "directory");
        let denied = handle_direct_stat(
            &state,
            "session-1",
            &json!({ "path": outside.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let secret = root.join("token-secret.txt");
        fs::write(&secret, "secret").unwrap();
        let sensitive = handle_direct_stat(
            &state,
            "session-1",
            &json!({ "path": secret.to_string_lossy() }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", sensitive.unwrap_err()).contains("denied sensitive path"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn search_files_enforces_root_containment() {
        let root = unique_test_dir("search-root");
        let outside = unique_test_dir("search-outside");
        fs::write(outside.join("file.txt"), "needle").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_search_files(
            &state,
            "session-1",
            &json!({ "path": outside.to_string_lossy(), "query": "needle" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", result.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn search_files_reports_result_truncation() {
        let root = unique_test_dir("search-truncated");
        fs::write(root.join("file.txt"), "needle one\nneedle two\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_search_files(
            &state,
            "session-1",
            &json!({ "path": root.to_string_lossy(), "query": "needle", "limit": 1 }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["returned_matches"], 1);
        assert_eq!(result["truncated"], true);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn list_directory_uses_policy_max_entries() {
        let root = unique_test_dir("list-policy");
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_list_directory(
            &state,
            "session-1",
            &json!({ "path": root.to_string_lossy(), "limit": 99 }),
            &ToolPolicy {
                max_entries: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["returned_entries"], 1);
        assert_eq!(result["policy"]["max_entries"], 1);
        assert_eq!(result["policy"]["applied_limit"], 1);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn search_files_uses_policy_limits_and_hidden_default() {
        let root = unique_test_dir("search-policy");
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join(".hidden").join("a.txt"), "needle hidden").unwrap();
        fs::write(root.join("b.txt"), "needle visible\nneedle again\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_search_files(
            &state,
            "session-1",
            &json!({ "path": root.to_string_lossy(), "query": "needle", "limit": 99 }),
            &ToolPolicy {
                max_results: Some(1),
                include_hidden_default: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["returned_matches"], 1);
        assert_eq!(result["include_hidden"], true);
        assert_eq!(result["policy"]["max_results"], 1);
        assert_eq!(result["policy"]["applied_limit"], 1);
        fs::write(root.join("secret-token.txt"), "needle secret").unwrap();
        let sensitive = handle_direct_search_files(
            &state,
            "session-1",
            &json!({ "path": root.to_string_lossy(), "query": "secret", "limit": 99 }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                include_hidden_default: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(sensitive["returned_matches"], 0);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn create_text_file_creates_new_file_and_refuses_overwrite() {
        let root = unique_test_dir("create-file");
        let file = root.join("new.txt");
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_create_text_file(
            &state,
            "session-1",
            &json!({ "path": file.to_string_lossy(), "content": "hello\n" }),
            &ToolPolicy {
                max_bytes: Some(1024),
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                deny_hidden_paths: Some(true),
                create_files: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["created"], true);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello\n");
        let denied = handle_direct_create_text_file(
            &state,
            "session-1",
            &json!({ "path": file.to_string_lossy(), "content": "again\n" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("already exists"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn create_text_file_can_create_parent_dirs() {
        let root = unique_test_dir("create-parents");
        let file = root.join("nested").join("new.txt");
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_create_text_file(
            &state,
            "session-1",
            &json!({ "path": file.to_string_lossy(), "content": "hello\n", "create_parent_dirs": true }),
            &ToolPolicy { create_files: Some(true), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(result["created"], true);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello\n");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn create_directory_creates_dir_and_refuses_existing_without_flag() {
        let root = unique_test_dir("create-directory");
        let dir = root.join("new-dir");
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_create_directory(
            &state,
            "session-1",
            &json!({ "path": dir.to_string_lossy() }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                deny_hidden_paths: Some(true),
                create_files: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["created"], true);
        assert!(dir.is_dir());

        let denied = handle_direct_create_directory(
            &state,
            "session-1",
            &json!({ "path": dir.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("already exists"));

        let existing = handle_direct_create_directory(
            &state,
            "session-1",
            &json!({ "path": dir.to_string_lossy(), "allow_existing": true }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(existing["created"], false);
        assert_eq!(existing["existed"], true);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn create_directory_creates_parents_and_denies_hidden_sensitive_and_outside_paths() {
        let root = unique_test_dir("create-directory-policy");
        let outside = unique_test_dir("create-directory-outside");
        let nested = root.join("a").join("b").join("c");
        let state = test_adapter_state("session-1", &root);
        let missing_parent = handle_direct_create_directory(
            &state,
            "session-1",
            &json!({ "path": nested.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", missing_parent.unwrap_err()).contains("parents=true"));

        let result = handle_direct_create_directory(
            &state,
            "session-1",
            &json!({ "path": nested.to_string_lossy(), "parents": true }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["created"], true);
        assert!(nested.is_dir());

        let hidden = handle_direct_create_directory(
            &state,
            "session-1",
            &json!({ "path": root.join(".hidden").to_string_lossy() }),
            &ToolPolicy {
                deny_hidden_paths: Some(true),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", hidden.unwrap_err()).contains("denied hidden path"));

        let sensitive = handle_direct_create_directory(
            &state,
            "session-1",
            &json!({ "path": root.join("secret-dir").to_string_lossy() }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", sensitive.unwrap_err()).contains("denied sensitive path"));

        let outside_denied = handle_direct_create_directory(
            &state,
            "session-1",
            &json!({ "path": outside.join("dir").to_string_lossy(), "parents": true }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", outside_denied.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn move_path_moves_file_and_directory_and_refuses_overwrite_by_default() {
        let root = unique_test_dir("move-path");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "hello").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_move_path(
            &state,
            "session-1",
            &json!({
                "source_path": source.to_string_lossy(),
                "destination_path": destination.to_string_lossy(),
                "expected_kind": "file"
            }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["moved"], true);
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "hello");

        let source_dir = root.join("dir");
        let destination_dir = root.join("renamed-dir");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("child.txt"), "child").unwrap();
        let result = handle_direct_move_path(
            &state,
            "session-1",
            &json!({
                "source_path": source_dir.to_string_lossy(),
                "destination_path": destination_dir.to_string_lossy(),
                "expected_kind": "directory"
            }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["kind"], "directory");
        assert!(!source_dir.exists());
        assert!(destination_dir.join("child.txt").exists());

        let second_source = root.join("second-source.txt");
        fs::write(&second_source, "second").unwrap();
        let denied = handle_direct_move_path(
            &state,
            "session-1",
            &json!({
                "source_path": second_source.to_string_lossy(),
                "destination_path": destination.to_string_lossy()
            }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("destination already exists"));
        assert!(second_source.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn move_path_supports_overwrite_and_denies_invalid_paths() {
        let root = unique_test_dir("move-path-policy");
        let outside = unique_test_dir("move-path-outside");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_move_path(
            &state,
            "session-1",
            &json!({
                "source_path": source.to_string_lossy(),
                "destination_path": destination.to_string_lossy(),
                "overwrite": true
            }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["overwrite"], true);
        assert_eq!(fs::read_to_string(&destination).unwrap(), "source");

        let hidden_source = root.join("visible.txt");
        fs::write(&hidden_source, "hidden").unwrap();
        let hidden = handle_direct_move_path(
            &state,
            "session-1",
            &json!({
                "source_path": hidden_source.to_string_lossy(),
                "destination_path": root.join(".hidden-dest").to_string_lossy()
            }),
            &ToolPolicy {
                deny_hidden_paths: Some(true),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", hidden.unwrap_err()).contains("denied hidden path"));

        let sensitive_source = root.join("plain.txt");
        fs::write(&sensitive_source, "secret").unwrap();
        let sensitive = handle_direct_move_path(
            &state,
            "session-1",
            &json!({
                "source_path": sensitive_source.to_string_lossy(),
                "destination_path": root.join("secret-dest").to_string_lossy()
            }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", sensitive.unwrap_err()).contains("denied sensitive path"));

        let outside_denied = handle_direct_move_path(
            &state,
            "session-1",
            &json!({
                "source_path": destination.to_string_lossy(),
                "destination_path": outside.join("moved.txt").to_string_lossy()
            }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", outside_denied.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn copy_path_copies_file_and_directory_with_limits() {
        let root = unique_test_dir("copy-path");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "hello").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": source.to_string_lossy(), "destination_path": destination.to_string_lossy() }),
            &ToolPolicy { max_bytes: Some(1024), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(result["copied"], true);
        assert_eq!(fs::read_to_string(&destination).unwrap(), "hello");

        let dir = root.join("dir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("child.txt"), "child").unwrap();
        let dir_copy = root.join("dir-copy");
        let denied = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": dir.to_string_lossy(), "destination_path": dir_copy.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("recursive=true"));
        let result = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": dir.to_string_lossy(), "destination_path": dir_copy.to_string_lossy(), "recursive": true }),
            &ToolPolicy { max_entries: Some(10), max_bytes: Some(1024), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(result["kind"], "directory");
        assert_eq!(
            fs::read_to_string(dir_copy.join("child.txt")).unwrap(),
            "child"
        );

        let too_large = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": source.to_string_lossy(), "destination_path": root.join("too-large.txt").to_string_lossy() }),
            &ToolPolicy { max_bytes: Some(1), ..Default::default() },
        )
        .await;
        assert!(format!("{:#}", too_large.unwrap_err()).contains("max_bytes"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn copy_path_refuses_overwrite_and_denies_hidden_sensitive_outside() {
        let root = unique_test_dir("copy-path-policy");
        let outside = unique_test_dir("copy-path-outside");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();
        let state = test_adapter_state("session-1", &root);
        let denied = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": source.to_string_lossy(), "destination_path": destination.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("destination already exists"));
        let overwritten = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": source.to_string_lossy(), "destination_path": destination.to_string_lossy(), "overwrite": true }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(overwritten["overwrite"], true);
        assert_eq!(fs::read_to_string(&destination).unwrap(), "source");
        let hidden = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": source.to_string_lossy(), "destination_path": root.join(".hidden-copy").to_string_lossy() }),
            &ToolPolicy { deny_hidden_paths: Some(true), ..Default::default() },
        )
        .await;
        assert!(format!("{:#}", hidden.unwrap_err()).contains("denied hidden path"));
        let sensitive = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": source.to_string_lossy(), "destination_path": root.join("secret-copy").to_string_lossy() }),
            &ToolPolicy { sensitive_path_policy: Some("deny_sensitive_paths".to_string()), ..Default::default() },
        )
        .await;
        assert!(format!("{:#}", sensitive.unwrap_err()).contains("denied sensitive path"));
        let outside_denied = handle_direct_copy_path(
            &state,
            "session-1",
            &json!({ "source_path": source.to_string_lossy(), "destination_path": outside.join("copy.txt").to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", outside_denied.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn apply_patch_creates_updates_deletes_and_dry_runs() {
        let root = unique_test_dir("apply-patch");
        let state = test_adapter_state("session-1", &root);
        let create_patch = "--- /dev/null\n+++ b/new.txt\n@@\n+hello\n";
        let dry = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": create_patch, "dry_run": true }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(dry["dry_run"], true);
        assert!(!root.join("new.txt").exists());
        handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": create_patch }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read_to_string(root.join("new.txt")).unwrap(), "hello\n");
        let update_patch = "--- a/new.txt\n+++ b/new.txt\n@@\n-hello\n+goodbye\n";
        handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": update_patch }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("new.txt")).unwrap(),
            "goodbye\n"
        );
        let delete_patch = "--- a/new.txt\n+++ /dev/null\n@@\n-goodbye\n";
        let denied = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": delete_patch }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("allow_delete=false"));
        handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": delete_patch, "allow_delete": true }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert!(!root.join("new.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn apply_patch_denies_invalid_sensitive_outside_and_disallowed_create() {
        let root = unique_test_dir("apply-patch-policy");
        let state = test_adapter_state("session-1", &root);
        let create_patch = "--- /dev/null\n+++ b/new.txt\n@@\n+hello\n";
        let denied_create = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": create_patch, "allow_create": false }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied_create.unwrap_err()).contains("allow_create=false"));
        let sensitive_patch = "--- /dev/null\n+++ b/secret-file\n@@\n+secret\n";
        let sensitive = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": sensitive_patch }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", sensitive.unwrap_err()).contains("denied sensitive path"));
        let outside_patch = "--- /dev/null\n+++ b/../outside.txt\n@@\n+bad\n";
        let outside = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": outside_patch }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", outside.unwrap_err()).contains("must be relative"));
        let invalid = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": "not a patch" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", invalid.unwrap_err()).contains("found no file diff headers"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn apply_patch_supports_multifile_and_enforces_limits() {
        let root = unique_test_dir("apply-patch-limits");
        let state = test_adapter_state("session-1", &root);
        let patch = "--- /dev/null\n+++ b/a.txt\n@@\n+a\n--- /dev/null\n+++ b/b.txt\n@@\n+b\n";
        handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": patch }),
            &ToolPolicy {
                max_entries: Some(2),
                max_bytes: Some(1024),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a\n");
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b\n");

        let too_many = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": patch }),
            &ToolPolicy {
                max_entries: Some(1),
                max_bytes: Some(1024),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", too_many.unwrap_err()).contains("max_entries"));

        let too_large = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": patch }),
            &ToolPolicy {
                max_bytes: Some(10),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", too_large.unwrap_err()).contains("max_bytes"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn apply_patch_denies_hidden_targets() {
        let root = unique_test_dir("apply-patch-hidden");
        let state = test_adapter_state("session-1", &root);
        let patch = "--- /dev/null\n+++ b/.hidden\n@@\n+hidden\n";
        let hidden = handle_direct_apply_patch(
            &state,
            "session-1",
            &json!({ "base_path": root.to_string_lossy(), "patch": patch }),
            &ToolPolicy {
                deny_hidden_paths: Some(true),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", hidden.unwrap_err()).contains("denied hidden path"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn delete_path_removes_file_and_denies_workspace_root() {
        let root = unique_test_dir("delete-file");
        let file = root.join("delete-me.txt");
        fs::write(&file, "bye").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_delete_path(
            &state,
            "session-1",
            &json!({ "path": file.to_string_lossy(), "expected_kind": "file" }),
            &ToolPolicy {
                max_entries: Some(100),
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                deny_hidden_paths: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["deleted"], true);
        assert!(!file.exists());
        let denied = handle_direct_delete_path(
            &state,
            "session-1",
            &json!({ "path": root.to_string_lossy(), "expected_kind": "directory" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("workspace root"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn delete_path_requires_recursive_for_non_empty_directory() {
        let root = unique_test_dir("delete-dir");
        let dir = root.join("dir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("file.txt"), "bye").unwrap();
        let state = test_adapter_state("session-1", &root);
        let denied = handle_direct_delete_path(
            &state,
            "session-1",
            &json!({ "path": dir.to_string_lossy(), "expected_kind": "directory" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err()).contains("recursive=true"));
        let result = handle_direct_delete_path(
            &state,
            "session-1",
            &json!({ "path": dir.to_string_lossy(), "expected_kind": "directory", "recursive": true }),
            &ToolPolicy { max_entries: Some(100), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(result["deleted"], true);
        assert!(!dir.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn search_files_supports_case_insensitive_extension_and_pattern_filters() {
        let root = unique_test_dir("search-filters");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("src").join("lib.rs"), "Needle visible\n").unwrap();
        fs::write(root.join("src").join("lib.txt"), "Needle wrong extension\n").unwrap();
        fs::write(root.join("docs").join("guide.rs"), "Needle wrong pattern\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_search_files(
            &state,
            "session-1",
            &json!({
                "path": root.to_string_lossy(),
                "query": "needle",
                "case_sensitive": false,
                "extensions": ["rs"],
                "pattern": "src/*"
            }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["returned_matches"], 1);
        assert_eq!(
            result["matches"][0]["path"].as_str().unwrap(),
            root.join("src").join("lib.rs").to_string_lossy()
        );
        assert_eq!(result["skipped_by_filter"], 2);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn git_status_and_diff_report_workspace_repo_state() {
        let root = unique_test_dir("git-tools");
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.test"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(root.join("tracked.txt"), "before\n").unwrap();
        Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(root.join("tracked.txt"), "after\n").unwrap();
        fs::write(root.join("untracked.txt"), "new\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let status = handle_git_status(
            context,
            &json!({ "repo_path": root.to_string_lossy() }),
            &ToolPolicy {
                max_bytes: Some(4096),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(status["clean"], false);
        assert!(status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "tracked.txt"));
        assert!(status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "untracked.txt"));

        let diff = handle_git_diff(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "paths": ["tracked.txt"] }),
            &ToolPolicy {
                max_bytes: Some(4096),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(diff["diff"].as_str().unwrap().contains("-before"));
        assert!(diff["diff"].as_str().unwrap().contains("+after"));
        assert_eq!(diff["truncated"], false);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn find_paths_supports_globstar_patterns() {
        let root = unique_test_dir("find-globstar");
        fs::create_dir_all(root.join("src").join("nested")).unwrap();
        fs::write(root.join("src").join("nested").join("lib.rs"), "").unwrap();
        fs::write(root.join("src").join("main.ts"), "").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_find_paths(
            &state,
            "session-1",
            &json!({ "root": root.to_string_lossy(), "glob": "**/*.rs" }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(result["returned_matches"], 1);
        assert_eq!(result["matches"][0]["relative_path"], "src/nested/lib.rs");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn git_tools_resolve_nested_repo_paths_to_toplevel() {
        let root = unique_test_dir("git-nested");
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.test"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(nested.join("file.txt"), "hello\n").unwrap();
        Command::new("git")
            .args(["add", "nested/file.txt"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(nested.join("file.txt"), "changed\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let status = handle_git_status(
            context,
            &json!({ "repo_path": nested.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            status["repo_path"].as_str().unwrap(),
            root.to_string_lossy()
        );
        assert!(status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "nested/file.txt"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn git_tools_reject_non_repo_and_diff_paths_outside_repo() {
        let root = unique_test_dir("git-invalid");
        let repo = root.join("repo");
        let sibling = root.join("sibling");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&repo)
            .output()
            .unwrap();
        fs::write(repo.join("file.txt"), "before\n").unwrap();
        Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&repo)
            .output()
            .unwrap();
        fs::write(repo.join("file.txt"), "after\n").unwrap();
        fs::write(sibling.join("outside.txt"), "outside\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let non_repo = handle_git_status(
            context,
            &json!({ "repo_path": sibling.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", non_repo.unwrap_err()).contains("not inside a git work tree"));
        let outside_path = handle_git_diff(
            context,
            &json!({ "repo_path": repo.to_string_lossy(), "paths": [sibling.join("outside.txt").to_string_lossy()] }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", outside_path.unwrap_err()).contains("outside repo"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn git_log_and_show_report_commits_and_files() {
        let root = unique_test_dir("git-log-show");
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.test"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(root.join("one.txt"), "one\n").unwrap();
        Command::new("git")
            .args(["add", "one.txt"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "first"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(root.join("two.txt"), "two\n").unwrap();
        Command::new("git")
            .args(["add", "two.txt"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "second"])
            .current_dir(&root)
            .output()
            .unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let log = handle_git_log(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "max_count": 1 }),
            &ToolPolicy {
                max_results: Some(100),
                max_bytes: Some(4096),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(log["returned_commits"], 1);
        assert_eq!(log["commits"][0]["subject"], "second");
        let show_file = handle_git_show(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "revision": "HEAD", "path": "two.txt" }),
            &ToolPolicy {
                max_bytes: Some(4096),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(show_file["output"], "two\n");
        let show_commit = handle_git_show(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "revision": "HEAD", "max_bytes": 32 }),
            &ToolPolicy {
                max_bytes: Some(32),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(show_commit["truncated"], true);
        assert!(show_commit["output"].as_str().unwrap().len() <= 32);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn git_log_and_show_enforce_path_and_revision_safety() {
        let root = unique_test_dir("git-log-show-safety");
        let outside = unique_test_dir("git-log-show-outside");
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.test"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(root.join("one.txt"), "one\n").unwrap();
        Command::new("git")
            .args(["add", "one.txt"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "first"])
            .current_dir(&root)
            .output()
            .unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let outside_log = handle_git_log(
            context,
            &json!({ "repo_path": outside.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", outside_log.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let outside_show = handle_git_show(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "revision": "HEAD", "path": outside.join("x.txt").to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", outside_show.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let bad_revision = handle_git_show(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "revision": "--help" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", bad_revision.unwrap_err()).contains("unsupported"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn git_tools_enforce_root_containment_and_diff_byte_limit() {
        let root = unique_test_dir("git-limit");
        let outside = unique_test_dir("git-outside");
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(root.join("file.txt"), "before\n").unwrap();
        Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.test"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(root.join("file.txt"), "after after after\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let denied = handle_git_status(
            context,
            &json!({ "repo_path": outside.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", denied.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let diff = handle_git_diff(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "max_bytes": 10 }),
            &ToolPolicy {
                max_bytes: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(diff["truncated"], true);
        assert!(diff["diff"].as_str().unwrap().len() <= 10);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn replace_text_successfully_edits_existing_file() {
        let root = unique_test_dir("replace-success");
        let file = root.join("a.txt");
        fs::write(&file, "hello old world\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_replace_text(
            &state,
            "session-1",
            &json!({
                "path": file.to_string_lossy(),
                "old_text": "old",
                "new_text": "new"
            }),
            &ToolPolicy {
                max_bytes: Some(1024),
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["replacements"], 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello new world\n");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn replace_text_preflight_preview_and_revalidates_before_write() {
        let root = unique_test_dir("replace-preflight");
        let file = root.join("a.txt");
        fs::write(&file, "hello old world\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = state.session_contexts.get("session-1").unwrap();
        let policy = ToolPolicy {
            max_bytes: Some(1024),
            sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
            max_replacements: Some(1),
            create_files: Some(false),
            allow_multiple: Some(false),
            deny_hidden_paths: Some(true),
            ..Default::default()
        };
        let args = ReplaceTextArgs::from_value(
            &json!({
                "path": file.to_string_lossy(),
                "old_text": "old",
                "new_text": "new"
            }),
            &policy,
        )
        .unwrap();
        let plan = ReplaceTextPlan::preflight(context, args, &policy).unwrap();
        assert!(plan.preview.contains("--- before"));
        assert!(plan.preview.contains("+++ after"));
        assert!(plan
            .permission_prompt("fs_replace_text", "approve?")
            .contains("hello old world"));
        fs::write(&file, "hello changed world\n").unwrap();
        let result = plan.apply(context, &policy);
        assert!(format!("{:#}", result.unwrap_err()).contains("stale preflight"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello changed world\n");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn edit_file_replace_all_replaces_every_match_when_policy_allows_it() {
        let root = unique_test_dir("edit-replace-all");
        let file = root.join("a.txt");
        fs::write(&file, "old old old\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_replace_text(
            &state,
            "session-1",
            &json!({
                "path": file.to_string_lossy(),
                "old_text": "old",
                "new_text": "new",
                "replace_all": true
            }),
            &ToolPolicy {
                max_replacements: Some(3),
                allow_multiple: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["replacements"], 3);
        assert_eq!(fs::read_to_string(&file).unwrap(), "new new new\n");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn replace_text_denies_multiple_matches_by_default() {
        let root = unique_test_dir("replace-multiple");
        let file = root.join("a.txt");
        fs::write(&file, "old old\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_replace_text(
            &state,
            "session-1",
            &json!({
                "path": file.to_string_lossy(),
                "old_text": "old",
                "new_text": "new"
            }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", result.unwrap_err()).contains("expected 1 match"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "old old\n");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn replace_text_denies_sensitive_paths() {
        let root = unique_test_dir("replace-sensitive");
        let file = root.join(".env");
        fs::write(&file, "TOKEN=old\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_replace_text(
            &state,
            "session-1",
            &json!({
                "path": file.to_string_lossy(),
                "old_text": "old",
                "new_text": "new"
            }),
            &ToolPolicy {
                sensitive_path_policy: Some("deny_sensitive_paths".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", result.unwrap_err()).contains("denied sensitive path"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "TOKEN=old\n");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn replace_text_applies_policy_max_bytes() {
        let root = unique_test_dir("replace-max-bytes");
        let file = root.join("a.txt");
        fs::write(&file, "old text longer than five bytes\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let result = handle_direct_replace_text(
            &state,
            "session-1",
            &json!({
                "path": file.to_string_lossy(),
                "old_text": "old",
                "new_text": "new"
            }),
            &ToolPolicy {
                max_bytes: Some(5),
                ..Default::default()
            },
        )
        .await;
        assert!(format!("{:#}", result.unwrap_err()).contains("exceeds policy max_bytes"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn git_add_commit_restore_and_stash_workflows() {
        let root = unique_test_dir("git-write");
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.test"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&root)
            .output()
            .unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        fs::write(root.join("file.txt"), "one\n").unwrap();
        let added = handle_git_add(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "paths": ["file.txt"] }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(added["ok"], true);
        let committed = handle_git_commit(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "message": "initial" }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(committed["ok"], true);
        fs::write(root.join("file.txt"), "two\n").unwrap();
        handle_git_restore(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "paths": ["file.txt"] }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read_to_string(root.join("file.txt")).unwrap(), "one\n");
        fs::write(root.join("scratch.txt"), "scratch\n").unwrap();
        let stashed = handle_git_stash(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "message": "scratch", "include_untracked": true }),
            &ToolPolicy::default(),
        ).await.unwrap();
        assert_eq!(stashed["ok"], true);
        assert!(!root.join("scratch.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn git_write_tools_reject_empty_or_outside_paths_and_bad_messages() {
        let root = unique_test_dir("git-write-policy");
        let outside = unique_test_dir("git-write-outside");
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.test"])
            .current_dir(&root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&root)
            .output()
            .unwrap();
        fs::write(root.join("file.txt"), "one\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let empty = handle_git_add(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "paths": [] }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", empty.unwrap_err()).contains("at least one path"));
        let outside_path = handle_git_add(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "paths": [outside.join("x.txt").to_string_lossy()] }),
            &ToolPolicy::default(),
        ).await;
        assert!(format!("{:#}", outside_path.unwrap_err())
            .contains("outside the ACP session workspace roots"));
        let bad_commit = handle_git_commit(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "message": "" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", bad_commit.unwrap_err()).contains("missing message"));
        let too_many = handle_git_add(
            context,
            &json!({ "repo_path": root.to_string_lossy(), "paths": ["file.txt"] }),
            &ToolPolicy {
                max_entries: Some(0),
                ..Default::default()
            },
        )
        .await;
        assert!(too_many.is_ok(), "max_entries clamps to at least 1");
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn process_run_executes_command_and_caps_output() {
        let root = unique_test_dir("process-run");
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let result = handle_process_run(
            context,
            "session-1",
            &json!({
                "command": "printf",
                "args": ["hello"],
                "cwd": root.to_string_lossy(),
                "max_output_bytes": 10,
                "reducer_mode": "none"
            }),
            &ToolPolicy {
                max_bytes: Some(64),
                total_timeout_ms: Some(10_000),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["stdout"], "hello");
        assert_eq!(result["truncated"], false);

        let truncated = handle_process_run(
            context,
            "session-1",
            &json!({
                "command": "printf",
                "args": ["hello world"],
                "cwd": root.to_string_lossy(),
                "max_output_bytes": 5,
                "reducer_mode": "none"
            }),
            &ToolPolicy {
                max_bytes: Some(5),
                total_timeout_ms: Some(10_000),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(truncated["stdout"], "hello");
        assert_eq!(truncated["stdout_truncated"], true);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn process_run_uses_optional_rtk_reducer() {
        let root = unique_test_dir("process-run-rtk");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let rtk = bin.join("rtk");
        fs::write(
            &rtk,
            "#!/bin/sh\nprintf 'RTK summary: compact failure context\\n'\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&rtk).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
            fs::set_permissions(&rtk, perms).unwrap();
        }

        let old_path = std::env::var("PATH").unwrap_or_default();
        let old_enabled = std::env::var("BEARS_PROCESS_RUN_RTK").ok();
        std::env::set_var("PATH", format!("{}:{}", bin.display(), old_path));
        std::env::set_var("BEARS_PROCESS_RUN_RTK", "1");

        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let result = handle_process_run(
            context,
            "session-1",
            &json!({
                "command": "printf",
                "args": ["very long noisy output"],
                "cwd": root.to_string_lossy(),
            }),
            &ToolPolicy {
                max_bytes: Some(1024),
                total_timeout_ms: Some(10_000),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        std::env::set_var("PATH", old_path);
        if let Some(value) = old_enabled {
            std::env::set_var("BEARS_PROCESS_RUN_RTK", value);
        } else {
            std::env::remove_var("BEARS_PROCESS_RUN_RTK");
        }

        assert_eq!(result["execution_wrapper"], Value::Null);
        assert_eq!(result["rtk_wrap_allowed"], false);
        assert_eq!(result["reduction"]["reducer"], "rtk");
        assert!(result["content"]
            .as_str()
            .unwrap()
            .contains("RTK summary: compact failure context"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn process_run_reports_nonzero_timeout_and_rejects_unsafe_inputs() {
        let root = unique_test_dir("process-run-policy");
        let outside = unique_test_dir("process-run-outside");
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let nonzero = handle_process_run(
            context,
            "session-1",
            &json!({ "command": "sh", "args": ["-c", "exit 7"], "cwd": root.to_string_lossy(), "reducer_mode": "none" }),
            &ToolPolicy {
                max_bytes: Some(1024),
                total_timeout_ms: Some(10_000),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(nonzero["ok"], false);
        assert_eq!(nonzero["exit_code"], 7);

        let timed_out = handle_process_run(
            context,
            "session-1",
            &json!({ "command": "sleep", "args": ["1"], "cwd": root.to_string_lossy(), "timeout_ms": 1, "reducer_mode": "none" }),
            &ToolPolicy { max_bytes: Some(1024), total_timeout_ms: Some(100), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(timed_out["timed_out"], true);

        let outside_cwd = handle_process_run(
            context,
            "session-1",
            &json!({ "command": "printf", "args": ["x"], "cwd": outside.to_string_lossy(), "reducer_mode": "none" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", outside_cwd.unwrap_err())
            .contains("outside the ACP session workspace roots"));

        let shell_string = handle_process_run(
            context,
            "session-1",
            &json!({ "command": "echo hello", "cwd": root.to_string_lossy() }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", shell_string.unwrap_err()).contains("shell command string"));

        let secret_env = handle_process_run(
            context,
            "session-1",
            &json!({ "command": "printf", "args": ["x"], "cwd": root.to_string_lossy(), "env": { "API_TOKEN": "secret" } }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", secret_env.unwrap_err()).contains("secret-like"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn process_run_redirects_git_commands_to_dedicated_tools_unless_overridden() {
        let root = unique_test_dir("process-run-redirect");
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();

        let redirected = handle_process_run(
            context,
            "session-1",
            &json!({
                "command": "git",
                "args": ["diff", "--", "src/main.rs"],
                "cwd": root.to_string_lossy(),
            }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(redirected["ok"], false);
        assert_eq!(redirected["kind"], "prefer_dedicated_tool");
        assert_eq!(redirected["suggested_tool"], "git_diff");
        assert_eq!(
            redirected["suggested_args"]["repo_path"],
            root.to_string_lossy().to_string()
        );

        let _ = std::process::Command::new("git")
            .arg("init")
            .current_dir(&root)
            .output()
            .unwrap();
        std::fs::write(root.join("file.txt"), "hello\n").unwrap();

        let executed = handle_process_run(
            context,
            "session-1",
            &json!({
                "command": "git",
                "args": ["diff"],
                "cwd": root.to_string_lossy(),
                "bypass_tool_redirect": true,
                "reducer_mode": "none"
            }),
            &ToolPolicy {
                max_bytes: Some(1024),
                total_timeout_ms: Some(10_000),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(executed.get("kind").is_none());
        assert_eq!(executed["command"], "git");

        let _ = fs::remove_dir_all(root);
    }

    #[ignore = "canonical web_fetch is Den-executed; adapter local fetch will be renamed if reintroduced"]
    #[tokio::test]
    async fn web_fetch_fetches_and_truncates_http_response() {
        std::env::set_var("DEN_ACP_ALLOW_LOCAL_WEB_FETCH_FOR_TESTS", "1");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let body = "hello world";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        let result = crate::tools::web::handle_local_web_fetch(
            "session-1",
            &json!({ "url": format!("http://{}", addr), "max_bytes": 5 }),
            &ToolPolicy {
                max_bytes: Some(5),
                total_timeout_ms: Some(10_000),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["status"], 200);
        assert_eq!(result["body"], "hello");
        assert_eq!(result["truncated"], true);
        std::env::remove_var("DEN_ACP_ALLOW_LOCAL_WEB_FETCH_FOR_TESTS");
    }

    #[tokio::test]
    async fn process_run_redirects_rg_and_grep_to_fs_search_files() {
        let root = std::env::temp_dir().join(format!("bear-armature-rg-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();

        for command in ["rg", "grep"] {
            let redirected = handle_process_run(
                context,
                "session-1",
                &json!({
                    "command": command,
                    "args": ["needle", "src"],
                    "cwd": root.to_string_lossy(),
                }),
                &ToolPolicy::default(),
            )
            .await
            .unwrap();
            assert_eq!(redirected["ok"], false, "{command}");
            assert_eq!(redirected["kind"], "prefer_dedicated_tool", "{command}");
            assert_eq!(redirected["suggested_tool"], "fs_search_files", "{command}");
            assert_eq!(
                redirected["suggested_args"]["path"],
                root.to_string_lossy().to_string(),
                "{command}"
            );
            assert_eq!(redirected["suggested_args"]["query"], "src", "{command}");
        }

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn process_run_redirects_sed_to_fs_replace_text() {
        let root = std::env::temp_dir().join(format!("bear-armature-sed-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("file.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();

        let redirected = handle_process_run(
            context,
            "session-1",
            &json!({
                "command": "sed",
                "args": ["-i", "s/hello/hi/", file.to_string_lossy()],
                "cwd": root.to_string_lossy(),
            }),
            &ToolPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(redirected["ok"], false);
        assert_eq!(redirected["kind"], "prefer_dedicated_tool");
        assert_eq!(redirected["suggested_tool"], "fs_replace_text");
        assert_eq!(
            redirected["suggested_args"]["path"],
            file.to_string_lossy().to_string()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[ignore = "canonical web_fetch is Den-executed; adapter local fetch will be renamed if reintroduced"]
    #[tokio::test]
    async fn web_fetch_rejects_unsafe_urls() {
        std::env::remove_var("DEN_ACP_ALLOW_LOCAL_WEB_FETCH_FOR_TESTS");
        let localhost = crate::tools::web::handle_local_web_fetch(
            "session-1",
            &json!({ "url": "http://localhost:3000" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", localhost.unwrap_err()).contains("localhost"));
        let metadata = crate::tools::web::handle_local_web_fetch(
            "session-1",
            &json!({ "url": "http://169.254.169.254/latest" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", metadata.unwrap_err()).contains("private"));
        let invalid = crate::tools::web::handle_local_web_fetch(
            "session-1",
            &json!({ "url": "file:///tmp/x" }),
            &ToolPolicy::default(),
        )
        .await;
        assert!(format!("{:#}", invalid.unwrap_err()).contains("http and https"));
    }

    #[test]
    fn policy_from_event_reads_den_limits() {
        let policy = policy_from_event(&json!({
            "policy": {
                "max_lines": 5,
                "max_entries": 7,
                "max_results": 9,
                "max_bytes": 11,
                "recursive_default": true,
                "include_hidden_default": true,
                "max_replacements": 3,
                "create_files": false,
                "allow_multiple": false,
                "deny_hidden_paths": true,
                "total_timeout_ms": 1234,
                "execution_target": "armature_local",
                "approval_policy": "never",
                "sensitive_path_policy": "deny_sensitive_paths",
                "target_policy": { "kind": "workspace_root_or_path", "arg": "root", "default_to_workspace_root": true, "required_kind": "directory" }
            }
        }));
        assert_eq!(policy.max_lines, Some(5));
        assert_eq!(policy.max_entries, Some(7));
        assert_eq!(policy.max_results, Some(9));
        assert_eq!(policy.max_bytes, Some(11));
        assert_eq!(policy.recursive_default, Some(true));
        assert_eq!(policy.include_hidden_default, Some(true));
        assert_eq!(policy.max_replacements, Some(3));
        assert_eq!(policy.create_files, Some(false));
        assert_eq!(policy.allow_multiple, Some(false));
        assert_eq!(policy.deny_hidden_paths, Some(true));
        assert_eq!(policy.total_timeout_ms, Some(1234));
        assert_eq!(policy.execution_target.as_deref(), Some("armature_local"));
        assert_eq!(policy.approval_policy.as_deref(), Some("never"));
        assert_eq!(
            policy.sensitive_path_policy.as_deref(),
            Some("deny_sensitive_paths")
        );
        assert_eq!(
            policy.target_policy.as_ref().unwrap()["kind"],
            "workspace_root_or_path"
        );
        let context = SessionContext {
            cwd: "/workspace".to_string(),
            roots: vec!["/workspace".to_string()],
            ..Default::default()
        };
        assert_eq!(
            policy_target_path(&context, &json!({ "glob": "**/Cargo.toml" }), &policy),
            Some(PathBuf::from("/workspace"))
        );
    }

    #[test]
    fn permission_denied_error_sets_status_and_diagnostic() {
        let err = LocalToolError::permission_denied("nope");
        assert_eq!(err.status_str(), "permission_denied");
        assert_eq!(err.diagnostic["reason"], "client_permission_rejected");
    }

    #[test]
    fn parses_typed_acp_read_text_file_response() {
        let response = serde_json::from_value::<ReadTextFileResponse>(json!({
            "content": "hello from file"
        }))
        .unwrap();
        assert_eq!(response.content, "hello from file");
    }

    #[tokio::test]
    async fn client_read_preflight_rejects_missing_and_non_file_targets() {
        let root =
            std::env::temp_dir().join(format!("bear-armature-read-preflight-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let existing = root.join("read-text-file-empty-ok.txt");
        tokio::fs::write(&existing, "").await.unwrap();
        let metadata = preflight_client_read_text_file_target(&existing)
            .await
            .unwrap();
        assert_eq!(metadata.len(), 0);

        let missing = root.join("read-text-file-missing.txt");
        let err = preflight_client_read_text_file_target(&missing)
            .await
            .expect_err("missing file must fail before ACP client request");
        assert!(err
            .to_string()
            .contains("does not exist before ACP request"));

        let dir = root.join("directory-target");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let err = preflight_client_read_text_file_target(&dir)
            .await
            .expect_err("directory target must not be delegated to ACP client read");
        assert!(err.to_string().contains("not a file before ACP request"));

        tokio::fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn acp_client_read_response_conformance_rejects_invalid_empty_success() {
        let root =
            std::env::temp_dir().join(format!("bear-armature-empty-read-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let existing = root.join("read-text-file-empty-ok.txt");
        tokio::fs::write(&existing, "").await.unwrap();
        verify_client_read_text_file_response(&existing, "")
            .await
            .unwrap();
        tokio::fs::remove_file(&existing).await.unwrap();

        let non_empty = root.join("read-text-file-non-empty.txt");
        tokio::fs::write(&non_empty, "not empty").await.unwrap();
        verify_client_read_text_file_response(&non_empty, "not empty")
            .await
            .expect("non-empty client content is valid for a non-empty file");
        let err = verify_client_read_text_file_response(&non_empty, "")
            .await
            .expect_err("empty client content must not pass for a non-empty file");
        assert!(err.to_string().contains("empty content for non-empty file"));

        let missing = root.join("read-text-file-missing.txt");
        let err = verify_client_read_text_file_response(&missing, "")
            .await
            .expect_err("missing file must not look like successful empty read");
        let err_chain = format!("{err:#}");
        assert!(err_chain.contains("local verification failed"));
        assert!(err_chain.contains("does not exist before ACP request"));

        tokio::fs::remove_dir_all(&root).await.unwrap();
    }

    #[test]
    fn bearwire_tool_result_response_classifier_flags_stalled_continuations() {
        assert_eq!(
            classify_bearwire_tool_result_response(&json!({
                "ok": true,
                "continuation": "started"
            })),
            BearWireToolResultResponseClass::Continued
        );
        assert_eq!(
            classify_bearwire_tool_result_response(&json!({
                "ok": true,
                "continuation": "waiting_for_more_client_results"
            })),
            BearWireToolResultResponseClass::WaitingForMore
        );
        let unavailable = classify_bearwire_tool_result_response(&json!({
            "ok": false,
            "status": "continuation_unavailable",
            "reason": "native_agent_loop_session_not_found"
        }));
        assert_eq!(
            unavailable,
            BearWireToolResultResponseClass::ContinuationUnavailable
        );
        assert!(unavailable.needs_attention());
        let late = classify_bearwire_tool_result_response(&json!({
            "ok": false,
            "status": "late_result_ignored"
        }));
        assert_eq!(late, BearWireToolResultResponseClass::LateIgnored);
        assert!(late.needs_attention());
    }

    #[test]
    fn replay_tool_result_inherits_request_arguments_without_persisting_them() {
        let mut requests = std::collections::HashMap::new();
        let request_message = ReloadHistoryMessage {
            kind: "tool_call".to_string(),
            arguments: json!({ "path": "/workspace/README.md" }),
            ..Default::default()
        };
        let request = replay_tool_request(
            &mut requests,
            &request_message,
            "call-read",
            "fs_read_text_file",
        );
        let result_message = ReloadHistoryMessage {
            kind: "tool_result".to_string(),
            ..Default::default()
        };
        let terminal = replay_tool_request(
            &mut requests,
            &result_message,
            "call-read",
            "fs_read_text_file",
        );
        assert_eq!(terminal.arguments, request.arguments);
        assert!(requests.is_empty());
        assert_eq!(
            tool_call_title("fs_read_text_file", &terminal.projection_event()),
            "Read file: /workspace/README.md"
        );
    }

    #[test]
    fn live_and_replay_file_results_share_the_same_projection() {
        let tool_call_id = "call-read";
        let tool_name = "fs_read_text_file";
        let arguments = json!({ "path": "/workspace/README.md" });
        let live_event = json!({
            "data": {
                "tool_call": {
                    "id": tool_call_id,
                    "name": tool_name,
                    "arguments": arguments,
                }
            }
        });
        let live_request =
            ToolRequestPresentation::from_event(tool_call_id, tool_name, &live_event);

        let mut replay_requests = std::collections::HashMap::new();
        let replay_start = ReloadHistoryMessage {
            kind: "tool_call".to_string(),
            arguments: json!({ "path": "/workspace/README.md" }),
            ..Default::default()
        };
        replay_tool_request(&mut replay_requests, &replay_start, tool_call_id, tool_name);
        let replay_result = ReloadHistoryMessage {
            kind: "tool_result".to_string(),
            ..Default::default()
        };
        let replay_request = replay_tool_request(
            &mut replay_requests,
            &replay_result,
            tool_call_id,
            tool_name,
        );

        let live = project_tool_call(
            &live_request,
            ToolOutcome::new("completed", "Read complete.", None, Vec::new()),
        );
        let replay = project_tool_call(
            &replay_request,
            ToolOutcome::new("completed", "Read complete.", None, Vec::new()),
        );
        let live = serde_json::to_value(live).unwrap();
        let replay = serde_json::to_value(replay).unwrap();

        assert_eq!(live, replay);
        assert_eq!(live["title"], "Read file: /workspace/README.md");
        assert!(live.to_string().contains("/workspace/README.md"));
        assert!(replay_requests.is_empty());
    }

    #[tokio::test]
    async fn surface_tool_status_cache_is_session_scoped_and_clearable() {
        let shared = test_shared_state();
        assert!(
            record_surface_tool_status(
                &shared,
                "session-a",
                "call-reused",
                SurfaceToolStatus::Completed,
            )
            .await
        );
        assert!(
            record_surface_tool_status(
                &shared,
                "session-b",
                "call-reused",
                SurfaceToolStatus::InProgress,
            )
            .await
        );

        clear_surface_tool_statuses_for_session(&shared, "session-a").await;

        assert_eq!(
            current_surface_tool_status(&shared, "session-a", "call-reused").await,
            None
        );
        assert_eq!(
            current_surface_tool_status(&shared, "session-b", "call-reused").await,
            Some(SurfaceToolStatus::InProgress)
        );
        assert!(
            record_surface_tool_status(
                &shared,
                "session-a",
                "call-reused",
                SurfaceToolStatus::Pending,
            )
            .await
        );
    }

    #[test]
    fn surface_tool_status_updates_are_monotonic_and_idempotent() {
        assert!(should_emit_surface_tool_status(
            None,
            SurfaceToolStatus::Pending
        ));
        assert!(should_emit_surface_tool_status(
            Some(SurfaceToolStatus::Pending),
            SurfaceToolStatus::InProgress
        ));
        assert!(should_emit_surface_tool_status(
            Some(SurfaceToolStatus::InProgress),
            SurfaceToolStatus::Failed
        ));
        assert!(!should_emit_surface_tool_status(
            Some(SurfaceToolStatus::Failed),
            SurfaceToolStatus::Failed
        ));
        assert!(!should_emit_surface_tool_status(
            Some(SurfaceToolStatus::Completed),
            SurfaceToolStatus::InProgress
        ));
        assert!(!should_emit_surface_tool_status(
            Some(SurfaceToolStatus::Completed),
            SurfaceToolStatus::Pending
        ));
        assert!(!should_emit_surface_tool_status(
            Some(SurfaceToolStatus::Failed),
            SurfaceToolStatus::InProgress
        ));
        assert!(!should_emit_surface_tool_status(
            Some(SurfaceToolStatus::Completed),
            SurfaceToolStatus::Completed
        ));
    }

    #[tokio::test]
    async fn den_owned_tool_projection_skips_after_terminal_surface() {
        let shared = test_shared_state();
        let session_id = format!("session-{}", Uuid::new_v4());
        let tool_call_id = format!("call-{}", Uuid::new_v4());
        let turn_token = Uuid::new_v4();
        shared.active_prompts.lock().await.insert(
            session_id.clone(),
            ActivePromptTurn {
                token: turn_token,
                response: PromptResponseGuard::new(json!("test")),
                conversation_id: None,
            },
        );
        let completed_event = json!({
            "type": "tool_call.completed",
            "run_id": "run-terminal-first",
            "data": {
                "tool_call": { "id": tool_call_id, "name": "checkpoint" },
                "summary": "Checkpoint accepted."
            }
        });
        let requested_event = json!({
            "type": "tool_call.requested",
            "run_id": "run-terminal-first",
            "data": {
                "policy": { "execution_target": "den" },
                "tool_call": {
                    "id": tool_call_id,
                    "name": "checkpoint",
                    "arguments": { "reason": "test" }
                }
            }
        });

        let (result, output) = capture_json_output_for_test(|| async {
            send_tool_call_update_for_turn(
                &shared,
                &session_id,
                turn_token,
                requested_event
                    .pointer("/data/tool_call/id")
                    .and_then(Value::as_str)
                    .unwrap(),
                "checkpoint",
                ToolCallUpdatePayload {
                    status: "completed",
                    text: "Checkpoint accepted.",
                    request: Some(ToolRequestPresentation::from_event(
                        &tool_call_id,
                        "checkpoint",
                        &completed_event,
                    )),
                    raw_output: None,
                    extra_content: Vec::new(),
                },
            )
            .await?;
            project_den_owned_tool_request(&shared, &session_id, &requested_event, turn_token)
                .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();
        let tool_frames = output
            .iter()
            .filter(|frame| {
                frame.get("method").and_then(Value::as_str) == Some("session/update")
                    && frame
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some("tool_call")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tool_frames.len(),
            1,
            "late Den-owned start regressed card: {output:#?}"
        );
        assert!(
            tool_frames[0].to_string().contains("completed"),
            "expected only terminal card: {output:#?}"
        );
    }

    #[tokio::test]
    async fn stale_session_info_projection_cannot_mutate_session_state() {
        let shared = test_shared_state();
        let session_id = format!("session-{}", Uuid::new_v4());
        let current_turn = Uuid::new_v4();
        shared.active_prompts.lock().await.insert(
            session_id.clone(),
            ActivePromptTurn {
                token: current_turn,
                conversation_id: Some("conv-current".to_string()),
                response: PromptResponseGuard::new(json!("test")),
            },
        );
        let mut adapter_state = AdapterState {
            client_capabilities: Value::Null,
            session_contexts: HashMap::new(),
            transport: shared.transport.clone(),
        };

        handle_session_info_projection(
            &mut adapter_state,
            &shared,
            &session_id,
            Uuid::new_v4(),
            Some("stale title".to_string()),
            None,
            Some(json!({"remaining": 1})),
            Some(json!({"state":"stale"})),
        )
        .await
        .expect("stale projection is ignored");

        handle_conversation_resolved_projection(
            &test_config("http://127.0.0.1:1".to_string()),
            &mut adapter_state,
            &shared,
            &session_id,
            Uuid::new_v4(),
            "conv-stale",
        )
        .await
        .expect("stale binding projection is ignored");

        assert!(!adapter_state.session_contexts.contains_key(&session_id));
        assert!(!shared
            .session_contexts
            .lock()
            .await
            .contains_key(&session_id));
        assert_eq!(
            shared
                .active_prompts
                .lock()
                .await
                .get(&session_id)
                .and_then(|turn| turn.conversation_id.as_deref()),
            Some("conv-current")
        );
    }

    #[tokio::test]
    async fn den_owned_tool_projection_is_turn_gated() {
        let shared = test_shared_state();
        let session_id = format!("session-{}", Uuid::new_v4());
        let requested_event = json!({
            "type": "tool_call.requested",
            "run_id": "run-stale-den-owned",
            "data": {
                "obligation_id": "obl-stale-den-owned",
                "policy": { "execution_target": "den" },
                "tool_call": {
                    "id": format!("call-{}", Uuid::new_v4()),
                    "name": "checkpoint",
                    "arguments": { "reason": "stale" }
                }
            }
        });

        let (result, output) = capture_json_output_for_test(|| async {
            project_den_owned_tool_request(&shared, &session_id, &requested_event, Uuid::new_v4())
                .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        result.unwrap();
        assert!(
            output.iter().all(|frame| {
                frame
                    .pointer("/params/update/sessionUpdate")
                    .and_then(Value::as_str)
                    != Some("tool_call")
            }),
            "stale Den-owned projection emitted a tool card: {output:#?}"
        );
    }

    #[test]
    fn session_context_extracts_zed_workspace_folder_uri() {
        let context = session_context_from_params(&json!({
            "workspaceFolders": [
                { "uri": "file:///Users/bear/project%20space", "name": "project space" }
            ]
        }))
        .unwrap();
        assert_eq!(context.cwd, "/Users/bear/project space");
        assert_eq!(context.raw["cwd"], "/Users/bear/project space");
        assert_eq!(
            context.raw["workspace_roots"][0],
            "/Users/bear/project space"
        );
    }

    #[test]
    fn session_context_prefers_explicit_cwd() {
        let context = session_context_from_params(&json!({
            "cwd": "/Users/bear/active",
            "workspaceFolders": [{ "path": "/Users/bear/project" }]
        }))
        .unwrap();
        assert_eq!(context.cwd, "/Users/bear/active");
        assert_eq!(context.roots, vec!["/Users/bear/project".to_string()]);
    }

    #[test]
    fn session_context_uses_cwd_as_workspace_root_when_roots_are_absent() {
        let context = session_context_from_params(&json!({
            "cwd": "/Users/bear/project"
        }))
        .unwrap();
        assert_eq!(context.cwd, "/Users/bear/project");
        assert_eq!(context.roots, vec!["/Users/bear/project".to_string()]);
        assert_eq!(
            context.raw["workspace_roots"],
            json!(["/Users/bear/project"])
        );
    }

    #[test]
    fn session_context_reads_nested_workspace_roots_shapes() {
        let context = session_context_from_params(&json!({
            "cwd": "/Users/bear/project-a",
            "workspace": {
                "roots": [
                    { "rootUri": "file:///Users/bear/project-b" },
                    "/Users/bear/project-a"
                ]
            }
        }))
        .unwrap();
        assert_eq!(
            context.roots,
            vec![
                "/Users/bear/project-a".to_string(),
                "/Users/bear/project-b".to_string(),
            ]
        );
    }

    #[test]
    fn session_context_reads_top_level_root_uri_and_workspace_roots() {
        let context = session_context_from_params(&json!({
            "rootUri": "file:///Users/bear/project-a",
            "workspace_roots": [{ "uri": "file:///Users/bear/project-b" }]
        }))
        .unwrap();
        assert_eq!(context.cwd, "/Users/bear/project-a");
        assert_eq!(
            context.roots,
            vec![
                "/Users/bear/project-a".to_string(),
                "/Users/bear/project-b".to_string(),
            ]
        );
    }

    #[test]
    fn normalize_client_capabilities_accepts_snake_case_read_file() {
        let normalized = normalize_client_capabilities(json!({
            "fs": { "read_text_file": true }
        }));
        assert_eq!(normalized["fs"]["readTextFile"], true);
    }

    #[test]
    fn normalize_client_capabilities_accepts_snake_case_write_file() {
        let normalized = normalize_client_capabilities(json!({
            "fs": { "write_text_file": true }
        }));
        assert_eq!(normalized["fs"]["writeTextFile"], true);
    }

    #[test]
    fn normalize_client_capabilities_preserves_read_and_write_file() {
        let normalized = normalize_client_capabilities(json!({
            "filesystem": {
                "read_text_file": { "supported": true },
                "write_text_file": { "supported": true }
            }
        }));
        assert_eq!(normalized["fs"]["readTextFile"], true);
        assert_eq!(normalized["fs"]["writeTextFile"], true);
    }

    #[test]
    fn session_context_starts_without_conversation_ids() {
        let context = session_context_from_params(&json!({ "cwd": "/tmp/workspace" })).unwrap();
        assert!(context.conversation_id.is_none());
        assert!(context.resolved_conversation_id.is_none());
    }

    #[test]
    fn session_context_rejects_relative_cwd() {
        let err = session_context_from_params(&json!({ "cwd": "relative/project" })).unwrap_err();
        assert!(format!("{err:#}").contains("absolute local path"));
    }

    #[test]
    fn session_context_rejects_non_empty_mcp_servers() {
        let err = session_context_from_params(&json!({
            "cwd": "/tmp/workspace",
            "mcpServers": { "local": { "command": "server" } }
        }))
        .unwrap_err();
        assert!(format!("{err:#}").contains("mcpServers are not supported"));
    }

    #[test]
    fn prompt_prefers_resolved_conversation_id() {
        let context = SessionContext {
            conversation_id: Some("new-acp-zed-abc12345".to_string()),
            resolved_conversation_id: Some("conv-resolved12345".to_string()),
            ..Default::default()
        };
        let selected = context
            .resolved_conversation_id
            .as_deref()
            .or(context.conversation_id.as_deref());
        assert_eq!(selected, Some("conv-resolved12345"));
    }

    #[test]
    fn history_conversation_id_uses_authoritative_den_field() {
        let v = json!({
            "conversation_id": "new-acp-zed-x",
            "resolved_conversation_id": "not-a-prefix-the-adapter-knows",
            "history_conversation_id": "canonical-history-id"
        });
        assert_eq!(
            history_conversation_id(&v).as_deref(),
            Some("canonical-history-id")
        );
    }

    #[test]
    fn history_conversation_id_ignores_missing_or_blank_values() {
        assert_eq!(history_conversation_id(&json!({})), None);
        assert_eq!(
            history_conversation_id(&json!({ "history_conversation_id": "  " })),
            None
        );
    }

    #[test]
    fn map_den_sessions_list_maps_next_cursor() {
        let den = json!({
            "sessions": [{
                "acp_session_id": "s1",
                "updated_at": "2026-01-01T00:00:00Z",
                "conversation_id": "conv-x",
                "resolved_conversation_id": Value::Null,
                "client": "zed",
                "cwd": "/tmp"
            }],
            "next_cursor": "abc"
        });
        let m = map_den_sessions_list_to_acp(&den).unwrap();
        assert_eq!(m["nextCursor"], "abc");
        assert_eq!(m["sessions"][0]["sessionId"], "s1");
        assert_eq!(m["sessions"][0]["cwd"], "/tmp");
    }

    #[test]
    fn map_den_sessions_list_prefers_conversation_title_over_legacy_title() {
        let den = json!({
            "sessions": [{
                "acp_session_id": "s1",
                "updated_at": "2026-01-01T00:00:00Z",
                "conversation_id": "conv-x",
                "resolved_conversation_id": "den-conv-x",
                "conversation_title": "Canonical title",
                "title": "Legacy title",
                "cwd": "/tmp"
            }]
        });
        let m = map_den_sessions_list_to_acp(&den).unwrap();
        assert_eq!(m["sessions"][0]["title"], "Canonical title");
    }

    #[test]
    fn session_context_from_den_session_prefers_conversation_title() {
        let context = session_context_from_den_session(
            &json!({ "cwd": "/tmp" }),
            &json!({
                "cwd": "/tmp",
                "conversation_id": "conv-x",
                "conversation_title": "Canonical title",
                "title": "Legacy title"
            }),
        )
        .unwrap();
        assert_eq!(context.thread_title.as_deref(), Some("Canonical title"));
    }

    #[test]
    fn session_title_mapping_falls_back_to_legacy_title() {
        let den = json!({
            "sessions": [{
                "acp_session_id": "s1",
                "updated_at": "2026-01-01T00:00:00Z",
                "conversation_id": "conv-x",
                "resolved_conversation_id": Value::Null,
                "title": "Legacy title",
                "cwd": "/tmp"
            }]
        });
        let mapped = map_den_sessions_list_to_acp(&den).unwrap();
        assert_eq!(mapped["sessions"][0]["title"], "Legacy title");

        let context = session_context_from_den_session(
            &json!({ "cwd": "/tmp" }),
            &json!({
                "cwd": "/tmp",
                "conversation_id": "conv-x",
                "title": "Legacy title"
            }),
        )
        .unwrap();
        assert_eq!(context.thread_title.as_deref(), Some("Legacy title"));
    }

    #[test]
    fn browser_bridge_config_from_args_reads_flags_and_normalizes_path() {
        let config = BrowserBridgeConfig::from_args(
            vec![
                "--bind".to_string(),
                "127.0.0.1:7777".to_string(),
                "--token".to_string(),
                "secret-token".to_string(),
                "--path".to_string(),
                "bridge/".to_string(),
                "--allow-origin".to_string(),
                "https://example.test".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(config.bind, "127.0.0.1:7777");
        assert_eq!(config.token, "secret-token");
        assert_eq!(config.path, "/bridge");
        assert_eq!(config.allowed_origins, vec!["https://example.test"]);
    }

    #[test]
    fn browser_bridge_config_requires_token() {
        let err =
            BrowserBridgeConfig::from_args(vec!["--token".to_string(), "".to_string()].into_iter())
                .unwrap_err();
        assert!(format!("{err:#}").contains("requires a bearer token"));
    }

    #[test]
    fn normalize_browser_bridge_path_defaults_to_mcp() {
        assert_eq!(normalize_browser_bridge_path(""), "/mcp");
        assert_eq!(normalize_browser_bridge_path("/"), "/mcp");
        assert_eq!(normalize_browser_bridge_path("bridge"), "/bridge");
        assert_eq!(normalize_browser_bridge_path("/bridge/"), "/bridge");
    }

    #[test]
    fn session_context_from_params_adds_host_browser_bridge_from_env() {
        let previous_url = std::env::var("BEARS_HOST_BROWSER_MCP_URL").ok();
        let previous_token = std::env::var("BEARS_HOST_BROWSER_MCP_TOKEN").ok();
        let previous_name = std::env::var("BEARS_HOST_BROWSER_MCP_SERVER_NAME").ok();
        std::env::set_var(
            "BEARS_HOST_BROWSER_MCP_URL",
            "http://host.docker.internal:3766/mcp",
        );
        std::env::set_var("BEARS_HOST_BROWSER_MCP_TOKEN", "secret-token");
        std::env::set_var("BEARS_HOST_BROWSER_MCP_SERVER_NAME", "host-browser");

        let context = session_context_from_params(&json!({
            "cwd": "/workspace",
            "workspaceFolders": [{ "path": "/workspace" }]
        }))
        .unwrap();
        assert!(context.mcp_sources.iter().any(|source| matches!(
            source,
            McpSourceConfig::HostBrowserBridge { name, url, token }
                if name == "host-browser"
                    && url == "http://host.docker.internal:3766/mcp"
                    && token == "secret-token"
        )));
        assert_eq!(context.raw["host_browser_bridge"]["configured"], true);

        if let Some(previous) = previous_url {
            std::env::set_var("BEARS_HOST_BROWSER_MCP_URL", previous);
        } else {
            std::env::remove_var("BEARS_HOST_BROWSER_MCP_URL");
        }
        if let Some(previous) = previous_token {
            std::env::set_var("BEARS_HOST_BROWSER_MCP_TOKEN", previous);
        } else {
            std::env::remove_var("BEARS_HOST_BROWSER_MCP_TOKEN");
        }
        if let Some(previous) = previous_name {
            std::env::set_var("BEARS_HOST_BROWSER_MCP_SERVER_NAME", previous);
        } else {
            std::env::remove_var("BEARS_HOST_BROWSER_MCP_SERVER_NAME");
        }
    }

    #[test]
    fn browser_bridge_authorized_accepts_expected_bearer_token() {
        let config = BrowserBridgeConfig {
            bind: "127.0.0.1:3766".to_string(),
            token: "topsecret".to_string(),
            path: "/mcp".to_string(),
            allowed_origins: Vec::new(),
        };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer topsecret"),
        );
        assert!(browser_bridge_authorized(&headers, &config));
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer nope"),
        );
        assert!(!browser_bridge_authorized(&headers, &config));
    }

    #[test]
    fn detect_local_chrome_executable_prefers_explicit_env_override() {
        let previous_chrome = std::env::var("BEARS_CHROME_EXECUTABLE").ok();
        let previous_browser = std::env::var("BEARS_BROWSER_EXECUTABLE").ok();
        let temp = env::temp_dir().join(format!(
            "bear-armature-chrome-override-{}",
            std::process::id()
        ));
        fs::write(&temp, "").unwrap();
        std::env::set_var("BEARS_CHROME_EXECUTABLE", &temp);
        std::env::remove_var("BEARS_BROWSER_EXECUTABLE");

        let detected = crate::tools::chrome::detect_local_chrome_executable();
        assert_eq!(detected.as_deref(), Some(temp.as_path()));

        if let Some(previous) = previous_chrome {
            std::env::set_var("BEARS_CHROME_EXECUTABLE", previous);
        } else {
            std::env::remove_var("BEARS_CHROME_EXECUTABLE");
        }
        if let Some(previous) = previous_browser {
            std::env::set_var("BEARS_BROWSER_EXECUTABLE", previous);
        } else {
            std::env::remove_var("BEARS_BROWSER_EXECUTABLE");
        }
        let _ = fs::remove_file(&temp);
    }

    #[test]
    fn chrome_open_rejects_non_http_schemes() {
        let args = json!({ "url": "javascript:alert(1)" });
        let policy = ToolPolicy::default();
        let future = crate::tools::chrome::handle_chrome_open(&args, &policy);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(future).unwrap_err();
        assert!(format!("{err:#}").contains("only allows http and https"));
    }

    #[test]
    fn chrome_network_redaction_redacts_sensitive_headers() {
        let redacted = serde_json::json!({
            "method": "Network.requestWillBeSentExtraInfo",
            "params": {
                "headers": {
                    "Authorization": "Bearer secret",
                    "Cookie": "a=b",
                    "X-Api-Key": "xyz",
                    "User-Agent": "ok"
                },
                "requestHeaders": {
                    "Proxy-Authorization": "Basic abc",
                    "Accept": "*/*"
                },
                "responseHeaders": {
                    "Set-Cookie": "session=1",
                    "Content-Type": "text/html"
                }
            }
        });
        let value = crate::tools::chrome::test_redact_network_event(redacted);
        assert_eq!(value["params"]["headers"]["Authorization"], "<redacted>");
        assert_eq!(value["params"]["headers"]["Cookie"], "<redacted>");
        assert_eq!(value["params"]["headers"]["X-Api-Key"], "<redacted>");
        assert_eq!(value["params"]["headers"]["User-Agent"], "ok");
        assert_eq!(
            value["params"]["requestHeaders"]["Proxy-Authorization"],
            "<redacted>"
        );
        assert_eq!(value["params"]["requestHeaders"]["Accept"], "*/*");
        assert_eq!(
            value["params"]["responseHeaders"]["Set-Cookie"],
            "<redacted>"
        );
        assert_eq!(
            value["params"]["responseHeaders"]["Content-Type"],
            "text/html"
        );
    }

    #[tokio::test]
    async fn browser_bridge_health_endpoint_returns_ok_json() {
        let config = BrowserBridgeConfig {
            bind: "127.0.0.1:0".to_string(),
            token: "secret-token".to_string(),
            path: "/mcp".to_string(),
            allowed_origins: Vec::new(),
        };
        let session_manager = Arc::new(LocalSessionManager::default());
        let service = Arc::new(TokioMutex::new(StreamableHttpService::new(
            || Ok(McpRouter::new(BrowserBridgeServer)),
            session_manager,
            StreamableHttpServerConfig::default().with_stateful_mode(false),
        )));
        let app = browser_bridge_router(config, service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let response = reqwest::get(format!("http://{addr}/health")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["ok"], true);
        assert_eq!(body["service"], "bears-host-browser-bridge");

        server.abort();
    }

    #[tokio::test]
    async fn browser_bridge_mcp_endpoint_rejects_missing_auth() {
        let config = BrowserBridgeConfig {
            bind: "127.0.0.1:0".to_string(),
            token: "secret-token".to_string(),
            path: "/mcp".to_string(),
            allowed_origins: Vec::new(),
        };
        let session_manager = Arc::new(LocalSessionManager::default());
        let service = Arc::new(TokioMutex::new(StreamableHttpService::new(
            || Ok(McpRouter::new(BrowserBridgeServer)),
            session_manager,
            StreamableHttpServerConfig::default().with_stateful_mode(false),
        )));
        let app = browser_bridge_router(config, service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{addr}/mcp"))
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.text().await.unwrap(), "unauthorized");

        server.abort();
    }

    #[tokio::test]
    async fn browser_bridge_mcp_endpoint_accepts_auth_and_reaches_service() {
        let config = BrowserBridgeConfig {
            bind: "127.0.0.1:0".to_string(),
            token: "secret-token".to_string(),
            path: "/mcp".to_string(),
            allowed_origins: Vec::new(),
        };
        let session_manager = Arc::new(LocalSessionManager::default());
        let service = Arc::new(TokioMutex::new(StreamableHttpService::new(
            || Ok(McpRouter::new(BrowserBridgeServer)),
            session_manager,
            StreamableHttpServerConfig::default().with_stateful_mode(false),
        )));
        let app = browser_bridge_router(config, service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{addr}/mcp"))
            .header(reqwest::header::AUTHORIZATION, "Bearer secret-token")
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            }))
            .send()
            .await
            .unwrap();
        let status = response.status();
        assert_ne!(status, StatusCode::UNAUTHORIZED);
        let body = response.text().await.unwrap();
        assert!(status.is_success(), "status={status} body={body}");
        assert_ne!(body, "unauthorized");

        server.abort();
    }

    #[test]
    fn browser_tool_source_summary_prefers_client_forwarded_then_host_bridge_then_local() {
        let mut context = SessionContext::default();
        context.raw = json!({
            "mcp": {
                "client_tools": [
                    { "x_bears": { "source": "client_forwarded" } },
                    { "x_bears": { "source": "host_browser_bridge" } }
                ]
            }
        });
        let summary = browser_tool_source_summary(&context);
        assert_eq!(summary["active_source"], "client_forwarded_mcp");
        assert_eq!(summary["total_client_tools"], 2);
        assert_eq!(summary["source_counts"]["client_forwarded"], 1);
        assert_eq!(summary["source_counts"]["host_browser_bridge"], 1);

        context.raw = json!({
            "mcp": {
                "client_tools": [
                    { "x_bears": { "source": "host_browser_bridge" } }
                ]
            }
        });
        let summary = browser_tool_source_summary(&context);
        assert_eq!(summary["active_source"], "host_browser_bridge");
        assert_eq!(summary["total_client_tools"], 1);
        assert_eq!(summary["source_counts"]["host_browser_bridge"], 1);
    }

    #[tokio::test]
    async fn runtime_report_includes_browser_tools_section() {
        let http = reqwest::Client::new();
        let config = Config {
            api_url: "http://127.0.0.1:1".to_string(),
            bear: "test-bear".to_string(),
            token: "token-test".to_string(),
            client: "zed".to_string(),
        };
        let mut adapter_state = AdapterState::default();
        adapter_state.session_contexts.insert(
            "session-1".to_string(),
            SessionContext {
                raw: json!({
                    "mcp": {
                        "client_tools": [
                            { "x_bears": { "source": "host_browser_bridge" } }
                        ]
                    }
                }),
                ..Default::default()
            },
        );
        let shared = test_shared_state();
        let report = runtime_report(
            Some(&http),
            Some(&config),
            &adapter_state,
            &shared,
            "session-1",
        )
        .await;
        assert!(report.contains("Browser tools:"));
        assert!(report.contains("host_browser_bridge"));
    }

    #[tokio::test]
    async fn bear_environment_reports_session_and_mcp_state() {
        let mut adapter_state = AdapterState::default();
        adapter_state.client_capabilities = json!({ "client": "zed" });
        adapter_state.session_contexts.insert(
            "session-1".to_string(),
            SessionContext {
                cwd: "/workspace".to_string(),
                roots: vec!["/workspace".to_string()],
                raw: json!({
                    "mcp": {
                        "servers": [
                            { "name": "host-browser", "source": "host_browser_bridge", "status": "ok", "tool_count": 2 }
                        ],
                        "client_tools": [
                            { "name": "mcp__host_browser__list_pages", "x_bears": { "source": "host_browser_bridge" } }
                        ]
                    }
                }),
                ..Default::default()
            },
        );

        let value = collect_bear_environment(
            &adapter_state,
            "session-1",
            None,
            None,
            &json!({
                "include_client_capabilities": true,
                "include_session_mcp": true,
                "inspect_den": false
            }),
        )
        .await
        .unwrap();

        assert_eq!(value["session"]["id"], "session-1");
        assert_eq!(value["session"]["cwd"], "/workspace");
        assert_eq!(value["runtime"]["kind"], "acp_adapter");
        assert_eq!(value["browser"]["active_source"], "host_browser_bridge");
        assert_eq!(
            value["environment_variants"]["acp_adapter"]["session_mcp"]["servers"][0]["source"],
            "host_browser_bridge"
        );
        assert_eq!(
            value["environment_variants"]["acp_adapter"]["client_capabilities"]["client"],
            "zed"
        );
    }

    #[test]
    fn render_status_report_uses_environment_snapshot_and_surfaces_degraded_den() {
        let environment = json!({
            "runtime": { "kind": "acp_adapter", "version": "0.1.0" },
            "session": { "id": "session-1", "resolved_conversation_id": "conv-123" },
            "services": {
                "den": {
                    "configured": true,
                    "reachable": false,
                    "status": "unreachable",
                    "error": "connect failed"
                }
            },
            "browser": { "active_source": "host_browser_bridge" },
            "environment_variants": {
                "acp_adapter": {
                    "session_mcp": {
                        "servers": [
                            { "source": "host_browser_bridge", "status": "ok" }
                        ]
                    }
                }
            },
            "diagnostics": {
                "status": "degraded",
                "warnings": ["Den runtime is unreachable from the adapter"]
            }
        });
        let report = render_status_report(&environment, &[]);
        assert!(report.contains("Overall: degraded"));
        assert!(report.contains("Runtime: acp_adapter 0.1.0"));
        assert!(report.contains("Den:"));
        assert!(report.contains("unreachable"));
        assert!(report.contains("Warning: Den runtime is unreachable from the adapter"));
    }
}

#[cfg(test)]
mod bearwire_tool_request_parser_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runtime_diagnostics_params_add_current_bear_slug() {
        assert_eq!(
            runtime_diagnostics_rpc_params("builder", json!({"work_run_id": "run-1"})).unwrap(),
            json!({"bear_slug": "builder", "work_run_id": "run-1"})
        );
    }

    #[test]
    fn runtime_diagnostics_params_reject_non_object_arguments() {
        assert!(runtime_diagnostics_rpc_params("builder", json!(null)).is_err());
    }

    #[test]
    fn canonical_runtime_diagnostics_name_is_handled_locally() {
        assert!(is_runtime_diagnostics_tool("den.runtime_diagnostics.list"));
        assert!(is_runtime_diagnostics_tool("list_runtime_diagnostics"));
        assert!(!is_runtime_diagnostics_tool("list_work_runs"));
    }

    #[test]
    fn parses_canonical_bearwire_tool_request_data() {
        let event = json!({
            "type": "tool_call.requested",
            "data": {
                "obligation_id": "obl-call-req-1",
                "tool_call": {
                    "id": "call-req-1",
                    "name": "fs_read_text_file",
                    "arguments": { "path": "README.md" }
                }
            }
        });

        let parsed = BearWireToolCallRequestData::parse(&event).unwrap();

        assert_eq!(parsed.tool_call.id, "call-req-1");
        assert_eq!(parsed.tool_call.name, "fs_read_text_file");
        assert_eq!(parsed.tool_call.arguments["path"], "README.md");
    }

    #[test]
    fn parses_canonical_bearwire_client_waiting_data() {
        let event = json!({
            "type": "client.waiting",
            "data": {
                "expected_client_method": "client.permission.result",
                "obligation_id": "obligation-1",
                "permission": {
                    "id": "permission-1",
                    "title": "Approve command",
                    "reason": "Need to run the command.",
                    "target": { "command": "cargo", "args": ["check"] }
                },
                "tool_call": {
                    "id": "call-perm-1",
                    "name": "process_run",
                    "arguments": { "command": "cargo", "args": ["test"] },
                    "display": { "title": "Run cargo" }
                }
            }
        });

        let parsed = BearWireClientWaitingData::parse(&event).unwrap();

        assert_eq!(parsed.permission.id, "permission-1");
        assert_eq!(parsed.obligation_id, "obligation-1");
        assert_eq!(parsed.tool_call.id, "call-perm-1");
        assert_eq!(parsed.tool_call.name, "process_run");
        assert_eq!(parsed.permission.target.unwrap()["command"], "cargo");
    }
}
