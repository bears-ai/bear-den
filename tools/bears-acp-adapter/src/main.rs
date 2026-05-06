use agent_client_protocol::schema::{
    AgentCapabilities, AuthEnvVar, AuthMethod, AuthMethodEnvVar, AuthenticateResponse,
    CloseSessionResponse, ContentBlock, ContentChunk, Diff, Implementation, InitializeResponse,
    ListSessionsResponse, LoadSessionResponse, McpCapabilities, NewSessionResponse,
    PermissionOption, PermissionOptionKind, PromptCapabilities, PromptResponse, ProtocolVersion,
    ReadTextFileRequest, ReadTextFileResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ResumeSessionResponse, SessionCapabilities,
    SessionCloseCapabilities, SessionInfo, SessionListCapabilities, SessionResumeCapabilities,
    SessionUpdate, StopReason, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Url;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{broadcast, mpsc, Mutex as TokioMutex},
};
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
struct JsonRpcTransport {
    pending_responses: Arc<TokioMutex<HashMap<String, tokio::sync::oneshot::Sender<Value>>>>,
}

#[derive(Default)]
struct AdapterState {
    client_capabilities: Value,
    session_contexts: HashMap<String, SessionContext>,
    transport: JsonRpcTransport,
}

#[derive(Clone)]
struct AdapterSharedState {
    transport: JsonRpcTransport,
    session_contexts: Arc<TokioMutex<HashMap<String, SessionContext>>>,
    tool_tasks: ToolTaskRegistry,
    approval_cache: ApprovalCache,
    cancellation_tx: broadcast::Sender<String>,
}

#[derive(Clone, Default)]
struct ApprovalCache {
    entries: Arc<TokioMutex<HashMap<String, ApprovalRecord>>>,
    persistence: Option<ApprovalPersistence>,
}

