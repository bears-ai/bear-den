use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Url;
use serde_json::{json, Value};
use std::{collections::HashMap, env};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
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
}

#[derive(Default)]
struct AdapterState {
    client_capabilities: Value,
    session_contexts: HashMap<String, Value>,
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
    let runtime = RuntimeConfig::from_env_and_args()?;
    eprintln!(
        "bears-acp-adapter: starting version={} git_sha={} conversation_id_mode=default",
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA")
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

    let stdin = BufReader::new(io::stdin());
    let mut lines = stdin.lines();

    let mut adapter_state = AdapterState::default();

    while let Some(line) = lines.next_line().await.context("read stdin")? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request = match parse_request(line) {
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

        if let Err(err) = handle_request(&http, &runtime, &mut adapter_state, request).await {
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
                api_url,
                bear,
                token,
                client,
            })
        } else {
            None
        };

        let runtime = Self {
            config,
            diagnostics,
            check_server,
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
        "bears-acp-adapter {}\nBuild git SHA: {}\nACP conversation id mode: default",
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA")
    );
}

fn print_help_to_stderr() {
    eprintln!(
        "bears-acp-adapter {}\nBuild git SHA: {}\nACP conversation id mode: default\n\n\
Usage: bears-acp-adapter --api-url <url> --bear <slug> [--client zed] [--token-env BEARS_DEN_TOKEN]\n\n\
Options:\n  --api-url <url>        Den API origin, for example https://api.bears.example\n  --bear <slug>          Bear slug to chat with\n  --token <token>        Den Code token with acp:chat and acp:tools scopes\n  --token-env <env-var>  Read the Den bearer token from this environment variable\n  --client <name>        Client label: zed, opencode, or acp_adapter\n  --check-config         Validate configuration and exit without starting ACP stdio\n  --check-server         Fetch Den /version and exit without starting ACP stdio\n  --version              Show version/build behavior and exit\n  --help                 Show this help\n\n\
Environment fallbacks:\n  BEARS_DEN_API_URL\n  BEARS_BEAR_SLUG\n  BEARS_DEN_TOKEN\n  BEARS_DEN_TOKEN_ENV\n  BEARS_ACP_CLIENT\n\n\
BEARS_DEN_API_URL should be the API origin only, not the full /acp/bears/... endpoint.",
        env!("CARGO_PKG_VERSION"),
        env!("BEARS_ACP_ADAPTER_GIT_SHA")
    );
}

fn normalize_client(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "zed" => "zed".to_string(),
        "opencode" => "opencode".to_string(),
        _ => "acp_adapter".to_string(),
    }
}

