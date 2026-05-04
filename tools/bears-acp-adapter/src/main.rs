use agent_client_protocol::schema::{
    AgentCapabilities, AuthEnvVar, AuthMethod, AuthMethodEnvVar, ContentBlock, ContentChunk,
    Implementation, InitializeResponse, ListSessionsResponse, LoadSessionResponse, McpCapabilities,
    NewSessionResponse, PermissionOption, PermissionOptionKind, PromptCapabilities, PromptResponse,
    ProtocolVersion, ReadTextFileRequest, ReadTextFileResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SessionCapabilities,
    SessionCloseCapabilities, SessionInfo, SessionListCapabilities, SessionResumeCapabilities,
    SessionUpdate, StopReason, ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind,
};
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Url;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    process::Command,
};
use tokio::{
    io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
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
    api_url: String,
    bear: String,
    token_env: String,
    client: String,
}

#[derive(Default)]
struct AdapterState {
    client_capabilities: Value,
    session_contexts: HashMap<String, SessionContext>,
    pending_responses: HashMap<String, tokio::sync::oneshot::Sender<Value>>,
}

#[derive(Clone, Debug, Default)]
struct SessionContext {
    cwd: String,
    roots: Vec<String>,
    raw: Value,
    conversation_id: Option<String>,
    resolved_conversation_id: Option<String>,
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
    saw_assistant_output: bool,
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
            "frames={}, events={}, event_types={:?}, unknown_samples={:?}, saw_turn_complete={}, saw_assistant_output={}, saw_error={}",
            self.frames,
            self.events,
            self.event_types,
            self.unknown_event_samples,
            self.saw_turn_complete,
            self.saw_assistant_output,
            self.saw_error,
        )
    }
}

