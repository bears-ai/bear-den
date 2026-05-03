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
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, Mutex},
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
}

#[derive(Clone, Debug, Default)]
struct SessionContext {
    cwd: String,
    roots: Vec<String>,
    raw: Value,
    conversation_id: Option<String>,
    resolved_conversation_id: Option<String>,
}

type PendingResponses = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

const CLIENT_TOOL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug)]
struct InboundMessage {
    value: Value,
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

#[derive(Default)]
struct SseFrameOutcome {
    saw_done: bool,
    saw_assistant_output: bool,
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
    let pending_responses: PendingResponses = Arc::new(Mutex::new(HashMap::new()));
    tokio::spawn(read_stdin_messages(inbound_tx, pending_responses.clone()));

    let mut adapter_state = AdapterState::default();

    while let Some(message) = inbound_rx.recv().await {
        let request = match request_from_value(message.value) {
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
            &mut inbound_rx,
            pending_responses.clone(),
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
                "Missing Den bearer token. Set BEARS_DEN_TOKEN, set BEARS_DEN_TOKEN_ENV to the name of an environment variable containing the token, pass --token <token>, or pass --token-env <env-var>. Den Code tokens include acp:chat and acp:tools scopes."
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
Options:\n  --api-url <url>        Den API origin, for example https://api.bears.example\n  --bear <slug>          Bear slug to chat with\n  --token <token>        Den Code token with acp:chat and acp:tools scopes\n  --token-env <env-var>  Read the Den bearer token from this environment variable\n  --client <name>        Client label: zed, opencode, or acp_adapter\n  --check-config         Validate configuration and exit without starting ACP stdio\n  --check-server         Fetch Den /version and exit without starting ACP stdio\n  --version              Show version/build behavior and exit\n  --help                 Show this help\n\n\
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

async fn read_stdin_messages(tx: mpsc::Sender<InboundMessage>, pending: PendingResponses) {
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
                        if route_pending_response(&pending, &value).await {
                            continue;
                        }
                        if tx.send(InboundMessage { value }).await.is_err() {
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

async fn route_pending_response(pending: &PendingResponses, value: &Value) -> bool {
    if value.get("method").is_some() {
        return false;
    }
    let Some(id) = response_id_key(value.get("id")) else {
        return false;
    };
    let sender = pending.lock().await.remove(&id);
    if let Some(sender) = sender {
        let _ = sender.send(value.clone());
        true
    } else {
        false
    }
}

fn response_id_key(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
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
    inbound_rx: &mut mpsc::Receiver<InboundMessage>,
    pending_responses: PendingResponses,
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
                write_response(id, Ok(initialize_result())).await?;
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
                    Ok(json!({
                        "sessionId": session_id,
                        "configOptions": null,
                        "modes": null,
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
                        let mapped = map_den_sessions_list_to_acp(&den);
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
                    Ok(()) => write_response(id, Ok(session_lifecycle_result())).await?,
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
                            "hint": "Generate a fresh Den Code token for this bear. Code tokens must include acp:chat and acp:tools."
                        })))),
                    )
                    .await?;
                    return Ok(());
                }

                if let Err(err) = handle_prompt(
                    http,
                    config,
                    adapter_state,
                    inbound_rx,
                    pending_responses.clone(),
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

fn token_env_for_auth_method() -> String {
    env::var("BEARS_DEN_TOKEN_ENV")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "BEARS_DEN_TOKEN".to_string())
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": true,
            "mcpCapabilities": {
                "http": false,
                "sse": false
            },
            "promptCapabilities": {
                "image": false,
                "audio": false,
                "embeddedContext": true
            },
            "sessionCapabilities": {
                "close": {},
                "list": {},
                "resume": {}
            }
        },
        "agentInfo": {
            "name": "bears",
            "title": "BEARS",
            "version": env!("CARGO_PKG_VERSION")
        },
        "authMethods": [
            {
                "id": "bears_den_token",
                "name": "BEARS Code Token",
                "type": "env_var",
                "vars": [
                    {
                        "name": token_env_for_auth_method(),
                        "label": "BEARS Code Token",
                        "secret": true
                    }
                ],
                "description": "Paste a Den Code token. Code tokens include acp:chat and acp:tools scopes.",
                "link": "https://github.com/silarsis/BEARS"
            }
        ]
    })
}

