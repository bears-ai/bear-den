mod approvals;
mod json_rpc;
mod paths;
mod tool_tasks;
mod tools;

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
use anyhow::{anyhow, Context, Result};

use approvals::{
    approval_url_host_scope, parse_permission_decision, permission_class_for_tool,
    permission_options_for_context, ApprovalCache, ApprovalScope, PermissionDecision,
};
use futures_util::StreamExt;
use json_rpc::{id_key, write_json, JsonRpcTransport};
use paths::{file_uri_or_path_to_path, is_absolute_local_path, normalize_requested_tool_path};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Url;
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
};
use tool_tasks::{log_tool_task_phase, ToolTaskPhase, ToolTaskRegistry};
use tools::chrome::{
    handle_chrome_console_messages, handle_chrome_network_requests, handle_chrome_open,
    handle_chrome_screenshot, handle_chrome_snapshot,
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
use tools::process::handle_process_run;
use tools::terminal::handle_terminal_run_command;
use tools::web::handle_local_web_fetch;

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
    api_url: String,
    bear: String,
    token_env: String,
    client: String,
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
    approval_cache: ApprovalCache,
    cancellation_tx: broadcast::Sender<String>,
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
    conversation_id: Option<String>,
    resolved_conversation_id: Option<String>,
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
    vec![SessionConfigOption::select(
        "mode",
        "Session Mode",
        mode.to_string(),
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

fn session_modes_for_mode(mode: &str) -> SessionModeState {
    SessionModeState::new(
        mode.to_string(),
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
    let commands = vec![
        AvailableCommand::new(
            "doctor",
            "Show BEARS ACP adapter, session, client, and Den configuration diagnostics.",
        ),
        AvailableCommand::new(
            "compact",
            "Ask Den to repair stale ACP/Letta approval state, then compact the conversation if needed.",
        ),
        AvailableCommand::new(
            "collapse",
            "Alias for /compact: repair stale approval state and compact if needed.",
        ),
        AvailableCommand::new(
            "conversation",
            "Show the current ACP session and Letta conversation binding.",
        ),
        AvailableCommand::new(
            "capabilities",
            "Show ACP client capabilities and adapter-local direct tools.",
        ),
        AvailableCommand::new(
            "runtime",
            "Show Den ACP runtime state and active adapter-local tool tasks for this session.",
        ),
        AvailableCommand::new("version", "Show BEARS adapter and Den version information."),
        AvailableCommand::new("debug-ui", "Show BEARS ACP debug UI environment status."),
    ];
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
        send_agent_thought_chunk(
            session_id,
            &format!(
                "BEARS debug: sent ACP plan update with {entry_count} entr{}; if your client supports ACP Agent Plan UI, you should now see a planning/task list.",
                if entry_count == 1 { "y" } else { "ies" }
            ),
        )
        .await?;
    }
    Ok(())
}

async fn notify_mode_state(session_id: &str, mode: &str) -> Result<()> {
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
                CurrentModeUpdate::new(mode.to_string())
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

#[derive(Default)]
struct SseFrameOutcome {
    saw_done: bool,
    saw_visible_output: bool,
    saw_tool_activity: bool,
    saw_error: bool,
    recover_and_retry: bool,
    upstream_errors: Vec<String>,
}

fn stream_has_successful_terminal_condition(
    saw_visible_output: bool,
    saw_error: bool,
    saw_done: bool,
    saw_tool_activity: bool,
) -> bool {
    saw_visible_output || saw_error || (saw_done && saw_tool_activity)
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
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        env!("BEARS_ACP_ADAPTER_BUILT_AT_UTC"),
        local_head_sha(),
        direct_tools_context()
    );
    if !runtime.doctor {
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
        approval_cache,
        cancellation_tx,
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

impl RuntimeConfig {
    fn from_env_and_args() -> Result<Self> {
        let mut api_url = env::var("BEARS_DEN_API_URL").unwrap_or_default();
        let mut bear = env::var("BEARS_BEAR_SLUG").unwrap_or_default();
        let mut token = env::var("BEARS_DEN_TOKEN").unwrap_or_default();
        let mut token_env = env::var("BEARS_DEN_TOKEN_ENV").unwrap_or_default();
        let mut client = env::var("BEARS_ACP_CLIENT").unwrap_or_else(|_| "zed".to_string());
        let mut check_config = false;
        let mut check_server = false;
        let mut doctor = false;

        let mut args = env::args().skip(1);
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
                    "BEARS_DEN_TOKEN_ENV points at {token_env:?}, but that environment variable is not set. Export {token_env} or change --token-env."
                )),
            }
        }

        api_url = api_url.trim().trim_end_matches('/').to_string();
        bear = bear.trim().to_string();
        token = token.trim().to_string();
        client = normalize_client(&client);

        validate_api_url(&api_url, &mut diagnostics);
        if bear.is_empty() {
            diagnostics
                .push("Missing bear slug. Set BEARS_BEAR_SLUG or pass --bear <slug>.".to_string());
        }
        if token.is_empty() {
            diagnostics.push(
                "Missing Den bearer token. Set BEARS_DEN_TOKEN, set BEARS_DEN_TOKEN_ENV to the name of an environment variable containing the token, pass --token <token>, or pass --token-env <env-var>. Den ACP tokens include the acp:chat scope."
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
            api_url,
            bear,
            token_env,
            client,
        };
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
            "\n\nExample:\n  BEARS_DEN_API_URL=https://api.bears.example\n  BEARS_BEAR_SLUG=my-bear\n  BEARS_DEN_TOKEN=...\n\nFor Zed, put those values in the custom agent server env block, or run with --token-env BEARS_DEN_TOKEN so the token can stay outside editor settings.",
        );
        message
    }
}

fn validate_api_url(api_url: &str, diagnostics: &mut Vec<String>) {
    if api_url.is_empty() {
        diagnostics.push(
            "Missing Den API URL. Set BEARS_DEN_API_URL or pass --api-url <url>. Use the API origin reachable from your editor process, for example https://api.bears.example."
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
            "BEARS_DEN_API_URL should be the Den API origin only, not the full ACP prompt endpoint. Use a value like https://api.bears.example, not a URL containing /acp/bears/..."
                .to_string(),
        );
    }
}

fn require_arg_value(flag: &str, value: Option<String>) -> Result<String> {
    value.ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn print_version_to_stderr() {
    eprintln!(
        "bears-acp-adapter {}\nBuild git SHA: {}\nLocal HEAD SHA: {}\nACP sessions: list/resume/load; conversations bound via Den\nDirect tools: {}",
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        local_head_sha(),
        direct_tools_context()
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

fn print_help_to_stderr() {
    eprintln!(
        "bears-acp-adapter {}\nBuild git SHA: {}\nLocal HEAD SHA: {}\nACP sessions: list/resume/load; conversations bound via Den\n\n\
Usage: bears-acp-adapter --api-url <url> --bear <slug> [--client zed] [--token-env BEARS_DEN_TOKEN]\n       bears-acp-adapter doctor\n\n\
Options:\n  --api-url <url>        Den API origin, for example https://api.bears.example\n  --bear <slug>          Bear slug to chat with\n  --token <token>        Den ACP token with acp:chat scope\n  --token-env <env-var>  Read the Den bearer token from this environment variable\n  --client <name>        Client label: zed, opencode, or acp_adapter\n  --check-config         Validate configuration and exit without starting ACP stdio\n  --check-server         Fetch Den /version and exit without starting ACP stdio\n  doctor, --doctor       Run user-friendly setup checks and exit\n  --version              Show version/build behavior and exit\n  --help                 Show this help\n\n\
Environment fallbacks:\n  BEARS_DEN_API_URL\n  BEARS_BEAR_SLUG\n  BEARS_DEN_TOKEN\n  BEARS_DEN_TOKEN_ENV\n  BEARS_ACP_CLIENT\n\n\
BEARS_DEN_API_URL should be the API origin only, not the full /acp/bears/... endpoint.",
        env!("CARGO_PKG_VERSION"),
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
                eprintln!(
                    "bears-acp-adapter: session/new session_id={} cwd={} roots={} direct_tools={}",
                    session_id,
                    context.cwd,
                    context.roots.join(","),
                    context
                        .raw
                        .get("direct_tools")
                        .cloned()
                        .unwrap_or(Value::Null)
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
                let mode = MODE_ASK;
                notify_mode_state(&session_id, mode).await?;
                let response = NewSessionResponse::new(session_id)
                    .config_options(session_config_options_for_mode(mode))
                    .modes(session_modes_for_mode(mode));
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
                    send_agent_thought_chunk(
                        session_id,
                        &format!(
                            "BEARS mode request `{requested_mode}` resolved to `{mode}`. {}",
                            den_response
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Den session policy adjusted the requested mode.")
                        ),
                    )
                    .await?;
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
                    send_agent_thought_chunk(
                        session_id,
                        &format!(
                            "BEARS mode request `{requested_mode}` resolved to `{mode}`. {}",
                            den_response
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Den session policy adjusted the requested mode.")
                        ),
                    )
                    .await?;
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
                if let Err(err) = validate_den_code_token(http, config).await {
                    write_response(id, Err(auth_check_json_rpc_error(&err, None))).await?;
                    return Ok(());
                }
                match restore_session_from_den(
                    http,
                    config,
                    adapter_state,
                    &shared_state,
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
                if let Err(err) = validate_den_code_token(http, config).await {
                    write_response(id, Err(auth_check_json_rpc_error(&err, None))).await?;
                    return Ok(());
                }
                match handle_session_load(
                    http,
                    config,
                    adapter_state,
                    &shared_state,
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
            if let Some(id) = request.id {
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
                match handle_session_close(http, config, &shared_state, request.params).await {
                    Ok(()) => {
                        write_response(id, Ok(serde_json::to_value(CloseSessionResponse::new())?))
                            .await?
                    }
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32003,
                                "BEARS session close failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
                    }
                }
            }
        }
        "session/cancel" => {
            if let Some(id) = request.id {
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
                match handle_session_cancel(http, config, &shared_state, request.params).await {
                    Ok(()) => {
                        write_response(id, Ok(serde_json::to_value(CloseSessionResponse::new())?))
                            .await?
                    }
                    Err(err) => {
                        write_response(
                            id,
                            Err(json_rpc_error(
                                -32003,
                                "BEARS session cancel failed",
                                Some(json!({ "message": format!("{err:#}") })),
                            )),
                        )
                        .await?;
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
    json!({
        "name": "bears-acp-adapter",
        "version": env!("CARGO_PKG_VERSION"),
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
            "chrome_open": { "supported": true, "version": 1 },
            "chrome_snapshot": { "supported": true, "version": 1 },
            "chrome_console_messages": { "supported": true, "version": 1 },
            "chrome_network_requests": { "supported": true, "version": 1 },
            "chrome_screenshot": { "supported": true, "version": 1 },
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
        "chrome_open": true,
        "chrome_snapshot": true,
        "chrome_console_messages": true,
        "chrome_network_requests": true,
        "chrome_screenshot": true,
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
    context.raw["adapter_version"] = json!(env!("CARGO_PKG_VERSION"));
    context.raw["adapter"] = adapter_capabilities_context();
    context.raw["direct_tools"] = direct_tools_context();
    if !context.cwd.trim().is_empty() {
        context.raw["cwd"] = json!(context.cwd.clone());
    }
    if !context.roots.is_empty() {
        context.raw["workspace_roots"] = json!(context.roots.clone());
    }
}

fn session_context_from_params(params: &Value) -> Result<SessionContext> {
    validate_mcp_servers_unsupported(params)?;
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
        "adapter_version": env!("CARGO_PKG_VERSION"),
        "adapter": adapter_capabilities_context(),
        "direct_tools": direct_tools_context(),
    });
    let mut context = SessionContext {
        cwd,
        roots,
        raw,
        conversation_id: None,
        resolved_conversation_id: None,
    };
    ensure_session_context_capabilities(&mut context);
    Ok(context)
}

fn validate_mcp_servers_unsupported(params: &Value) -> Result<()> {
    let Some(mcp_servers) = params
        .get("mcpServers")
        .or_else(|| params.get("mcp_servers"))
    else {
        return Ok(());
    };
    let non_empty = match mcp_servers {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
        _ => true,
    };
    if non_empty {
        return Err(anyhow!(
            "ACP-provided mcpServers are not supported by the BEARS local adapter yet; configure BEARS/Den tools instead"
        ));
    }
    Ok(())
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
        .unwrap_or("bears_den_token");
    if method_id != "bears_den_token" {
        return Err(anyhow!("unsupported BEARS auth method: {method_id}"));
    }
    let config = runtime_config_from_current_env(runtime)?;
    validate_den_code_token(http, &config).await?;
    runtime.config = Some(config);
    runtime.diagnostics.clear();
    Ok(())
}

fn runtime_config_from_current_env(runtime: &RuntimeConfig) -> Result<Config> {
    let mut token = env::var("BEARS_DEN_TOKEN").unwrap_or_default();
    let token_env = runtime.token_env.trim();
    if !token_env.is_empty() {
        token = env::var(token_env).with_context(|| {
            format!("BEARS_DEN_TOKEN_ENV points at {token_env:?}, but that environment variable is not set")
        })?;
    }
    let api_url = runtime.api_url.trim().trim_end_matches('/').to_string();
    let bear = runtime.bear.trim().to_string();
    let token = token.trim().to_string();
    if api_url.is_empty() {
        return Err(anyhow!(
            "Missing BEARS_DEN_API_URL / --api-url for BEARS authentication"
        ));
    }
    if bear.is_empty() {
        return Err(anyhow!(
            "Missing BEARS_BEAR_SLUG / --bear for BEARS authentication"
        ));
    }
    if token.is_empty() {
        return Err(anyhow!("Missing BEARS_DEN_TOKEN. Paste a Den Code token when prompted, or configure BEARS_DEN_TOKEN in Zed."));
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
    env::var("BEARS_DEN_TOKEN_ENV")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "BEARS_DEN_TOKEN".to_string())
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
    let info =
        Implementation::new("bears", env!("CARGO_PKG_VERSION")).title(Some("BEARS".to_string()));
    let auth_methods = if runtime.should_advertise_auth_method() {
        vec![AuthMethod::EnvVar(
            AuthMethodEnvVar::new(
                "bears_den_token",
                "BEARS Den Code Token",
                vec![AuthEnvVar::new(token_env_for_auth_method())
                    .label(Some("BEARS Den Code Token".to_string()))
                    .secret(true)],
            )
            .description(Some(
                "Bear-scoped Den Code token. Requires BEARS_DEN_API_URL and BEARS_BEAR_SLUG to be configured in the ACP agent server environment. This auth flow cannot fix Den server outages or deployment/version mismatches."
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
    session_context_from_params(params)
}

fn session_context_from_den_session(params: &Value, den_session: &Value) -> Result<SessionContext> {
    validate_mcp_servers_unsupported(params)?;
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
        conversation_id: den_session
            .get("conversation_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        resolved_conversation_id: den_session
            .get("resolved_conversation_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    ctx.raw = json!({
        "cwd": ctx.cwd.clone(),
        "workspace_roots": ctx.roots.clone(),
        "adapter_version": env!("CARGO_PKG_VERSION"),
        "adapter": adapter_capabilities_context(),
        "direct_tools": direct_tools_context(),
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

async fn fetch_conversation_history_chronological(
    http: &reqwest::Client,
    config: &Config,
    conversation_id: &str,
) -> Result<Vec<ReloadHistoryMessage>> {
    let mut chunks: Vec<Vec<ReloadHistoryMessage>> = Vec::new();
    let mut before: Option<String> = None;
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
        chunks.push(page);
        let has_more = body
            .get("has_more")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        before = body
            .get("next_before")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if !has_more {
            break;
        }
        if before.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
            break;
        }
    }
    Ok(chunks.into_iter().rev().flatten().collect())
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
    let den = match den_get_acp_session(http, config, session_id).await {
        Ok(den) => Some(den),
        Err(err)
            if err
                .downcast_ref::<DenHttpError>()
                .is_some_and(|http| http.status == reqwest::StatusCode::NOT_FOUND) =>
        {
            eprintln!(
                "bears-acp-adapter: session/resume session_id={} not found in Den; restoring as local pending session",
                session_id
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
    eprintln!(
        "bears-acp-adapter: session/resume session_id={} cwd={} roots={} direct_tools={}",
        session_id,
        context.cwd,
        context.roots.join(","),
        context
            .raw
            .get("direct_tools")
            .cloned()
            .unwrap_or(Value::Null)
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
    let den = match den_get_acp_session(http, config, session_id).await {
        Ok(den) => Some(den),
        Err(err)
            if err
                .downcast_ref::<DenHttpError>()
                .is_some_and(|http| http.status == reqwest::StatusCode::NOT_FOUND) =>
        {
            eprintln!(
                "bears-acp-adapter: session/load session_id={} not found in Den; loading as local pending session",
                session_id
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
    eprintln!(
        "bears-acp-adapter: session/load session_id={} cwd={} roots={} direct_tools={}",
        session_id,
        context.cwd,
        context.roots.join(","),
        context
            .raw
            .get("direct_tools")
            .cloned()
            .unwrap_or(Value::Null)
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
    let _ = shared_state.cancellation_tx.send(session_id.to_string());
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

fn render_compact_recovery_result(result: Value) -> String {
    let approval_recovery = result
        .get("approval_recovery")
        .cloned()
        .unwrap_or_else(|| json!({ "attempted": false }));
    let compacted = result
        .get("compacted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let compact_result = result.get("compact_result").cloned().unwrap_or(Value::Null);

    format!(
        "BEARS ACP recovery requested for this session. Retry your last prompt. stale_approval_recovery={approval_recovery}; compacted={compacted}; compact_result={compact_result}"
    )
}

async fn handle_prompt(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response_id: Value,
    params: Value,
) -> Result<()> {
    handle_prompt_with_retry(
        http,
        config,
        adapter_state,
        shared_state,
        response_id,
        params,
        true,
    )
    .await
}

async fn handle_prompt_with_retry(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response_id: Value,
    params: Value,
    allow_recovery_retry: bool,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/prompt params missing sessionId"))?;
    let prompt = prompt_text_from_params(&params)?;
    let display_prompt = prompt_display_text_from_params(&params).unwrap_or_else(|| prompt.clone());
    eprintln!(
        "bears-acp-adapter: session/prompt session_id={} prompt_len={} display_prompt_len={} prompt_has_trusted_mode_suffix={} display_has_trusted_mode_suffix={} prompt_has_system_reminder={} display_has_system_reminder={}",
        session_id,
        prompt.len(),
        display_prompt.len(),
        prompt.contains("Trusted ACP session mode this turn:"),
        display_prompt.contains("Trusted ACP session mode this turn:"),
        prompt.contains("<system-reminder>"),
        display_prompt.contains("<system-reminder>"),
    );
    if let Some(command) = parse_local_slash_command(&prompt) {
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
                    "adapter_version": env!("CARGO_PKG_VERSION"),
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
    eprintln!(
        "bears-acp-adapter: session/prompt session_id={} bear={} conversation_id={} client={} direct_tools={}",
        session_id,
        config.bear,
        conversation_log,
        config.client,
        client_context.raw.get("direct_tools").cloned().unwrap_or(Value::Null)
    );

    if !prompt_contains_resources(&params) {
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

    let mut den_payload = json!({
        "message": prompt,
        "client": config.client,
        "client_capabilities": shared_state.client_capabilities.lock().await.clone(),
        "client_context": client_context.raw,
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
        send_agent_message_chunk(
            session_id,
            &format!(
                "BEARS could not complete this turn because Den/Letta returned an error. The ACP session is still alive, so you can use `/compact` or `/collapse` to try recovery.\n\n{message}"
            ),
        )
        .await?;
        write_response(
            response_id,
            Ok(serde_json::to_value(PromptResponse::new(
                StopReason::EndTurn,
            ))?),
        )
        .await?;
        return Ok(());
    }

    let mut stream_diagnostics = SseStreamDiagnostics::default();
    let mut saw_done = false;
    let mut saw_visible_output = false;
    let mut saw_tool_activity = false;
    let mut saw_error = false;
    let mut recover_and_retry = false;
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
            )
            .await?;
            saw_done |= outcome.saw_done;
            saw_visible_output |= outcome.saw_visible_output;
            saw_tool_activity |= outcome.saw_tool_activity;
            saw_error |= outcome.saw_error;
            recover_and_retry |= outcome.recover_and_retry;
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
        )
        .await?;
        saw_done |= outcome.saw_done;
        saw_visible_output |= outcome.saw_visible_output;
        saw_tool_activity |= outcome.saw_tool_activity;
        saw_error |= outcome.saw_error;
        recover_and_retry |= outcome.recover_and_retry;
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
                send_agent_thought_chunk(
                    session_id,
                    &format!(
                        "BEARS detected stale approval state, asked Den to deny pending approvals and compact if needed, and is retrying your prompt automatically. recovery_result={}",
                        result
                    ),
                )
                .await?;
                return Box::pin(handle_prompt_with_retry(
                    http,
                    config,
                    adapter_state,
                    shared_state,
                    response_id,
                    params,
                    false,
                ))
                .await;
            }
            Err(err) => {
                send_agent_message_chunk(
                    session_id,
                    &format!(
                        "BEARS detected stale Letta approval state, but Den recovery failed. This ACP session's conversation may still be wedged; please start a new ACP session. Recovery error: {err:#}"
                    ),
                )
                .await?;
                saw_visible_output = true;
                upstream_errors.clear();
            }
        }
    }

    if recover_and_retry && !allow_recovery_retry && !saw_visible_output && !saw_tool_activity {
        send_agent_message_chunk(
            session_id,
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
        } else {
            let message = format!(
                "BEARS upstream stream reported error: {}",
                upstream_errors.join("; ")
            );
            eprintln!(
                "bears-acp-adapter: converting upstream stream error into terminal ACP turn session_id={} message={}",
                session_id, message
            );
            send_agent_message_chunk(
                session_id,
                &format!(
                    "{message}\n\nThe ACP session is still alive, so you can use `/compact` or `/collapse` to try recovery."
                ),
            )
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

    let stop_reason = if saw_done {
        StopReason::EndTurn
    } else {
        StopReason::EndTurn
    };
    write_response(
        response_id,
        Ok(serde_json::to_value(PromptResponse::new(stop_reason))?),
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalSlashCommand {
    Doctor,
    Compact,
    Conversation,
    Capabilities,
    Runtime,
    Version,
    DebugUi,
}

fn parse_local_slash_command(prompt: &str) -> Option<LocalSlashCommand> {
    match prompt.trim().split_whitespace().next()? {
        "/doctor" => Some(LocalSlashCommand::Doctor),
        "/compact" | "/collapse" => Some(LocalSlashCommand::Compact),
        "/conversation" => Some(LocalSlashCommand::Conversation),
        "/capabilities" => Some(LocalSlashCommand::Capabilities),
        "/runtime" => Some(LocalSlashCommand::Runtime),
        "/version" => Some(LocalSlashCommand::Version),
        "/debug-ui" => Some(LocalSlashCommand::DebugUi),
        _ => None,
    }
}

async fn handle_local_slash_command(
    http: &reqwest::Client,
    config: &Config,
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
            match compact_session_conversation(http, config, session_id).await {
                Ok(result) => render_compact_recovery_result(result),
                Err(err) => format!("BEARS ACP recovery failed: {err:#}"),
            }
        }
        LocalSlashCommand::Conversation => conversation_report(adapter_state, session_id),
        LocalSlashCommand::Capabilities => capabilities_report(adapter_state),
        LocalSlashCommand::Runtime => runtime_report(http, config, shared_state, session_id).await,
        LocalSlashCommand::Version => version_report(http, config).await,
        LocalSlashCommand::DebugUi => debug_ui_report(),
    }
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

fn capabilities_report(adapter_state: &AdapterState) -> String {
    format!(
        "BEARS ACP capabilities\n\nClient capabilities:\n{}\n\nAdapter direct tools:\n{}",
        serde_json::to_string_pretty(&adapter_state.client_capabilities)
            .unwrap_or_else(|_| adapter_state.client_capabilities.to_string()),
        serde_json::to_string_pretty(&direct_tools_context())
            .unwrap_or_else(|_| direct_tools_context().to_string()),
    )
}

async fn runtime_report(
    http: &reqwest::Client,
    config: &Config,
    shared_state: &AdapterSharedState,
    session_id: &str,
) -> String {
    let mut lines = vec!["BEARS ACP runtime".to_string(), String::new()];
    match fetch_den_runtime_state(http, config, session_id).await {
        Ok(value) => {
            lines.push("Den runtime state:".to_string());
            lines.push(serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()));
        }
        Err(err) => {
            lines.push(format!("Den runtime state unavailable: {err:#}"));
        }
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

async fn fetch_den_runtime_state(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
) -> Result<Value> {
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/runtime",
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
        .with_context(|| format!("get ACP runtime state from Den at {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(den_status_error_message(status, body.trim())));
    }
    Ok(serde_json::from_str(&body).unwrap_or_else(|_| json!({ "raw": body })))
}

async fn version_report(http: &reqwest::Client, config: &Config) -> String {
    let den = match fetch_server_version(http, config).await {
        Ok(version) => version.summary(),
        Err(err) => format!("Den server unreachable: {err:#}"),
    };
    format!(
        "BEARS ACP version\n\nAdapter: version={} git_sha={} built_at_utc={} contract={} v{}\nDen: {}",
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        env!("BEARS_ACP_ADAPTER_BUILT_AT_UTC"),
        BEARS_ACP_ADAPTER_CONTRACT_NAME,
        BEARS_ACP_ADAPTER_CONTRACT_VERSION,
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
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &AdapterState,
    context: &SessionContext,
) -> String {
    let den_status = match fetch_server_version(http, config).await {
        Ok(version) => version.summary(),
        Err(err) => format!("Den server unreachable: {err:#}"),
    };
    let token_status = match validate_den_code_token(http, config).await {
        Ok(()) => "valid for this Bear".to_string(),
        Err(err) => format!("not validated: {err:#}"),
    };
    format!(
        "BEARS ACP doctor\n\nAdapter:\n- version: {}\n- git_sha: {}\n- contract: {} v{}\n\nDen:\n- api_url: {}\n- bear: {}\n- server: {}\n- token: {}\n\nClient capabilities:\n{}\n\nSession:\n- cwd: {}\n- roots: {}\n- resolved_conversation_id: {}\n\nDirect tools: {}",
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        BEARS_ACP_ADAPTER_CONTRACT_NAME,
        BEARS_ACP_ADAPTER_CONTRACT_VERSION,
        config.api_url,
        config.bear,
        den_status,
        token_status,
        serde_json::to_string_pretty(&adapter_state.client_capabilities).unwrap_or_else(|_| adapter_state.client_capabilities.to_string()),
        context.cwd,
        if context.roots.is_empty() { "<none>".to_string() } else { context.roots.join(", ") },
        context.resolved_conversation_id.as_deref().unwrap_or("<none>"),
        direct_tools_context(),
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
    eprintln!("  version: {}", env!("CARGO_PKG_VERSION"));
    eprintln!("  build_git_sha: {}", env!("BEARS_ACP_ADAPTER_GIT_SHA"));
    eprintln!("  built_at_utc: {}", env!("BEARS_ACP_ADAPTER_BUILT_AT_UTC"));
    eprintln!("  local_head_sha: {}", local_head_sha());
    eprintln!("  os_arch: {} {}", env::consts::OS, env::consts::ARCH);
    if let Ok(exe) = env::current_exe() {
        eprintln!("  executable: {}", exe.display());
    }
    eprintln!("  direct_tools: {}", direct_tools_context());
    eprintln!();

    if runtime.api_url.trim().is_empty() {
        failed = true;
        eprintln!("✗ BEARS_DEN_API_URL is missing");
    } else {
        eprintln!("✓ BEARS_DEN_API_URL is set");
        eprintln!("  {}", runtime.api_url);
    }

    if runtime.bear.trim().is_empty() {
        failed = true;
        eprintln!("✗ BEARS_BEAR_SLUG is missing");
    } else {
        eprintln!("✓ BEARS_BEAR_SLUG is set");
        eprintln!("  {}", runtime.bear);
    }

    if runtime.token_env.trim().is_empty() {
        eprintln!("• BEARS_DEN_TOKEN_ENV is not set; checking BEARS_DEN_TOKEN/--token directly");
    } else {
        eprintln!("✓ BEARS_DEN_TOKEN_ENV is set");
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
    eprintln!("  BEARS_DEN_API_URL={api_url_hint}");
    eprintln!("  BEARS_BEAR_SLUG={bear_hint}");
    if runtime.token_env.is_empty() {
        eprintln!("  BEARS_DEN_TOKEN=...");
    } else {
        eprintln!("  {}=...", runtime.token_env);
        eprintln!("  BEARS_DEN_TOKEN_ENV={}", runtime.token_env);
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
        "could not connect to the BEARS Den API at {url}. Check that BEARS_DEN_API_URL is the Den API origin reachable from this editor process, that the API service is running with ACP_GATEWAY_ENABLED=true, and that the network/VPN/firewall permits the connection"
    )
}

fn den_status_error_message(status: reqwest::StatusCode, body: &str) -> String {
    if let Some(message) = den_compatibility_status_message(body) {
        return message;
    }
    let hint = match status.as_u16() {
        401 => "The bearer token was rejected. Check BEARS_DEN_TOKEN or --token-env and make sure the token is an active Den Code token.",
        403 => "The token authenticated but is not allowed to use this bear or ACP. Check bear membership and token scopes.",
        404 => "The ACP gateway endpoint was not found. Check BEARS_DEN_API_URL, BEARS_BEAR_SLUG, and that Den is running with ACP_GATEWAY_ENABLED=true on the API service.",
        405 => "The server exists but did not accept the ACP prompt method. Check that BEARS_DEN_API_URL points to the Den API origin, not the web UI origin or a proxy route with method restrictions.",
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

fn prompt_text_from_params(params: &Value) -> Result<String> {
    prompt_text_from_params_with_resource_mode(params, true)
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
    prompt_text_blocks_from_params(params)
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

fn prompt_contains_resources(params: &Value) -> bool {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|block| {
                matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("resource") | Some("resource_link")
                )
            })
        })
}

fn prompt_text_blocks_from_params(params: &Value) -> Result<String> {
    let prompt = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("session/prompt params missing prompt array"))?;

    let text = prompt
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string();

    if text.is_empty() {
        Err(anyhow!("prompt did not contain text content for display"))
    } else {
        Ok(text)
    }
}

fn prompt_text_from_params_with_resource_mode(
    params: &Value,
    include_resource_contents: bool,
) -> Result<String> {
    let prompt = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("session/prompt params missing prompt array"))?;

    let mut parts = Vec::new();
    for block in prompt {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            "resource_link" => {
                let uri = block.get("uri").and_then(Value::as_str).unwrap_or("");
                let name = block.get("name").and_then(Value::as_str).unwrap_or(uri);
                if !uri.is_empty() || !name.is_empty() {
                    parts.push(format!("Referenced resource: {name} ({uri})"));
                }
            }
            "resource" => {
                if let Some(resource) = block.get("resource") {
                    let uri = resource.get("uri").and_then(Value::as_str).unwrap_or("");
                    let name = resource
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(uri);
                    if include_resource_contents {
                        if let Some(text) = resource.get("text").and_then(Value::as_str) {
                            parts.push(format!(
                                "Referenced resource: {name} ({uri})\n<bears-acp-resource uri={uri:?} name={name:?}>\n{text}\n</bears-acp-resource>"
                            ));
                        }
                    } else if !uri.is_empty() || !name.is_empty() {
                        parts.push(format!("Referenced resource: {name} ({uri})"));
                    }
                }
            }
            _ => {}
        }
    }

    let text = parts.join("\n\n").trim().to_string();
    if text.is_empty() {
        Err(anyhow!("prompt did not contain supported text content"))
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
        } else if ty == "tool_request" {
            outcome.saw_tool_activity = true;
            diagnostics.saw_tool_activity = true;
        } else if ty == "error" {
            outcome.saw_error = true;
            diagnostics.saw_error = true;
            let formatted = format_den_event_error(&event);
            if looks_like_waiting_for_approval_error(&formatted) {
                outcome.recover_and_retry = true;
                outcome.upstream_errors.push(
                    "Letta was waiting for stale approval; BEARS will ask Den to deny pending approvals and compact if needed before retrying the prompt."
                        .to_string(),
                );
            } else {
                outcome.upstream_errors.push(formatted);
            }
        }
        if outcome.recover_and_retry {
            continue;
        }
        let handled =
            handle_den_event(config, adapter_state, shared_state, session_id, &event).await?;
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

fn looks_like_waiting_for_approval_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("waiting for approval") || message.contains("please approve or deny")
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
        out.push_str(&context.to_string());
    }
    if let Some(request_id) = event.get("request_id").and_then(Value::as_str) {
        out.push_str("\nDen request_id: ");
        out.push_str(request_id);
    }
    out
}

fn spawn_tool_request_task(
    config: Config,
    shared_state: AdapterSharedState,
    session_id: String,
    event: Value,
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
            .register(&session_id, &tool_call_id, &tool_name)
            .await;
        let mut task_state = AdapterState {
            client_capabilities: shared_state.client_capabilities.lock().await.clone(),
            session_contexts: shared_state.session_contexts.lock().await.clone(),
            transport: shared_state.transport.clone(),
        };
        let mut cancellation_rx = shared_state.cancellation_tx.subscribe();
        let tool_future = handle_tool_request_event(
            &config,
            &mut task_state,
            &shared_state.tool_tasks,
            &shared_state.approval_cache,
            &session_id,
            &event,
        );
        let result = tokio::select! {
            result = tool_future => result,
            cancelled = cancellation_rx.recv() => {
                match cancelled {
                    Ok(cancelled_session_id) if cancelled_session_id == session_id => {
                        shared_state
                            .tool_tasks
                            .set_phase(&session_id, &tool_call_id, &tool_name, ToolTaskPhase::Cancelled)
                            .await;
                        log_tool_task_phase(&session_id, &tool_call_id, &tool_name, ToolTaskPhase::Cancelled);
                        let local_err = LocalToolError::cancelled("ACP session was cancelled before local tool completed");
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
                        let _ = shared_state.tool_tasks.remove(&session_id, &tool_call_id).await;
                        return;
                    }
                    _ => Ok(()),
                }
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
        "pending",
        &preparing,
        Some(event),
        None,
        Vec::new(),
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
            "pending",
            &permission,
            Some(event),
            None,
            replace_plan
                .as_ref()
                .map(|plan| vec![replace_text_diff_content(plan)])
                .unwrap_or_default(),
        )
        .await?;
        let replace_plan_ref = replace_plan.as_ref();
        let permission_decision = request_tool_permission(
            adapter_state,
            session_id,
            tool_call_id,
            tool_name,
            event,
            replace_plan_ref,
            &policy,
            context_for_approval.as_ref(),
            target_path_for_approval.as_deref(),
            target_url_for_approval.as_deref(),
            target_command_for_approval.as_deref(),
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
                        target_path_for_approval.as_deref(),
                        target_url_for_approval.as_deref(),
                        target_command_for_approval.as_deref(),
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
        "pending",
        &running,
        Some(event),
        None,
        Vec::new(),
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
        execute_local_tool(adapter_state, session_id, tool_name, args, &policy).await
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
            "adapter_version": env!("CARGO_PKG_VERSION"),
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
                "completed",
                &preview,
                Some(event),
                Some(raw_output),
                extra_content,
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
                "failed",
                payload["content"].as_str().unwrap_or("Local tool failed"),
                Some(event),
                None,
                Vec::new(),
            )
            .await?;
        }
    }
    if let Err(err) = post_tool_result(config, session_id, tool_call_id, payload).await {
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
            "failed",
            &message,
            Some(event),
            Some(json!({
                "component": "bears-acp-adapter",
                "phase": "result_post_failed",
                "error": format!("{err:#}"),
            })),
            Vec::new(),
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

fn tool_completion_preview(tool_name: &str, value: &Value) -> String {
    let content = value.get("content").and_then(Value::as_str).unwrap_or("");
    let mut text = if content.trim().is_empty() {
        format!("Local tool {tool_name} completed.")
    } else {
        format!("Local tool {tool_name} completed.\n\n{content}")
    };
    let max_chars = 4_000;
    if text.chars().count() > max_chars {
        text = text.chars().take(max_chars).collect::<String>();
        text.push_str("\n... truncated");
    }
    text
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
            "adapter_version": env!("CARGO_PKG_VERSION"),
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
        "failed",
        payload["content"].as_str().unwrap_or("Local tool failed"),
        Some(event),
        None,
        Vec::new(),
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

async fn request_tool_permission(
    adapter_state: &mut AdapterState,
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    event: &Value,
    replace_plan: Option<&ReplaceTextPlan>,
    policy: &ToolPolicy,
    context: Option<&SessionContext>,
    target_path: Option<&Path>,
    target_url: Option<&str>,
    target_command: Option<&str>,
) -> Result<PermissionDecision> {
    let path = event
        .get("args")
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
        .or(target_url)
        .or(target_command)
        .unwrap_or("the requested target");
    let title = event
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| tool_call_title(tool_name, event));
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
    let display = tool_display(tool_name);
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
) -> Result<bool> {
    match event.get("type").and_then(Value::as_str).unwrap_or("") {
        "assistant_text_delta" => {
            let text = event.get("text").and_then(Value::as_str).unwrap_or("");
            if !text.is_empty() {
                send_agent_message_chunk(session_id, text).await?;
            }
            Ok(false)
        }
        "status_text" => {
            let text = event.get("text").and_then(Value::as_str).unwrap_or("");
            if !text.is_empty() {
                send_agent_thought_chunk(session_id, normalize_thought_chunk_text(text).as_ref())
                    .await?;
            }
            Ok(false)
        }
        "error" => {
            let message = format_den_event_error(event);
            send_agent_thought_chunk(session_id, &message).await?;
            Ok(false)
        }
        "tool_request" => {
            spawn_tool_request_task(
                config.clone(),
                shared_state.clone(),
                session_id.to_string(),
                event.clone(),
            );
            Ok(false)
        }
        "permission_request" => {
            handle_permission_request_event(config, adapter_state, session_id, event).await?;
            Ok(false)
        }
        "session_info_update" => {
            let title = event
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string);
            let updated_at = event
                .get("updated_at")
                .and_then(Value::as_str)
                .map(str::to_string);
            send_session_info_update(session_id, title, updated_at).await?;
            Ok(false)
        }
        "plan_update" => {
            if let Some(fallback) = event.get("approval_fallback") {
                if let Some(message) = plan_approval_fallback_message(fallback) {
                    send_agent_message_chunk(session_id, &message).await?;
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
                send_plan_update(session_id, entries).await?;
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
                shared_state
                    .session_contexts
                    .lock()
                    .await
                    .entry(session_id.to_string())
                    .or_default()
                    .resolved_conversation_id = Some(conversation_id.to_string());
                eprintln!(
                    "bears-acp-adapter: session_id={} resolved conversation_id={}",
                    session_id, conversation_id
                );
            }
            Ok(false)
        }

        "turn_result" => Ok(true),
        "turn_complete" => Ok(true),
        _ => Ok(false),
    }
}

async fn handle_permission_request_event(
    config: &Config,
    adapter_state: &mut AdapterState,
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
        display.title = "Approve implementation plan";
        display.kind = ToolKind::SwitchMode;
        display.verb = "Reviewing plan";
        display.permission_operation = "approve this implementation plan";
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
        .content(Some(content.drain(..).collect()))
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
                    "failed",
                    &message,
                    Some(event),
                    Some(json!({
                        "component": "bears-acp-adapter",
                        "phase": "permission_request_failed",
                        "permission_id": permission_id,
                        "error": format!("{err:#}"),
                    })),
                    Vec::new(),
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
        let result = execute_local_tool(adapter_state, session_id, tool_name, args, &policy).await;
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
                    "content": value.get("content").cloned().unwrap_or_else(|| json!("")),
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

struct ToolDisplay {
    title: &'static str,
    kind: ToolKind,
    verb: &'static str,
    permission_operation: &'static str,
}

fn tool_display(tool_name: &str) -> ToolDisplay {
    match tool_name {
        "fs_read_text_file" | "fs.read_text_file" => ToolDisplay {
            title: "Read file",
            kind: ToolKind::Read,
            verb: "Reading",
            permission_operation: "read this file",
        },
        "fs_list_directory" => ToolDisplay {
            title: "List directory",
            kind: ToolKind::Read,
            verb: "Listing",
            permission_operation: "list this directory",
        },
        "fs_find_paths" => ToolDisplay {
            title: "Find paths",
            kind: ToolKind::Search,
            verb: "Finding paths under",
            permission_operation: "find paths",
        },
        "fs_search_files" => ToolDisplay {
            title: "Search files",
            kind: ToolKind::Search,
            verb: "Searching",
            permission_operation: "search files",
        },
        "fs_stat" => ToolDisplay {
            title: "Stat path",
            kind: ToolKind::Read,
            verb: "Inspecting",
            permission_operation: "inspect this path",
        },
        "git_status" => ToolDisplay {
            title: "Git status",
            kind: ToolKind::Read,
            verb: "Checking git status for",
            permission_operation: "read git status",
        },
        "git_diff" => ToolDisplay {
            title: "Git diff",
            kind: ToolKind::Read,
            verb: "Reading git diff for",
            permission_operation: "read git diff",
        },
        "git_log" => ToolDisplay {
            title: "Git log",
            kind: ToolKind::Read,
            verb: "Reading git log for",
            permission_operation: "read git log",
        },
        "git_show" => ToolDisplay {
            title: "Git show",
            kind: ToolKind::Read,
            verb: "Reading git revision for",
            permission_operation: "read git revision",
        },
        "git_add" => ToolDisplay {
            title: "Git add",
            kind: ToolKind::Edit,
            verb: "Staging git paths in",
            permission_operation: "stage git paths",
        },
        "git_restore" => ToolDisplay {
            title: "Git restore",
            kind: ToolKind::Edit,
            verb: "Restoring git paths in",
            permission_operation: "restore git paths",
        },
        "git_commit" => ToolDisplay {
            title: "Git commit",
            kind: ToolKind::Edit,
            verb: "Creating git commit in",
            permission_operation: "create git commit",
        },
        "git_stash" => ToolDisplay {
            title: "Git stash",
            kind: ToolKind::Edit,
            verb: "Creating git stash in",
            permission_operation: "create git stash",
        },
        "web_fetch" => ToolDisplay {
            title: "Fetch URL",
            kind: ToolKind::Fetch,
            verb: "Fetching",
            permission_operation: "fetch this URL",
        },
        "process_run" => ToolDisplay {
            title: "Run process",
            kind: ToolKind::Execute,
            verb: "Running process in",
            permission_operation: "run this command",
        },
        "terminal_run_command" => ToolDisplay {
            title: "Run terminal command",
            kind: ToolKind::Execute,
            verb: "Running terminal command in",
            permission_operation: "run this terminal command",
        },
        "chrome_open" => ToolDisplay {
            title: "Chrome open",
            kind: ToolKind::Fetch,
            verb: "Opening Chrome URL",
            permission_operation: "open this Chrome URL",
        },
        "chrome_snapshot" => ToolDisplay {
            title: "Chrome snapshot",
            kind: ToolKind::Read,
            verb: "Reading Chrome snapshot",
            permission_operation: "read Chrome snapshot",
        },
        "chrome_console_messages" => ToolDisplay {
            title: "Chrome console",
            kind: ToolKind::Read,
            verb: "Reading Chrome console",
            permission_operation: "read Chrome console messages",
        },
        "chrome_network_requests" => ToolDisplay {
            title: "Chrome network",
            kind: ToolKind::Read,
            verb: "Reading Chrome network",
            permission_operation: "read Chrome network requests",
        },
        "chrome_screenshot" => ToolDisplay {
            title: "Chrome screenshot",
            kind: ToolKind::Read,
            verb: "Capturing Chrome screenshot",
            permission_operation: "capture Chrome screenshot",
        },
        "fs_edit_file" | "fs_replace_text" => ToolDisplay {
            title: "Edit file",
            kind: ToolKind::Edit,
            verb: "Editing",
            permission_operation: "modify this file",
        },
        "fs_create_text_file" => ToolDisplay {
            title: "Create file",
            kind: ToolKind::Edit,
            verb: "Creating",
            permission_operation: "create this file",
        },
        "fs_create_directory" => ToolDisplay {
            title: "Create directory",
            kind: ToolKind::Edit,
            verb: "Creating directory",
            permission_operation: "create this directory",
        },
        "fs_move_path" => ToolDisplay {
            title: "Move path",
            kind: ToolKind::Move,
            verb: "Moving",
            permission_operation: "move this path",
        },
        "fs_copy_path" => ToolDisplay {
            title: "Copy path",
            kind: ToolKind::Edit,
            verb: "Copying",
            permission_operation: "copy this path",
        },
        "fs_apply_patch" => ToolDisplay {
            title: "Apply patch",
            kind: ToolKind::Edit,
            verb: "Applying patch to",
            permission_operation: "apply this patch",
        },
        "fs_delete_path" => ToolDisplay {
            title: "Delete path",
            kind: ToolKind::Delete,
            verb: "Deleting",
            permission_operation: "delete this path",
        },
        _ => ToolDisplay {
            title: "Local tool",
            kind: ToolKind::Read,
            verb: "Running",
            permission_operation: "run this local tool",
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
    tool_display(tool_name).title.to_string()
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
    let display = tool_display(tool_name);
    let path = tool_path(event).unwrap_or("the selected workspace path");
    match phase {
        "preparing" => format!("Preparing to {} `{path}`…", display.verb.to_lowercase()),
        "permission" => format!(
            "Waiting for approval to {} `{path}`.",
            display.verb.to_lowercase()
        ),
        "running" => format!("{} `{path}`…", display.verb),
        _ => format!("{} `{path}`…", display.verb),
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

async fn send_tool_call_update(
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    status: &str,
    text: &str,
    event: Option<&Value>,
    raw_output: Option<Value>,
    extra_content: Vec<ToolCallContent>,
) -> Result<()> {
    let display = tool_display(tool_name);
    let mut content = vec![ToolCallContent::from(text.to_string())];
    content.extend(extra_content);
    let title = event
        .map(|event| tool_call_title(tool_name, event))
        .unwrap_or_else(|| display.title.to_string());
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
            "hint": "Configure BEARS_DEN_API_URL, BEARS_BEAR_SLUG, and BEARS_DEN_TOKEN/BEARS_DEN_TOKEN_ENV in the ACP agent server environment, then restart the agent server.",
        })));
    }
    auth_check_json_rpc_error(err, None)
}

fn auth_check_json_rpc_error(err: &anyhow::Error, token_hint: Option<&str>) -> Value {
    let message = format!("{err:#}");
    if looks_like_den_connectivity_error(err) {
        return den_connectivity_error(Some(json!({
            "message": format!("Could not reach the BEARS Den server while checking the Code token: {message}"),
            "hint": "Check that BEARS_DEN_API_URL is correct and that the Den API server is online/reachable. This does not necessarily mean your token is invalid.",
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
        message.contains("Missing BEARS_DEN_TOKEN")
            || message.contains("Missing BEARS_DEN_API_URL")
            || message.contains("Missing BEARS_BEAR_SLUG")
            || message.contains("BEARS_DEN_TOKEN_ENV points at")
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
            approval_cache: ApprovalCache::default(),
            cancellation_tx,
        }
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
        )
        .await
        .unwrap();

        assert!(outcome.saw_error);
        assert!(!outcome.saw_visible_output);
        assert!(outcome.recover_and_retry);
        assert_eq!(outcome.upstream_errors.len(), 1);
        assert!(outcome.upstream_errors[0].contains("deny pending approvals"));
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
        assert_eq!(
            tool_completion_preview("fs_list_directory", &value),
            "Local tool fs_list_directory completed.\n\nabc"
        );
        let long = json!({ "content": "x".repeat(4_100) });
        let preview = tool_completion_preview("fs_read_text_file", &long);
        assert!(preview.contains("... truncated"));
        assert!(preview.chars().count() < 4_050);
    }

    #[test]
    fn friendly_tool_status_mentions_path_and_action() {
        let event = json!({ "args": { "path": "/workspace/README.md" } });
        assert_eq!(
            friendly_tool_status("fs_replace_text", &event, "permission"),
            "Waiting for approval to editing `/workspace/README.md`."
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
}
