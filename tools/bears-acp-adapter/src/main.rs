use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};
use std::env;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

#[derive(Clone, Debug)]
struct Config {
    api_url: String,
    bear: String,
    token: String,
    client: String,
}

#[derive(Debug)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    params: Value,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("bears-acp-adapter: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let config = Config::from_env_and_args()?;
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .context("build HTTP client")?;

    let stdin = BufReader::new(io::stdin());
    let mut lines = stdin.lines();

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

        if let Err(err) = handle_request(&http, &config, request).await {
            eprintln!("bears-acp-adapter: request handling failed: {err:#}");
        }
    }

    Ok(())
}

impl Config {
    fn from_env_and_args() -> Result<Self> {
        let mut api_url = env::var("BEARS_DEN_API_URL").unwrap_or_default();
        let mut bear = env::var("BEARS_BEAR_SLUG").unwrap_or_default();
        let mut token = env::var("BEARS_DEN_TOKEN").unwrap_or_default();
        let mut token_env = env::var("BEARS_DEN_TOKEN_ENV").unwrap_or_default();
        let mut client = env::var("BEARS_ACP_CLIENT").unwrap_or_else(|_| "zed".to_string());

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--api-url" => api_url = require_arg_value("--api-url", args.next())?,
                "--bear" => bear = require_arg_value("--bear", args.next())?,
                "--token" => token = require_arg_value("--token", args.next())?,
                "--token-env" => token_env = require_arg_value("--token-env", args.next())?,
                "--client" => client = require_arg_value("--client", args.next())?,
                "--help" | "-h" => {
                    print_help_to_stderr();
                    std::process::exit(0);
                }
                unknown => return Err(anyhow!("unknown argument {unknown:?}; use --help")),
            }
        }

        if !token_env.trim().is_empty() {
            token = env::var(token_env.trim()).with_context(|| {
                format!("read bearer token from environment variable {token_env:?}")
            })?;
        }

        api_url = api_url.trim_end_matches('/').to_string();
        bear = bear.trim().to_string();
        token = token.trim().to_string();
        client = normalize_client(&client);

        if api_url.is_empty() {
            return Err(anyhow!(
                "missing Den API URL; set BEARS_DEN_API_URL or pass --api-url"
            ));
        }
        if bear.is_empty() {
            return Err(anyhow!(
                "missing bear slug; set BEARS_BEAR_SLUG or pass --bear"
            ));
        }
        if token.is_empty() {
            return Err(anyhow!(
                "missing bearer token; set BEARS_DEN_TOKEN, BEARS_DEN_TOKEN_ENV, --token, or --token-env"
            ));
        }

        Ok(Self {
            api_url,
            bear,
            token,
            client,
        })
    }
}

fn require_arg_value(flag: &str, value: Option<String>) -> Result<String> {
    value.ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn print_help_to_stderr() {
    eprintln!(
        "Usage: bears-acp-adapter --api-url <url> --bear <slug> [--client zed] [--token-env BEARS_DEN_TOKEN]\n\n\
Environment fallbacks:\n  BEARS_DEN_API_URL\n  BEARS_BEAR_SLUG\n  BEARS_DEN_TOKEN\n  BEARS_DEN_TOKEN_ENV\n  BEARS_ACP_CLIENT"
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
    config: &Config,
    request: JsonRpcRequest,
) -> Result<()> {
    match request.method.as_str() {
        "initialize" => {
            if let Some(id) = request.id {
                write_response(id, Ok(initialize_result())).await?;
            }
        }
        "session/new" => {
            if let Some(id) = request.id {
                let session_id = format!("acp-{}", Uuid::new_v4());
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
                if let Err(err) = handle_prompt(http, config, id.clone(), request.params).await {
                    write_response(
                        id,
                        Err(json_rpc_error(
                            -32000,
                            "BEARS prompt failed",
                            Some(json!(err.to_string())),
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
    response_id: Value,
    params: Value,
) -> Result<()> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session/prompt params missing sessionId"))?;
    let prompt = prompt_text_from_params(&params)?;

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
        .post(url)
        .headers(headers)
        .json(&json!({
            "message": prompt,
            "conversation_id": session_id,
            "client": config.client,
        }))
        .send()
        .await
        .context("send prompt to Den API")?;

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_else(|_| "".to_string());
        return Err(anyhow!("Den API returned {status}: {text}"));
    }

    let mut saw_done = false;
    let mut buffer = Vec::<u8>::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read Den SSE chunk")?;
        buffer.extend_from_slice(&chunk);
        while let Some(pos) = buffer.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = buffer.drain(..pos + 2).collect();
            saw_done |= handle_sse_frame(session_id, &frame).await?;
        }
    }
    if !buffer.is_empty() {
        let frame = std::mem::take(&mut buffer);
        saw_done |= handle_sse_frame(session_id, &frame).await?;
    }

    let stop_reason = if saw_done { "end_turn" } else { "end_turn" };
    write_response(response_id, Ok(json!({ "stopReason": stop_reason }))).await?;
    Ok(())
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

async fn handle_sse_frame(session_id: &str, frame: &[u8]) -> Result<bool> {
    let text = String::from_utf8_lossy(frame);
    let mut saw_done = false;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(data).context("parse Den SSE event JSON")?;
        saw_done |= handle_den_event(session_id, &event).await?;
    }
    Ok(saw_done)
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
            let message = event
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("BEARS upstream error");
            send_agent_thought_chunk(session_id, message).await?;
            Ok(false)
        }
        "done" => Ok(true),
        _ => Ok(false),
    }
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