fn map_den_sessions_list_to_acp(den: &Value) -> Value {
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
        let mut row = json!({
            "sessionId": session_id,
            "updatedAt": updated_at,
            "cwd": cwd,
        });
        if let Some(title) = s
            .get("resolved_conversation_id")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| {
                s.get("conversation_id")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
            })
        {
            row["title"] = json!(title);
        }
        row["_meta"] = json!({
            "conversation_id": s.get("conversation_id"),
            "resolved_conversation_id": s.get("resolved_conversation_id"),
            "client": s.get("client"),
            "closed_at": s.get("closed_at"),
        });
        sessions_out.push(row);
    }
    let mut result = json!({ "sessions": sessions_out });
    if let Some(c) = den.get("next_cursor").and_then(Value::as_str) {
        result["nextCursor"] = json!(c);
    }
    result
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

    write_response(response_id, Ok(session_lifecycle_result())).await?;
    Ok(())
}

fn session_lifecycle_result() -> Value {
    json!({
        "configOptions": null,
        "modes": null,
    })
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
    inbound_rx: &mut mpsc::Receiver<InboundMessage>,
    pending_responses: PendingResponses,
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
    let cwd = client_context.cwd.clone();
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

    let mut saw_done = false;
    let mut saw_assistant_output = false;
    let mut upstream_errors = Vec::new();
    let mut buffer = Vec::<u8>::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read Den SSE chunk")?;
        buffer.extend_from_slice(&chunk);
        while let Some(pos) = buffer.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = buffer.drain(..pos + 2).collect();
            let outcome = handle_sse_frame(
                http,
                config,
                adapter_state,
                inbound_rx,
                pending_responses.clone(),
                session_id,
                &cwd,
                &frame,
            )
            .await?;
            saw_done |= outcome.saw_done;
            saw_assistant_output |= outcome.saw_assistant_output;
            upstream_errors.extend(outcome.upstream_errors);
        }
    }
    if !buffer.is_empty() {
        let frame = std::mem::take(&mut buffer);
        let outcome = handle_sse_frame(
            http,
            config,
            adapter_state,
            inbound_rx,
            pending_responses.clone(),
            session_id,
            &cwd,
            &frame,
        )
        .await?;
        saw_done |= outcome.saw_done;
        saw_assistant_output |= outcome.saw_assistant_output;
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

    let stop_reason = if saw_done { "end_turn" } else { "end_turn" };
    write_response(response_id, Ok(json!({ "stopReason": stop_reason }))).await?;
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
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    inbound_rx: &mut mpsc::Receiver<InboundMessage>,
    pending_responses: PendingResponses,
    session_id: &str,
    cwd: &str,
    frame: &[u8],
) -> Result<SseFrameOutcome> {
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
        match event.get("type").and_then(Value::as_str) {
            Some("agent_message_chunk") => {
                let text = event
                    .get("content")
                    .and_then(|c| c.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                outcome.saw_assistant_output |= !text.is_empty();
            }
            Some("error") => outcome.upstream_errors.push(format_den_event_error(&event)),
            _ => {}
        }
        outcome.saw_done |= handle_den_event(
            http,
            config,
            adapter_state,
            inbound_rx,
            pending_responses.clone(),
            session_id,
            cwd,
            &event,
        )
        .await?;
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
        out.push_str("\nCodepool request_id: ");
        out.push_str(request_id);
    }
    out
}