#[derive(Clone, Debug)]
struct ApprovalPersistence {
    path: PathBuf,
    api_url: String,
    bear: String,
    client: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct ApprovalRecord {
    api_url: String,
    bear: String,
    client: String,
    tool_name: String,
    permission_class: String,
    root_fingerprint: String,
    risk: String,
    created_at_secs: u64,
    expires_at_secs: u64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ApprovalCacheFile {
    version: u32,
    entries: Vec<ApprovalRecord>,
}

#[derive(Clone, Default)]
struct ToolTaskRegistry {
    tasks: Arc<TokioMutex<HashMap<String, ToolTaskRecord>>>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct ToolTaskRecord {
    session_id: String,
    tool_call_id: String,
    tool_name: String,
    phase: ToolTaskPhase,
    started_at: std::time::Instant,
    updated_at: std::time::Instant,
}

impl ApprovalCache {
    fn key(
        api_url: &str,
        bear: &str,
        client: &str,
        root_fingerprint: &str,
        permission_class: &str,
    ) -> String {
        format!("{api_url}\n{bear}\n{client}\n{root_fingerprint}\n{permission_class}")
    }

    async fn load_for_runtime(runtime: &RuntimeConfig) -> Self {
        if env_bool("BEARS_ACP_DISABLE_PERSISTENT_APPROVALS") {
            return Self::default();
        }
        let Some(config) = runtime.config.as_ref() else {
            return Self::default();
        };
        let path = approval_cache_path();
        let persistence = ApprovalPersistence {
            path: path.clone(),
            api_url: config.api_url.clone(),
            bear: config.bear.clone(),
            client: config.client.clone(),
        };
        let cache = Self {
            entries: Arc::new(TokioMutex::new(HashMap::new())),
            persistence: Some(persistence),
        };
        if env_bool("BEARS_ACP_CLEAR_APPROVALS") {
            let _ = fs::remove_file(&path);
            return cache;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(file) = serde_json::from_str::<ApprovalCacheFile>(&raw) {
                let now = now_secs();
                let mut entries = cache.entries.lock().await;
                for mut record in file.entries.into_iter().filter(|r| r.expires_at_secs > now) {
                    if record.permission_class.trim().is_empty() {
                        record.permission_class =
                            permission_class_for_tool(&record.tool_name).to_string();
                    }
                    let key = Self::key(
                        &record.api_url,
                        &record.bear,
                        &record.client,
                        &record.root_fingerprint,
                        &record.permission_class,
                    );
                    entries.insert(key, record);
                }
            }
        }
        cache
    }

    async fn remember(&self, context: &SessionContext, tool_name: &str, risk: &str) {
        let Some(persistence) = self.persistence.as_ref() else {
            return;
        };
        let root_fingerprint = approval_root_fingerprint(context);
        let now = now_secs();
        let record = ApprovalRecord {
            api_url: persistence.api_url.clone(),
            bear: persistence.bear.clone(),
            client: persistence.client.clone(),
            tool_name: tool_name.to_string(),
            permission_class: permission_class_for_tool(tool_name).to_string(),
            root_fingerprint: root_fingerprint.clone(),
            risk: risk.to_string(),
            created_at_secs: now,
            expires_at_secs: now + approval_ttl_secs(risk),
        };
        self.entries.lock().await.insert(
            Self::key(
                &record.api_url,
                &record.bear,
                &record.client,
                &record.root_fingerprint,
                &record.permission_class,
            ),
            record,
        );
        self.save().await;
    }

    async fn is_allowed(&self, context: &SessionContext, tool_name: &str) -> bool {
        let Some(persistence) = self.persistence.as_ref() else {
            return false;
        };
        let root_fingerprint = approval_root_fingerprint(context);
        let key = Self::key(
            &persistence.api_url,
            &persistence.bear,
            &persistence.client,
            &root_fingerprint,
            permission_class_for_tool(tool_name),
        );
        let now = now_secs();
        let mut entries = self.entries.lock().await;
        entries.retain(|_, record| record.expires_at_secs > now);
        entries.get(&key).is_some()
    }

    async fn clear_session(&self, _session_id: &str) {
        // Persistent approvals intentionally survive ACP session boundaries.
        // Use BEARS_ACP_CLEAR_APPROVALS=1 or remove the cache file to revoke.
    }

    async fn save(&self) {
        let Some(persistence) = self.persistence.as_ref() else {
            return;
        };
        let now = now_secs();
        let entries = self
            .entries
            .lock()
            .await
            .values()
            .filter(|record| record.expires_at_secs > now)
            .cloned()
            .collect::<Vec<_>>();
        let file = ApprovalCacheFile {
            version: 1,
            entries,
        };
        if let Some(parent) = persistence.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp = persistence.path.with_extension("tmp");
        if let Ok(raw) = serde_json::to_string_pretty(&file) {
            if fs::write(&tmp, raw).is_ok() {
                let _ = fs::rename(tmp, &persistence.path);
            }
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn env_bool(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn approval_ttl_secs(risk: &str) -> u64 {
    if matches!(risk, "writes_workspace" | "deletes_workspace") {
        7 * 24 * 60 * 60
    } else {
        28 * 24 * 60 * 60
    }
}

fn approval_cache_path() -> PathBuf {
    if let Ok(path) = env::var("BEARS_ACP_APPROVALS_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("bears")
            .join("acp-approvals.json");
    }
    PathBuf::from(".bears-acp-approvals.json")
}

fn permission_class_for_tool(tool_name: &str) -> &'static str {
    match tool_name {
        "fs_read_text_file" | "fs_list_directory" | "fs_search_files" | "fs.read_text_file"
        | "read_text_file" => "read_files",
        "fs_replace_text" | "fs_create_text_file" => "edit_files",
        "fs_delete_path" => "delete_files",
        _ => "local_files",
    }
}

fn approval_root_fingerprint(context: &SessionContext) -> String {
    let roots = if context.roots.is_empty() {
        vec![context.cwd.clone()]
    } else {
        context.roots.clone()
    };
    roots.join("|")
}

impl ToolTaskRegistry {
    fn key(session_id: &str, tool_call_id: &str) -> String {
        format!("{session_id}\n{tool_call_id}")
    }

    async fn register(&self, session_id: &str, tool_call_id: &str, tool_name: &str) {
        let now = std::time::Instant::now();
        self.tasks.lock().await.insert(
            Self::key(session_id, tool_call_id),
            ToolTaskRecord {
                session_id: session_id.to_string(),
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                phase: ToolTaskPhase::Received,
                started_at: now,
                updated_at: now,
            },
        );
    }

    async fn set_phase(
        &self,
        session_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        phase: ToolTaskPhase,
    ) {
        let mut tasks = self.tasks.lock().await;
        let key = Self::key(session_id, tool_call_id);
        let now = std::time::Instant::now();
        let entry = tasks.entry(key).or_insert_with(|| ToolTaskRecord {
            session_id: session_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            phase,
            started_at: now,
            updated_at: now,
        });
        let previous_phase = entry.phase;
        let previous_elapsed_ms = now.duration_since(entry.updated_at).as_millis();
        let total_elapsed_ms = now.duration_since(entry.started_at).as_millis();
        entry.phase = phase;
        entry.updated_at = now;
        eprintln!(
            "bears-acp-adapter: tool_task transition session_id={} tool_call_id={} tool_name={} from_phase={} to_phase={} phase_duration_ms={} total_duration_ms={}",
            session_id,
            tool_call_id,
            tool_name,
            previous_phase.as_str(),
            phase.as_str(),
            previous_elapsed_ms,
            total_elapsed_ms,
        );
    }

    async fn remove(&self, session_id: &str, tool_call_id: &str) -> Option<ToolTaskRecord> {
        let removed = self
            .tasks
            .lock()
            .await
            .remove(&Self::key(session_id, tool_call_id));
        if let Some(record) = removed.as_ref() {
            eprintln!(
                "bears-acp-adapter: tool_task finished session_id={} tool_call_id={} tool_name={} final_phase={} total_duration_ms={}",
                record.session_id,
                record.tool_call_id,
                record.tool_name,
                record.phase.as_str(),
                record.started_at.elapsed().as_millis(),
            );
        }
        removed
    }

    #[allow(dead_code)]
    async fn list_for_session(&self, session_id: &str) -> Vec<ToolTaskRecord> {
        self.tasks
            .lock()
            .await
            .values()
            .filter(|task| task.session_id == session_id)
            .cloned()
            .collect()
    }
}

impl JsonRpcTransport {
    async fn route_response(&self, id: &Value, value: Value) -> bool {
        if let Some(tx) = self.pending_responses.lock().await.remove(&id_key(id)) {
            let _ = tx.send(value);
            true
        } else {
            false
        }
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        let id = json!(format!("req-{}", Uuid::new_v4()));
        let key = id_key(&id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_responses.lock().await.insert(key.clone(), tx);
        write_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(anyhow!(
                "client response channel closed for {method} id={key}"
            )),
            Err(_) => {
                self.pending_responses.lock().await.remove(&key);
                Err(anyhow!(
                    "timed out waiting for client response to {method} id={key}"
                ))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        write_json(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }
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
    permission_timeout_ms: Option<u64>,
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

#[derive(Clone, Debug)]
struct ReplaceTextArgs {
    path: String,
    old_text: String,
    new_text: String,
    expected_replacements: usize,
}

#[derive(Clone, Debug)]
struct ReplaceTextPlan {
    args: ReplaceTextArgs,
    path: PathBuf,
    replacements: usize,
    bytes_before: usize,
    bytes_after: usize,
    preview: String,
    policy_max_bytes: u64,
    policy_max_replacements: usize,
    policy_create_files: bool,
    policy_allow_multiple: bool,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolTaskPhase {
    Received,
    PermissionRequested,
    PermissionGranted,
    PermissionDenied,
    PermissionTimeout,
    ExecutionStarted,
    ExecutionSucceeded,
    ExecutionFailed,
    ResultPosted,
    ResultPostFailed,
    Cancelled,
}

impl ToolTaskPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::PermissionRequested => "permission_requested",
            Self::PermissionGranted => "permission_granted",
            Self::PermissionDenied => "permission_denied",
            Self::PermissionTimeout => "permission_timeout",
            Self::ExecutionStarted => "execution_started",
            Self::ExecutionSucceeded => "execution_succeeded",
            Self::ExecutionFailed => "execution_failed",
            Self::ResultPosted => "result_posted",
            Self::ResultPostFailed => "result_post_failed",
            Self::Cancelled => "cancelled",
        }
    }
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

fn log_tool_task_phase(
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    phase: ToolTaskPhase,
) {
    eprintln!(
        "bears-acp-adapter: tool_task phase={} session_id={} tool_call_id={} tool_name={}",
        phase.as_str(),
        session_id,
        tool_call_id,
        tool_name
    );
}

#[derive(Debug)]
struct LocalToolError {
    status: LocalToolStatus,
    message: String,
    diagnostic: Value,
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

fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        _ => id.to_string(),
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
        .timeout(std::time::Duration::from_secs(300))
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
        session_contexts: Arc::new(TokioMutex::new(HashMap::new())),
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
            if let Some(id) = request.id {
                write_response(id, Ok(initialize_result()?)).await?;
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
                        write_response(
                            id,
                            Err(auth_challenge_error(Some(json!({
                                "message": format!("{err:#}")
                            })))),
                        )
                        .await?;
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
                write_response(
                    id,
                    Ok(serde_json::to_value(NewSessionResponse::new(session_id))?),
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
                        Err(token_validation_error(Some(json!({
                            "message": format!("BEARS Code token authentication failed: {err:#}"),
                            "hint": "Generate a fresh Den Code token for this bear."
                        })))),
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
                    write_response(
                        id,
                        Err(token_validation_error(Some(json!({
                            "message": format!("BEARS Code token authentication failed: {err:#}"),
                        })))),
                    )
                    .await?;
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
                    Ok(()) => {
                        write_response(id, Ok(serde_json::to_value(ResumeSessionResponse::new())?))
                            .await?
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
                    write_response(
                        id,
                        Err(token_validation_error(Some(json!({
                            "message": format!("BEARS Code token authentication failed: {err:#}"),
                        })))),
                    )
                    .await?;
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
                        Err(token_validation_error(Some(json!({
                            "message": format!("BEARS Code token authentication failed: {err:#}"),
                            "hint": "Generate a fresh Den Code token for this bear. Code tokens must include acp:chat."
                        })))),
                    )
                    .await?;
                    return Ok(());
                }

                if let Err(err) = handle_prompt(
                    http,
                    config,
                    adapter_state,
                    &shared_state,
                    id.clone(),
                    request.params,
                )
                .await
                {
                    let server_version = fetch_server_version(http, config).await.ok();
                    let mut message = format!("{err:#}");
                    if let Some(server_version) = &server_version {
                        message.push_str("\n\n");
                        message.push_str(&server_version.summary());
                    }
                    write_response(
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
                    .await?;
                }
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

fn adapter_capabilities_context() -> Value {
    json!({
        "name": "bears-acp-adapter",
        "version": env!("CARGO_PKG_VERSION"),
        "direct_tools": {
            "fs_read_text_file": { "supported": true, "version": 1 },
            "fs_list_directory": { "supported": true, "version": 1 },
            "fs_search_files": { "supported": true, "version": 1 },
            "fs_replace_text": { "supported": true, "version": 1 },
            "fs_create_text_file": { "supported": true, "version": 1 },
            "fs_delete_path": { "supported": true, "version": 1 }
        }
    })
}

fn direct_tools_context() -> Value {
    json!({
        "fs_read_text_file": true,
        "fs_list_directory": true,
        "fs_search_files": true,
        "fs_replace_text": true,
        "fs_create_text_file": true,
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

fn is_absolute_local_path(path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    Path::new(path).is_absolute()
        || path.starts_with("\\\\")
        || (path.len() >= 3
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':'
            && matches!(path.as_bytes()[2], b'/' | b'\\'))
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
    Err(anyhow!(
        "BEARS Code token validation failed with HTTP {status}: {}",
        body.trim()
    ))
}

fn client_supports_read_text_file(adapter_state: &AdapterState) -> bool {
    adapter_state
        .client_capabilities
        .pointer("/fs/readTextFile")
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
        .ok_or_else(|| anyhow!("bears/read_text_file params missing sessionId"))?;
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("bears/read_text_file params missing path"))?;
    let line = params
        .get("line")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let policy_max_lines = policy.max_lines.unwrap_or(2_000).clamp(1, 2_000);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_lines as u64) as usize)
        .unwrap_or(400.min(policy_max_lines));
    let context = adapter_state
        .session_contexts
        .get(session_id)
        .ok_or_else(|| anyhow!("ACP session {session_id} is not known to this adapter"))?;
    let path = normalize_requested_tool_path(path)?;
    ensure_path_allowed_for_session(context, &path)?;
    let started = std::time::Instant::now();
    let raw = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("read text file {}", path.display()))?;
    let total_lines = raw.lines().count();
    let selected: Vec<&str> = raw
        .lines()
        .skip(line.saturating_sub(1))
        .take(limit)
        .collect();
    let truncated = line.saturating_sub(1) + selected.len() < total_lines;
    let mut content = selected.join("\n");
    if raw.ends_with('\n') && !content.is_empty() && !truncated {
        content.push('\n');
    }
    eprintln!(
        "bears-acp-adapter: read_text_file session_id={} path={} line={} limit={} bytes={} total_lines={} returned_lines={} truncated={} duration_ms={}",
        session_id,
        path.display(),
        line,
        limit,
        raw.len(),
        total_lines,
        selected.len(),
        truncated,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "content": content,
        "line": line,
        "returned_lines": selected.len(),
        "total_lines": total_lines,
        "truncated": truncated,
        "bytes": raw.len(),
        "policy": {
            "max_lines": policy_max_lines,
            "applied_limit": limit,
        },
    }))
}

#[derive(Clone, Debug, Default)]
struct SearchFilters {
    case_sensitive: bool,
    pattern: Option<String>,
    extensions: Vec<String>,
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
        "fs_search_files" => {
            handle_direct_search_files(adapter_state, session_id, &args, policy).await
        }
        "fs_replace_text" => {
            handle_direct_replace_text(adapter_state, session_id, &args, policy).await
        }
        "fs_create_text_file" => {
            handle_direct_create_text_file(adapter_state, session_id, &args, policy).await
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
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_list_directory args missing path"))?;
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .or(policy.recursive_default)
        .unwrap_or(false);
    let include_hidden = args
        .get("include_hidden")
        .and_then(Value::as_bool)
        .or(policy.include_hidden_default)
        .unwrap_or(false);
    let policy_max_entries = policy.max_entries.unwrap_or(1_000).clamp(1, 1_000);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_entries as u64) as usize)
        .unwrap_or(200.min(policy_max_entries));
    let context = session_context(adapter_state, session_id)?;
    let path = normalize_requested_tool_path(path)?;
    ensure_path_allowed_for_session(context, &path)?;
    let started = std::time::Instant::now();
    let mut entries = Vec::new();
    let mut total_entries_seen = 0usize;
    let mut truncated = false;
    let mut queue = VecDeque::from([path.clone()]);
    while let Some(dir) = queue.pop_front() {
        ensure_path_allowed_for_session(context, &dir)?;
        let mut dir_entries = fs::read_dir(&dir)
            .with_context(|| format!("list directory {}", dir.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        dir_entries.sort_by_key(|entry| entry.path());
        for entry in dir_entries {
            let entry_path = entry.path();
            if !include_hidden && is_hidden_path_component(&entry_path, &path) {
                continue;
            }
            ensure_path_allowed_for_session(context, &entry_path)?;
            total_entries_seen += 1;
            let metadata = entry.metadata().ok();
            let kind = metadata
                .as_ref()
                .map(|m| {
                    if m.is_dir() {
                        "directory"
                    } else if m.is_file() {
                        "file"
                    } else {
                        "other"
                    }
                })
                .unwrap_or("unknown");
            if entries.len() < limit {
                entries.push(json!({
                    "path": entry_path.to_string_lossy(),
                    "name": entry.file_name().to_string_lossy(),
                    "kind": kind,
                    "size": metadata.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
                }));
            } else {
                truncated = true;
                break;
            }
            if recursive && metadata.as_ref().is_some_and(|m| m.is_dir()) {
                queue.push_back(entry_path);
            }
        }
    }
    let truncated = truncated || total_entries_seen > entries.len() || !queue.is_empty();
    let content = format_directory_listing(&path, &entries, truncated);
    eprintln!(
        "bears-acp-adapter: list_directory session_id={} path={} recursive={} include_hidden={} limit={} returned_entries={} total_entries_seen={} truncated={} duration_ms={}",
        session_id,
        path.display(),
        recursive,
        include_hidden,
        limit,
        entries.len(),
        total_entries_seen,
        truncated,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "entries": entries,
        "total_entries_seen": total_entries_seen,
        "returned_entries": entries.len(),
        "truncated": truncated,
        "recursive": recursive,
        "include_hidden": include_hidden,
        "source": "adapter_local",
        "content": content,
        "policy": {
            "max_entries": policy_max_entries,
            "applied_limit": limit,
            "recursive_default": policy.recursive_default,
            "include_hidden_default": policy.include_hidden_default,
        },
    }))
}

async fn handle_direct_search_files(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_search_files args missing path"))?;
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("fs_search_files args missing non-empty query"))?;
    let policy_max_results = policy.max_results.unwrap_or(200).clamp(1, 200);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_results as u64) as usize)
        .unwrap_or(50.min(policy_max_results));
    let policy_max_bytes = policy.max_bytes.unwrap_or(1_048_576).clamp(1, 5_242_880);
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_bytes))
        .unwrap_or(policy_max_bytes);
    let include_hidden = args
        .get("include_hidden")
        .and_then(Value::as_bool)
        .or(policy.include_hidden_default)
        .unwrap_or(false);
    let filters = search_filters_from_args(args)?;
    let context = session_context(adapter_state, session_id)?;
    let path = normalize_requested_tool_path(path)?;
    ensure_path_allowed_for_session(context, &path)?;
    let started = std::time::Instant::now();
    let mut files = Vec::new();
    let mut file_collection_truncated = false;
    let mut skipped_by_filter = 0usize;
    collect_search_files(
        context,
        &path,
        &path,
        include_hidden,
        &filters,
        5_000,
        &mut file_collection_truncated,
        &mut skipped_by_filter,
        &mut files,
    )?;
    files.sort();

    let mut matches = Vec::new();
    let mut files_scanned = 0usize;
    let mut bytes_scanned = 0u64;
    let mut truncated = file_collection_truncated;
    for file in files {
        ensure_path_allowed_for_session(context, &file)?;
        let metadata = match fs::metadata(&file) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => continue,
        };
        if bytes_scanned.saturating_add(metadata.len()) > max_bytes {
            truncated = true;
            break;
        }
        let raw = match fs::read_to_string(&file) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        bytes_scanned = bytes_scanned.saturating_add(metadata.len());
        files_scanned += 1;
        for (idx, line) in raw.lines().enumerate() {
            if line_matches_query(line, query, filters.case_sensitive) {
                matches.push(json!({
                    "path": file.to_string_lossy(),
                    "line": idx + 1,
                    "preview": truncate_for_log(line.trim(), 240),
                }));
                if matches.len() >= limit {
                    truncated = true;
                    break;
                }
            }
        }
        if matches.len() >= limit {
            break;
        }
    }
    let content = format_search_results(query, &matches, truncated);
    eprintln!(
        "bears-acp-adapter: search_files session_id={} path={} query_len={} limit={} max_bytes={} files_scanned={} bytes_scanned={} matches={} truncated={} duration_ms={}",
        session_id,
        path.display(),
        query.len(),
        limit,
        max_bytes,
        files_scanned,
        bytes_scanned,
        matches.len(),
        truncated,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "query": query,
        "matches": matches,
        "returned_matches": matches.len(),
        "truncated": truncated,
        "files_scanned": files_scanned,
        "bytes_scanned": bytes_scanned,
        "max_bytes": max_bytes,
        "include_hidden": include_hidden,
        "case_sensitive": filters.case_sensitive,
        "pattern": filters.pattern,
        "extensions": filters.extensions,
        "skipped_by_filter": skipped_by_filter,
        "source": "adapter_local",
        "content": content,
        "policy": {
            "max_results": policy_max_results,
            "applied_limit": limit,
            "max_bytes": policy_max_bytes,
            "applied_max_bytes": max_bytes,
            "include_hidden_default": policy.include_hidden_default,
        },
    }))
}

