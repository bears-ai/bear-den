mod approvals;
mod json_rpc;
mod paths;
mod tool_tasks;
mod tools;
mod update;

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

use approvals::{
    approval_url_host_scope, parse_permission_decision, permission_class_for_tool,
    permission_options_for_context, ApprovalCache, ApprovalScope, ApprovalTarget,
    PermissionDecision,
};
use axum::{extract::State, response::IntoResponse};
use futures_util::StreamExt;
use http::StatusCode;
use json_rpc::{id_key, write_json, JsonRpcTransport};
use paths::{file_uri_or_path_to_path, is_absolute_local_path, normalize_requested_tool_path};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
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
    sync::Arc,
};
use tokio::{
    io::{self, AsyncBufReadExt, BufReader},
    sync::{broadcast, mpsc, Mutex as TokioMutex},
    time::{timeout, Duration},
};
use tool_tasks::{log_tool_task_phase, ToolTaskPhase, ToolTaskRegistry};
use tools::chrome::{
    chrome_capability_status_line, chrome_tools_available, handle_chrome_console_messages,
    handle_chrome_network_requests, handle_chrome_open, handle_chrome_screenshot,
    handle_chrome_snapshot,
};
use tower_service::Service;

use tools::adapter_env::{
    collect_bear_environment, fetch_den_runtime_state, handle_bear_environment,
};
use tools::fs::{
    handle_apply_patch, handle_copy_path, handle_create_directory, handle_create_text_file,
    handle_delete_path, handle_find_paths, handle_list_directory, handle_move_path,
    handle_read_text_file, handle_replace_text, handle_search_files, handle_stat, ReplaceTextArgs,
    ReplaceTextPlan,
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
use tools::terminal::handle_terminal_run_command;
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
    tool_tasks: ToolTaskRegistry,
    mcp_registry: McpRegistry,
    approval_cache: ApprovalCache,
    cancellation_tx: broadcast::Sender<CancellationNotice>,
    active_prompts: Arc<TokioMutex<HashMap<String, ActivePromptTurn>>>,
}

#[derive(Clone, Debug)]
struct CancellationNotice {
    session_id: String,
    turn_token: Option<Uuid>,
    conversation_id: Option<String>,
}

#[derive(Clone, Debug)]
struct ActivePromptTurn {
    token: Uuid,
    conversation_id: Option<String>,
}

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
    sensitive_path_policy: Option<String>,
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
const BEARS_ACP_ADAPTER_CONTRACT_NAME: &str = "bears.acp.adapter";
const BEARS_ACP_ADAPTER_CONTRACT_VERSION: u32 = 1;
const LOCAL_DEN_INSPECTION_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn adapter_version() -> &'static str {
    env!("BEARS_ACP_ADAPTER_VERSION")
}

impl ToolPolicy {
    fn risk(&self) -> &str {
        if self.create_files.is_some()
            || self.allow_multiple.is_some()
            || self.max_replacements.is_some()
            || self.sensitive_path_policy.as_deref() == Some("deny_sensitive_paths")
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
    vec![SessionConfigOption::select(
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
    .category(SessionConfigOptionCategory::Mode)]
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

fn plan_entries_from_plan_update_event(event: &Value) -> Vec<PlanEntry> {
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
                    "bears-acp-adapter: failed to collect adapter environment for publish session_id={} error={err:#}",
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
            eprintln!(
                "bears-acp-adapter: failed to publish adapter environment session_id={} error={err:#}",
                session_id
            );
        }
    });
}

async fn send_session_info_update(
    session_id: &str,
    title: Option<String>,
    updated_at: Option<String>,
) -> Result<()> {
    let mut update = SessionInfoUpdate::new();
    if let Some(title) = title {
        update = update.title(title);
    }
    if let Some(updated_at) = updated_at {
        update = update.updated_at(updated_at);
    }
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": serde_json::to_value(SessionUpdate::SessionInfoUpdate(update))?,
        }),
    )
    .await
}

async fn send_bears_runtime_session_info_update(
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
    if env_bool("BEARS_ACP_DEBUG_UI") {
        eprintln!(
            "bears-acp-adapter: debug ui sent ACP plan update session_id={} entry_count={}",
            session_id, entry_count
        );
    }
    Ok(())
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
                "component": "bears-acp-adapter",
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
                "component": "bears-acp-adapter",
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
                "component": "bears-acp-adapter",
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
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
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

    fn summary(&self) -> String {
        format!(
            "frames={}, events={}, event_types={:?}, unknown_samples={:?}, saw_turn_complete={}, saw_visible_output={}, saw_tool_activity={}, saw_error={}",
            self.frames,
            self.events,
            self.event_types,
            self.unknown_event_samples,
            self.saw_turn_complete,
            self.saw_visible_output,
            self.saw_tool_activity,
            self.saw_error,
        )
    }
}