async fn handle_den_event(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    _inbound_rx: &mut mpsc::Receiver<InboundMessage>,
    pending_responses: PendingResponses,
    session_id: &str,
    cwd: &str,
    event: &Value,
) -> Result<bool> {
    match event.get("type").and_then(Value::as_str).unwrap_or("") {
        "agent_message_chunk" => {
            let text = event
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !text.is_empty() {
                send_agent_message_chunk(session_id, text).await?;
            }
            Ok(false)
        }
        "status" => {
            let text = event
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
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
        "client_tool_request" => {
            eprintln!(
                "bears-acp-adapter: received client_tool_request session_id={} call_id={} tool={}",
                session_id,
                event
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                event
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            );
            send_client_tool_call_update(session_id, event).await?;
            execute_and_post_client_tool_result(
                http,
                config,
                pending_responses.clone(),
                session_id,
                cwd,
                event,
            )
            .await?;
            Ok(false)
        }
        "done" => Ok(true),
        _ => Ok(false),
    }
}

async fn execute_and_post_client_tool_result(
    http: &reqwest::Client,
    config: &Config,
    pending_responses: PendingResponses,
    session_id: &str,
    cwd: &str,
    event: &Value,
) -> Result<()> {
    let call_id = event
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("client_tool_request missing call_id"))?;
    let request_id = event
        .get("request_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("client_tool_request missing request_id"))?;
    let conversation_id = event
        .get("conversation_id")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let tool_name = event
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("acp_client_tool");

    send_client_tool_progress_update(session_id, call_id, tool_name, event).await?;
    let tool_result = match tool_name {
        "acp_fs_read_text_file" => {
            execute_fs_read_text_file(pending_responses.clone(), session_id, cwd, event).await
        }
        "acp_fs_write_text_file" => {
            // Zed's `fs/write_text_file` request is the authority for local editor
            // write approval. Avoid an extra ACP `session/request_permission` round-trip
            // here because it can leave Codepool's external-tool waiter timing out
            // before the actual file write request is ever sent.
            execute_fs_write_text_file(pending_responses.clone(), session_id, cwd, event).await
        }
        other => Err(anyhow!("unsupported ACP client tool: {other}")),
    };

    let body = match tool_result {
        Ok(result) => {
            send_client_tool_result_update(
                session_id,
                call_id,
                tool_name,
                event,
                "completed",
                Some(&result),
                None,
            )
            .await?;
            json!({
                "request_id": request_id,
                "conversation_id": conversation_id,
                "tool_name": tool_name,
                "status": "ok",
                "result": result,
                "client_observation": {
                    "adapter_version": env!("CARGO_PKG_VERSION")
                }
            })
        }
        Err(err) => {
            let error_message = format!("{err:#}");
            send_client_tool_result_update(
                session_id,
                call_id,
                tool_name,
                event,
                "failed",
                None,
                Some(&error_message),
            )
            .await?;
            json!({
                "request_id": request_id,
                "conversation_id": conversation_id,
                "tool_name": tool_name,
                "status": "error",
                "error": {
                    "code": "adapter_client_tool_error",
                    "message": error_message
                },
                "client_observation": {
                    "adapter_version": env!("CARGO_PKG_VERSION")
                }
            })
        }
    };
    post_tool_result_to_den(
        http, config, session_id, call_id, request_id, tool_name, &body,
    )
    .await
}

async fn execute_fs_read_text_file(
    pending_responses: PendingResponses,
    session_id: &str,
    cwd: &str,
    event: &Value,
) -> Result<Value> {
    let path = event
        .get("arguments")
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("fs/read_text_file requires arguments.path"))?;
    let absolute_path = absolutize_client_path(path, cwd);
    eprintln!("bears-acp-adapter: requesting fs/read_text_file path={absolute_path}");
    let rpc_id = format!("bears-tool-{}", Uuid::new_v4());
    let request = json!({
        "jsonrpc": "2.0",
        "id": rpc_id,
        "method": "fs/read_text_file",
        "params": {
            "sessionId": session_id,
            "path": absolute_path
        }
    });
    let response = send_client_request_with_waiter(pending_responses, &rpc_id, request).await?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("ACP client fs/read_text_file failed: {error}"));
    }
    let result = response
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("ACP client fs/read_text_file response missing result"))?;
    Ok(normalize_read_text_file_result(result))
}

async fn execute_fs_write_text_file(
    pending_responses: PendingResponses,
    session_id: &str,
    cwd: &str,
    event: &Value,
) -> Result<Value> {
    let arguments = write_text_file_arguments(event, cwd)?;
    eprintln!(
        "bears-acp-adapter: requesting fs/write_text_file path={}",
        arguments.path
    );
    let rpc_id = format!("bears-tool-{}", Uuid::new_v4());
    let request = build_write_text_file_request(
        rpc_id.clone(),
        session_id,
        &arguments.path,
        &arguments.content,
    );
    let response = send_client_request_with_waiter(pending_responses, &rpc_id, request).await?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("ACP client fs/write_text_file failed: {error}"));
    }
    Ok(response.get("result").cloned().unwrap_or_else(|| json!({})))
}

#[derive(Debug, PartialEq, Eq)]
struct WriteTextFileArguments {
    path: String,
    content: String,
}

fn write_text_file_arguments(event: &Value, cwd: &str) -> Result<WriteTextFileArguments> {
    let path = event
        .get("arguments")
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("fs/write_text_file requires arguments.path"))?;
    let content = event
        .get("arguments")
        .and_then(|v| v.get("content"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs/write_text_file requires arguments.content"))?;
    Ok(WriteTextFileArguments {
        path: absolutize_client_path(path, cwd),
        content: content.to_string(),
    })
}

fn build_write_text_file_request(
    rpc_id: impl Into<Value>,
    session_id: &str,
    path: &str,
    content: &str,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": rpc_id.into(),
        "method": "fs/write_text_file",
        "params": {
            "sessionId": session_id,
            "path": path,
            "content": content
        }
    })
}