async fn handle_direct_replace_text(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    let args = ReplaceTextArgs::from_value(args, policy)?;
    let started = std::time::Instant::now();
    let plan = ReplaceTextPlan::preflight(context, args, policy)?;
    let applied = plan.apply(context, policy)?;
    eprintln!(
        "bears-acp-adapter: replace_text session_id={} path={} bytes_before={} bytes_after={} duration_ms={}",
        session_id,
        applied["path"].as_str().unwrap_or(""),
        applied["bytes_before"].as_u64().unwrap_or(0),
        applied["bytes_after"].as_u64().unwrap_or(0),
        started.elapsed().as_millis(),
    );
    Ok(applied)
}

impl ReplaceTextArgs {
    fn from_value(args: &Value, policy: &ToolPolicy) -> Result<Self> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("fs_replace_text args missing path"))?
            .to_string();
        let old_text = args
            .get("old_text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("fs_replace_text args missing old_text"))?
            .to_string();
        let new_text = args
            .get("new_text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("fs_replace_text args missing new_text"))?
            .to_string();
        if old_text.is_empty() {
            return Err(anyhow!("fs_replace_text old_text must not be empty"));
        }
        let create_if_missing = args
            .get("create_if_missing")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let policy_create_files = policy.create_files.unwrap_or(false);
        if create_if_missing && !policy_create_files {
            return Err(anyhow!("fs_replace_text does not create files yet"));
        }
        let allow_multiple = args
            .get("allow_multiple")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let policy_allow_multiple = policy.allow_multiple.unwrap_or(false);
        if allow_multiple && !policy_allow_multiple {
            return Err(anyhow!(
                "fs_replace_text does not allow multiple replacements yet"
            ));
        }
        let expected_replacements = args
            .get("expected_replacements")
            .and_then(Value::as_u64)
            .unwrap_or(1) as usize;
        let policy_max_replacements = policy.max_replacements.unwrap_or(1).clamp(1, 100);
        if expected_replacements == 0 || expected_replacements > policy_max_replacements {
            return Err(anyhow!(
                "fs_replace_text expected_replacements exceeds policy max_replacements={policy_max_replacements}"
            ));
        }
        if expected_replacements != 1 || allow_multiple || policy_allow_multiple {
            return Err(anyhow!(
                "fs_replace_text currently supports exactly one replacement"
            ));
        }
        Ok(Self {
            path,
            old_text,
            new_text,
            expected_replacements,
        })
    }
}