fn stream_has_successful_terminal_condition(
    saw_visible_output: bool,
    saw_error: bool,
    saw_done: bool,
    saw_tool_activity: bool,
) -> bool {
    saw_visible_output || saw_error || (saw_done && saw_tool_activity)
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
                .with_description("Browser-only MCP bridge served by bears-acp-adapter."),
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
        "bears-acp-adapter: browser-bridge listening addr={} path={} chrome={} origins={:?}",
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
        eprintln!("bears-acp-adapter: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let mut runtime = RuntimeConfig::from_env_and_args()?;
    eprintln!(
        "bears-acp-adapter: starting version={} build_git_sha={} built_at_utc={} local_head_sha={} ACP sessions=list/resume/load supported direct_tools={}",
        adapter_version(),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        env!("BEARS_ACP_ADAPTER_BUILT_AT_UTC"),
        local_head_sha(),
        direct_tools_context()
    );
    eprintln!(
        "bears-acp-adapter: chrome tools {}",
        chrome_capability_status_line()
    );
    if let Some(browser_bridge) = runtime.browser_bridge.clone() {
        run_browser_bridge(browser_bridge).await?;
        return Ok(());
    }

    if !runtime.doctor && runtime.update_command.is_none() {
        if runtime.is_configured() {
            eprintln!("bears-acp-adapter: configuration looks valid");
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

    let (inbound_tx, mut inbound_rx) = mpsc::channel::<InboundMessage>(128);
    let mut adapter_state = AdapterState::default();
    let approval_cache = ApprovalCache::load_for_runtime(&runtime).await;
    let (cancellation_tx, _) = broadcast::channel(64);
    let shared_state = AdapterSharedState {
        transport: adapter_state.transport.clone(),
        client_capabilities: Arc::new(TokioMutex::new(Value::Null)),
        session_contexts: Arc::new(TokioMutex::new(HashMap::new())),
        last_plan_update_hashes: Arc::new(TokioMutex::new(HashMap::new())),
        tool_tasks: ToolTaskRegistry::default(),
        mcp_registry: McpRegistry::default(),
        approval_cache,
        cancellation_tx,
        active_prompts: Arc::new(TokioMutex::new(HashMap::new())),
    };
    tokio::spawn(read_stdin_messages(
        inbound_tx,
        adapter_state.transport.clone(),
    ));

    while let Some(message) = inbound_rx.recv().await {
        let value = match message {
            InboundMessage::Request(value) => value,
            InboundMessage::Response { id, value } => {
                if !adapter_state.transport.route_response(&id, value).await {
                    eprintln!(
                        "bears-acp-adapter: unmatched JSON-RPC response id={}",
                        id_key(&id)
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

        if let Err(err) = handle_request(
            &http,
            &mut runtime,
            &mut adapter_state,
            &shared_state,
            request,
        )
        .await
        {
            eprintln!("bears-acp-adapter: request handling failed: {err:#}");
        }
    }

    Ok(())
}

impl BrowserBridgeConfig {
    fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut bind = env::var("BEARS_HOST_BROWSER_MCP_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3766".to_string());
        let mut token = env::var("BEARS_HOST_BROWSER_MCP_TOKEN").unwrap_or_default();
        let mut path =
            env::var("BEARS_HOST_BROWSER_MCP_PATH").unwrap_or_else(|_| "/mcp".to_string());
        let mut allowed_origins = env::var("BEARS_HOST_BROWSER_MCP_ALLOWED_ORIGINS")
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
                "--allow-origin" => allowed_origins.push(require_arg_value("--allow-origin", args.next())?),
                "--help" | "-h" => {
                    print_browser_bridge_help_to_stderr();
                    std::process::exit(0);
                }
                unknown => bail!("unknown browser-bridge argument {unknown:?}; use `bears-acp-adapter browser-bridge --help`"),
            }
        }

        bind = bind.trim().to_string();
        token = token.trim().to_string();
        if bind.is_empty() {
            bail!("browser-bridge requires a non-empty bind address; pass --bind <host:port>");
        }
        if token.is_empty() {
            bail!("browser-bridge requires a bearer token; set BEARS_HOST_BROWSER_MCP_TOKEN or pass --token <token>");
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

impl RuntimeConfig {
    fn from_env_and_args() -> Result<Self> {
        let mut api_url = env::var("DEN_API_URL").unwrap_or_default();
        let mut bear = env::var("BEAR_SLUG").unwrap_or_default();
        let mut token = env::var("DEN_TOKEN").unwrap_or_default();
        let mut token_env = env::var("DEN_TOKEN_ENV").unwrap_or_default();
        let mut client = env::var("BEARS_ACP_CLIENT").unwrap_or_else(|_| "zed".to_string());
        let mut check_config = false;
        let mut check_server = false;
        let mut doctor = false;
        let mut update_command: Option<UpdateCommand> = None;
        let mut browser_bridge: Option<BrowserBridgeConfig> = None;

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "browser-bridge" => {
                    browser_bridge = Some(BrowserBridgeConfig::from_args(args)?);
                    break;
                }
                "update-check" => {
                    update_command = Some(UpdateCommand::Check(UpdateOptions::from_args(args)?));
                    break;
                }
                "update" => {
                    update_command = Some(UpdateCommand::Update(UpdateOptions::from_args(args)?));
                    break;
                }
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
                    print_help_to_stderr();
                    std::process::exit(0);
                }
                unknown => return Err(anyhow!("unknown argument {unknown:?}; use --help")),
            }
        }

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
                "Missing Den bearer token. Set DEN_TOKEN, set DEN_TOKEN_ENV to the name of an environment variable containing the token, pass --token <token>, or pass --token-env <env-var>. Den ACP tokens include the acp:chat scope."
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
                eprintln!("bears-acp-adapter: configuration looks valid");
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
            "bears-acp-adapter: configuration is incomplete, so prompts cannot be sent to BEARS yet. The adapter will stay running so the ACP client can display this message instead of reporting that the server shut down unexpectedly.\n\nFix the following:",
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
        "bears-acp-adapter {}\nBuild git SHA: {}\nLocal HEAD SHA: {}\nACP sessions: list/resume/load; conversations bound via Den\nDirect tools: {}\nChrome tools: {}",
        adapter_version(),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
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
        "bears-acp-adapter browser-bridge\n\nUsage: bears-acp-adapter browser-bridge [--bind 127.0.0.1:3766] [--path /mcp] [--token <token>] [--allow-origin <origin>]...\n\nOptions:\n  --bind <host:port>      Bind address for the host browser MCP bridge HTTP server\n  --path <path>           MCP HTTP path, default /mcp\n  --token <token>         Required bearer token for Authorization: Bearer <token>\n  --allow-origin <url>    Allowed Origin value for browser requests; repeatable\n  --help                  Show this help\n\nEnvironment fallbacks:\n  BEARS_HOST_BROWSER_MCP_BIND\n  BEARS_HOST_BROWSER_MCP_PATH\n  BEARS_HOST_BROWSER_MCP_TOKEN\n  BEARS_HOST_BROWSER_MCP_ALLOWED_ORIGINS  comma-separated list",
    );
}

fn print_help_to_stderr() {
    eprintln!(
        "bears-acp-adapter {}\nBuild git SHA: {}\nLocal HEAD SHA: {}\nACP sessions: list/resume/load; conversations bound via Den\n\n\
Usage: bears-acp-adapter --api-url <url> --bear <slug> [--client zed] [--token-env DEN_TOKEN]\n       bears-acp-adapter doctor\n       bears-acp-adapter update-check [--channel stable]\n       bears-acp-adapter update [--open|--install|--download-only] [--yes]\n       bears-acp-adapter browser-bridge [--bind 127.0.0.1:3766] [--path /mcp] [--token <token>]\n\n\
Options:\n  --api-url <url>        Den API origin, for example https://api.bears.example\n  --bear <slug>          Bear slug to chat with\n  --token <token>        Den ACP token with acp:chat scope\n  --token-env <env-var>  Read the Den bearer token from this environment variable\n  --client <name>        Client label: zed, opencode, or acp_adapter\n  --check-config         Validate configuration and exit without starting ACP stdio\n  --check-server         Fetch Den /version and exit without starting ACP stdio\n  doctor, --doctor       Run user-friendly setup checks and exit\n  update-check           Check for a newer signed macOS package\n  update                 Download, verify, and install/open a newer macOS package\n  browser-bridge         Serve browser-only MCP tools over local Streamable HTTP\n  --version              Show version/build behavior and exit\n  --help                 Show this help\n\n\
Environment fallbacks:\n  DEN_API_URL\n  BEAR_SLUG\n  DEN_TOKEN\n  DEN_TOKEN_ENV\n  BEARS_ACP_CLIENT\n  BEARS_ACP_UPDATE_CHANNEL\n  BEARS_ACP_UPDATE_MANIFEST_URL\n\n\
DEN_API_URL should be the API origin only, not the full /acp/bears/... endpoint.",
        adapter_version(),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
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
                eprintln!("bears-acp-adapter: failed to read stdin: {err:#}");
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
                eprintln!(
                    "bears-acp-adapter: session/new session_id={} cwd={} roots={} direct_tools={} mcp={}",
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
                shared_state
                    .session_contexts
                    .lock()
                    .await
                    .insert(session_id.clone(), context.clone());
                adapter_state
                    .session_contexts
                    .insert(session_id.clone(), context);
                send_available_commands_update(&session_id).await?;
                if let Some(config) = runtime.config.as_ref() {
                    spawn_adapter_environment_publish(
                        config.clone(),
                        session_id.clone(),
                        adapter_state.clone(),
                        None,
                    );
                }
                let mode = MODE_ASK;
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
                notify_mode_state(&session_id, mode).await?;
            }
        }
        "session/set_config_option" => {
            if let Some(id) = request.id.clone() {
                let config_id = request
                    .params
                    .get("configId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if config_id != "mode" {
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
                eprintln!(
                    "bears-acp-adapter: session/set_config_option mode request session_id={} requested_mode={} effective_mode={} den_message={}",
                    session_id,
                    requested_mode,
                    mode,
                    den_response
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("<none>")
                );
                if requested_mode != mode {
                    eprintln!(
                        "bears-acp-adapter: Den adjusted client-requested mode={} for session_id={} to effective mode={}",
                        requested_mode, session_id, mode
                    );
                    let deferred = den_response
                        .get("deferred")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !deferred {
                        eprintln!(
                            "bears-acp-adapter: mode request adjusted session_id={} requested_mode={} effective_mode={} message={}",
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
                eprintln!(
                    "bears-acp-adapter: session/set_mode request session_id={} requested_mode={} effective_mode={} den_message={}",
                    session_id,
                    requested_mode,
                    mode,
                    den_response
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("<none>")
                );
                if requested_mode != mode {
                    eprintln!(
                        "bears-acp-adapter: Den adjusted client-requested legacy mode={} for session_id={} to effective mode={}",
                        requested_mode, session_id, mode
                    );
                    let deferred = den_response
                        .get("deferred")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !deferred {
                        eprintln!(
                            "bears-acp-adapter: mode request adjusted session_id={} requested_mode={} effective_mode={} message={}",
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
                    Ok(mode) => {
                        let response = ResumeSessionResponse::new()
                            .config_options(session_config_options_for_mode(mode))
                            .modes(session_modes_for_mode(mode));
                        write_response(id, Ok(serde_json::to_value(response)?)).await?
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
                    let prompt_state = AdapterState {
                        client_capabilities: shared_state.client_capabilities.lock().await.clone(),
                        session_contexts: shared_state.session_contexts.lock().await.clone(),
                        transport: shared_state.transport.clone(),
                    };
                    tokio::spawn(async move {
                        if let Err(err) = handle_local_slash_prompt(
                            Some(&http),
                            config.as_ref(),
                            &prompt_state,
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
                    write_response(
                        id,
                        Err(auth_check_json_rpc_error(
                            &err,
                            Some("Generate a fresh Den Code token for this bear. Code tokens must include acp:chat."),
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
                let previous = register_prompt_turn_for_session(
                    shared_state,
                    &session_id,
                    turn_token,
                    conversation_id_for_turn.clone(),
                )
                .await;
                if let Some(previous) = previous {
                    let same_conversation = prompt_conversations_overlap(
                        previous.conversation_id.as_deref(),
                        conversation_id_for_turn.as_deref(),
                    );
                    if same_conversation {
                        eprintln!(
                            "bears-acp-adapter: steering prompt for same conversation session_id={} previous_turn={} new_turn={} conversation={:?}; cancelling previous turn and gating stale UI text updates",
                            session_id, previous.token, turn_token, conversation_id_for_turn
                        );
                    } else {
                        eprintln!(
                            "bears-acp-adapter: overlapping prompt for different conversation session_id={} previous_turn={} new_turn={} previous_conversation={:?} new_conversation={:?}; keeping previous runtime alive and gating stale UI updates",
                            session_id, previous.token, turn_token, previous.conversation_id, conversation_id_for_turn
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
                        id.clone(),
                        request.params,
                        turn_token,
                    )
                    .await
                    {
                        Ok(()) => {}
                        Err(err) => {
                            let server_version = fetch_server_version(&http, &config).await.ok();
                            let mut message = format!("{err:#}");
                            if let Some(server_version) = &server_version {
                                message.push_str("\n\n");
                                message.push_str(&server_version.summary());
                            }
                            let _ = write_response(
                                id,
                                Err(json_rpc_error(
                                    -32003,
                                    "BEARS prompt failed",
                                    Some(json!({
                                        "message": message,
                                        "server_version": server_version.map(server_version_json),
                                    })),
                                )),
                            )
                            .await;
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
                        "bears-acp-adapter: ignoring session/close notification because adapter is not configured"
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
                                "BEARS session close failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                    } else {
                        eprintln!(
                            "bears-acp-adapter: session/close notification failed error={err:#}"
                        );
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
                        "bears-acp-adapter: ignoring session/cancel notification because adapter is not configured"
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
                            "bears-acp-adapter: session/cancel notification failed error={err:#}"
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
        "name": BEARS_ACP_ADAPTER_CONTRACT_NAME,
        "version": BEARS_ACP_ADAPTER_CONTRACT_VERSION,
    })
}

fn adapter_capabilities_context() -> Value {
    adapter_capabilities_context_with_client_mcp(false)
}

fn adapter_capabilities_context_with_client_mcp(has_client_mcp_tools: bool) -> Value {
    let chrome_supported = chrome_tools_available() && !has_client_mcp_tools;
    json!({
        "name": "bears-acp-adapter",
        "version": adapter_version(),
        "git_sha": env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        "built_at_utc": env!("BEARS_ACP_ADAPTER_BUILT_AT_UTC"),
        "api_contract": adapter_contract_context(),
        "direct_tools": {
            "fs_read_text_file": { "supported": true, "version": 1 },
            "fs_list_directory": { "supported": true, "version": 1 },
            "fs_find_paths": { "supported": true, "version": 1 },
            "fs_search_files": { "supported": true, "version": 1 },
            "fs_stat": { "supported": true, "version": 1 },
            "git_status": { "supported": true, "version": 1 },
            "git_diff": { "supported": true, "version": 1 },
            "git_log": { "supported": true, "version": 1 },
            "git_show": { "supported": true, "version": 1 },
            "git_add": { "supported": true, "version": 1 },
            "git_restore": { "supported": true, "version": 1 },
            "git_commit": { "supported": true, "version": 1 },
            "git_stash": { "supported": true, "version": 1 },
            "process_run": { "supported": true, "version": 1 },
            "terminal_run_command": { "supported": true, "version": 1 },
            "bear_environment": { "supported": true, "version": 1 },
            "chrome_open": { "supported": chrome_supported, "version": 1, "fallback_disabled_reason": if has_client_mcp_tools { "external_browser_mcp_tools_present" } else { "" } },
            "chrome_snapshot": { "supported": chrome_supported, "version": 1, "fallback_disabled_reason": if has_client_mcp_tools { "external_browser_mcp_tools_present" } else { "" } },
            "chrome_console_messages": { "supported": chrome_supported, "version": 1, "fallback_disabled_reason": if has_client_mcp_tools { "external_browser_mcp_tools_present" } else { "" } },
            "chrome_network_requests": { "supported": chrome_supported, "version": 1, "fallback_disabled_reason": if has_client_mcp_tools { "external_browser_mcp_tools_present" } else { "" } },
            "chrome_screenshot": { "supported": chrome_supported, "version": 1, "fallback_disabled_reason": if has_client_mcp_tools { "external_browser_mcp_tools_present" } else { "" } },
            "fs_edit_file": { "supported": true, "version": 1 },
            "fs_create_text_file": { "supported": true, "version": 1 },
            "fs_create_directory": { "supported": true, "version": 1 },
            "fs_move_path": { "supported": true, "version": 1 },
            "fs_copy_path": { "supported": true, "version": 1 },
            "fs_apply_patch": { "supported": true, "version": 1 },
            "fs_delete_path": { "supported": true, "version": 1 }
        }
    })
}

fn direct_tools_context() -> Value {
    direct_tools_context_with_client_mcp(false)
}

fn direct_tools_context_with_client_mcp(has_client_mcp_tools: bool) -> Value {
    let chrome_available = chrome_tools_available() && !has_client_mcp_tools;
    json!({
        "fs_read_text_file": true,
        "fs_list_directory": true,
        "fs_find_paths": true,
        "fs_search_files": true,
        "fs_stat": true,
        "git_status": true,
        "git_diff": true,
        "git_log": true,
        "git_show": true,
        "git_add": true,
        "git_restore": true,
        "git_commit": true,
        "git_stash": true,
        "process_run": true,
        "terminal_run_command": true,
        "bear_environment": true,
        "chrome_open": chrome_available,
        "chrome_snapshot": chrome_available,
        "chrome_console_messages": chrome_available,
        "chrome_network_requests": chrome_available,
        "chrome_screenshot": chrome_available,
        "client_mcp_tools_present": has_client_mcp_tools,
        "chrome_tools_disabled_reason": if has_client_mcp_tools { "external_browser_mcp_tools_present" } else { "" },
        "fs_edit_file": true,
        "fs_create_text_file": true,
        "fs_create_directory": true,
        "fs_move_path": true,
        "fs_copy_path": true,
        "fs_apply_patch": true,
        "fs_delete_path": true,
    })
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
    }
}

fn session_context_from_params(params: &Value) -> Result<SessionContext> {
    eprintln!(
        "bears-acp-adapter: session_context_from_params mcp_summary={}",
        summarize_acp_mcp_servers_param(params)
    );
    let mut mcp_sources = parse_acp_mcp_servers(params)?;
    if let Some(host_browser_bridge) = host_browser_bridge_config_from_env() {
        mcp_sources.push(host_browser_bridge);
    }
    let roots = workspace_roots_from_params(params);
    let cwd = explicit_cwd_from_params(params)
        .transpose()?
        .or_else(|| fallback_cwd_from_params(params))
        .or_else(|| roots.first().cloned())
        .ok_or_else(|| anyhow!("ACP session requires an absolute cwd; provide params.cwd as an absolute local path"))?;
    if !is_absolute_local_path(&cwd) {
        return Err(anyhow!(
            "ACP session cwd must be an absolute local path; got {cwd:?}"
        ));
    }
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
    validate_den_code_token(http, &config).await?;
    runtime.config = Some(config);
    runtime.diagnostics.clear();
    Ok(())
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
        return Err(anyhow!("Missing DEN_TOKEN. Paste a Den Code token when prompted, or configure DEN_TOKEN in Zed."));
    }
    Ok(Config {
        api_url,
        bear,
        token,
        client: runtime.client.clone(),
    })
}

async fn validate_den_code_token(http: &reqwest::Client, config: &Config) -> Result<()> {
    let url = format!(
        "{}/acp/bears/{}/auth-check",
        config.api_url,
        urlencoding::encode(&config.bear)
    );
    let response = http
        .get(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .send()
        .await
        .with_context(|| format!("validate BEARS Code token with Den at {url}"))?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(anyhow!(DenHttpError { status, body }))
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

async fn handle_client_read_text_file(
    adapter_state: &mut AdapterState,
    session_id: &str,
    args: &Value,
) -> Result<Value> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_read_text_file args missing path"))?;
    let mut request = ReadTextFileRequest::new(session_id.to_string(), PathBuf::from(path));
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
    eprintln!(
        "bears-acp-adapter: client fs/read_text_file path={} bytes={} duration_ms={}",
        path,
        content.len(),
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path,
        "content": content,
        "source": "acp_client",
        "raw_result": result,
        "bytes": content.len(),
    }))
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
    handle_read_text_file(context, &session_id, params, policy).await
}

fn policy_from_event(event: &Value) -> ToolPolicy {
    let policy = event.get("policy").unwrap_or(&Value::Null);
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
        sensitive_path_policy: policy
            .get("sensitive_path_policy")
            .and_then(Value::as_str)
            .map(str::to_string),
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
    adapter_state: &mut AdapterState,
    mcp_registry: &McpRegistry,
    session_id: &str,
    tool_name: &str,
    args: Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    match tool_name {
        "fs_read_text_file" | "fs.read_text_file" => {
            if client_supports_read_text_file(adapter_state) {
                handle_client_read_text_file(adapter_state, session_id, &args).await
            } else {
                eprintln!(
                    "bears-acp-adapter: client did not advertise fs/read_text_file; using adapter-local fallback"
                );
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
            )
            .await
        }
        "bear_environment" => {
            handle_bear_environment(adapter_state, session_id, None, None, &args).await
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

async fn handle_direct_list_directory(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_list_directory(context, session_id, args, policy).await
}

async fn handle_direct_find_paths(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_find_paths(context, session_id, args, policy).await
}

async fn handle_direct_search_files(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    handle_search_files(context, session_id, args, policy).await
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
    let content = event
        .get("args")
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
        let title = s
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
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

fn conversation_id_for_history(den_session: &Value) -> Option<String> {
    if let Some(r) = den_session
        .get("resolved_conversation_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if r.starts_with("conv-") || r == "default" {
            return Some(r.to_string());
        }
    }
    if let Some(c) = den_session
        .get("conversation_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if c.starts_with("conv-") || c == "default" {
            return Some(c.to_string());
        }
    }
    None
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
                "bears-acp-adapter: using adapter current directory as local session fallback cwd={} reason={err:#}",
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
    eprintln!(
        "bears-acp-adapter: session_context_from_den_session mcp_summary={}",
        summarize_acp_mcp_servers_param(params)
    );
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
        thread_title: den_session
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
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
    let mut url = format!(
        "{}/acp/bears/{}/sessions",
        config.api_url,
        urlencoding::encode(&config.bear)
    );
    let mut qs = Vec::new();
    if let Some(cwd) = params
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        qs.push(format!("cwd={}", urlencoding::encode(cwd)));
    }
    if let Some(cursor) = params
        .get("cursor")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        qs.push(format!("cursor={}", urlencoding::encode(cursor)));
    }
    if params
        .get("includeClosed")
        .or_else(|| params.get("include_closed"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        qs.push("include_closed=true".to_string());
    }
    if !qs.is_empty() {
        url.push('?');
        url.push_str(&qs.join("&"));
    }
    let response = http
        .get(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .send()
        .await
        .with_context(|| format!("list ACP sessions at {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "Den session list returned HTTP {status}: {}",
            body.trim()
        ));
    }
    serde_json::from_str(&body).with_context(|| "parse Den session list JSON")
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
    http: &reqwest::Client,
    config: Option<&Config>,
    session_id: &str,
    requested_mode: &str,
) -> Result<(&'static str, Value)> {
    let Some(config) = config else {
        return Ok((MODE_ASK, json!({ "message": "adapter is not configured" })));
    };
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/mode",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id),
    );
    let payload = with_adapter_contract(json!({
        "mode": requested_mode,
        "reason": "User selected ACP session mode"
    }));
    let response = http
        .post(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("post ACP session mode to Den at {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        if status == reqwest::StatusCode::NOT_FOUND && body.contains("ACP session not found") {
            eprintln!(
                "bears-acp-adapter: Den session mode deferred because session is not known yet session_id={} requested_mode={} status={} body={}",
                session_id,
                requested_mode,
                status,
                truncate_for_log(body.trim(), 240)
            );
            let pending_mode = normalize_mode(requested_mode);
            return Ok((
                pending_mode,
                json!({
                    "message": "Den session is not created yet; keeping the client-selected mode locally and applying it when the first prompt binds the session.",
                    "deferred": true,
                    "status": status.as_u16(),
                    "source": "adapter.den_session_mode_not_found",
                    "pending_mode": pending_mode,
                }),
            ));
        }
        return Err(anyhow!(
            "Den session mode endpoint returned HTTP {status}: {}",
            body.trim()
        ));
    }
    let value = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({ "raw": body }));
    eprintln!(
        "bears-acp-adapter: Den mode response session_id={} requested_mode={} response={}",
        session_id,
        requested_mode,
        serde_json::to_string(&value).unwrap_or_else(|_| "<unserializable>".to_string())
    );
    let effective = value
        .get("effective_mode")
        .and_then(Value::as_str)
        .and_then(|mode| match mode {
            MODE_ASK => Some(MODE_ASK),
            MODE_PLAN => Some(MODE_PLAN),
            MODE_WRITE => Some(MODE_WRITE),
            _ => None,
        })
        .unwrap_or(MODE_ASK);
    Ok((effective, value))
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
    let url = format!(
        "{}/acp/bears/{}/sessions/{}",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id),
    );
    let response = http
        .get(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .send()
        .await
        .with_context(|| format!("get ACP session at {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(DenHttpError { status, body }));
    }
    serde_json::from_str(&body).with_context(|| "parse Den get session JSON")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReloadHistoryMessage {
    id: Option<String>,
    role: String,
    text: String,
}

fn flatten_history_pages_chronological(
    pages_newest_first: Vec<Vec<ReloadHistoryMessage>>,
) -> Vec<ReloadHistoryMessage> {
    pages_newest_first.into_iter().rev().flatten().collect()
}

async fn fetch_conversation_history_chronological(
    http: &reqwest::Client,
    config: &Config,
    conversation_id: &str,
) -> Result<Vec<ReloadHistoryMessage>> {
    let mut pages_newest_first: Vec<Vec<ReloadHistoryMessage>> = Vec::new();
    let mut before: Option<String> = None;
    let mut seen_cursors = std::collections::HashSet::new();
    let mut page_idx = 0usize;
    loop {
        let mut url = format!(
            "{}/acp/bears/{}/conversations/{}/history?limit=50",
            config.api_url,
            urlencoding::encode(&config.bear),
            urlencoding::encode(conversation_id),
        );
        if let Some(b) = before.as_ref() {
            url.push_str("&before=");
            url.push_str(&urlencoding::encode(b));
        }
        let response = http
            .get(&url)
            .header(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", config.token))?,
            )
            .send()
            .await
            .with_context(|| format!("get conversation history at {url}"))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "Den history returned HTTP {status}: {}",
                body.trim()
            ));
        }
        let body: Value = serde_json::from_str(&body).context("parse history JSON")?;
        let messages = body
            .get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();
        let mut page = Vec::new();
        for m in messages {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("");
            let text = m.get("text").and_then(Value::as_str).unwrap_or("");
            if text.trim().is_empty() {
                continue;
            }
            page.push(ReloadHistoryMessage {
                id: m.get("id").and_then(Value::as_str).map(str::to_string),
                role: role.to_string(),
                text: text.to_string(),
            });
        }
        let first_id = page
            .first()
            .and_then(|m| m.id.as_deref())
            .unwrap_or("<none>")
            .to_string();
        let last_id = page
            .last()
            .and_then(|m| m.id.as_deref())
            .unwrap_or("<none>")
            .to_string();
        eprintln!(
            "bears-acp-adapter: history_page conversation_id={} page={} before={:?} messages={} first_id={} last_id={}",
            conversation_id,
            page_idx,
            before,
            page.len(),
            first_id,
            last_id
        );
        pages_newest_first.push(page);
        let has_more = body
            .get("has_more")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let next_before = body
            .get("next_before")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if !has_more {
            break;
        }
        let Some(next_before) = next_before.filter(|s| !s.is_empty()) else {
            break;
        };
        if !seen_cursors.insert(next_before.clone()) {
            return Err(anyhow!(
                "Den history pagination repeated cursor {next_before:?} for conversation {conversation_id}"
            ));
        }
        before = Some(next_before);
        page_idx += 1;
    }
    Ok(flatten_history_pages_chronological(pages_newest_first))
}

async fn replay_history_for_den_session(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    den: &Value,
    lifecycle_method: &str,
) -> Result<()> {
    if let Some(conv) = conversation_id_for_history(den) {
        let messages = fetch_conversation_history_chronological(http, config, &conv).await?;
        eprintln!(
            "bears-acp-adapter: {} session_id={} replaying {} history messages for conversation_id={}",
            lifecycle_method,
            session_id,
            messages.len(),
            conv
        );
        eprintln!(
            "bears-acp-adapter: {} session_id={} history_ids={:?}",
            lifecycle_method,
            session_id,
            messages
                .iter()
                .map(|m| m.id.as_deref().unwrap_or("<none>").to_string())
                .collect::<Vec<_>>()
        );
        for message in messages {
            match message.role.as_str() {
                "user" => send_user_message_chunk(session_id, &message.text).await?,
                "assistant" => send_agent_message_chunk(session_id, &message.text).await?,
                _ => {}
            }
        }
    } else {
        eprintln!(
            "bears-acp-adapter: {} session_id={} has no conv-/default history yet (pending new- thread); skipping replay",
            lifecycle_method,
            session_id
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
) -> Result<&'static str> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session params missing sessionId"))?;
    let den = match den_get_acp_session_for_lifecycle(http, config, session_id).await {
        Ok(den) => Some(den),
        Err(err) if den_session_error_allows_local_fallback(&err) => {
            eprintln!(
                "bears-acp-adapter: session/resume session_id={} could not load Den session ({}); restoring as local pending session",
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
    eprintln!(
        "bears-acp-adapter: session/resume session_id={} cwd={} roots={} direct_tools={} mcp={}",
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
    shared_state
        .session_contexts
        .lock()
        .await
        .insert(session_id.to_string(), context.clone());
    adapter_state
        .session_contexts
        .insert(session_id.to_string(), context);
    send_available_commands_update(session_id).await?;
    spawn_adapter_environment_publish(
        config.clone(),
        session_id.to_string(),
        adapter_state.clone(),
        None,
    );
    if let Some(den) = den.as_ref() {
        replay_history_for_den_session(http, config, session_id, den, "session/resume").await?;
        surface_submitted_plan_fallback(session_id, den).await?;
    }
    Ok(den
        .as_ref()
        .map(infer_mode_from_den_session)
        .unwrap_or(MODE_ASK))
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
                "bears-acp-adapter: session/load session_id={} could not load Den session ({}); loading as local pending session",
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
    eprintln!(
        "bears-acp-adapter: session/load session_id={} cwd={} roots={} direct_tools={} mcp={}",
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
    shared_state
        .session_contexts
        .lock()
        .await
        .insert(session_id.to_string(), context.clone());
    adapter_state
        .session_contexts
        .insert(session_id.to_string(), context);
    send_available_commands_update(session_id).await?;
    spawn_adapter_environment_publish(
        config.clone(),
        session_id.to_string(),
        adapter_state.clone(),
        None,
    );
    if let Some(den) = den.as_ref() {
        replay_history_for_den_session(http, config, session_id, den, "session/load").await?;
        surface_submitted_plan_fallback(session_id, den).await?;
    }

    let mode = den
        .as_ref()
        .map(infer_mode_from_den_session)
        .unwrap_or(MODE_ASK);
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
    shared_state.active_prompts.lock().await.remove(session_id);
    shared_state.tool_tasks.cancel_session(session_id).await;
    let _ = shared_state.cancellation_tx.send(CancellationNotice {
        session_id: session_id.to_string(),
        turn_token: None,
        conversation_id: None,
    });
    post_session_lifecycle_action(http, config, session_id, "close").await
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
    shared_state.active_prompts.lock().await.remove(session_id);
    shared_state.tool_tasks.cancel_session(session_id).await;
    let _ = shared_state.cancellation_tx.send(CancellationNotice {
        session_id: session_id.to_string(),
        turn_token: None,
        conversation_id: None,
    });
    post_session_lifecycle_action(http, config, session_id, "cancel").await
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
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    action: &str,
    payload: Value,
) -> Result<()> {
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/{}",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id),
        action,
    );
    let response = http
        .post(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("post ACP session {action} to Den at {url}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Den session {action} endpoint returned HTTP {status}: {}",
            body.trim()
        ));
    }
    Ok(())
}

async fn post_session_lifecycle_action_json(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    action: &str,
) -> Result<Value> {
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/{}",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id),
        action,
    );
    let response = http
        .post(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .json(&json!({ "adapter_contract": adapter_contract_context() }))
        .send()
        .await
        .with_context(|| format!("post ACP session {action} to Den at {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "Den session {action} endpoint returned HTTP {status}: {}",
            body.trim()
        ));
    }
    Ok(serde_json::from_str(&body).unwrap_or_else(|_| json!({ "raw": body })))
}

async fn compact_session_conversation(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
) -> Result<Value> {
    post_session_lifecycle_action_json(http, config, session_id, "compact").await
}

const STALE_APPROVAL_RECOVERY_RETRY_MESSAGE: &str =
    "BEARS ran stale-approval recovery and is retrying your prompt.";

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
        "Stale approval recovery was not needed.".to_string()
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
    adapter_state: &AdapterState,
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
    let display_prompt = prompt_display_text_from_params(&params).unwrap_or(prompt);
    send_user_message_chunk(session_id, &display_prompt).await?;
    let report = handle_local_slash_command(
        http,
        config,
        adapter_state,
        shared_state,
        session_id,
        command,
    )
    .await;
    send_agent_message_chunk(session_id, &report).await?;
    write_prompt_end_turn_response(response_id).await
}

async fn handle_prompt(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response_id: Value,
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
            response_id,
            params,
            turn_token,
            allow_recovery_retry: true,
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
    response_id: Value,
    params: Value,
    turn_token: Uuid,
    allow_recovery_retry: bool,
}

async fn handle_prompt_with_retry(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    retry: PromptRetryContext,
) -> Result<()> {
    let PromptRetryContext {
        response_id,
        params,
        turn_token,
        allow_recovery_retry,
    } = retry;
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/prompt params missing sessionId"))?;
    let prompt_shape = prompt_block_shape(&params);
    let prompt_context = prompt_context_from_params(&params)?;
    let prompt = require_human_prompt_text(prompt_context.human_message.clone())?;
    let den_prompt = prompt_den_message_from_context(&prompt_context)?;
    let display_prompt = prompt_display_text_from_params(&params).unwrap_or_else(|| prompt.clone());
    eprintln!(
        "bears-acp-adapter: session/prompt session_id={} prompt_len={} den_prompt_len={} display_prompt_len={} prompt_blocks={{text:{}, resource:{}, resource_link:{}, other:{}}} prompt_provenance={{human_text:{}, human_pasted_debug_text:{}, client_resource:{}, client_synthetic_context:{}, unsupported:{}}} prompt_context={{references:{}, synthetic_omitted:{}, resource_bodies_not_in_human_message:{}}} prompt_has_trusted_mode_suffix={} den_prompt_has_trusted_mode_suffix={} display_has_trusted_mode_suffix={} prompt_has_system_reminder={} den_prompt_has_system_reminder={} display_has_system_reminder={}",
        session_id,
        prompt.len(),
        den_prompt.len(),
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
        prompt_context.diagnostics.resource_bodies_not_in_human_message,
        prompt.contains("Trusted ACP session mode this turn:"),
        den_prompt.contains("Trusted ACP session mode this turn:"),
        display_prompt.contains("Trusted ACP session mode this turn:"),
        prompt.contains("<system-reminder>"),
        den_prompt.contains("<system-reminder>"),
        display_prompt.contains("<system-reminder>"),
    );
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
        write_prompt_end_turn_response(response_id).await?;
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
                "bears-acp-adapter: session/prompt session_id={} had no cached session context; using fallback direct tool context",
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
    eprintln!(
        "bears-acp-adapter: session/prompt session_id={} bear={} conversation_id={} client={} direct_tools={} mcp_servers={} mcp_tool_count={} mcp_tool_names={:?}",
        session_id,
        config.bear,
        conversation_log,
        config.client,
        client_context.raw.get("direct_tools").cloned().unwrap_or(Value::Null),
        client_context.raw.pointer("/mcp/servers").cloned().unwrap_or(Value::Null),
        prompt_mcp_tool_names.len(),
        prompt_mcp_tool_names
    );

    if should_echo_user_message_chunk(&prompt_context) {
        send_user_message_chunk(session_id, &display_prompt).await?;
    }

    let url = format!(
        "{}/acp/bears/{}/sessions/{}/prompt",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id)
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", config.token))?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let requested_mode = client_context
        .current_mode
        .as_deref()
        .map(normalize_mode)
        .unwrap_or(MODE_ASK);
    let mut den_payload = json!({
        "message": den_prompt,
        "client": config.client,
        "client_capabilities": shared_state.client_capabilities.lock().await.clone(),
        "client_context": client_context.raw,
        "requested_mode": requested_mode,
        "adapter_contract": adapter_contract_context(),
    });
    if let Some(conversation_id) = conversation_id.as_deref() {
        den_payload["conversation_id"] = json!(conversation_id);
    }

    let response = http
        .post(&url)
        .headers(headers)
        .json(&den_payload)
        .send()
        .await
        .with_context(|| den_request_context(&url))?;

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_else(|_| "".to_string());
        let message = den_status_error_message(status, text.trim());
        eprintln!(
            "bears-acp-adapter: Den prompt returned non-success status session_id={} status={} message={}",
            session_id, status, message
        );
        send_agent_message_chunk_for_turn(
            shared_state,
            session_id,
            turn_token,
            &format!(
                "BEARS could not complete this turn because Den/Letta returned an error. The ACP session is still alive, so you can use `/compact` or `/collapse` to try recovery.\n\n{message}"
            ),
        )
        .await?;
        write_prompt_end_turn_response(response_id).await?;
        return Ok(());
    }

    let mut stream_diagnostics = SseStreamDiagnostics::default();
    let mut saw_done = false;
    let mut saw_visible_output = false;
    let mut saw_tool_activity = false;
    let mut saw_error = false;
    let mut recover_and_retry = false;
    let mut saw_cancellation_error = false;
    let mut terminal_outcome: Option<String> = None;
    let mut recovery_hint: Option<String> = None;
    let mut terminal_user_message: Option<String> = None;
    let mut upstream_errors = Vec::new();
    let mut buffer = Vec::<u8>::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(chunk) => chunk,
            Err(err)
                if stream_has_successful_terminal_condition(
                    saw_visible_output,
                    saw_error,
                    saw_done,
                    saw_tool_activity,
                ) =>
            {
                eprintln!(
                    "bears-acp-adapter: Den SSE stream ended with recoverable read error after terminal/progress events session_id={} error={err:#}",
                    session_id
                );
                break;
            }
            Err(err) => return Err(err).context("read Den SSE chunk"),
        };
        buffer.extend_from_slice(&chunk);
        while let Some(pos) = buffer.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = buffer.drain(..pos + 2).collect();
            let outcome = handle_sse_frame(
                config,
                adapter_state,
                shared_state,
                session_id,
                &frame,
                &mut stream_diagnostics,
                turn_token,
            )
            .await?;
            saw_done |= outcome.saw_done;
            saw_visible_output |= outcome.saw_visible_output;
            saw_tool_activity |= outcome.saw_tool_activity;
            saw_error |= outcome.saw_error;
            recover_and_retry |= outcome.recover_and_retry;
            saw_cancellation_error |= outcome.saw_cancellation_error;
            if terminal_outcome.is_none() {
                terminal_outcome = outcome.terminal_outcome;
            }
            if recovery_hint.is_none() {
                recovery_hint = outcome.recovery_hint;
            }
            if terminal_user_message.is_none() {
                terminal_user_message = outcome.terminal_user_message;
            }
            upstream_errors.extend(outcome.upstream_errors);
        }
    }
    if !buffer.is_empty() {
        let frame = std::mem::take(&mut buffer);
        let outcome = handle_sse_frame(
            config,
            adapter_state,
            shared_state,
            session_id,
            &frame,
            &mut stream_diagnostics,
            turn_token,
        )
        .await?;
        saw_done |= outcome.saw_done;
        saw_visible_output |= outcome.saw_visible_output;
        saw_tool_activity |= outcome.saw_tool_activity;
        saw_error |= outcome.saw_error;
        recover_and_retry |= outcome.recover_and_retry;
        saw_cancellation_error |= outcome.saw_cancellation_error;
        if terminal_outcome.is_none() {
            terminal_outcome = outcome.terminal_outcome;
        }
        if recovery_hint.is_none() {
            recovery_hint = outcome.recovery_hint;
        }
        if terminal_user_message.is_none() {
            terminal_user_message = outcome.terminal_user_message;
        }
        upstream_errors.extend(outcome.upstream_errors);
    }

    if recover_and_retry && allow_recovery_retry && !saw_visible_output && !saw_tool_activity {
        eprintln!(
            "bears-acp-adapter: asking Den to recover stuck conversation before retry session_id={} errors={}",
            session_id,
            upstream_errors.join("; ")
        );
        match compact_session_conversation(http, config, session_id).await {
            Ok(result) => {
                eprintln!(
                    "bears-acp-adapter: Den stale-approval recovery completed session_id={} result={}",
                    session_id, result
                );
                send_agent_message_chunk_for_turn(
                    shared_state,
                    session_id,
                    turn_token,
                    STALE_APPROVAL_RECOVERY_RETRY_MESSAGE,
                )
                .await?;
                return Box::pin(handle_prompt_with_retry(
                    http,
                    config,
                    adapter_state,
                    shared_state,
                    PromptRetryContext {
                        response_id,
                        params,
                        turn_token,
                        allow_recovery_retry: false,
                    },
                ))
                .await;
            }
            Err(err) => {
                eprintln!(
                    "bears-acp-adapter: Den stale-approval recovery failed session_id={} error={err:#}",
                    session_id
                );
                send_agent_message_chunk_for_turn(
                    shared_state,
                    session_id,
                    turn_token,
                    "BEARS detected stale Letta approval state, but recovery failed. This ACP session's conversation may still be wedged; please start a new ACP session.",
                )
                .await?;
                saw_visible_output = true;
                upstream_errors.clear();
            }
        }
    }

    if recover_and_retry && !allow_recovery_retry && !saw_visible_output && !saw_tool_activity {
        send_agent_message_chunk_for_turn(
            shared_state,
            session_id,
            turn_token,
            "BEARS retried after Den stale-approval recovery, but stale approval state persisted. Please start a new ACP session.",
        )
        .await?;
        saw_visible_output = true;
        upstream_errors.clear();
    }

    if !upstream_errors.is_empty() {
        if saw_visible_output {
            eprintln!(
                "bears-acp-adapter: ignoring upstream error after visible output: {}",
                upstream_errors.join("; ")
            );
        } else if saw_cancellation_error || terminal_outcome.as_deref() == Some("cancelled") {
            eprintln!(
                "bears-acp-adapter: suppressing recovery hint for cancellation session_id={} errors={}",
                session_id,
                upstream_errors.join("; ")
            );
            let message = terminal_user_message
                .as_deref()
                .unwrap_or("BEARS request was cancelled.");
            send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, message)
                .await?;
            saw_visible_output = true;
            upstream_errors.clear();
        } else {
            let message = format!(
                "BEARS upstream stream reported error: {}",
                upstream_errors.join("; ")
            );
            let rendered = match recovery_hint.as_deref() {
                Some("compact_and_retry") => format!(
                    "{message}\n\nThe ACP session is still alive, so you can use `/compact` or `/collapse` to try recovery."
                ),
                Some("check_upstream_logs") => terminal_user_message.clone().unwrap_or_else(|| {
                    format!(
                        "{message}\n\nBEARS recommends checking Codepool/Letta logs before retrying."
                    )
                }),
                _ => terminal_user_message.clone().unwrap_or(message.clone()),
            };
            eprintln!(
                "bears-acp-adapter: converting upstream stream error into terminal ACP turn session_id={} message={}",
                session_id, rendered
            );
            send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, &rendered)
                .await?;
            saw_visible_output = true;
            upstream_errors.clear();
        }
    }

    eprintln!(
        "bears-acp-adapter: Den stream summary session_id={} {}",
        session_id,
        stream_diagnostics.summary()
    );
    if !stream_has_successful_terminal_condition(
        saw_visible_output,
        saw_error,
        saw_done,
        saw_tool_activity,
    ) {
        return Err(anyhow!(
            "BEARS ACP stream completed without visible output, tool activity, or an error. Diagnostics: {}",
            stream_diagnostics.summary()
        ));
    }

    write_prompt_end_turn_response(response_id).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalSlashCommand {
    Doctor,
    Compact,
    Conversation,
    Capabilities,
    Runtime,
    Status,
    Version,
    DebugUi,
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
        description: "Ask Den to repair stale ACP/Letta approval state, then compact the conversation if needed.",
        command: LocalSlashCommand::Compact,
        den_required: true,
    },
    LocalSlashCommandDescriptor {
        name: "conversation",
        aliases: &[],
        description: "Show the current ACP session and Letta conversation binding.",
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
        name: "version",
        aliases: &[],
        description: "Show BEARS adapter version/build metadata plus optional Den version.",
        command: LocalSlashCommand::Version,
        den_required: false,
    },
    LocalSlashCommandDescriptor {
        name: "debug-ui",
        aliases: &[],
        description: "Show BEARS ACP debug UI environment status.",
        command: LocalSlashCommand::DebugUi,
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
                        "bears-acp-adapter: manual ACP recovery completed session_id={} result={}",
                        session_id, result
                    );
                    render_compact_recovery_result(&result)
                }
                Err(err) => {
                    eprintln!(
                        "bears-acp-adapter: manual ACP recovery failed session_id={} error={err:#}",
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
        LocalSlashCommand::Version => version_report(http, config).await,
        LocalSlashCommand::DebugUi => debug_ui_report(),
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
        if context.roots.is_empty() { "<none>".to_string() } else { context.roots.join(", ") },
        context.conversation_id.as_deref().unwrap_or("<none>"),
        context.resolved_conversation_id.as_deref().unwrap_or("<none>"),
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
    render_status_report(&environment, &tasks)
}

fn compact_json_for_status(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    truncate_for_log(&text, 600)
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
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        env!("BEARS_ACP_ADAPTER_BUILT_AT_UTC"),
        BEARS_ACP_ADAPTER_CONTRACT_NAME,
        BEARS_ACP_ADAPTER_CONTRACT_VERSION,
        serde_json::to_string_pretty(&adapter).unwrap_or_else(|_| adapter.to_string()),
        serde_json::to_string_pretty(&host_bridge_env)
            .unwrap_or_else(|_| host_bridge_env.to_string()),
        den,
    )
}

fn debug_ui_report() -> String {
    let enabled = env_bool("BEARS_ACP_DEBUG_UI");
    let stream_tokens =
        env::var("BEARS_ACP_STREAM_TOKENS").unwrap_or_else(|_| "<unset>".to_string());
    let chunk_chars =
        env::var("BEARS_ACP_TEXT_CHUNK_CHARS").unwrap_or_else(|_| "<unset>".to_string());
    format!(
        "BEARS ACP debug UI\n\n- BEARS_ACP_DEBUG_UI: {}\n- BEARS_ACP_STREAM_TOKENS: {}\n- BEARS_ACP_TEXT_CHUNK_CHARS: {}",
        if enabled { "enabled" } else { "disabled" },
        stream_tokens,
        chunk_chars,
    )
}

async fn acp_doctor_report(
    http: Option<&reqwest::Client>,
    config: Option<&Config>,
    adapter_state: &AdapterState,
    context: &SessionContext,
) -> String {
    let (api_url, bear, den_status, token_status) =
        if let (Some(http), Some(config)) = (http, config) {
            let den_status = match fetch_server_version_for_diagnostics(http, config).await {
                Ok(version) => version.summary(),
                Err(err) => format!("Den server unreachable: {err:#}"),
            };
            let token_status = match validate_den_code_token_for_diagnostics(http, config).await {
                Ok(()) => "valid for this Bear".to_string(),
                Err(err) => format!("not validated: {err:#}"),
            };
            (
                config.api_url.clone(),
                config.bear.clone(),
                den_status,
                token_status,
            )
        } else {
            (
                "<not configured>".to_string(),
                "<not configured>".to_string(),
                "Den not configured in this adapter process".to_string(),
                "not validated: Den not configured".to_string(),
            )
        };
    format!(
        "BEARS ACP doctor\n\nAdapter:\n- version: {}\n- git_sha: {}\n- built_at_utc: {}\n- contract: {} v{}\n\nDen:\n- api_url: {}\n- bear: {}\n- server: {}\n- token: {}\n\nClient capabilities:\n{}\n\nSession:\n- cwd: {}\n- roots: {}\n- resolved_conversation_id: {}\n\nDirect tools: {}\n\nBrowser tool source:\n{}\n\nHost browser bridge env:\n{}\n\nSession MCP state:\n{}",
        adapter_version(),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        env!("BEARS_ACP_ADAPTER_BUILT_AT_UTC"),
        BEARS_ACP_ADAPTER_CONTRACT_NAME,
        BEARS_ACP_ADAPTER_CONTRACT_VERSION,
        api_url,
        bear,
        den_status,
        token_status,
        serde_json::to_string_pretty(&adapter_state.client_capabilities).unwrap_or_else(|_| adapter_state.client_capabilities.to_string()),
        context.cwd,
        if context.roots.is_empty() { "<none>".to_string() } else { context.roots.join(", ") },
        context.resolved_conversation_id.as_deref().unwrap_or("<none>"),
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
    eprintln!("  build_git_sha: {}", env!("BEARS_ACP_ADAPTER_GIT_SHA"));
    eprintln!("  built_at_utc: {}", env!("BEARS_ACP_ADAPTER_BUILT_AT_UTC"));
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
        eprintln!("• Client label is empty; ACP protocol still works, but set BEARS_ACP_CLIENT if you want labeled requests");
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
    } else {
        eprintln!("• Skipping server reachability check until configuration is fixed");
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
        eprintln!("Doctor found setup problems. Fix the items marked ✗ above, then run `bears-acp-adapter doctor` again.");
        std::process::exit(2);
    }

    eprintln!("Setup looks good.");
    Ok(())
}

fn installed_or_current_command_hint() -> String {
    let installed = Path::new("/usr/local/bin/bears-acp-adapter");
    if installed.exists() {
        return installed.display().to_string();
    }
    env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "bears-acp-adapter".to_string())
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

fn server_version_json(server_version: ServerVersion) -> Value {
    json!({
        "service": server_version.service,
        "version": server_version.version,
        "git_sha": server_version.git_sha,
        "built_at_utc": server_version.built_at_utc,
    })
}

fn den_request_context(url: &str) -> String {
    format!(
        "could not connect to the BEARS Den API at {url}. Check that DEN_API_URL is the Den API origin reachable from this editor process, that the API service is running with ACP_GATEWAY_ENABLED=true, and that the network/VPN/firewall permits the connection"
    )
}

fn den_status_error_message(status: reqwest::StatusCode, body: &str) -> String {
    if let Some(message) = den_compatibility_status_message(body) {
        return message;
    }
    let hint = match status.as_u16() {
        401 => "The bearer token was rejected. Check DEN_TOKEN or --token-env and make sure the token is an active Den Code token.",
        403 => "The token authenticated but is not allowed to use this bear or ACP. Check bear membership and token scopes.",
        404 => "The ACP gateway endpoint was not found. Check DEN_API_URL, BEAR_SLUG, and that Den is running with ACP_GATEWAY_ENABLED=true on the API service.",
        405 => "The server exists but did not accept the ACP prompt method. Check that DEN_API_URL points to the Den API origin, not the web UI origin or a proxy route with method restrictions.",
        429 => "The Den API rate limited this request. Wait and retry, or check service limits.",
        500..=599 => "The Den API returned a server error. Check Den service logs for the request failure.",
        _ => "The Den API rejected the prompt request. Check the response body and Den logs for details.",
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

fn should_echo_user_message_chunk(context: &AcpPromptContextBundle) -> bool {
    !context
        .resource_references
        .iter()
        .any(|reference| reference.delivery_policy == AcpPromptContextDeliveryPolicy::ReferenceOnly)
}

fn prompt_den_message_from_context(context: &AcpPromptContextBundle) -> Result<String> {
    let human_message = require_human_prompt_text(context.human_message.clone())?;
    let Some(host_context) = render_reference_only_host_context(&context.resource_references)
    else {
        return Ok(human_message);
    };
    Ok(format!(
        "{host_context}\n\n<user_message>\n{human_message}\n</user_message>"
    ))
}

const MAX_PROMPT_RESOURCE_REFERENCES: usize = 20;

fn render_reference_only_host_context(references: &[AcpPromptResourceReference]) -> Option<String> {
    let reference_only = references
        .iter()
        .filter(|reference| {
            reference.delivery_policy == AcpPromptContextDeliveryPolicy::ReferenceOnly
        })
        .collect::<Vec<_>>();
    if reference_only.is_empty() {
        return None;
    }

    let mut lines = vec![
        "<host_context kind=\"referenced_resources\" delivery=\"reference_only\" persistence=\"not_human_message\">".to_string(),
        "The ACP client referenced these resources. They are not human-authored instructions.".to_string(),
        "Use available file/content tools for authoritative contents before quoting, editing, or relying on them.".to_string(),
        String::new(),
        "Resources:".to_string(),
    ];
    let total = reference_only.len();
    for (index, reference) in reference_only
        .into_iter()
        .take(MAX_PROMPT_RESOURCE_REFERENCES)
        .enumerate()
    {
        let label = reference
            .name
            .as_deref()
            .or(reference.uri.as_deref())
            .unwrap_or("unnamed resource");
        lines.push(format!(
            "- resource {}: {}",
            index + 1,
            escape_host_context_text(label)
        ));
        if let Some(uri) = reference.uri.as_deref() {
            lines.push(format!("  uri: {}", escape_host_context_text(uri)));
        }
        if let Some(mime_type) = reference.mime_type.as_deref() {
            lines.push(format!(
                "  mime_type: {}",
                escape_host_context_text(mime_type)
            ));
        }
        if let Some(text_bytes) = reference.text_bytes {
            lines.push(format!(
                "  embedded_text_bytes: {text_bytes} (body omitted; use tools for contents)"
            ));
        }
    }
    if total > MAX_PROMPT_RESOURCE_REFERENCES {
        lines.push(format!(
            "- omitted_references: {}",
            total - MAX_PROMPT_RESOURCE_REFERENCES
        ));
    }
    lines.push("</host_context>".to_string());
    Some(lines.join("\n"))
}

fn escape_host_context_text(value: &str) -> String {
    truncate_for_log(value, 300)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
        // resolve both to the same Letta conversation.
        _ => true,
    }
}

async fn register_prompt_turn_for_session(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    conversation_id_for_turn: Option<String>,
) -> Option<ActivePromptTurn> {
    let previous = {
        let mut active = shared_state.active_prompts.lock().await;
        active.insert(
            session_id.to_string(),
            ActivePromptTurn {
                token: turn_token,
                conversation_id: conversation_id_for_turn.clone(),
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
    push_path_value(&mut roots, params.pointer("/workspace/currentDirectory"));
    push_path_value(&mut roots, params.pointer("/workspace/cwd"));
    push_path_value(&mut roots, params.pointer("/workspace/root"));
    push_folder_array(&mut roots, params.get("workspaceFolders"));
    push_folder_array(&mut roots, params.pointer("/workspace/folders"));
    roots.sort();
    roots.dedup();
    roots
}

fn push_folder_array(roots: &mut Vec<String>, value: Option<&Value>) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        push_path_value(roots, item.get("path").or_else(|| item.get("uri")));
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

async fn handle_sse_frame(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    frame: &[u8],
    diagnostics: &mut SseStreamDiagnostics,
    turn_token: Uuid,
) -> Result<SseFrameOutcome> {
    diagnostics.frames += 1;
    let text = String::from_utf8_lossy(frame);
    let mut outcome = SseFrameOutcome::default();
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(data).context("parse Den SSE event JSON")?;
        diagnostics.observe_event(&event);
        let ty = event.get("type").and_then(Value::as_str).unwrap_or("");
        if ty == "assistant_text_delta" || ty == "status_text" {
            let text = event.get("text").and_then(Value::as_str).unwrap_or("");
            outcome.saw_visible_output |= !text.is_empty();
        } else if den_event_type_is_tool_activity(ty) {
            outcome.saw_tool_activity = true;
            diagnostics.saw_tool_activity = true;
            if ty == "permission_request" {
                // Permission requests are visible ACP client activity. Some Den-side
                // permission workflows settle via the permission result endpoint rather
                // than a later streamed terminal event, so a stream containing only a
                // permission request is not an empty/failed turn.
                outcome.saw_visible_output = true;
                diagnostics.saw_visible_output = true;
            }
        } else if ty == "error" {
            outcome.saw_error = true;
            diagnostics.saw_error = true;
            if let Some(terminal) = event.get("terminal") {
                if let Some(value) = terminal.get("outcome") {
                    outcome.terminal_outcome = value.as_str().map(str::to_string);
                }
                if let Some(value) = terminal.get("recovery_hint") {
                    outcome.recovery_hint = value.as_str().map(str::to_string);
                }
                if let Some(value) = terminal.get("user_message") {
                    outcome.terminal_user_message = value.as_str().map(str::to_string);
                }
            }
            let formatted = format_den_event_error(&event);
            if looks_like_waiting_for_approval_error(&formatted)
                || outcome.recovery_hint.as_deref() == Some("compact_and_retry")
            {
                outcome.recover_and_retry = true;
                outcome.upstream_errors.push(
                    "Letta was waiting for stale approval; BEARS will ask Den to deny pending approvals and compact if needed before retrying the prompt."
                        .to_string(),
                );
            } else {
                outcome.saw_cancellation_error = outcome.terminal_outcome.as_deref()
                    == Some("cancelled")
                    || outcome.recovery_hint.as_deref() == Some("none")
                        && event
                            .get("terminal")
                            .and_then(|terminal| terminal.get("outcome"))
                            .and_then(Value::as_str)
                            == Some("cancelled")
                    || looks_like_cancellation_error(&formatted);
                outcome.upstream_errors.push(formatted);
            }
        }
        if outcome.recover_and_retry {
            continue;
        }
        let handled = handle_den_event(
            config,
            adapter_state,
            shared_state,
            session_id,
            &event,
            turn_token,
        )
        .await?;
        if ty == "turn_result" || ty == "turn_complete" || ty == "done" {
            if let Some(value) = event.get("outcome").and_then(Value::as_str) {
                outcome.terminal_outcome = Some(value.to_string());
            }
            if let Some(value) = event.get("recovery_hint").and_then(Value::as_str) {
                outcome.recovery_hint = Some(value.to_string());
            }
            if let Some(value) = event.get("user_message").and_then(Value::as_str) {
                outcome.terminal_user_message = Some(value.to_string());
            }
        }
        outcome.saw_done |= handled;
        diagnostics.saw_turn_complete |= handled;
        diagnostics.saw_visible_output |= outcome.saw_visible_output;
        if !matches!(
            ty,
            "assistant_text_delta"
                | "status_text"
                | "error"
                | "tool_request"
                | "permission_request"
                | "session_info_update"
                | "plan_update"
                | "mode_update"
                | "conversation_resolved"
                | "turn_complete"
                | "turn_result"
                | "done"
        ) {
            diagnostics.observe_unknown(&event);
            eprintln!(
                "bears-acp-adapter: unknown Den ACP event type {:?}; sample={}",
                ty,
                truncate_for_log(&event.to_string(), 240)
            );
        }
    }
    Ok(outcome)
}

fn den_event_type_is_tool_activity(ty: &str) -> bool {
    matches!(ty, "tool_request" | "permission_request")
}

fn looks_like_waiting_for_approval_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("waiting for approval") || message.contains("please approve or deny")
}

fn looks_like_cancellation_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("stop_reason: cancelled")
        || message.contains("stop_reason=cancelled")
        || message.contains("stop_reason: canceled")
        || message.contains("stop_reason=canceled")
        || message.contains("producing assistant output: cancelled")
        || message.contains("producing assistant output: canceled")
        || message == "cancelled"
        || message == "canceled"
        || message.ends_with(": cancelled")
        || message.ends_with(": canceled")
}

fn format_den_event_error(event: &Value) -> String {
    let message = event
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("BEARS upstream error");
    let mut out = match event.get("detail").and_then(Value::as_str) {
        Some(detail) if !detail.trim().is_empty() => format!("{message}: {detail}"),
        _ => message.to_string(),
    };
    if let Some(context) = event.get("context") {
        out.push_str("\nContext: ");
        out.push_str(&format_error_context_for_display(context));
    }
    if let Some(request_id) = event.get("request_id").and_then(Value::as_str) {
        out.push_str("\nDen request_id: ");
        out.push_str(request_id);
    }
    out
}

fn format_error_context_for_display(context: &Value) -> String {
    let Some(object) = context.as_object() else {
        return context.to_string();
    };
    if object.get("preview").and_then(Value::as_str).is_some() {
        let mut compact = serde_json::Map::new();
        for key in [
            "tool_name",
            "available_client_tools",
            "client_tools_count",
            "client_tools_bytes",
        ] {
            if let Some(value) = object.get(key) {
                compact.insert(key.to_string(), value.clone());
            }
        }
        compact.insert(
            "preview".to_string(),
            json!({
                "redacted": true,
                "reason": "assistant text preview omitted from ACP display; see Den request logs for details"
            }),
        );
        return Value::Object(compact).to_string();
    }
    context.to_string()
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

enum ToolTaskWaitOutcome<T> {
    ToolFinished(T),
    Cancelled(CancellationNotice),
}

async fn wait_for_tool_future_or_matching_cancellation<F>(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    conversation_id: Option<&str>,
    tool_future: F,
) -> ToolTaskWaitOutcome<F::Output>
where
    F: std::future::Future,
{
    let mut cancellation_rx = shared_state.cancellation_tx.subscribe();
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
                    Ok(notice) => {
                        eprintln!(
                            "bears-acp-adapter: ignored unrelated cancellation notice while local tool was running session_id={} turn_token={} notice_session_id={} notice_turn_token={:?}",
                            session_id,
                            turn_token,
                            notice.session_id,
                            notice.turn_token,
                        );
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        eprintln!(
                            "bears-acp-adapter: local tool cancellation receiver lagged session_id={} turn_token={} skipped={}",
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

fn spawn_tool_request_task(
    config: Config,
    shared_state: AdapterSharedState,
    session_id: String,
    event: Value,
    turn_token: Uuid,
) {
    tokio::spawn(async move {
        let tool_call_id = event
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let tool_name = event
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        shared_state
            .tool_tasks
            .register(&session_id, &tool_call_id, &tool_name, Some(turn_token))
            .await;
        let mut task_state = AdapterState {
            client_capabilities: shared_state.client_capabilities.lock().await.clone(),
            session_contexts: shared_state.session_contexts.lock().await.clone(),
            transport: shared_state.transport.clone(),
        };
        let tool_future = handle_tool_request_event(
            &config,
            &mut task_state,
            &shared_state.tool_tasks,
            &shared_state.mcp_registry,
            &shared_state.approval_cache,
            &session_id,
            &event,
        );
        let result = match wait_for_tool_future_or_matching_cancellation(
            &shared_state,
            &session_id,
            turn_token,
            None,
            tool_future,
        )
        .await
        {
            ToolTaskWaitOutcome::ToolFinished(result) => result,
            ToolTaskWaitOutcome::Cancelled(_notice) => {
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
                return;
            }
        };
        if let Err(err) = result {
            eprintln!(
                "bears-acp-adapter: local tool task failed session_id={} tool_call_id={} tool_name={} error={err:#}",
                session_id, tool_call_id, tool_name
            );
            let local_err = LocalToolError::error(format!("local tool task failed: {err:#}"));
            let _ = post_local_tool_error_result(
                &config,
                &session_id,
                &tool_call_id,
                &tool_name,
                &event,
                local_err,
                std::time::Instant::now(),
            )
            .await;
        }
        let _ = shared_state
            .tool_tasks
            .remove(&session_id, &tool_call_id)
            .await;
    });
}

async fn handle_tool_request_event(
    config: &Config,
    adapter_state: &mut AdapterState,
    task_registry: &ToolTaskRegistry,
    mcp_registry: &McpRegistry,
    approval_cache: &ApprovalCache,
    session_id: &str,
    event: &Value,
) -> Result<()> {
    let tool_call_id = event
        .get("tool_call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tool_request missing tool_call_id"))?;
    let tool_name = event
        .get("tool_name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tool_request missing tool_name"))?;
    task_registry
        .set_phase(session_id, tool_call_id, tool_name, ToolTaskPhase::Received)
        .await;
    log_tool_task_phase(session_id, tool_call_id, tool_name, ToolTaskPhase::Received);
    let preparing = friendly_tool_status(tool_name, event, "preparing");
    send_tool_call_update(
        session_id,
        tool_call_id,
        tool_name,
        ToolCallUpdatePayload {
            status: "pending",
            text: &preparing,
            event: Some(event),
            raw_output: None,
            extra_content: Vec::new(),
        },
    )
    .await?;
    let args = event.get("args").cloned().unwrap_or_else(|| json!({}));
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
    let target_path_for_approval =
        tool_path(event).and_then(|path| normalize_requested_tool_path(path).ok());
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
    if approval_reused {
        eprintln!(
            "bears-acp-adapter: approval_reused session_id={} tool_name={} path={}",
            session_id,
            tool_name,
            tool_path(event).unwrap_or("<unknown>")
        );
    }
    if !approval_reused
        && event
            .get("approval")
            .and_then(|v| v.get("required"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
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
        send_tool_call_update(
            session_id,
            tool_call_id,
            tool_name,
            ToolCallUpdatePayload {
                status: "pending",
                text: &permission,
                event: Some(event),
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
                    "bears-acp-adapter: approval_remembered session_id={} tool_name={} scope={}",
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
    send_tool_call_update(
        session_id,
        tool_call_id,
        tool_name,
        ToolCallUpdatePayload {
            status: "pending",
            text: &running,
            event: Some(event),
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
            "approval_request_id": event.get("approval_request_id").and_then(Value::as_str),
            "tool_name": tool_name,
            "status": local_err.status_str(),
            "content": local_err.message,
            "structured_content": {},
            "diagnostic": {
                "component": "bears-acp-adapter",
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
                    "bears-acp-adapter: late unexpected server-tool result ignored because Den turn is gone session_id={} tool_call_id={} tool_name={} error={:#}",
                    session_id,
                    tool_call_id,
                    tool_name,
                    err
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
        )
        .await
    } else {
        execute_local_tool(
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
    let mut payload = json!({
        "turn_id": event.get("turn_id").and_then(Value::as_str),
        "request_id": event.get("request_id").and_then(Value::as_str),
        "tool_call_id": tool_call_id,
        "approval_request_id": event.get("approval_request_id").and_then(Value::as_str),
        "tool_name": tool_name,
        "diagnostic": {
            "component": "bears-acp-adapter",
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
            if tool_name == "update_plan" {
                let entries = value
                    .get("plan")
                    .map(plan_entries_from_work_plan_args)
                    .unwrap_or_default();
                send_plan_update(session_id, entries).await?;
            }
            if let Some(mode) = value.get("mode_update").and_then(Value::as_str) {
                if matches!(mode, MODE_ASK | MODE_PLAN | MODE_WRITE) {
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
            send_tool_call_update(
                session_id,
                tool_call_id,
                tool_name,
                ToolCallUpdatePayload {
                    status: "completed",
                    text: &preview,
                    event: Some(event),
                    raw_output: Some(raw_output),
                    extra_content,
                },
            )
            .await?;
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
            payload["status"] = json!(status);
            payload["content"] = json!(local_err.message);
            payload["diagnostic"]["phase"] = json!("adapter_execution_failed");
            merge_diagnostic(&mut payload["diagnostic"], local_err.diagnostic);
            send_tool_call_update(
                session_id,
                tool_call_id,
                tool_name,
                ToolCallUpdatePayload {
                    status: "failed",
                    text: payload["content"].as_str().unwrap_or("Local tool failed"),
                    event: Some(event),
                    raw_output: None,
                    extra_content: Vec::new(),
                },
            )
            .await?;
        }
    }
    if let Err(err) = post_tool_result(config, session_id, tool_call_id, payload).await {
        if is_turn_missing_error(&err) {
            eprintln!(
                "bears-acp-adapter: late local tool result ignored because Den turn is gone session_id={} tool_call_id={} tool_name={} error={:#}",
                session_id,
                tool_call_id,
                tool_name,
                err
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
        let _ = send_tool_call_update(
            session_id,
            tool_call_id,
            tool_name,
            ToolCallUpdatePayload {
                status: "failed",
                text: &message,
                event: Some(event),
                raw_output: Some(json!({
                    "component": "bears-acp-adapter",
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

fn tool_completion_preview(tool_name: &str, value: &Value) -> String {
    if matches!(tool_name, "fs_read_text_file" | "fs.read_text_file") {
        return read_text_file_completion_preview(value);
    }
    if matches!(tool_name, "process_run" | "terminal_run_command") {
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

    if matches!(tool_name, "update_plan" | "request_work_handoff") {
        return String::new();
    }

    let content = value
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let mut text = if content.is_empty() {
        String::new()
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
        "approval_request_id": event.get("approval_request_id").and_then(Value::as_str),
        "tool_name": tool_name,
        "status": local_err.status_str(),
        "content": local_err.message,
        "structured_content": {},
        "diagnostic": {
            "component": "bears-acp-adapter",
            "adapter_version": adapter_version(),
            "phase": "adapter_execution_failed",
            "session_id": session_id,
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "duration_ms": started.elapsed().as_millis(),
        }
    });
    merge_diagnostic(&mut payload["diagnostic"], local_err.diagnostic);
    send_tool_call_update(
        session_id,
        tool_call_id,
        tool_name,
        ToolCallUpdatePayload {
            status: "failed",
            text: payload["content"].as_str().unwrap_or("Local tool failed"),
            event: Some(event),
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
    let path = event
        .get("args")
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
        .or(target_url)
        .or(target_command)
        .unwrap_or("the requested target");
    let display = ToolDisplay::from_event(tool_name, event);
    let title = display.title.clone();
    let reason = event
        .get("approval")
        .and_then(|v| v.get("reason"))
        .and_then(Value::as_str)
        .unwrap_or("Letta requested approval before running this local ACP tool.");
    eprintln!(
        "bears-acp-adapter: requesting permission session_id={} tool_call_id={} tool_name={} path={}",
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
    if let Some(args) = event.get("args") {
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
    let decision = send_permission_request(
        adapter_state,
        request,
        std::time::Duration::from_millis(policy.permission_timeout_ms.unwrap_or(120_000)),
    )
    .await?;
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
    parse_permission_decision(&result)
}

async fn post_permission_result(
    config: &Config,
    session_id: &str,
    permission_id: &str,
    payload: Value,
) -> Result<Value> {
    let payload = with_adapter_contract(payload);
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/permissions/{}",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id),
        urlencoding::encode(permission_id),
    );
    let response = reqwest::Client::new()
        .post(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("post ACP permission decision to Den at {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(den_status_error_message(status, body.trim())));
    }
    Ok(serde_json::from_str(&body).unwrap_or_else(|_| json!({ "raw": body })))
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
    let title = conversation_title
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let payload = with_adapter_contract(json!({
        "environment": environment,
        "conversation_title": title,
    }));
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/adapter-environment",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id),
    );
    let response = reqwest::Client::new()
        .post(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("post ACP adapter environment to Den at {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(den_status_error_message(status, body.trim())));
    }
    Ok(())
}

async fn post_tool_result(
    config: &Config,
    session_id: &str,
    tool_call_id: &str,
    payload: Value,
) -> Result<()> {
    let payload = with_adapter_contract(payload);
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/tool-results/{}",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id),
        urlencoding::encode(tool_call_id),
    );
    let response = reqwest::Client::new()
        .post(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("post ACP tool result to Den at {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(den_status_error_message(status, body.trim())));
    }
    eprintln!(
        "bears-acp-adapter: posted tool result session_id={} tool_call_id={} response={}",
        session_id,
        tool_call_id,
        body.trim()
    );
    Ok(())
}

async fn handle_den_event(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    event: &Value,
    turn_token: Uuid,
) -> Result<bool> {
    match event.get("type").and_then(Value::as_str).unwrap_or("") {
        "assistant_text_delta" => {
            let text = event.get("text").and_then(Value::as_str).unwrap_or("");
            if !text.is_empty() {
                send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, text)
                    .await?;
            }
            Ok(false)
        }
        "status_text" => {
            let text = event.get("text").and_then(Value::as_str).unwrap_or("");
            if classify_thought_status(text) == ThoughtStatusDisposition::KeepThought {
                send_agent_thought_chunk_for_turn(
                    shared_state,
                    session_id,
                    turn_token,
                    normalize_thought_chunk_text(text).as_ref(),
                )
                .await?;
            }
            Ok(false)
        }
        "error" => Ok(false),
        "tool_request" => {
            let has_tool_name = event.get("tool_name").and_then(Value::as_str).is_some();
            let has_tool_call_id = event.get("tool_call_id").and_then(Value::as_str).is_some();
            if has_tool_name && has_tool_call_id {
                spawn_tool_request_task(
                    config.clone(),
                    shared_state.clone(),
                    session_id.to_string(),
                    event.clone(),
                    turn_token,
                );
            } else {
                eprintln!(
                    "bears-acp-adapter: ignoring malformed Den tool_request event session_id={} event={}",
                    session_id,
                    truncate_for_log(&event.to_string(), 400)
                );
            }
            Ok(false)
        }
        "permission_request" => {
            handle_permission_request_event(
                config,
                adapter_state,
                &shared_state.mcp_registry,
                session_id,
                event,
            )
            .await?;
            Ok(false)
        }
        "session_info_update" => {
            let title = event
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            if let Some(context) = adapter_state.session_contexts.get_mut(session_id) {
                context.thread_title = title.clone();
            }
            if let Some(context) = shared_state
                .session_contexts
                .lock()
                .await
                .get_mut(session_id)
            {
                context.thread_title = title.clone();
            }
            let updated_at = event
                .get("updated_at")
                .and_then(Value::as_str)
                .map(str::to_string);
            send_session_info_update(session_id, title, updated_at).await?;
            let runtime = event
                .pointer("/meta/bears/runtime")
                .cloned()
                .or_else(|| event.pointer("/_meta/bears/runtime").cloned())
                .or_else(|| event.get("runtime").cloned());
            let context_budget = event
                .pointer("/meta/bears/context_budget")
                .cloned()
                .or_else(|| event.pointer("/_meta/bears/context_budget").cloned())
                .or_else(|| event.get("context_budget").cloned());
            send_bears_runtime_session_info_update(session_id, runtime, context_budget).await?;
            Ok(false)
        }
        "plan_update" => {
            if let Some(fallback) = event.get("approval_fallback") {
                if let Some(message) = plan_approval_fallback_message(fallback) {
                    send_agent_message_chunk_for_turn(
                        shared_state,
                        session_id,
                        turn_token,
                        &message,
                    )
                    .await?;
                }
            }
            let entries = plan_entries_from_plan_update_event(event);
            if entries.is_empty() {
                if env_bool("BEARS_ACP_DEBUG_UI") {
                    eprintln!(
                        "bears-acp-adapter: received empty plan update for session_id={}; not sending ACP plan UI update",
                        session_id
                    );
                }
            } else if should_send_plan_update(shared_state, session_id, &entries).await? {
                if is_current_prompt_turn(shared_state, session_id, turn_token, "plan_update").await
                {
                    send_plan_update(session_id, entries).await?;
                }
            } else if env_bool("BEARS_ACP_DEBUG_UI") {
                eprintln!(
                    "bears-acp-adapter: skipped unchanged plan update for session_id={}",
                    session_id
                );
            }
            Ok(false)
        }
        "mode_update" => {
            if let Some(mode) = event.get("mode").and_then(Value::as_str) {
                if matches!(mode, MODE_ASK | MODE_PLAN | MODE_WRITE) {
                    notify_mode_state(session_id, mode).await?;
                }
            }
            Ok(false)
        }
        "conversation_resolved" => {
            if let Some(conversation_id) = event
                .get("conversation_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| s.starts_with("conv-"))
            {
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
                            post_adapter_environment(config, session_id, snapshot, Some(title))
                                .await
                        {
                            eprintln!(
                                "bears-acp-adapter: failed to publish adapter environment after conversation_resolved session_id={} error={err:#}",
                                session_id
                            );
                        }
                    }
                }
                eprintln!(
                    "bears-acp-adapter: session_id={} resolved conversation_id={}",
                    session_id, conversation_id
                );
            }
            Ok(false)
        }

        "turn_result" => Ok(true),
        "turn_complete" => Ok(true),
        "done" => Ok(true),
        _ => Ok(false),
    }
}

async fn handle_permission_request_event(
    config: &Config,
    adapter_state: &mut AdapterState,
    mcp_registry: &McpRegistry,
    session_id: &str,
    event: &Value,
) -> Result<()> {
    let permission_id = event
        .get("permission_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("permission_request missing permission_id"))?;
    let tool_call_id = event
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap_or(permission_id);
    let tool_name = event
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("web_fetch");
    let title = event
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Permission request");
    let reason = event
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("BEARS requests permission.");
    let target = event.get("target").cloned().unwrap_or_else(|| json!({}));
    let url = target.get("url").and_then(Value::as_str);
    let host = target.get("host").and_then(Value::as_str);
    let plan_mode_id = target.get("plan_mode_id").and_then(Value::as_str);
    let target_kind = target.get("kind").and_then(Value::as_str);
    let is_plan_mode = target_kind == Some("acp_plan_mode") || plan_mode_id.is_some();
    let mut display = tool_display(tool_name);
    if is_plan_mode {
        display.title = "Approve implementation plan".to_string();
        display.kind = ToolKind::SwitchMode;
        display.verb = "Reviewing plan".to_string();
        display.permission_operation = "approve this implementation plan".to_string();
    }
    let plan_body = target.get("body").and_then(Value::as_str);
    let artifact_path = target.get("artifact_path").and_then(Value::as_str);
    let target_label = if is_plan_mode {
        artifact_path
            .or(plan_mode_id)
            .unwrap_or("submitted plan artifact")
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
    } else {
        format!("{reason}\n\nTool: {tool_name}\nTarget: {target_label}")
    };
    let mut content = vec![ToolCallContent::from(permission_body)];
    let fields = ToolCallUpdateFields::new()
        .kind(Some(display.kind))
        .status(Some(ToolCallStatus::Pending))
        .title(Some(title.to_string()))
        .content(Some(std::mem::take(&mut content)))
        .raw_input(Some(target.clone()));
    let tool_call = ToolCallUpdate::new(tool_call_id.to_string(), fields).meta(Some({
        let mut meta = serde_json::Map::new();
        meta.insert("toolName".to_string(), json!(tool_name));
        meta.insert("permissionId".to_string(), json!(permission_id));
        if let Some(url) = url {
            meta.insert("targetUrl".to_string(), json!(url));
        }
        if let Some(host) = host {
            meta.insert("targetHost".to_string(), json!(host));
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
    } else {
        let mut options = vec![agent_client_protocol::schema::PermissionOption::new(
            "allow_once",
            "Allow this fetch once",
            agent_client_protocol::schema::PermissionOptionKind::AllowOnce,
        )];
        if url.is_some() {
            options.push(agent_client_protocol::schema::PermissionOption::new(
                "allow_url",
                "Always allow this exact URL",
                agent_client_protocol::schema::PermissionOptionKind::AllowAlways,
            ));
        }
        if let Some(host) = host {
            options.push(agent_client_protocol::schema::PermissionOption::new(
                "allow_host",
                format!("Always allow this host: {host}"),
                agent_client_protocol::schema::PermissionOptionKind::AllowAlways,
            ));
        }
        options.push(agent_client_protocol::schema::PermissionOption::new(
            "reject_once",
            "Deny this fetch",
            agent_client_protocol::schema::PermissionOptionKind::RejectOnce,
        ));
        options
    };
    let request = RequestPermissionRequest::new(session_id.to_string(), tool_call, options);
    let decision =
        match send_permission_request(adapter_state, request, std::time::Duration::from_secs(120))
            .await
        {
            Ok(decision) => decision,
            Err(err) => {
                let message = format!("Permission request timed out or failed: {err:#}");
                let _ = post_permission_result(
                    config,
                    session_id,
                    permission_id,
                    json!({ "decision": "timeout", "plan_mode_id": plan_mode_id }),
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
                        event: Some(event),
                        raw_output: Some(json!({
                            "component": "bears-acp-adapter",
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
        };
    let decision_str = if is_plan_mode {
        if decision.approved {
            "approve"
        } else {
            "reject"
        }
    } else {
        match decision.scope {
            ApprovalScope::Host if decision.approved => "allow_host",
            ApprovalScope::Workspace
            | ApprovalScope::Directory
            | ApprovalScope::Command
            | ApprovalScope::Global
                if decision.approved =>
            {
                if decision.remember && url.is_some() {
                    "allow_url"
                } else {
                    "allow_once"
                }
            }
            _ => "reject_once",
        }
    };
    let response = post_permission_result(
        config,
        session_id,
        permission_id,
        json!({ "decision": decision_str, "plan_mode_id": plan_mode_id }),
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
        let tool_call_id = local_tool
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or(tool_call_id);
        let tool_name = local_tool
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("local_web_fetch");
        let result_tool_name = local_tool
            .get("result_tool_name")
            .and_then(Value::as_str)
            .unwrap_or(tool_name);
        let args = local_tool.get("args").cloned().unwrap_or_else(|| json!({}));
        let policy = policy_from_event(local_tool);
        let result = execute_local_tool(
            adapter_state,
            mcp_registry,
            session_id,
            tool_name,
            args,
            &policy,
        )
        .await;
        let started = std::time::Instant::now();
        match result {
            Ok(value) => {
                if tool_name == "update_plan" {
                    let entries = value
                        .get("plan")
                        .map(plan_entries_from_work_plan_args)
                        .unwrap_or_default();
                    send_plan_update(session_id, entries).await?;
                }
                if let Some(mode) = value.get("mode_update").and_then(Value::as_str) {
                    if matches!(mode, MODE_ASK | MODE_PLAN | MODE_WRITE) {
                        notify_mode_state(session_id, mode).await?;
                    }
                }
                let payload = json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": result_tool_name,
                    "status": "ok",
                    "content": "",
                    "structured_content": value,
                    "diagnostic": { "component": "bears-acp-adapter", "phase": "permission_local_tool_completed", "duration_ms": started.elapsed().as_millis() }
                });
                post_tool_result(config, session_id, tool_call_id, payload).await?;
            }
            Err(err) => {
                let payload = json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": result_tool_name,
                    "status": "error",
                    "content": format!("{err:#}"),
                    "structured_content": {},
                    "diagnostic": { "component": "bears-acp-adapter", "phase": "permission_local_tool_failed", "duration_ms": started.elapsed().as_millis() }
                });
                post_tool_result(config, session_id, tool_call_id, payload).await?;
            }
        }
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
        let Some(event_display) = event.get("display") else {
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
        display
    }
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
        "web_fetch" => {
            ToolDisplay::builtin("Fetch URL", ToolKind::Fetch, "Fetching", "fetch this URL")
        }
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
        "bear_environment" => ToolDisplay::builtin(
            "Inspect bear environment",
            ToolKind::Read,
            "Inspecting bear environment for",
            "inspect the bear environment",
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
        _ => ToolDisplay::builtin(
            "Local tool",
            ToolKind::Read,
            "Running",
            "run this local tool",
        ),
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
        "network" => "network access",
        "browser" => "browser use",
        _ => "similar local actions",
    }
}

fn tool_call_title(tool_name: &str, event: &Value) -> String {
    if matches!(tool_name, "process_run" | "terminal_run_command") {
        let command = event
            .get("args")
            .and_then(|args| args.get("command"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let args = event
            .get("args")
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
            } else {
                format!("Run process: {rendered}")
            };
        }
    }
    if matches!(tool_name, "fs_search_files") {
        let query = event
            .get("args")
            .and_then(|args| args.get("query"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("paths");
        return format!("Search files: {}", truncate_title(query));
    }
    if matches!(tool_name, "fs_find_paths") {
        let glob = event
            .get("args")
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
        let source = event
            .get("args")
            .and_then(|a| a.get("source_path"))
            .and_then(Value::as_str)
            .unwrap_or("source");
        let destination = event
            .get("args")
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
    if matches!(tool_name, "chrome_open") {
        if let Some(url) = tool_url(event) {
            return format!("Chrome open: {}", truncate_title(url));
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
    event
        .get("args")
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
    event
        .get("args")
        .and_then(|v| v.get("url"))
        .and_then(Value::as_str)
}

fn tool_command(event: &Value) -> Option<&str> {
    event
        .get("args")
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
    if let Some(line) = event
        .get("args")
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
        "fs_delete_path" => event
            .get("args")
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

fn tool_status_from_str(status: &str) -> ToolCallStatus {
    match status {
        "pending" => ToolCallStatus::Pending,
        "running" | "in_progress" => ToolCallStatus::InProgress,
        "completed" => ToolCallStatus::Completed,
        "failed" | "error" => ToolCallStatus::Failed,
        _ => ToolCallStatus::Pending,
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

struct ToolCallUpdatePayload<'a> {
    status: &'a str,
    text: &'a str,
    event: Option<&'a Value>,
    raw_output: Option<Value>,
    extra_content: Vec<ToolCallContent>,
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
        event,
        raw_output,
        extra_content,
    } = payload;
    let display = event
        .map(|event| ToolDisplay::from_event(tool_name, event))
        .unwrap_or_else(|| tool_display(tool_name));
    let mut content = Vec::new();
    let trimmed_text = text.trim();
    if !trimmed_text.is_empty() && trimmed_text != "Completed." {
        content.push(ToolCallContent::from(trimmed_text.to_string()));
    }
    content.extend(extra_content);
    let title = event
        .map(|event| tool_call_title(tool_name, event))
        .unwrap_or_else(|| display.title.clone());
    let mut tool_call = ToolCall::new(tool_call_id.to_string(), title)
        .kind(display.kind)
        .status(tool_status_from_str(status))
        .content(content);
    if let Some(event) = event {
        if let Some(locations) = tool_locations_from_event(tool_name, event) {
            tool_call = tool_call.locations(locations);
        }
        if let Some(args) = event.get("args") {
            tool_call = tool_call.raw_input(Some(args.clone()));
        }
    }
    if let Some(raw_output) = raw_output {
        tool_call = tool_call.raw_output(Some(raw_output));
    }
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
            "bears-acp-adapter: dropped stale turn update session_id={} turn_token={} update_kind={}",
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

/// Adapter-local mirror of Den's `core::letta::normalize_display_status_text`.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThoughtStatusDisposition {
    Suppress,
    KeepThought,
}

fn classify_thought_status(text: &str) -> ThoughtStatusDisposition {
    let normalized = text.trim();
    if normalized.is_empty() {
        return ThoughtStatusDisposition::Suppress;
    }

    let lower = normalized.to_ascii_lowercase();

    if lower.contains("stale approval")
        || lower.contains("retrying your prompt")
        || lower.contains("recovery failed")
        || lower.contains("please start a new acp session")
        || lower.contains("waiting for approval")
        || lower.contains("please approve or deny")
    {
        return ThoughtStatusDisposition::KeepThought;
    }

    if lower.starts_with("preparing")
        || lower.starts_with("running")
        || lower.starts_with("waiting")
        || lower.starts_with("completed")
        || lower.starts_with("posting")
        || lower.starts_with("posted")
        || lower.starts_with("executing")
        || lower.starts_with("calling tool")
        || lower.starts_with("invoking tool")
        || lower.starts_with("tool ")
        || lower.contains("permission request")
        || lower.contains("tool result")
        || lower.contains("local tool")
    {
        return ThoughtStatusDisposition::Suppress;
    }

    ThoughtStatusDisposition::Suppress
}

async fn write_notification(method: &str, params: Value) -> Result<()> {
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
            "hint": "Check that DEN_API_URL is correct and that the Den API server is online/reachable. This does not necessarily mean your token is invalid.",
        })));
    }
    let mut data = json!({
        "message": format!("BEARS Code token authentication failed: {message}"),
    });
    if let Some(hint) = token_hint {
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
        false
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
                .unwrap_or("Update bears-acp-adapter and restart your ACP client.");
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("bears-acp-adapter-{name}-{nonce}"));
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

    fn test_shared_state() -> AdapterSharedState {
        let (cancellation_tx, _) = broadcast::channel(8);
        AdapterSharedState {
            transport: JsonRpcTransport::default(),
            client_capabilities: Arc::new(TokioMutex::new(Value::Null)),
            session_contexts: Arc::new(TokioMutex::new(HashMap::new())),
            last_plan_update_hashes: Arc::new(TokioMutex::new(HashMap::new())),
            tool_tasks: ToolTaskRegistry::default(),
            mcp_registry: McpRegistry::default(),
            approval_cache: ApprovalCache::default(),
            cancellation_tx,
            active_prompts: Arc::new(TokioMutex::new(HashMap::new())),
        }
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
    fn user_message_chunk_echo_is_suppressed_for_reference_resources() {
        let params = json!({
            "prompt": [
                {"type": "text", "text": "Please read "},
                {"type": "resource", "resource": {
                    "uri": "file:///workspace/README.md",
                    "name": "README.md",
                    "text": "# README"
                }},
                {"type": "text", "text": " and respond simply with ✅"}
            ]
        });
        let bundle = prompt_context_from_params(&params).unwrap();

        assert!(!should_echo_user_message_chunk(&bundle));
        assert_eq!(bundle.resource_references.len(), 1);
        assert_eq!(
            bundle.resource_references[0].delivery_policy,
            AcpPromptContextDeliveryPolicy::ReferenceOnly
        );
    }

    #[test]
    fn user_message_chunk_echo_is_kept_for_diagnostic_only_synthetic_resources() {
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

        assert!(should_echo_user_message_chunk(&bundle));
        assert_eq!(
            bundle.resource_references[0].delivery_policy,
            AcpPromptContextDeliveryPolicy::DiagnosticOnly
        );
    }

    #[test]
    fn prompt_den_message_without_resources_is_plain_human_message() {
        let params = json!({
            "prompt": [{"type": "text", "text": "Please continue."}]
        });
        let bundle = prompt_context_from_params(&params).unwrap();
        let den_prompt = prompt_den_message_from_context(&bundle).unwrap();

        assert_eq!(den_prompt, "Please continue.");
        assert!(!den_prompt.contains("host_context"));
        assert!(!den_prompt.contains("<user_message>"));
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
        let den_prompt = prompt_den_message_from_context(&bundle).unwrap();

        assert!(den_prompt.contains("<host_context kind=\"referenced_resources\""));
        assert!(den_prompt.contains("delivery=\"reference_only\""));
        assert!(den_prompt.contains("file:///tmp/a"));
        assert!(den_prompt.contains("a.txt"));
        assert!(den_prompt.contains("embedded_text_bytes: 13 (body omitted"));
        assert!(den_prompt.contains("<user_message>\nPlease inspect this.\n</user_message>"));
        assert!(!den_prompt.contains("file contents"));
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
        let den_prompt = prompt_den_message_from_context(&bundle).unwrap();

        assert_eq!(den_prompt, "Please continue.");
        assert!(!den_prompt.contains("host_context"));
        assert!(!den_prompt.contains("system_alert"));
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
                conversation_id: Some("conv-1".to_string()),
            },
        );
        shared
            .tool_tasks
            .register(
                "acp-session",
                "call-1",
                "fs_read_text_file",
                Some(turn_token),
            )
            .await;
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let http = reqwest::Client::new();

        handle_session_cancel(
            &http,
            &config,
            &shared,
            json!({ "sessionId": "acp-session" }),
        )
        .await
        .unwrap();

        assert!(shared
            .active_prompts
            .lock()
            .await
            .get("acp-session")
            .is_none());
        let tasks = shared.tool_tasks.list_for_session("acp-session").await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].phase, ToolTaskPhase::Cancelled);
        let notice = cancel_rx.recv().await.expect("cancellation notice");
        assert_eq!(notice.session_id, "acp-session");
        assert_eq!(notice.turn_token, None);
        assert_eq!(notice.conversation_id, None);
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
                conversation_id: Some("conv-1".to_string()),
            },
        );
        shared
            .tool_tasks
            .register(
                "acp-session",
                "call-1",
                "fs_read_text_file",
                Some(turn_token),
            )
            .await;
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let http = reqwest::Client::new();

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
        .unwrap();

        let request_line = request_line.lock().await.clone().unwrap_or_default();
        assert!(
            request_line.starts_with("POST /acp/bears/test-bear/sessions/acp-session/close "),
            "request_line={request_line:?}"
        );
        assert!(shared
            .active_prompts
            .lock()
            .await
            .get("acp-session")
            .is_none());
        let tasks = shared.tool_tasks.list_for_session("acp-session").await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].phase, ToolTaskPhase::Cancelled);
        let notice = cancel_rx.recv().await.expect("close cancellation notice");
        assert_eq!(notice.session_id, "acp-session");
        assert_eq!(notice.turn_token, None);
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
                conversation_id: Some("conv-1".to_string()),
            },
        );
        shared
            .tool_tasks
            .register(
                "acp-session",
                "call-1",
                "fs_read_text_file",
                Some(turn_token),
            )
            .await;
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
        let tasks = shared.tool_tasks.list_for_session("acp-session").await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].phase, ToolTaskPhase::Cancelled);
        let notice = cancel_rx.recv().await.expect("cancellation notice");
        assert_eq!(notice.session_id, "acp-session");
        assert_eq!(notice.turn_token, None);
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
        )
        .await;
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let previous = register_prompt_turn_for_session(
            &shared,
            "acp-session",
            next_token,
            Some("conv-1".to_string()),
        )
        .await
        .expect("previous turn returned");
        let notice = cancel_rx.recv().await.expect("cancellation notice");

        assert_eq!(previous.token, previous_token);
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
        )
        .await;
        let mut cancel_rx = shared.cancellation_tx.subscribe();
        let previous = register_prompt_turn_for_session(
            &shared,
            "acp-session",
            next_token,
            Some("conv-2".to_string()),
        )
        .await
        .expect("previous turn returned");

        assert_eq!(previous.token, previous_token);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), cancel_rx.recv())
                .await
                .is_err(),
            "different-conversation overlap should not send cancellation"
        );
    }

    #[tokio::test]
    async fn adapter_tool_wait_ignores_unrelated_cancellation_notice() {
        let shared = test_shared_state();
        let turn_token = Uuid::new_v4();
        let sender = shared.cancellation_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let _ = sender.send(CancellationNotice {
                session_id: "other-session".to_string(),
                turn_token: None,
                conversation_id: None,
            });
        });

        let outcome = wait_for_tool_future_or_matching_cancellation(
            &shared,
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
    async fn adapter_tool_wait_stops_on_matching_cancellation_notice() {
        let shared = test_shared_state();
        let turn_token = Uuid::new_v4();
        let sender = shared.cancellation_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let _ = sender.send(CancellationNotice {
                session_id: "acp-session".to_string(),
                turn_token: Some(turn_token),
                conversation_id: None,
            });
        });

        let outcome = wait_for_tool_future_or_matching_cancellation(
            &shared,
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

    #[test]
    fn waiting_for_approval_detection_is_case_insensitive() {
        assert!(looks_like_waiting_for_approval_error(
            "Letta stopped before producing assistant output: error; upstream is Waiting For Approval"
        ));
        assert!(looks_like_waiting_for_approval_error(
            "Please Approve Or Deny this stale request"
        ));
    }

    #[test]
    fn cancellation_detection_matches_cancelled_and_canceled_errors() {
        assert!(looks_like_cancellation_error(
            "Letta stopped before producing assistant output: cancelled"
        ));
        assert!(looks_like_cancellation_error(
            "Letta stopped before producing assistant output: canceled"
        ));
        assert!(looks_like_cancellation_error("cancelled"));
        assert!(!looks_like_cancellation_error(
            "Letta stopped before producing assistant output: max_steps"
        ));
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
        assert_eq!(response["source"], "adapter.den_session_mode_not_found");
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
                "attempted": true,
                "denied_count": 1,
                "denied_tool_call_ids": ["tool-call-secret"],
                "denied_source_message_ids": ["message-secret"],
            },
            "compact_result": {
                "debug": "raw upstream compaction response",
            },
        });

        let rendered = render_compact_recovery_result(&result);
        assert!(rendered.contains("Closed 1 stale approval request."));
        assert!(rendered.contains("The conversation was compacted."));
        assert!(!rendered.contains("approval_recovery"));
        assert!(!rendered.contains("compact_result"));
        assert!(!rendered.contains("tool-call-secret"));
        assert!(!rendered.contains("message-secret"));
        assert!(!rendered.contains("raw upstream compaction response"));

        assert!(!STALE_APPROVAL_RECOVERY_RETRY_MESSAGE.contains("recovery_result"));
        assert!(!STALE_APPROVAL_RECOVERY_RETRY_MESSAGE.contains('{'));
        assert!(!STALE_APPROVAL_RECOVERY_RETRY_MESSAGE.contains('}'));
    }

    #[tokio::test]
    async fn stale_approval_event_requests_recovery_retry() {
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
            token: "token".to_string(),
            client: "test".to_string(),
        };
        let root = unique_test_dir("stale-approval");
        let mut adapter_state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();
        let mut diagnostics = SseStreamDiagnostics::default();
        let frame = br#"data: {"type":"error","message":"Letta stopped before producing assistant output: error","detail":"conversation is waiting for approval"}

"#;

        let outcome = handle_sse_frame(
            &config,
            &mut adapter_state,
            &shared_state,
            "session-1",
            frame,
            &mut diagnostics,
            Uuid::new_v4(),
        )
        .await
        .unwrap();

        assert!(outcome.saw_error);
        assert!(!outcome.saw_visible_output);
        assert!(outcome.recover_and_retry);
        assert_eq!(outcome.recovery_hint.as_deref(), None);
        assert_eq!(outcome.upstream_errors.len(), 1);
        assert!(outcome.upstream_errors[0].contains("deny pending approvals"));
    }

    #[tokio::test]
    async fn typed_terminal_metadata_is_captured_from_error_and_done_events() {
        let config = Config {
            api_url: "http://example.invalid".to_string(),
            bear: "test-bear".to_string(),
            token: "token".to_string(),
            client: "test".to_string(),
        };
        let root = unique_test_dir("typed-terminal");
        let mut adapter_state = test_adapter_state("session-1", &root);
        let shared_state = test_shared_state();
        let mut diagnostics = SseStreamDiagnostics::default();
        let frame = br#"data: {"type":"error","message":"No response from the assistant.","detail":"empty stream","terminal":{"outcome":"empty_fallback","recovery_hint":"check_upstream_logs","user_message":"Check Codepool/Letta logs and retry if appropriate."}}

data: {"type":"done","outcome":"empty_fallback","recovery_hint":"check_upstream_logs","user_message":"Check Codepool/Letta logs and retry if appropriate."}

"#;

        let outcome = handle_sse_frame(
            &config,
            &mut adapter_state,
            &shared_state,
            "session-1",
            frame,
            &mut diagnostics,
            Uuid::new_v4(),
        )
        .await
        .unwrap();

        assert!(outcome.saw_error);
        assert!(outcome.saw_done);
        assert_eq!(outcome.terminal_outcome.as_deref(), Some("empty_fallback"));
        assert_eq!(
            outcome.recovery_hint.as_deref(),
            Some("check_upstream_logs")
        );
        assert_eq!(
            outcome.terminal_user_message.as_deref(),
            Some("Check Codepool/Letta logs and retry if appropriate.")
        );
    }

    #[test]
    fn classify_thought_status_suppresses_routine_tool_progress() {
        assert_eq!(
            classify_thought_status("Preparing tool execution"),
            ThoughtStatusDisposition::Suppress
        );
        assert_eq!(
            classify_thought_status("Running ripgrep in workspace"),
            ThoughtStatusDisposition::Suppress
        );
        assert_eq!(
            classify_thought_status("Posted tool result to Den"),
            ThoughtStatusDisposition::Suppress
        );
    }

    #[test]
    fn classify_thought_status_keeps_recovery_notices() {
        assert_eq!(
            classify_thought_status("Please approve or deny the pending request"),
            ThoughtStatusDisposition::KeepThought
        );
        assert_eq!(
            classify_thought_status("Waiting for approval before continuing"),
            ThoughtStatusDisposition::KeepThought
        );
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
            })
            .collect::<Vec<_>>();
        assert_eq!(page[0].id.as_deref(), Some("msg-1"));
        assert_eq!(page[1].id.as_deref(), Some("msg-2"));
    }

    #[test]
    fn history_pages_flatten_oldest_to_newest_across_desc_pagination() {
        let pages = vec![
            vec![
                ReloadHistoryMessage {
                    id: Some("m3".to_string()),
                    role: "user".to_string(),
                    text: "ask 2".to_string(),
                },
                ReloadHistoryMessage {
                    id: Some("m4".to_string()),
                    role: "assistant".to_string(),
                    text: "reply 2".to_string(),
                },
            ],
            vec![
                ReloadHistoryMessage {
                    id: Some("m1".to_string()),
                    role: "user".to_string(),
                    text: "ask 1".to_string(),
                },
                ReloadHistoryMessage {
                    id: Some("m2".to_string()),
                    role: "assistant".to_string(),
                    text: "reply 1".to_string(),
                },
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
    fn permission_request_counts_as_tool_activity() {
        assert!(den_event_type_is_tool_activity("permission_request"));
        assert!(den_event_type_is_tool_activity("tool_request"));
        assert!(!den_event_type_is_tool_activity("conversation_resolved"));
    }

    #[test]
    fn permission_request_activity_is_successful_without_stream_terminal() {
        assert!(stream_has_successful_terminal_condition(
            true, false, false, true
        ));
    }

    #[test]
    fn stream_terminal_condition_allows_visible_output_error_or_tool_completion() {
        assert!(stream_has_successful_terminal_condition(
            true, false, false, false
        ));
        assert!(stream_has_successful_terminal_condition(
            false, true, false, false
        ));
        assert!(stream_has_successful_terminal_condition(
            false, false, true, true
        ));
        assert!(!stream_has_successful_terminal_condition(
            false, false, true, false
        ));
        assert!(!stream_has_successful_terminal_condition(
            false, false, false, true
        ));
    }

    #[test]
    fn command_tool_titles_include_command_details() {
        let event = json!({ "args": { "command": "cargo", "args": ["test", "--manifest-path", "tools/bears-acp-adapter/Cargo.toml"] } });
        assert_eq!(
            tool_call_title("terminal_run_command", &event),
            "Run terminal command: cargo test --manifest-path tools/bears-acp-adapter/Cargo.toml"
        );
        assert_eq!(
            tool_call_title("process_run", &event),
            "Run process: cargo test --manifest-path tools/bears-acp-adapter/Cargo.toml"
        );
    }

    #[test]
    fn tool_display_uses_specific_titles() {
        assert_eq!(tool_display("fs_read_text_file").title, "Read file");
        assert_eq!(tool_display("fs_list_directory").title, "List directory");
        assert_eq!(tool_display("fs_search_files").title, "Search files");
        assert_eq!(tool_display("fs_edit_file").title, "Edit file");
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
            "cwd": "/workspace/tools/bears-acp-adapter",
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
    fn update_plan_completion_preview_is_suppressed() {
        let value = json!({ "content": "Local tool update_plan completed." });
        assert_eq!(tool_completion_preview("update_plan", &value), "");
    }

    #[test]
    fn generic_empty_completion_preview_is_suppressed() {
        let value = json!({ "content": "" });
        assert_eq!(tool_completion_preview("fs_stat", &value), "");
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
        assert!(format!("{:#}", invalid.unwrap_err()).contains("did not contain"));
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
                "max_output_bytes": 10
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
                "max_output_bytes": 5
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
    async fn process_run_reports_nonzero_timeout_and_rejects_unsafe_inputs() {
        let root = unique_test_dir("process-run-policy");
        let outside = unique_test_dir("process-run-outside");
        let state = test_adapter_state("session-1", &root);
        let context = session_context(&state, "session-1").unwrap();
        let nonzero = handle_process_run(
            context,
            "session-1",
            &json!({ "command": "sh", "args": ["-c", "exit 7"], "cwd": root.to_string_lossy() }),
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
            &json!({ "command": "sleep", "args": ["1"], "cwd": root.to_string_lossy(), "timeout_ms": 1 }),
            &ToolPolicy { max_bytes: Some(1024), total_timeout_ms: Some(100), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(timed_out["timed_out"], true);

        let outside_cwd = handle_process_run(
            context,
            "session-1",
            &json!({ "command": "printf", "args": ["x"], "cwd": outside.to_string_lossy() }),
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

    #[ignore = "canonical web_fetch is Den-executed; adapter local fetch will be renamed if reintroduced"]
    #[tokio::test]
    async fn web_fetch_fetches_and_truncates_http_response() {
        std::env::set_var("BEARS_ACP_ALLOW_LOCAL_WEB_FETCH_FOR_TESTS", "1");
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
                    body.len(), body
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
        std::env::remove_var("BEARS_ACP_ALLOW_LOCAL_WEB_FETCH_FOR_TESTS");
    }

    #[ignore = "canonical web_fetch is Den-executed; adapter local fetch will be renamed if reintroduced"]
    #[tokio::test]
    async fn web_fetch_rejects_unsafe_urls() {
        std::env::remove_var("BEARS_ACP_ALLOW_LOCAL_WEB_FETCH_FOR_TESTS");
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
                "sensitive_path_policy": "deny_sensitive_paths"
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
        assert_eq!(
            policy.sensitive_path_policy.as_deref(),
            Some("deny_sensitive_paths")
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
    fn conversation_id_for_history_prefers_resolved_conv() {
        let v = json!({
            "conversation_id": "new-acp-zed-x",
            "resolved_conversation_id": "conv-abc"
        });
        assert_eq!(conversation_id_for_history(&v).as_deref(), Some("conv-abc"));
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
            "bears-acp-adapter-chrome-override-{}",
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

        let value = handle_bear_environment(
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