async fn send_client_request_with_waiter(
    pending_responses: PendingResponses,
    rpc_id: &str,
    request: Value,
) -> Result<Value> {
    let (tx, rx) = oneshot::channel();
    pending_responses
        .lock()
        .await
        .insert(rpc_id.to_string(), tx);
    write_json(request).await?;
    match tokio::time::timeout(CLIENT_TOOL_RESPONSE_TIMEOUT, rx).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(anyhow!("ACP client response waiter dropped for {rpc_id}")),
        Err(_) => {
            pending_responses.lock().await.remove(rpc_id);
            Err(anyhow!(
                "ACP client did not return a response for {rpc_id} within {}s",
                CLIENT_TOOL_RESPONSE_TIMEOUT.as_secs()
            ))
        }
    }
}

fn absolutize_client_path(path: &str, cwd: &str) -> String {
    let path = path.trim();
    if path.starts_with("file://") || Path::new(path).is_absolute() || cwd.trim().is_empty() {
        return path.to_string();
    }
    PathBuf::from(cwd.trim())
        .join(path)
        .to_string_lossy()
        .to_string()
}

fn normalize_read_text_file_result(result: Value) -> Value {
    if result.get("content").and_then(Value::as_str).is_some() {
        return result;
    }
    if let Some(text) = result.get("text").and_then(Value::as_str) {
        return json!({ "content": text });
    }
    if let Some(text) = result.as_str() {
        return json!({ "content": text });
    }
    result
}

async fn post_tool_result_to_den(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    call_id: &str,
    request_id: &str,
    tool_name: &str,
    body: &Value,
) -> Result<()> {
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/tool-results/{}",
        config.api_url,
        urlencoding::encode(&config.bear),
        urlencoding::encode(session_id),
        urlencoding::encode(call_id),
    );
    let payload_summary = tool_result_payload_summary(body);
    eprintln!(
        "bears-acp-adapter: posting tool result request_id={} session_id={} call_id={} tool={} {}",
        request_id, session_id, call_id, tool_name, payload_summary
    );
    let response = http
        .post(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .json(body)
        .send()
        .await
        .with_context(|| format!("post ACP tool result to Den at {url}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let response_body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Den tool result endpoint returned HTTP {status} for request_id={request_id} session_id={session_id} call_id={call_id} tool={tool_name} {payload_summary}: {}",
            response_body.trim()
        ));
    }
    Ok(())
}

fn tool_result_payload_summary(body: &Value) -> String {
    let status = body
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let result_bytes = body
        .get("result")
        .map(|value| value.to_string().len())
        .unwrap_or(0);
    let content_bytes = body
        .pointer("/result/content")
        .and_then(Value::as_str)
        .map(str::len)
        .unwrap_or(0);
    let error = body
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("");
    format!(
        "status={} result_bytes={} content_bytes={} error_code={}",
        status, result_bytes, content_bytes, error
    )
}

async fn send_client_tool_progress_update(
    session_id: &str,
    call_id: &str,
    tool_name: &str,
    event: &Value,
) -> Result<()> {
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": call_id,
                "title": tool_title(tool_name),
                "kind": tool_kind_for_event(event),
                "status": "in_progress",
                "rawInput": event.get("arguments").cloned().unwrap_or_else(|| json!({})),
                "locations": tool_locations_for_event(event),
            }
        }),
    )
    .await
}

async fn send_client_tool_result_update(
    session_id: &str,
    call_id: &str,
    tool_name: &str,
    event: &Value,
    status: &str,
    result: Option<&Value>,
    error_message: Option<&str>,
) -> Result<()> {
    let preview = tool_result_preview(tool_name, event, result, error_message);
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": call_id,
                "title": tool_title(tool_name),
                "kind": tool_kind_for_event(event),
                "status": status,
                "locations": tool_locations_for_event(event),
                "rawOutput": match error_message {
                    Some(message) => json!({ "error": message }),
                    None => result.cloned().unwrap_or_else(|| json!({})),
                },
                "content": [{
                    "type": "content",
                    "content": { "type": "text", "text": preview }
                }]
            }
        }),
    )
    .await
}