impl ReplaceTextPlan {
    fn preflight(
        context: &SessionContext,
        args: ReplaceTextArgs,
        policy: &ToolPolicy,
    ) -> Result<Self> {
        let policy_max_bytes = policy.max_bytes.unwrap_or(1_048_576).clamp(1, 5_242_880);
        let policy_max_replacements = policy.max_replacements.unwrap_or(1).clamp(1, 100);
        let policy_create_files = policy.create_files.unwrap_or(false);
        let policy_allow_multiple = policy.allow_multiple.unwrap_or(false);
        let path = normalize_requested_tool_path(&args.path)?;
        ensure_path_allowed_for_session(context, &path)?;
        ensure_replace_text_path_allowed(&path, policy)?;
        let raw = read_replace_text_input(&path, policy_max_bytes)?;
        let replacements = raw.matches(&args.old_text).count();
        if replacements != args.expected_replacements {
            return Err(anyhow!(
                "fs_replace_text expected {} match for old_text, found {replacements}",
                args.expected_replacements
            ));
        }
        let updated = raw.replacen(&args.old_text, &args.new_text, args.expected_replacements);
        let preview = replace_text_preview(&path, &args, &raw, &updated);
        Ok(Self {
            args,
            path,
            replacements,
            bytes_before: raw.len(),
            bytes_after: updated.len(),
            preview,
            policy_max_bytes,
            policy_max_replacements,
            policy_create_files,
            policy_allow_multiple,
        })
    }

    fn apply(&self, context: &SessionContext, policy: &ToolPolicy) -> Result<Value> {
        ensure_path_allowed_for_session(context, &self.path)?;
        ensure_replace_text_path_allowed(&self.path, policy)?;
        let raw = read_replace_text_input(&self.path, self.policy_max_bytes)?;
        let replacements = raw.matches(&self.args.old_text).count();
        if replacements != self.args.expected_replacements {
            return Err(anyhow!(
                "fs_replace_text stale preflight: expected {} match for old_text, found {replacements}",
                self.args.expected_replacements
            ));
        }
        let updated = raw.replacen(
            &self.args.old_text,
            &self.args.new_text,
            self.args.expected_replacements,
        );
        fs::write(&self.path, updated.as_bytes())
            .with_context(|| format!("write replaced text file {}", self.path.display()))?;
        let content = format!(
            "Replaced {} occurrence in {} ({} bytes -> {} bytes)",
            self.replacements,
            self.path.display(),
            raw.len(),
            updated.len()
        );
        Ok(json!({
            "ok": true,
            "path": self.path.to_string_lossy(),
            "replacements": self.replacements,
            "bytes_before": raw.len(),
            "bytes_after": updated.len(),
            "source": "adapter_local",
            "content": content,
            "preview": self.preview,
            "policy": {
                "max_bytes": self.policy_max_bytes,
                "sensitive_path_policy": policy.sensitive_path_policy,
                "max_replacements": self.policy_max_replacements,
                "create_files": self.policy_create_files,
                "allow_multiple": self.policy_allow_multiple,
                "deny_hidden_paths": policy.deny_hidden_paths.unwrap_or(true),
            },
        }))
    }