fn parse_request(line: &str) -> Result<JsonRpcRequest> {
    let value: Value = serde_json::from_str(line)?;
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
    runtime: &RuntimeConfig,
    adapter_state: &mut AdapterState,
    request: JsonRpcRequest,
) -> Result<()> {
    match request.method.as_str() {
        "initialize" => {
            adapter_state.client_capabilities = request
                .params
                .get("clientCapabilities")
                .or_else(|| request.params.get("capabilities"))
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(id) = request.id {
                write_response(id, Ok(initialize_result())).await?;
            }
        }
        "session/new" => {
            if let Some(id) = request.id {
                let session_id = format!("acp-{}", Uuid::new_v4());
                let context = session_context_from_params(&request.params);
                adapter_state
                    .session_contexts
                    .insert(session_id.clone(), context);
                write_response(
                    id,
                    Ok(json!({
                        "sessionId": session_id,
                        "sessionConfigOptions": null,
                        "mode": null,
                    })),
                )
                .await?;
            }
        }
        "session/prompt" => {
            if let Some(id) = request.id {
                let Some(config) = runtime.config.as_ref() else {
                    write_response(
                        id,
                        Err(json_rpc_error(
                            -32000,
                            "BEARS adapter is not configured",
                            Some(json!({
                                "message": runtime.configuration_error_message(),
                                "problems": runtime.diagnostics,
                            })),
                        )),
                    )
                    .await?;
                    return Ok(());
                };

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
                            -32000,
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
        "session/cancel" => {
            eprintln!("bears-acp-adapter: session/cancel received; remote cancellation is not implemented yet");
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

fn session_context_from_params(params: &Value) -> Value {
    let cwd = params
        .get("cwd")
        .or_else(|| params.get("workspaceUri"))
        .or_else(|| params.pointer("/workspace/currentDirectory"))
        .and_then(Value::as_str)
        .unwrap_or("");
    json!({
        "cwd": cwd,
        "adapter_version": env!("CARGO_PKG_VERSION"),
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": false,
            "mcpCapabilities": {
                "http": false,
                "sse": false
            },
            "promptCapabilities": {
                "image": false,
                "audio": false,
                "embeddedContext": true
            },
            "sessionCapabilities": {}
        },
        "agentInfo": {
            "name": "bears",
            "title": "BEARS",
            "version": env!("CARGO_PKG_VERSION")
        },
        "authMethods": []
    })
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
    let conversation_id = "default";
    let client_context = adapter_state
        .session_contexts
        .get(session_id)
        .cloned()
        .unwrap_or_else(|| json!({}));
    eprintln!(
        "bears-acp-adapter: session/prompt session_id={} bear={} conversation_id={} client={}",
        session_id, config.bear, conversation_id, config.client
    );

    send_user_message_chunk(session_id, &prompt).await?;

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

    let response = http
        .post(&url)
        .headers(headers)
        .json(&json!({
            "message": prompt,
            "conversation_id": conversation_id,
            "client": config.client,
            "client_capabilities": adapter_state.client_capabilities,
            "client_context": client_context,
        }))
        .send()
        .await
        .with_context(|| den_request_context(&url))?;

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_else(|_| "".to_string());
        return Err(anyhow!("{}", den_status_error_message(status, text.trim())));
    }

    let mut saw_done = false;
    let mut upstream_errors = Vec::new();
    let mut buffer = Vec::<u8>::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read Den SSE chunk")?;
        buffer.extend_from_slice(&chunk);
        while let Some(pos) = buffer.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = buffer.drain(..pos + 2).collect();
            let outcome = handle_sse_frame(session_id, &frame).await?;
            saw_done |= outcome.saw_done;
            upstream_errors.extend(outcome.upstream_errors);
        }
    }
    if !buffer.is_empty() {
        let frame = std::mem::take(&mut buffer);
        let outcome = handle_sse_frame(session_id, &frame).await?;
        saw_done |= outcome.saw_done;
        upstream_errors.extend(outcome.upstream_errors);
    }

    if !upstream_errors.is_empty() {
        return Err(anyhow!(
            "BEARS upstream stream reported error: {}",
            upstream_errors.join("; ")
        ));
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
                    if let Some(text) = resource.get("text").and_then(Value::as_str) {
                        parts.push(format!("Resource {uri}:\n{text}"));
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

async fn handle_sse_frame(session_id: &str, frame: &[u8]) -> Result<SseFrameOutcome> {
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
        if event.get("type").and_then(Value::as_str) == Some("error") {
            outcome.upstream_errors.push(format_den_event_error(&event));
        }
        outcome.saw_done |= handle_den_event(session_id, &event).await?;
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

async fn handle_den_event(session_id: &str, event: &Value) -> Result<bool> {
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
        "client_tool_request" => {
            send_client_tool_call_update(session_id, event).await?;
            Ok(false)
        }
        "done" => Ok(true),
        _ => Ok(false),
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
                "title": tool_name,
                "kind": "read",
                "status": "pending",
                "content": { "type": "text", "text": summary }
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

fn json_rpc_error(code: i64, message: &str, data: Option<Value>) -> Value {
    match data {
        Some(data) => json!({ "code": code, "message": message, "data": data }),
        None => json!({ "code": code, "message": message }),
    }
}