fn tool_kind_for_event(event: &Value) -> &'static str {
    match event.get("tool_name").and_then(Value::as_str).unwrap_or("") {
        "acp_fs_write_text_file" => "edit",
        "acp_fs_read_text_file" => "read",
        _ => "other",
    }
}

fn tool_title(tool_name: &str) -> &'static str {
    match tool_name {
        "acp_fs_write_text_file" => "Write text file",
        "acp_fs_read_text_file" => "Read text file",
        _ => "ACP client tool",
    }
}

fn tool_locations_for_event(event: &Value) -> Vec<Value> {
    event
        .get("arguments")
        .and_then(|arguments| arguments.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(|path| vec![json!({ "path": path })])
        .unwrap_or_default()
}

fn tool_result_preview(
    tool_name: &str,
    event: &Value,
    result: Option<&Value>,
    error_message: Option<&str>,
) -> String {
    if let Some(message) = error_message {
        return format!("{} failed: {}", tool_title(tool_name), message);
    }
    let path = event
        .get("arguments")
        .and_then(|arguments| arguments.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("file");
    match tool_name {
        "acp_fs_write_text_file" => {
            let bytes = event
                .get("arguments")
                .and_then(|arguments| arguments.get("content"))
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or(0);
            format!("Wrote {bytes} bytes to {path}")
        }
        "acp_fs_read_text_file" => result
            .and_then(|value| value.get("content"))
            .and_then(Value::as_str)
            .map(|content| format!("Read {} bytes from {}", content.len(), path))
            .unwrap_or_else(|| {
                result
                    .map(|value| value.to_string().chars().take(500).collect::<String>())
                    .unwrap_or_else(|| format!("{} completed", tool_title(tool_name)))
            }),
        _ => result
            .map(|value| value.to_string().chars().take(500).collect::<String>())
            .unwrap_or_else(|| format!("{} completed", tool_title(tool_name))),
    }
}

async fn send_client_tool_call_update(session_id: &str, event: &Value) -> Result<()> {
    let call_id = event
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or("acp-client-tool-call");
    let tool_name = event
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("acp_client_tool");
    let args = event.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let summary = format!(
        "Requested local editor tool `{}` with arguments {}",
        tool_name, args
    );
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": call_id,
                "title": tool_title(tool_name),
                "kind": tool_kind_for_event(event),
                "status": "pending",
                "locations": tool_locations_for_event(event),
                "rawInput": args,
                "content": [{
                    "type": "content",
                    "content": { "type": "text", "text": summary }
                }]
            }
        }),
    )
    .await
}

async fn send_user_message_chunk(session_id: &str, text: &str) -> Result<()> {
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "user_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }),
    )
    .await
}

async fn send_agent_message_chunk(session_id: &str, text: &str) -> Result<()> {
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }),
    )
    .await
}

async fn send_agent_thought_chunk(session_id: &str, text: &str) -> Result<()> {
    write_notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_thought_chunk",
                "content": { "type": "text", "text": text }
            }
        }),
    )
    .await
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
    fn build_write_text_file_request_uses_acp_shape() {
        let request = build_write_text_file_request(
            "rpc-1",
            "session-1",
            "/Users/bear/project/README.md",
            "hello",
        );
        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["id"], "rpc-1");
        assert_eq!(request["method"], "fs/write_text_file");
        assert_eq!(request["params"]["sessionId"], "session-1");
        assert_eq!(request["params"]["path"], "/Users/bear/project/README.md");
        assert_eq!(request["params"]["content"], "hello");
    }

    #[test]
    fn write_text_file_arguments_resolves_relative_path() {
        let event = json!({
            "arguments": {
                "path": "src/main.rs",
                "content": "fn main() {}"
            }
        });
        assert_eq!(
            write_text_file_arguments(&event, "/Users/bear/project").unwrap(),
            WriteTextFileArguments {
                path: "/Users/bear/project/src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
            }
        );
    }

    #[test]
    fn relative_tool_paths_are_resolved_against_client_cwd() {
        assert_eq!(
            absolutize_client_path("README.md", "/Users/bear/project"),
            "/Users/bear/project/README.md"
        );
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
        let m = map_den_sessions_list_to_acp(&den);
        assert_eq!(m["nextCursor"], "abc");
        assert_eq!(m["sessions"][0]["sessionId"], "s1");
        assert_eq!(m["sessions"][0]["cwd"], "/tmp");
    }
}