    fn permission_summary(&self, tool_name: &str, reason: &str) -> String {
        format!(
            "{reason}\n\nTool: {tool_name}\nPath: {}\nReplacing {} occurrence\nBytes: {} -> {}\n\nReview the diff below before approving.",
            self.path.display(),
            self.replacements,
            self.bytes_before,
            self.bytes_after,
        )
    }

    #[cfg(test)]
    fn permission_prompt(&self, tool_name: &str, reason: &str) -> String {
        format!(
            "{}\n\n{}",
            self.permission_summary(tool_name, reason),
            self.preview
        )
    }
}

fn read_replace_text_input(path: &Path, policy_max_bytes: u64) -> Result<String> {
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!("fs_replace_text path must be an existing file"));
    }
    if metadata.len() > policy_max_bytes {
        return Err(anyhow!(
            "fs_replace_text file exceeds policy max_bytes: {} > {}",
            metadata.len(),
            policy_max_bytes
        ));
    }
    fs::read_to_string(path)
        .with_context(|| format!("read text file for replace {}", path.display()))
}

fn replace_text_preview(path: &Path, args: &ReplaceTextArgs, raw: &str, updated: &str) -> String {
    let before = preview_around(raw, &args.old_text, 320);
    let after = preview_around(updated, &args.new_text, 320);
    format!(
        "Preview for {}\n--- before\n{}\n+++ after\n{}",
        path.display(),
        before,
        after,
    )
}

fn preview_around(text: &str, needle: &str, max_chars: usize) -> String {
    let Some(pos) = text.find(needle) else {
        return truncate_chars(text, max_chars);
    };
    let before = truncate_chars_reverse(&text[..pos], max_chars / 2);
    let after_start = pos.saturating_add(needle.len()).min(text.len());
    let after = truncate_chars(&text[after_start..], max_chars / 2);
    let mut out = String::new();
    if before.chars().count() < text[..pos].chars().count() {
        out.push_str("...");
    }
    out.push_str(&before);
    out.push_str(needle);
    out.push_str(&after);
    if after.chars().count() < text[after_start..].chars().count() {
        out.push_str("...");
    }
    out
}

fn truncate_chars_reverse(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        text.to_string()
    } else {
        text.chars().skip(count - max_chars).collect()
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut out = text.chars().take(max_chars).collect::<String>();
        out.push_str("...");
        out
    }
}

async fn handle_direct_create_text_file(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let raw_path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_create_text_file args missing path"))?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_create_text_file args missing content"))?;
    if args
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(anyhow!(
            "fs_create_text_file does not support overwrite yet"
        ));
    }
    let create_parent_dirs = args
        .get("create_parent_dirs")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_bytes = policy.max_bytes.unwrap_or(1_048_576).clamp(1, 5_242_880);
    if content.len() as u64 > max_bytes {
        return Err(anyhow!(
            "fs_create_text_file content exceeds policy max_bytes: {} > {}",
            content.len(),
            max_bytes
        ));
    }
    let context = session_context(adapter_state, session_id)?;
    let path = normalize_requested_tool_path(raw_path)?;
    ensure_path_allowed_for_session(context, &path)?;
    ensure_replace_text_path_allowed(&path, policy)?;
    if path.exists() {
        return Err(anyhow!("fs_create_text_file path already exists"));
    }
    if let Some(parent) = path.parent() {
        ensure_path_allowed_for_session(context, parent)?;
        if !parent.exists() {
            if create_parent_dirs {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create parent directories {}", parent.display()))?;
            } else {
                return Err(anyhow!(
                    "fs_create_text_file parent directory does not exist; set create_parent_dirs=true"
                ));
            }
        }
    }
    let started = std::time::Instant::now();
    fs::write(&path, content.as_bytes())
        .with_context(|| format!("create text file {}", path.display()))?;
    let result = json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "bytes": content.len(),
        "created": true,
        "source": "adapter_local",
        "content": format!("Created text file {} ({} bytes)", path.display(), content.len()),
        "policy": {
            "max_bytes": max_bytes,
            "create_files": policy.create_files.unwrap_or(true),
            "deny_hidden_paths": policy.deny_hidden_paths.unwrap_or(true),
            "sensitive_path_policy": policy.sensitive_path_policy,
        }
    });
    eprintln!(
        "bears-acp-adapter: create_text_file session_id={} path={} bytes={} duration_ms={}",
        session_id,
        path.display(),
        content.len(),
        started.elapsed().as_millis(),
    );
    Ok(result)
}

async fn handle_direct_delete_path(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let raw_path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_delete_path args missing path"))?;
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .or(policy.recursive_default)
        .unwrap_or(false);
    let allow_missing = args
        .get("allow_missing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let expected_kind = args
        .get("expected_kind")
        .and_then(Value::as_str)
        .unwrap_or("any");
    let max_entries = policy.max_entries.unwrap_or(100).clamp(1, 1_000);
    let context = session_context(adapter_state, session_id)?;
    let path = normalize_requested_tool_path(raw_path)?;
    ensure_path_allowed_for_session(context, &path)?;
    ensure_delete_path_allowed(context, &path, policy)?;
    let started = std::time::Instant::now();
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && allow_missing => {
            return Ok(json!({
                "ok": true,
                "path": path.to_string_lossy(),
                "deleted": false,
                "missing": true,
                "source": "adapter_local",
                "content": format!("Path {} was already missing.", path.display()),
            }));
        }
        Err(err) => return Err(anyhow!(err).context(format!("stat {}", path.display()))),
    };
    let kind = if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else {
        "other"
    };
    if expected_kind != "any" && expected_kind != kind {
        return Err(anyhow!(
            "fs_delete_path expected kind {expected_kind}, found {kind}"
        ));
    }
    let mut entries = Vec::new();
    if metadata.is_dir() {
        collect_delete_entries(&path, &mut entries, max_entries + 1)?;
        if entries.len() > max_entries {
            return Err(anyhow!(
                "fs_delete_path directory has more than policy max_entries={max_entries}"
            ));
        }
        if !recursive && !entries.is_empty() {
            return Err(anyhow!(
                "fs_delete_path requires recursive=true for non-empty directories"
            ));
        }
        if recursive {
            fs::remove_dir_all(&path)
                .with_context(|| format!("delete directory {}", path.display()))?;
        } else {
            fs::remove_dir(&path)
                .with_context(|| format!("delete directory {}", path.display()))?;
        }
    } else if metadata.is_file() {
        fs::remove_file(&path).with_context(|| format!("delete file {}", path.display()))?;
    } else {
        return Err(anyhow!("fs_delete_path only deletes files or directories"));
    }
    let content = format!(
        "Deleted {kind} {}{}",
        path.display(),
        if metadata.is_dir() {
            format!(" ({} entries)", entries.len())
        } else {
            String::new()
        }
    );
    eprintln!(
        "bears-acp-adapter: delete_path session_id={} path={} kind={} recursive={} entries={} duration_ms={}",
        session_id,
        path.display(),
        kind,
        recursive,
        entries.len(),
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "deleted": true,
        "kind": kind,
        "recursive": recursive,
        "entries": entries.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "source": "adapter_local",
        "content": content,
        "policy": {
            "max_entries": max_entries,
            "deny_hidden_paths": policy.deny_hidden_paths.unwrap_or(true),
            "sensitive_path_policy": policy.sensitive_path_policy,
        }
    }))
}