#[derive(Default)]
struct SseFrameOutcome {
    saw_done: bool,
    saw_assistant_output: bool,
    saw_error: bool,
    upstream_errors: Vec<String>,
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
        "bears-acp-adapter: starting version={} build_git_sha={} local_head_sha={} ACP sessions=list/resume/load supported",
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        local_head_sha()
    );
    if runtime.is_configured() {
        eprintln!("bears-acp-adapter: configuration looks valid");
    } else {
        eprintln!("{}", runtime.configuration_error_message());
    }

    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .context("build HTTP client")?;

    if runtime.check_server {
        let Some(config) = runtime.config.as_ref() else {
            return Err(anyhow!(runtime.configuration_error_message()));
        };
        check_server_version(&http, config).await?;
        return Ok(());
    }

    let (inbound_tx, mut inbound_rx) = mpsc::channel::<InboundMessage>(128);
    tokio::spawn(read_stdin_messages(inbound_tx));

    let mut adapter_state = AdapterState::default();

    while let Some(message) = inbound_rx.recv().await {
        let value = match message {
            InboundMessage::Request(value) => value,
            InboundMessage::Response { id, value } => {
                if let Some(tx) = adapter_state.pending_responses.remove(&id_key(&id)) {
                    let _ = tx.send(value);
                } else {
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

        if let Err(err) = handle_request(&http, &mut runtime, &mut adapter_state, request).await {
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
        "bears-acp-adapter {}\nBuild git SHA: {}\nLocal HEAD SHA: {}\nACP sessions: list/resume/load; conversations bound via Den",
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA"),
        local_head_sha()
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
Usage: bears-acp-adapter --api-url <url> --bear <slug> [--client zed] [--token-env BEARS_DEN_TOKEN]\n\n\
Options:\n  --api-url <url>        Den API origin, for example https://api.bears.example\n  --bear <slug>          Bear slug to chat with\n  --token <token>        Den ACP token with acp:chat scope\n  --token-env <env-var>  Read the Den bearer token from this environment variable\n  --client <name>        Client label: zed, opencode, or acp_adapter\n  --check-config         Validate configuration and exit without starting ACP stdio\n  --check-server         Fetch Den /version and exit without starting ACP stdio\n  --version              Show version/build behavior and exit\n  --help                 Show this help\n\n\
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

async fn read_stdin_messages(tx: mpsc::Sender<InboundMessage>) {
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
                        let message = if value.get("method").and_then(Value::as_str).is_some() {
                            InboundMessage::Request(value)
                        } else if let Some(id) = value.get("id").cloned() {
                            InboundMessage::Response { id, value }
                        } else {
                            InboundMessage::Request(value)
                        };
                        if tx.send(message).await.is_err() {
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
                match handle_direct_read_text_file(adapter_state, request.params).await {
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
                    Ok(()) => write_response(id, Ok(json!({}))).await?,
                    Err(err) => {
                        write_response(
                            id,
                            Err(auth_required_error(Some(
                                json!({ "message": format!("{err:#}") }),
                            ))),
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
                    "bears-acp-adapter: session/new session_id={} cwd={} roots={}",
                    session_id,
                    context.cwd,
                    context.roots.join(",")
                );
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
                        Err(auth_required_error(Some(json!({
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
                        Err(auth_required_error(Some(json!({
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
                        Err(auth_required_error(Some(json!({
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
                        Err(auth_required_error(Some(json!({
                            "message": format!("BEARS Code token authentication failed: {err:#}"),
                        })))),
                    )
                    .await?;
                    return Ok(());
                }
                match restore_session_from_den(http, config, adapter_state, &request.params).await {
                    Ok(()) => write_response(id, Ok(session_lifecycle_result()?)).await?,
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
                        Err(auth_required_error(Some(json!({
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
                        Err(auth_required_error(Some(json!({
                            "message": format!("BEARS Code token authentication failed: {err:#}"),
                        })))),
                    )
                    .await?;
                    return Ok(());
                }
                match handle_session_load(http, config, adapter_state, id.clone(), &request.params)
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
                        Err(auth_required_error(Some(json!({
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
                        Err(auth_required_error(Some(json!({
                            "message": format!("BEARS Code token authentication failed: {err:#}"),
                            "hint": "Generate a fresh Den Code token for this bear. Code tokens must include acp:chat."
                        })))),
                    )
                    .await?;
                    return Ok(());
                }

                if let Err(err) =
                    handle_prompt(http, config, adapter_state, id.clone(), request.params).await
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
                        Err(auth_required_error(Some(json!({
                            "message": runtime.configuration_error_message(),
                            "problems": runtime.diagnostics,
                        })))),
                    )
                    .await?;
                    return Ok(());
                };
                match handle_session_close(http, config, request.params).await {
                    Ok(()) => write_response(id, Ok(json!({}))).await?,
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
                        Err(auth_required_error(Some(json!({
                            "message": runtime.configuration_error_message(),
                            "problems": runtime.diagnostics,
                        })))),
                    )
                    .await?;
                    return Ok(());
                };
                match handle_session_cancel(http, config, request.params).await {
                    Ok(()) => write_response(id, Ok(json!({}))).await?,
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
        "direct_tools": {
            "fs_read_text_file": true,
        },
    });
    Ok(SessionContext {
        cwd,
        roots,
        raw,
        conversation_id: None,
        resolved_conversation_id: None,
    })
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
    let response = send_request_and_wait(
        adapter_state,
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
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(400)
        .clamp(1, 2_000) as usize;
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
    }))
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
        "direct_tools": {
            "fs_read_text_file": true,
        },
        "den_acp_session": den_session.clone(),
    });
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
        return Err(anyhow!(
            "Den get session returned HTTP {status}: {}",
            body.trim()
        ));
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
    params: &Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session params missing sessionId"))?;
    let den = den_get_acp_session(http, config, session_id).await?;
    let context = session_context_from_den_session(params, &den)?;
    adapter_state
        .session_contexts
        .insert(session_id.to_string(), context);
    replay_history_for_den_session(http, config, session_id, &den, "session/resume").await?;
    Ok(())
}

async fn handle_session_load(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    response_id: Value,
    params: &Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/load params missing sessionId"))?;
    let den = den_get_acp_session(http, config, session_id).await?;
    let context = session_context_from_den_session(params, &den)?;
    adapter_state
        .session_contexts
        .insert(session_id.to_string(), context);

    replay_history_for_den_session(http, config, session_id, &den, "session/load").await?;

    write_response(response_id, Ok(session_lifecycle_result()?)).await?;
    Ok(())
}

fn session_lifecycle_result() -> Result<Value> {
    Ok(serde_json::to_value(LoadSessionResponse::new())?)
}

async fn handle_session_close(
    http: &reqwest::Client,
    config: &Config,
    params: Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/close params missing sessionId"))?;
    post_session_lifecycle_action(http, config, session_id, "close").await
}

async fn handle_session_cancel(
    http: &reqwest::Client,
    config: &Config,
    params: Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/cancel params missing sessionId"))?;
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
    response_id: Value,
    params: Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/prompt params missing sessionId"))?;
    let prompt = prompt_text_from_params(&params)?;
    let display_prompt = prompt_display_text_from_params(&params).unwrap_or_else(|| prompt.clone());
    let client_context = adapter_state
        .session_contexts
        .get(session_id)
        .cloned()
        .unwrap_or_default();
    let conversation_id = client_context
        .resolved_conversation_id
        .as_deref()
        .or(client_context.conversation_id.as_deref())
        .map(str::to_string);
    let conversation_log = conversation_id.as_deref().unwrap_or("<den-selected>");
    eprintln!(
        "bears-acp-adapter: session/prompt session_id={} bear={} conversation_id={} client={}",
        session_id, config.bear, conversation_log, config.client
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
    let mut saw_assistant_output = false;
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
                session_id,
                &frame,
                &mut stream_diagnostics,
            )
            .await?;
            saw_done |= outcome.saw_done;
            saw_assistant_output |= outcome.saw_assistant_output;
            saw_error |= outcome.saw_error;
            upstream_errors.extend(outcome.upstream_errors);
        }
    }
    if !buffer.is_empty() {
        let frame = std::mem::take(&mut buffer);
        let outcome = handle_sse_frame(
            config,
            adapter_state,
            session_id,
            &frame,
            &mut stream_diagnostics,
        )
        .await?;
        saw_done |= outcome.saw_done;
        saw_assistant_output |= outcome.saw_assistant_output;
        saw_error |= outcome.saw_error;
        upstream_errors.extend(outcome.upstream_errors);
    }

    if !upstream_errors.is_empty() {
        if saw_assistant_output {
            eprintln!(
                "bears-acp-adapter: ignoring upstream error after assistant output: {}",
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
    if !saw_assistant_output && !saw_error {
        return Err(anyhow!(
            "BEARS ACP stream completed without assistant output or an error. Diagnostics: {}",
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
        if ty == "assistant_text_delta" {
            let text = event.get("text").and_then(Value::as_str).unwrap_or("");
            outcome.saw_assistant_output |= !text.is_empty();
        } else if ty == "error" {
            outcome.saw_error = true;
            diagnostics.saw_error = true;
            outcome.upstream_errors.push(format_den_event_error(&event));
        }
        let handled = handle_den_event(config, adapter_state, session_id, &event).await?;
        outcome.saw_done |= handled;
        diagnostics.saw_turn_complete |= handled;
        diagnostics.saw_assistant_output |= outcome.saw_assistant_output;
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

async fn handle_tool_request_event(
    config: &Config,
    adapter_state: &mut AdapterState,
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
    send_tool_call_update(session_id, tool_call_id, "pending", "Preparing local tool").await?;
    if event
        .get("approval")
        .and_then(|v| v.get("required"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        send_tool_call_update(
            session_id,
            tool_call_id,
            "pending",
            "Waiting for permission",
        )
        .await?;
        request_tool_permission(adapter_state, session_id, tool_call_id, tool_name, event).await?;
    }
    send_tool_call_update(session_id, tool_call_id, "pending", "Running local tool").await?;
    let started = std::time::Instant::now();
    let result = match tool_name {
        "fs_read_text_file" | "fs.read_text_file" => {
            let args = event.get("args").cloned().unwrap_or_else(|| json!({}));
            if client_supports_read_text_file(adapter_state) {
                handle_client_read_text_file(adapter_state, session_id, &args).await
            } else {
                eprintln!(
                    "bears-acp-adapter: client did not advertise fs/read_text_file; using adapter-local fallback"
                );
                let mut params = args;
                params["sessionId"] = json!(session_id);
                handle_direct_read_text_file(adapter_state, params).await
            }
        }
        _ => Err(anyhow!(
            "unsupported Den tool_request tool_name {tool_name}"
        )),
    };
    let status;
    let mut payload = json!({
        "turn_id": event.get("turn_id").and_then(Value::as_str),
        "request_id": event.get("request_id").and_then(Value::as_str),
        "tool_call_id": tool_call_id,
        "approval_request_id": event.get("approval_request_id").and_then(Value::as_str),
        "tool_name": tool_name,
        "diagnostic": {
            "adapter_version": env!("CARGO_PKG_VERSION"),
            "duration_ms": started.elapsed().as_millis(),
        }
    });
    match result {
        Ok(value) => {
            status = "ok";
            payload["status"] = json!(status);
            payload["content"] = value.get("content").cloned().unwrap_or_else(|| json!(""));
            payload["structured_content"] = value;
            send_tool_call_update(
                session_id,
                tool_call_id,
                "completed",
                "Local tool completed",
            )
            .await?;
        }
        Err(err) => {
            status = "error";
            payload["status"] = json!(status);
            payload["content"] = json!(format!("{err:#}"));
            send_tool_call_update(session_id, tool_call_id, "failed", &format!("{err:#}")).await?;
        }
    }
    post_tool_result(config, session_id, tool_call_id, payload).await?;
    Ok(())
}

async fn request_tool_permission(
    adapter_state: &mut AdapterState,
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    event: &Value,
) -> Result<()> {
    let path = event
        .get("args")
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("the requested file");
    let title = event
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Run local tool");
    let reason = event
        .get("approval")
        .and_then(|v| v.get("reason"))
        .and_then(Value::as_str)
        .unwrap_or("Letta requested approval before running this local ACP tool.");
    eprintln!(
        "bears-acp-adapter: requesting permission session_id={} tool_call_id={} tool_name={} path={}",
        session_id, tool_call_id, tool_name, path
    );
    let fields = ToolCallUpdateFields::new()
        .kind(Some(ToolKind::Read))
        .status(Some(ToolCallStatus::Pending))
        .title(Some(title.to_string()))
        .content(Some(vec![ToolCallContent::from(format!(
            "{reason}\n\nTool: {tool_name}\nPath: {path}"
        ))]));
    let tool_call = ToolCallUpdate::new(tool_call_id.to_string(), fields);
    let request = RequestPermissionRequest::new(
        session_id.to_string(),
        tool_call,
        vec![
            PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
            PermissionOption::new("reject", "Deny", PermissionOptionKind::RejectOnce),
        ],
    );
    let response = send_request_and_wait(
        adapter_state,
        "session/request_permission",
        serde_json::to_value(request)?,
        std::time::Duration::from_secs(120),
    )
    .await?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("permission request failed: {error}"));
    }
    let result = response.get("result").cloned().unwrap_or(Value::Null);
    let approved = parse_permission_approved(&result)?;
    if approved {
        Ok(())
    } else {
        Err(anyhow!("permission denied for {tool_name} on {path}"))
    }
}

fn parse_permission_approved(result: &Value) -> Result<bool> {
    if let Ok(response) = serde_json::from_value::<RequestPermissionResponse>(result.clone()) {
        return Ok(match response.outcome {
            RequestPermissionOutcome::Selected(selected) => {
                let id = selected.option_id.to_string();
                matches!(id.as_str(), "allow" | "approve" | "approved" | "yes")
            }
            RequestPermissionOutcome::Cancelled => false,
            _ => false,
        });
    }
    Ok(result
        .get("approved")
        .or_else(|| result.get("approve"))
        .or_else(|| result.get("granted"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            // Some clients answer `{}` after applying their own auto-approval policy.
            result.is_object()
        }))
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
            handle_tool_request_event(config, adapter_state, session_id, event).await?;
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
    status: &str,
    text: &str,
) -> Result<()> {
    let tool_call = ToolCall::new(tool_call_id.to_string(), "Read file")
        .kind(ToolKind::Read)
        .status(tool_status_from_str(status))
        .content(vec![ToolCallContent::from(text.to_string())]);
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

async fn send_request_and_wait(
    adapter_state: &mut AdapterState,
    method: &str,
    params: Value,
    timeout: std::time::Duration,
) -> Result<Value> {
    let id = json!(format!("req-{}", Uuid::new_v4()));
    let key = id_key(&id);
    let (tx, rx) = tokio::sync::oneshot::channel();
    adapter_state.pending_responses.insert(key.clone(), tx);
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
            adapter_state.pending_responses.remove(&key);
            Err(anyhow!(
                "timed out waiting for client response to {method} id={key}"
            ))
        }
    }
}

async fn write_notification(method: &str, params: Value) -> Result<()> {
    write_json(json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .await
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

fn auth_required_error(data: Option<Value>) -> Value {
    json_rpc_error(-32000, "Authentication required", data)
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