fn ensure_delete_path_allowed(
    context: &SessionContext,
    path: &Path,
    policy: &ToolPolicy,
) -> Result<()> {
    let roots = if context.roots.is_empty() {
        vec![PathBuf::from(&context.cwd)]
    } else {
        context.roots.iter().map(PathBuf::from).collect::<Vec<_>>()
    };
    if roots.iter().any(|root| path == root) {
        return Err(anyhow!("fs_delete_path refuses to delete a workspace root"));
    }
    if path.parent().is_none() {
        return Err(anyhow!("fs_delete_path refuses to delete filesystem root"));
    }
    if policy.sensitive_path_policy.as_deref() == Some("deny_sensitive_paths")
        && is_sensitive_path(path)
    {
        return Err(anyhow!(
            "fs_delete_path denied sensitive path {}",
            path.display()
        ));
    }
    if policy.deny_hidden_paths.unwrap_or(true) && is_hidden_path_component(path, Path::new("/")) {
        return Err(anyhow!(
            "fs_delete_path denied hidden path {}",
            path.display()
        ));
    }
    Ok(())
}

fn collect_delete_entries(path: &Path, out: &mut Vec<PathBuf>, limit: usize) -> Result<()> {
    if out.len() >= limit {
        return Ok(());
    }
    for entry in fs::read_dir(path).with_context(|| format!("scan directory {}", path.display()))? {
        let entry = entry?;
        let path = entry.path();
        out.push(path.clone());
        if out.len() >= limit {
            return Ok(());
        }
        if entry.metadata().map(|m| m.is_dir()).unwrap_or(false) {
            collect_delete_entries(&path, out, limit)?;
        }
    }
    Ok(())
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

fn ensure_replace_text_path_allowed(path: &Path, policy: &ToolPolicy) -> Result<()> {
    if policy.sensitive_path_policy.as_deref() == Some("deny_sensitive_paths")
        && is_sensitive_path(path)
    {
        return Err(anyhow!(
            "fs_replace_text denied sensitive path {}",
            path.display()
        ));
    }
    if policy.deny_hidden_paths.unwrap_or(true) && is_hidden_path_component(path, Path::new("/")) {
        return Err(anyhow!(
            "fs_replace_text denied hidden path {}",
            path.display()
        ));
    }
    Ok(())
}

fn is_sensitive_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    if path_str.contains("/.git/") || path_str.ends_with("/.git") {
        return true;
    }
    path.components().any(|component| {
        let Some(part) = component.as_os_str().to_str() else {
            return false;
        };
        let lower = part.to_ascii_lowercase();
        lower == ".env"
            || lower.starts_with(".env.")
            || lower.contains("id_rsa")
            || lower.contains("id_ed25519")
            || lower.contains("private_key")
            || lower.contains("secret")
            || lower.contains("token")
            || lower.ends_with(".pem")
            || lower.ends_with(".key")
    })
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

fn collect_search_files(
    context: &SessionContext,
    root: &Path,
    path: &Path,
    include_hidden: bool,
    filters: &SearchFilters,
    max_files: usize,
    truncated: &mut bool,
    skipped_by_filter: &mut usize,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    if *truncated {
        return Ok(());
    }
    if !include_hidden && is_hidden_path_component(path, root) {
        return Ok(());
    }
    ensure_path_allowed_for_session(context, path)?;
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if metadata.is_file() {
        if !search_file_passes_filters(root, path, filters) {
            *skipped_by_filter += 1;
            return Ok(());
        }
        if out.len() >= max_files {
            *truncated = true;
        } else {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in
        fs::read_dir(path).with_context(|| format!("search directory {}", path.display()))?
    {
        let entry = entry?;
        collect_search_files(
            context,
            root,
            &entry.path(),
            include_hidden,
            filters,
            max_files,
            truncated,
            skipped_by_filter,
            out,
        )?;
        if *truncated {
            break;
        }
    }
    Ok(())
}

fn search_filters_from_args(args: &Value) -> Result<SearchFilters> {
    let case_sensitive = args
        .get("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let extensions = args
        .get("extensions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(normalize_extension)
                .filter(|s| !s.is_empty())
                .take(10)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(SearchFilters {
        case_sensitive,
        pattern,
        extensions,
    })
}

fn normalize_extension(raw: &str) -> String {
    raw.trim().trim_start_matches('.').to_ascii_lowercase()
}

fn search_file_passes_filters(root: &Path, path: &Path, filters: &SearchFilters) -> bool {
    if !filters.extensions.is_empty() {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default();
        if !filters.extensions.iter().any(|allowed| allowed == &ext) {
            return false;
        }
    }
    if let Some(pattern) = filters.pattern.as_deref() {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if !wildcard_match(pattern, &relative) {
            return false;
        }
    }
    true
}

fn line_matches_query(line: &str, query: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        line.contains(query)
    } else {
        line.to_lowercase().contains(&query.to_lowercase())
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut p = 0usize;
    let mut t = 0usize;
    let mut star = None;
    let mut match_after_star = 0usize;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            match_after_star = t;
            p += 1;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            match_after_star += 1;
            t = match_after_star;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn is_hidden_path_component(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|s| s.starts_with('.') && s != "." && s != "..")
        })
}

fn format_directory_listing(path: &Path, entries: &[Value], truncated: bool) -> String {
    let mut lines = vec![format!("Directory listing for {}", path.display())];
    for entry in entries {
        let kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let entry_path = entry.get("path").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("{kind}\t{entry_path}"));
    }
    if truncated {
        lines.push("... truncated".to_string());
    }
    lines.join("\n")
}

fn format_search_results(query: &str, matches: &[Value], truncated: bool) -> String {
    let mut lines = vec![format!("Search results for {query:?}")];
    for item in matches {
        let path = item.get("path").and_then(Value::as_str).unwrap_or("");
        let line = item.get("line").and_then(Value::as_u64).unwrap_or(0);
        let preview = item.get("preview").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("{path}:{line}: {preview}"));
    }
    if matches.is_empty() {
        lines.push("No matches found.".to_string());
    }
    if truncated {
        lines.push("... truncated".to_string());
    }
    lines.join("\n")
}

fn normalize_requested_tool_path(path: &str) -> Result<PathBuf> {
    let path = file_uri_or_path_to_path(path).ok_or_else(|| anyhow!("path must not be empty"))?;
    if !is_absolute_local_path(&path) {
        return Err(anyhow!(
            "tool path must be an absolute local path; got {path:?}"
        ));
    }
    Ok(PathBuf::from(path))
}

fn ensure_path_allowed_for_session(context: &SessionContext, path: &Path) -> Result<()> {
    let roots = if context.roots.is_empty() {
        vec![context.cwd.as_str()]
    } else {
        context.roots.iter().map(String::as_str).collect::<Vec<_>>()
    };
    let allowed = roots.iter().any(|root| {
        let root_path = Path::new(root);
        path == root_path || path.starts_with(root_path)
    });
    if allowed {
        Ok(())
    } else {
        Err(anyhow!(
            "tool path {} is outside the ACP session workspace roots",
            path.display()
        ))
    }
}

fn token_env_for_auth_method() -> String {
    env::var("BEARS_DEN_TOKEN_ENV")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "BEARS_DEN_TOKEN".to_string())
}

fn initialize_result() -> Result<Value> {
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
    let auth = AuthMethod::EnvVar(
        AuthMethodEnvVar::new(
            "bears_den_token",
            "BEARS Code Token",
            vec![AuthEnvVar::new(token_env_for_auth_method())
                .label(Some("BEARS Code Token".to_string()))
                .secret(true)],
        )
        .description(Some(
            "Paste a Den ACP token. Tokens include the acp:chat scope.".to_string(),
        ))
        .link(Some("https://github.com/silarsis/BEARS".to_string())),
    );
    Ok(serde_json::to_value(
        InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(capabilities)
            .agent_info(Some(info))
            .auth_methods(vec![auth]),
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
            .get("resolved_conversation_id")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
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

async fn fetch_conversation_history_chronological(
    http: &reqwest::Client,
    config: &Config,
    conversation_id: &str,
) -> Result<Vec<(String, String)>> {
    let mut chunks: Vec<Vec<(String, String)>> = Vec::new();
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
            page.push((role.to_string(), text.to_string()));
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
        for (role, text) in messages {
            match role.as_str() {
                "user" => send_user_message_chunk(session_id, &text).await?,
                "assistant" => send_agent_message_chunk(session_id, &text).await?,
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
) -> Result<()> {
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
    if let Some(den) = den.as_ref() {
        replay_history_for_den_session(http, config, session_id, den, "session/resume").await?;
    }
    Ok(())
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

    if let Some(den) = den.as_ref() {
        replay_history_for_den_session(http, config, session_id, den, "session/load").await?;
    }

    write_response(response_id, Ok(session_lifecycle_result()?)).await?;
    Ok(())
}

fn session_lifecycle_result() -> Result<Value> {
    Ok(serde_json::to_value(LoadSessionResponse::new())?)
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
    let _ = shared_state.cancellation_tx.send(session_id.to_string());
    post_session_lifecycle_action(http, config, session_id, "cancel").await
}

async fn post_session_lifecycle_action(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    action: &str,
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

async fn handle_prompt(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response_id: Value,
    params: Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/prompt params missing sessionId"))?;
    let prompt = prompt_text_from_params(&params)?;
    let display_prompt = prompt_display_text_from_params(&params).unwrap_or_else(|| prompt.clone());
    let mut client_context = adapter_state
        .session_contexts
        .get(session_id)
        .cloned()
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

    send_user_message_chunk(session_id, &display_prompt).await?;

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
        "client_capabilities": adapter_state.client_capabilities,
        "client_context": client_context.raw,
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
        return Err(anyhow!("{}", den_status_error_message(status, text.trim())));
    }

    let mut stream_diagnostics = SseStreamDiagnostics::default();
    let mut saw_done = false;
    let mut saw_visible_output = false;
    let mut saw_tool_activity = false;
    let mut saw_error = false;
    let mut upstream_errors = Vec::new();
    let mut buffer = Vec::<u8>::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read Den SSE chunk")?;
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
        upstream_errors.extend(outcome.upstream_errors);
    }

    if !upstream_errors.is_empty() {
        if saw_visible_output {
            eprintln!(
                "bears-acp-adapter: ignoring upstream error after visible output: {}",
                upstream_errors.join("; ")
            );
        } else {
            return Err(anyhow!(
                "BEARS upstream stream reported error: {}",
                upstream_errors.join("; ")
            ));
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
    capabilities
}

fn capability_bool(capabilities: &Value, pointers: &[&str]) -> bool {
    pointers.iter().any(|pointer| {
        capabilities
            .pointer(pointer)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
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

fn file_uri_or_path_to_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.starts_with("file://") {
        return Some(trimmed.to_string());
    }
    let without_scheme = trimmed.trim_start_matches("file://");
    #[cfg(windows)]
    let path = without_scheme.trim_start_matches('/').to_string();
    #[cfg(not(windows))]
    let path = format!("/{}", without_scheme.trim_start_matches('/'));
    Some(percent_decode_file_path(&path))
}

fn percent_decode_file_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| path.to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn prompt_display_text_from_params(params: &Value) -> Option<String> {
    prompt_text_blocks_from_params(params).ok()
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
                            parts.push(format!("Resource {uri}:\n{text}"));
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
            outcome.upstream_errors.push(format_den_event_error(&event));
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
                | "conversation_resolved"
                | "turn_complete"
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
            client_capabilities: Value::Null,
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
    let replace_plan = if tool_name == "fs_replace_text" {
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
    let approval_reused = if let Some(context) = context_for_approval.as_ref() {
        approval_cache.is_allowed(context, tool_name).await
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
                approval_cache
                    .remember(context, tool_name, policy.risk())
                    .await;
                eprintln!(
                    "bears-acp-adapter: approval_remembered session_id={} tool_name={} scope=workspace_roots",
                    session_id, tool_name
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
) -> Result<PermissionDecision> {
    let path = event
        .get("args")
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("the requested file");
    let title = event
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_else(|| tool_display(tool_name).title);
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
        .title(Some(title.to_string()))
        .content(Some(content));
    if let Some(locations) = tool_locations_from_event(event) {
        fields = fields.locations(Some(locations));
    }
    if let Some(args) = event.get("args") {
        fields = fields.raw_input(Some(args.clone()));
    }
    let tool_call = ToolCallUpdate::new(tool_call_id.to_string(), fields);
    let permission_class = permission_class_for_tool(tool_name);
    let allow_always_label = match permission_class {
        "read_files" => "Always allow reading files in this workspace",
        "edit_files" => "Always allow editing files in this workspace",
        "delete_files" => "Always allow deleting files in this workspace",
        _ => "Always allow matching local file operations",
    };
    let reject_always_label = match permission_class {
        "read_files" => "Always deny reading files in this workspace",
        "edit_files" => "Always deny editing files in this workspace",
        "delete_files" => "Always deny deleting files in this workspace",
        _ => "Always deny matching local file operations",
    };
    let request = RequestPermissionRequest::new(
        session_id.to_string(),
        tool_call,
        vec![
            PermissionOption::new("allow", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new(
                "allow_always",
                allow_always_label,
                PermissionOptionKind::AllowAlways,
            ),
            PermissionOption::new("reject", "Deny once", PermissionOptionKind::RejectOnce),
            PermissionOption::new(
                "reject_always",
                reject_always_label,
                PermissionOptionKind::RejectAlways,
            ),
        ],
    );
    let response = adapter_state
        .transport
        .request(
            "session/request_permission",
            serde_json::to_value(request)?,
            std::time::Duration::from_millis(policy.permission_timeout_ms.unwrap_or(120_000)),
        )
        .await?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("permission request failed: {error}"));
    }
    let result = response.get("result").cloned().unwrap_or(Value::Null);
    let decision = parse_permission_decision(&result)?;
    if decision.approved {
        Ok(decision)
    } else {
        Err(anyhow!("permission denied for {tool_name} on {path}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PermissionDecision {
    approved: bool,
    remember: bool,
}

fn parse_permission_decision(result: &Value) -> Result<PermissionDecision> {
    if let Ok(response) = serde_json::from_value::<RequestPermissionResponse>(result.clone()) {
        return Ok(match response.outcome {
            RequestPermissionOutcome::Selected(selected) => {
                let id = selected.option_id.to_string();
                PermissionDecision {
                    approved: matches!(
                        id.as_str(),
                        "allow" | "allow_always" | "approve" | "approved" | "yes"
                    ),
                    remember: matches!(id.as_str(), "allow_always"),
                }
            }
            RequestPermissionOutcome::Cancelled => PermissionDecision {
                approved: false,
                remember: false,
            },
            _ => PermissionDecision {
                approved: false,
                remember: false,
            },
        });
    }
    let approved = result
        .get("approved")
        .or_else(|| result.get("approve"))
        .or_else(|| result.get("granted"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            // Some clients answer `{}` after applying their own auto-approval policy.
            result.is_object()
        });
    Ok(PermissionDecision {
        approved,
        remember: false,
    })
}

#[allow(dead_code)]
fn parse_permission_approved(result: &Value) -> Result<bool> {
    Ok(parse_permission_decision(result)?.approved)
}

async fn post_tool_result(
    config: &Config,
    session_id: &str,
    tool_call_id: &str,
    payload: Value,
) -> Result<()> {
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
        return Err(anyhow!(
            "Den tool result endpoint returned HTTP {status}: {}",
            body.trim()
        ));
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
                send_agent_thought_chunk(session_id, text).await?;
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

        "turn_complete" => Ok(true),
        _ => Ok(false),
    }
}

struct ToolDisplay {
    title: &'static str,
    kind: ToolKind,
    verb: &'static str,
}

fn tool_display(tool_name: &str) -> ToolDisplay {
    match tool_name {
        "fs_read_text_file" | "fs.read_text_file" => ToolDisplay {
            title: "Read file",
            kind: ToolKind::Read,
            verb: "Reading",
        },
        "fs_list_directory" => ToolDisplay {
            title: "List directory",
            kind: ToolKind::Read,
            verb: "Listing",
        },
        "fs_search_files" => ToolDisplay {
            title: "Search files",
            kind: ToolKind::Search,
            verb: "Searching",
        },
        "fs_replace_text" => ToolDisplay {
            title: "Edit file",
            kind: ToolKind::Edit,
            verb: "Editing",
        },
        "fs_create_text_file" => ToolDisplay {
            title: "Create file",
            kind: ToolKind::Edit,
            verb: "Creating",
        },
        "fs_delete_path" => ToolDisplay {
            title: "Delete path",
            kind: ToolKind::Delete,
            verb: "Deleting",
        },
        _ => ToolDisplay {
            title: "Local tool",
            kind: ToolKind::Read,
            verb: "Running",
        },
    }
}

fn tool_path(event: &Value) -> Option<&str> {
    event
        .get("args")
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
}

fn tool_locations_from_event(event: &Value) -> Option<Vec<ToolCallLocation>> {
    let path = tool_path(event)?;
    let mut location = ToolCallLocation::new(PathBuf::from(path));
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
    let mut tool_call = ToolCall::new(tool_call_id.to_string(), display.title)
        .kind(display.kind)
        .status(tool_status_from_str(status))
        .content(content);
    if let Some(event) = event {
        if let Some(locations) = tool_locations_from_event(event) {
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

async fn write_json(value: Value) -> Result<()> {
    let mut stdout = io::stdout();
    let line = serde_json::to_string(&value)?;
    stdout.write_all(line.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

fn auth_challenge_error(data: Option<Value>) -> Value {
    json_rpc_error(-32000, "Authentication required", data)
}

fn configuration_error(data: Option<Value>) -> Value {
    json_rpc_error(-32010, "BEARS configuration incomplete", data)
}

fn token_validation_error(data: Option<Value>) -> Value {
    json_rpc_error(-32011, "BEARS Code token validation failed", data)
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
    fn tool_display_uses_specific_titles() {
        assert_eq!(tool_display("fs_read_text_file").title, "Read file");
        assert_eq!(tool_display("fs_list_directory").title, "List directory");
        assert_eq!(tool_display("fs_search_files").title, "Search files");
        assert_eq!(tool_display("fs_replace_text").title, "Edit file");
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

    #[test]
    fn wildcard_match_supports_star_and_question_mark() {
        assert!(wildcard_match("src/*.rs", "src/lib.rs"));
        assert!(wildcard_match("src/lib.?s", "src/lib.rs"));
        assert!(!wildcard_match("src/*.rs", "tests/lib.rs"));
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

    #[tokio::test]
    async fn json_rpc_transport_routes_matching_response() {
        let transport = JsonRpcTransport::default();
        let id = json!("req-test");
        let key = id_key(&id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        transport.pending_responses.lock().await.insert(key, tx);
        assert!(
            transport
                .route_response(&id, json!({ "id": "req-test", "result": { "ok": true } }))
                .await
        );
        let routed = rx.await.unwrap();
        assert_eq!(routed["result"]["ok"], true);
    }

    #[tokio::test]
    async fn json_rpc_transport_reports_unmatched_response() {
        let transport = JsonRpcTransport::default();
        assert!(
            !transport
                .route_response(&json!("missing"), json!({ "id": "missing" }))
                .await
        );
    }

    #[tokio::test]
    async fn approval_cache_remembers_persistent_scope() {
        let cache = ApprovalCache {
            entries: Arc::new(TokioMutex::new(HashMap::new())),
            persistence: Some(ApprovalPersistence {
                path: env::temp_dir()
                    .join(format!("bears-acp-approval-test-{}.json", Uuid::new_v4())),
                api_url: "http://den.test".to_string(),
                bear: "meta".to_string(),
                client: "zed".to_string(),
            }),
        };
        let context = SessionContext {
            cwd: "/workspace".to_string(),
            roots: vec!["/workspace".to_string()],
            ..Default::default()
        };
        assert!(!cache.is_allowed(&context, "fs_read_text_file").await);
        cache
            .remember(&context, "fs_list_directory", "read_only")
            .await;
        assert!(cache.is_allowed(&context, "fs_read_text_file").await);
        assert!(cache.is_allowed(&context, "fs_search_files").await);
        assert!(!cache.is_allowed(&context, "fs_replace_text").await);
        cache.clear_session("session-1").await;
        assert!(cache.is_allowed(&context, "fs_read_text_file").await);
    }

    #[test]
    fn approval_ttl_matches_product_policy() {
        assert_eq!(approval_ttl_secs("writes_workspace"), 7 * 24 * 60 * 60);
        assert_eq!(approval_ttl_secs("deletes_workspace"), 7 * 24 * 60 * 60);
        assert_eq!(approval_ttl_secs("read_only"), 28 * 24 * 60 * 60);
    }

    #[test]
    fn parse_permission_decision_remembers_allow_always() {
        let result = json!({
            "outcome": {
                "outcome": "selected",
                "optionId": "allow_always"
            }
        });
        let decision = parse_permission_decision(&result).unwrap();
        assert!(decision.approved);
        assert!(decision.remember);
    }

    #[tokio::test]
    async fn tool_task_registry_tracks_phase_and_session_entries() {
        let registry = ToolTaskRegistry::default();
        registry
            .register("session-1", "call-1", "fs_list_directory")
            .await;
        registry
            .set_phase(
                "session-1",
                "call-1",
                "fs_list_directory",
                ToolTaskPhase::PermissionRequested,
            )
            .await;
        let items = registry.list_for_session("session-1").await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].phase, ToolTaskPhase::PermissionRequested);
        assert_eq!(items[0].tool_name, "fs_list_directory");
        assert!(items[0].updated_at >= items[0].started_at);
        let removed = registry.remove("session-1", "call-1").await.unwrap();
        assert_eq!(removed.tool_call_id, "call-1");
        assert!(registry.list_for_session("session-1").await.is_empty());
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
    fn parses_typed_acp_permission_selection() {
        let result = json!({
            "outcome": {
                "outcome": "selected",
                "optionId": "allow"
            }
        });
        assert!(parse_permission_approved(&result).unwrap());
    }

    #[test]
    fn parses_typed_acp_permission_cancelled() {
        let result = json!({
            "outcome": {
                "outcome": "cancelled"
            }
        });
        assert!(!parse_permission_approved(&result).unwrap());
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
