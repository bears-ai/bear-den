use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderName, HeaderValue, AUTHORIZATION};
use rmcp::{
    model::CallToolRequestParams,
    service::RunningService,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, ConfigureCommandExt,
        StreamableHttpClientTransport, TokioChildProcess,
    },
    RoleClient, ServiceExt,
};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tokio::sync::Mutex as TokioMutex;

const DEFAULT_HOST_BROWSER_MCP_SERVER_NAME: &str = "host-browser";
pub(crate) const HOST_BROWSER_MCP_URL_ENV: &str = "BEARS_HOST_BROWSER_MCP_URL";
pub(crate) const HOST_BROWSER_MCP_TOKEN_ENV: &str = "BEARS_HOST_BROWSER_MCP_TOKEN";
pub(crate) const HOST_BROWSER_MCP_SERVER_NAME_ENV: &str = "BEARS_HOST_BROWSER_MCP_SERVER_NAME";

#[derive(Clone, Debug)]
pub(crate) enum McpSourceConfig {
    ClientForwardedStdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    HostBrowserBridge {
        name: String,
        url: String,
        token: String,
    },
}

impl McpSourceConfig {
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::ClientForwardedStdio { name, .. } | Self::HostBrowserBridge { name, .. } => name,
        }
    }

    pub(crate) fn source_kind(&self) -> &'static str {
        match self {
            Self::ClientForwardedStdio { .. } => "client_forwarded",
            Self::HostBrowserBridge { .. } => "host_browser_bridge",
        }
    }

    pub(crate) fn transport(&self) -> &'static str {
        match self {
            Self::ClientForwardedStdio { .. } => "stdio",
            Self::HostBrowserBridge { .. } => "streamable_http",
        }
    }

    pub(crate) fn trust_boundary(&self) -> &'static str {
        match self {
            Self::ClientForwardedStdio { .. } => "client_forwarded_mcp",
            Self::HostBrowserBridge { .. } => "host_browser_only",
        }
    }

    pub(crate) fn safe_summary_for_session_context(&self) -> Value {
        match self {
            Self::ClientForwardedStdio {
                name,
                command,
                args,
                env,
            } => json!({
                "name": name,
                "source": self.source_kind(),
                "transport": self.transport(),
                "command": command,
                "args_count": args.len(),
                "env_count": env.len(),
            }),
            Self::HostBrowserBridge { name, url, .. } => json!({
                "name": name,
                "source": self.source_kind(),
                "transport": self.transport(),
                "url": redact_url_for_log(url),
                "auth": "bearer",
            }),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct McpRegistry {
    sessions: Arc<TokioMutex<HashMap<String, McpSession>>>,
}

#[derive(Default)]
struct McpSession {
    tools: HashMap<String, McpToolRoute>,
}

#[derive(Clone)]
struct McpToolRoute {
    source: McpSourceConfig,
    original_tool_name: String,
}

pub(crate) fn summarize_acp_mcp_servers_param(params: &Value) -> Value {
    let Some(raw) = params
        .get("mcpServers")
        .or_else(|| params.get("mcp_servers"))
    else {
        return json!({ "present": false, "count": 0, "servers": [] });
    };
    let Some(items) = raw.as_array() else {
        return json!({
            "present": true,
            "shape": match raw {
                Value::Object(_) => "object",
                Value::String(_) => "string",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::Null => "null",
                Value::Array(_) => "array",
            },
            "count": null,
            "servers": [],
        });
    };
    json!({
        "present": true,
        "shape": "array",
        "count": items.len(),
        "servers": items.iter().map(summarize_mcp_server_param).collect::<Vec<_>>(),
    })
}

fn summarize_mcp_server_param(item: &Value) -> Value {
    let transport_type = item.get("type").and_then(Value::as_str).unwrap_or("stdio");
    json!({
        "name": item.get("name").and_then(Value::as_str).unwrap_or("<missing>"),
        "type": transport_type,
        "has_command": item.get("command").and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty()),
        "command": item.get("command").and_then(Value::as_str).unwrap_or(""),
        "args_count": item.get("args").and_then(Value::as_array).map(|items| items.len()).unwrap_or(0),
        "env_names": env_names(item.get("env")),
        "has_url": item.get("url").and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty()),
        "url": item.get("url").and_then(Value::as_str).map(redact_url_for_log),
        "header_names": header_names(item.get("headers")),
    })
}

fn env_names(raw: Option<&Value>) -> Vec<String> {
    match raw {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_string))
            .collect(),
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn header_names(raw: Option<&Value>) -> Vec<String> {
    match raw {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_string))
            .collect(),
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn redact_url_for_log(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_username("").ok();
            parsed.set_password(None).ok();
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => "<invalid-url>".to_string(),
    }
}

pub(crate) fn parse_acp_mcp_servers(params: &Value) -> Result<Vec<McpSourceConfig>> {
    let Some(raw) = params
        .get("mcpServers")
        .or_else(|| params.get("mcp_servers"))
    else {
        eprintln!("bears-acp-adapter: acp_mcp_params present=false count=0");
        return Ok(Vec::new());
    };
    let Some(items) = raw.as_array() else {
        eprintln!(
            "bears-acp-adapter: acp_mcp_params invalid_shape summary={}",
            summarize_acp_mcp_servers_param(params)
        );
        return Err(anyhow!(
            "ACP mcpServers are not supported in object form; mcpServers must be an array"
        ));
    };
    eprintln!(
        "bears-acp-adapter: acp_mcp_params summary={}",
        summarize_acp_mcp_servers_param(params)
    );
    let mut servers = Vec::new();
    for item in items {
        let transport_type = item.get("type").and_then(Value::as_str).unwrap_or("stdio");
        if transport_type != "stdio" {
            eprintln!(
                "bears-acp-adapter: acp_mcp_parse unsupported_transport name={} transport={} summary={}",
                item.get("name").and_then(Value::as_str).unwrap_or("<unnamed>"),
                transport_type,
                summarize_mcp_server_param(item)
            );
            if transport_type == "sse" {
                return Err(anyhow!(
                    "ACP MCP server {:?} uses unsupported transport {transport_type:?}; BEARS currently supports stdio and streamable HTTP MCP servers, but not SSE",
                    item.get("name").and_then(Value::as_str).unwrap_or("<unnamed>")
                ));
            }
            return Err(anyhow!(
                "ACP MCP server {:?} uses unsupported transport {transport_type:?}; BEARS currently supports stdio and streamable HTTP MCP servers forwarded by Zed",
                item.get("name").and_then(Value::as_str).unwrap_or("<unnamed>")
            ));
        }
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("ACP stdio MCP server missing name"))?
            .to_string();
        let command = item
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("ACP stdio MCP server {name:?} missing command"))?
            .to_string();
        let args = item
            .get("args")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let env = parse_env(item.get("env"))?;
        eprintln!(
            "bears-acp-adapter: acp_mcp_parse accepted_stdio source_kind=client_forwarded name={} command={} args_count={} env_names={:?}",
            name,
            command,
            args.len(),
            env.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>()
        );
        servers.push(McpSourceConfig::ClientForwardedStdio {
            name,
            command,
            args,
            env,
        });
    }
    eprintln!(
        "bears-acp-adapter: acp_mcp_parse complete accepted_count={}",
        servers.len()
    );
    Ok(servers)
}

pub(crate) fn host_browser_bridge_config_from_env() -> Option<McpSourceConfig> {
    let url = std::env::var(HOST_BROWSER_MCP_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let token = std::env::var(HOST_BROWSER_MCP_TOKEN_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let name = std::env::var(HOST_BROWSER_MCP_SERVER_NAME_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_HOST_BROWSER_MCP_SERVER_NAME.to_string());
    token.map(|token| McpSourceConfig::HostBrowserBridge { name, url, token })
}

pub(crate) fn host_browser_bridge_env_summary() -> Value {
    let configured_url = std::env::var(HOST_BROWSER_MCP_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let has_token = std::env::var(HOST_BROWSER_MCP_TOKEN_ENV)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let server_name = std::env::var(HOST_BROWSER_MCP_SERVER_NAME_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_HOST_BROWSER_MCP_SERVER_NAME.to_string());
    let configured = configured_url.is_some() && has_token;
    let status = if configured {
        "configured"
    } else if configured_url.is_some() {
        "missing_token"
    } else {
        "unconfigured"
    };
    json!({
        "configured": configured,
        "status": status,
        "server_name": server_name,
        "url": configured_url.map(|url| redact_url_for_log(&url)),
        "url_env": HOST_BROWSER_MCP_URL_ENV,
        "token_env": HOST_BROWSER_MCP_TOKEN_ENV,
        "server_name_env": HOST_BROWSER_MCP_SERVER_NAME_ENV,
        "has_token": has_token,
    })
}

fn parse_env(raw: Option<&Value>) -> Result<Vec<(String, String)>> {
    match raw {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("ACP MCP env entry missing name"))?;
                let value = item
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("ACP MCP env entry {name:?} missing value"))?;
                Ok((name.to_string(), value.to_string()))
            })
            .collect(),
        Some(Value::Object(map)) => Ok(map
            .iter()
            .filter_map(|(name, value)| value.as_str().map(|v| (name.clone(), v.to_string())))
            .collect()),
        Some(_) => Err(anyhow!("ACP MCP env must be an array or object")),
    }
}

pub(crate) fn mcp_provider_name(server_name: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_name_part(server_name),
        sanitize_name_part(tool_name)
    )
}

fn sanitize_name_part(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = false;
    for ch in raw.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };
        if normalized == '_' {
            if !prev_underscore && !out.is_empty() {
                out.push('_');
            }
            prev_underscore = true;
        } else {
            out.push(normalized);
            prev_underscore = false;
        }
    }
    out.trim_matches('_').to_string()
}

impl McpRegistry {
    pub(crate) async fn configure_session(
        &self,
        session_id: &str,
        sources: Vec<McpSourceConfig>,
    ) -> Result<Value> {
        let mut tools = HashMap::new();
        let mut descriptors = Vec::new();
        let mut server_summaries = Vec::new();
        eprintln!(
            "bears-acp-adapter: acp_mcp_configure session_id={} source_count={}",
            session_id,
            sources.len()
        );
        for source in &sources {
            match source {
                McpSourceConfig::ClientForwardedStdio {
                    name,
                    command,
                    args,
                    env,
                } => {
                    eprintln!(
                        "bears-acp-adapter: acp_mcp_discovery_start session_id={} source_kind={} server={} command={} args_count={} env_count={}",
                        session_id,
                        source.source_kind(),
                        name,
                        command,
                        args.len(),
                        env.len()
                    );
                }
                McpSourceConfig::HostBrowserBridge { name, url, .. } => {
                    eprintln!(
                        "bears-acp-adapter: acp_mcp_discovery_start session_id={} source_kind={} server={} url={}",
                        session_id,
                        source.source_kind(),
                        name,
                        redact_url_for_log(url)
                    );
                }
            }
            match discover_server_tools(source).await {
                Ok(server_tools) => {
                    let tool_names = server_tools
                        .iter()
                        .filter_map(|tool| {
                            tool.get("name").and_then(Value::as_str).map(str::to_string)
                        })
                        .collect::<Vec<_>>();
                    eprintln!(
                        "bears-acp-adapter: acp_mcp_discovery_ok session_id={} source_kind={} server={} transport={} tool_count={} tool_names={:?}",
                        session_id,
                        source.source_kind(),
                        source.name(),
                        source.transport(),
                        server_tools.len(),
                        tool_names
                    );
                    server_summaries.push(match source {
                        McpSourceConfig::ClientForwardedStdio { name, command, .. } => json!({
                            "name": name,
                            "source": source.source_kind(),
                            "transport": source.transport(),
                            "trust_boundary": source.trust_boundary(),
                            "command": command,
                            "tool_count": server_tools.len(),
                            "status": "ok",
                        }),
                        McpSourceConfig::HostBrowserBridge { name, url, .. } => json!({
                            "name": name,
                            "source": source.source_kind(),
                            "transport": source.transport(),
                            "trust_boundary": source.trust_boundary(),
                            "url": redact_url_for_log(url),
                            "tool_count": server_tools.len(),
                            "status": "ok",
                        }),
                    });
                    for tool in server_tools {
                        let original_name = tool
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let provider_name = mcp_provider_name(source.name(), &original_name);
                        tools.insert(
                            provider_name.clone(),
                            McpToolRoute {
                                source: source.clone(),
                                original_tool_name: original_name,
                            },
                        );
                        descriptors.push(mcp_client_tool_descriptor(&provider_name, source, tool));
                    }
                }
                Err(err) => {
                    eprintln!(
                        "bears-acp-adapter: acp_mcp_discovery_error session_id={} source_kind={} server={} transport={} error={err:#}",
                        session_id,
                        source.source_kind(),
                        source.name(),
                        source.transport(),
                    );
                    server_summaries.push(match source {
                        McpSourceConfig::ClientForwardedStdio { name, command, .. } => json!({
                            "name": name,
                            "source": source.source_kind(),
                            "transport": source.transport(),
                            "trust_boundary": source.trust_boundary(),
                            "command": command,
                            "tool_count": 0,
                            "status": "error",
                            "error": format!("{err:#}"),
                        }),
                        McpSourceConfig::HostBrowserBridge { name, url, .. } => json!({
                            "name": name,
                            "source": source.source_kind(),
                            "transport": source.transport(),
                            "trust_boundary": source.trust_boundary(),
                            "url": redact_url_for_log(url),
                            "tool_count": 0,
                            "status": "error",
                            "error": format!("{err:#}"),
                        }),
                    });
                }
            }
        }
        let mut sessions = self.sessions.lock().await;
        let tool_count = tools.len();
        let tool_names = tools.keys().cloned().collect::<Vec<_>>();
        let source_counts = summarize_descriptor_source_counts(&descriptors);
        let browser_source = active_browser_source_from_descriptors(&descriptors);
        let browser_tool_count = count_browser_tools(&descriptors);
        sessions.insert(session_id.to_string(), McpSession { tools });
        eprintln!(
            "bears-acp-adapter: acp_mcp_configure_complete session_id={} dynamic_tool_count={} browser_tool_count={} source_counts={} active_browser_source={} dynamic_tool_names={:?}",
            session_id,
            tool_count,
            browser_tool_count,
            serde_json::to_string(&source_counts).unwrap_or_else(|_| source_counts.to_string()),
            browser_source,
            tool_names
        );
        Ok(json!({
            "servers": server_summaries,
            "client_tools": descriptors,
        }))
    }

    pub(crate) async fn has_tool(&self, session_id: &str, provider_name: &str) -> bool {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .is_some_and(|session| session.tools.contains_key(provider_name))
    }

    pub(crate) async fn call_tool(
        &self,
        session_id: &str,
        provider_name: &str,
        args: Value,
    ) -> Result<Value> {
        let route = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .and_then(|session| session.tools.get(provider_name).cloned())
            .ok_or_else(|| {
                anyhow!("MCP tool {provider_name:?} is not registered for ACP session {session_id}")
            })?;
        call_server_tool(&route.source, &route.original_tool_name, args).await
    }
}

async fn discover_server_tools(source: &McpSourceConfig) -> Result<Vec<Value>> {
    with_server_client(source, |client| async move {
        let tools = client.peer().list_all_tools().await?;
        let values = tools
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(values)
    })
    .await
}

async fn call_server_tool(source: &McpSourceConfig, tool_name: &str, args: Value) -> Result<Value> {
    eprintln!(
        "bears-acp-adapter: acp_mcp_call_start source_kind={} server={} transport={} tool={} args_keys={:?}",
        source.source_kind(),
        source.name(),
        source.transport(),
        tool_name,
        args.as_object()
            .map(|map| map.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    );
    with_server_client(source, |client| async move {
        let arguments = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            other => {
                let mut map = Map::new();
                map.insert("value".to_string(), other);
                Some(map)
            }
        };
        let mut params = CallToolRequestParams::new(tool_name.to_string());
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }
        let result = client.peer().call_tool(params).await?;
        let structured = serde_json::to_value(&result)?;
        let content = mcp_tool_result_content(&structured);
        eprintln!(
            "bears-acp-adapter: acp_mcp_call_ok source_kind={} server={} transport={} tool={} is_error={:?} content_items={} structured={} content_preview={:?}",
            source.source_kind(),
            source.name(),
            source.transport(),
            tool_name,
            result.is_error,
            result.content.len(),
            result.structured_content.is_some(),
            truncate_for_mcp_log(&content, 500)
        );
        Ok(json!({
            "ok": result.is_error != Some(true),
            "content": content,
            "mcp_result": structured,
        }))
    })
    .await
}

fn stdio_safe_command_args(command: &str, args: &[String], server_name: &str) -> Vec<String> {
    if command != "docker" || !args.iter().any(|arg| arg == "exec") {
        return args.to_vec();
    }

    let mut rewritten = Vec::with_capacity(args.len());
    let mut changed = false;
    for arg in args {
        match arg.as_str() {
            "-it" | "-ti" => {
                rewritten.push("-i".to_string());
                changed = true;
            }
            "-t" | "--tty" => {
                changed = true;
            }
            _ => rewritten.push(arg.clone()),
        }
    }

    if changed {
        eprintln!(
            "bears-acp-adapter: acp_mcp_spawn_rewrite server={} reason=remove_docker_tty_for_stdio_mcp original_args={:?} rewritten_args={:?}",
            server_name,
            args,
            rewritten
        );
    }

    rewritten
}

async fn with_server_client<F, Fut, T>(source: &McpSourceConfig, f: F) -> Result<T>
where
    F: FnOnce(RunningService<RoleClient, ()>) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    match source {
        McpSourceConfig::ClientForwardedStdio {
            name,
            command,
            args,
            env,
        } => {
            let mut command_process = tokio::process::Command::new(command);
            let args = stdio_safe_command_args(command, args, name);
            command_process.args(&args);
            for (name, value) in env {
                command_process.env(name, value);
            }
            eprintln!(
                "bears-acp-adapter: acp_mcp_spawn source_kind={} server={} command={} args={:?} env_names={:?}",
                source.source_kind(),
                name,
                command,
                args,
                env.iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>()
            );
            let transport = TokioChildProcess::new(command_process.configure(|cmd| {
                cmd.kill_on_drop(true);
            }))
            .with_context(|| format!("spawn MCP stdio server {name}"))?;
            let client = ().serve(transport).await?;
            f(client).await
        }
        McpSourceConfig::HostBrowserBridge { name, url, token } => {
            eprintln!(
                "bears-acp-adapter: acp_mcp_connect_http source_kind={} server={} url={} auth=bearer",
                source.source_kind(),
                name,
                redact_url_for_log(url)
            );
            let headers = bearer_auth_headers(token)?;
            let transport = StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url.clone())
                    .custom_headers(headers)
                    .reinit_on_expired_session(true),
            );
            let client = ().serve(transport).await?;
            f(client).await
        }
    }
}

fn bearer_auth_headers(token: &str) -> Result<std::collections::HashMap<HeaderName, HeaderValue>> {
    let mut headers = std::collections::HashMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}"))
            .context("invalid bearer token for MCP Authorization header")?,
    );
    headers.insert(
        HeaderName::from_static("x-bears-mcp-source"),
        HeaderValue::from_static("host_browser_bridge"),
    );
    Ok(headers)
}

fn mcp_client_tool_descriptor(provider_name: &str, source: &McpSourceConfig, tool: Value) -> Value {
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or(match source {
            McpSourceConfig::ClientForwardedStdio { .. } => "MCP tool forwarded by Zed over ACP.",
            McpSourceConfig::HostBrowserBridge { .. } => {
                "Browser MCP tool served by the BEARS host browser bridge."
            }
        });
    let input_schema = tool
        .get("inputSchema")
        .or_else(|| tool.get("input_schema"))
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let (scope, orientation, rendered_description) = match source {
        McpSourceConfig::ClientForwardedStdio { name, .. } => (
            "client_mcp_server",
            "This tool comes from a Zed context server forwarded to BEARS over ACP. Use it when the server name and tool description match the user's request.",
            format!("MCP server `{name}`: {description}"),
        ),
        McpSourceConfig::HostBrowserBridge { name, .. } => (
            "host_browser_bridge",
            "This tool runs against the host browser bridge outside the container. It can inspect or control the host browser, but it does not expose host filesystem, host shell, or host git tools.",
            format!("Host browser MCP server `{name}`: {description}"),
        ),
    };
    json!({
        "name": provider_name,
        "description": rendered_description,
        "input_schema": input_schema,
        "scope": scope,
        "side_effect_class": "unknown_external_tool",
        "approval_sensitivity": "request_client_permission_unless_policy_allows",
        "orientation": orientation,
        "x_bears": {
            "source": source.source_kind(),
            "server": source.name(),
            "original_tool": tool.get("name").cloned().unwrap_or(Value::Null),
            "transport": source.transport(),
            "trust_boundary": source.trust_boundary(),
        }
    })
}

fn summarize_descriptor_source_counts(descriptors: &[Value]) -> Value {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for descriptor in descriptors {
        let source = descriptor
            .pointer("/x_bears/source")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        *counts.entry(source).or_default() += 1;
    }
    json!(counts)
}

fn count_browser_tools(descriptors: &[Value]) -> usize {
    descriptors
        .iter()
        .filter(|descriptor| {
            descriptor
                .pointer("/x_bears/source")
                .and_then(Value::as_str)
                .is_some_and(|source| matches!(source, "client_forwarded" | "host_browser_bridge"))
        })
        .count()
}

fn active_browser_source_from_descriptors(descriptors: &[Value]) -> &'static str {
    let has_client_forwarded = descriptors.iter().any(|descriptor| {
        descriptor
            .pointer("/x_bears/source")
            .and_then(Value::as_str)
            .is_some_and(|source| source == "client_forwarded")
    });
    if has_client_forwarded {
        return "client_forwarded_mcp";
    }
    let has_host_bridge = descriptors.iter().any(|descriptor| {
        descriptor
            .pointer("/x_bears/source")
            .and_then(Value::as_str)
            .is_some_and(|source| source == "host_browser_bridge")
    });
    if has_host_bridge {
        return "host_browser_bridge";
    }
    "none"
}

fn truncate_for_mcp_log(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    out
}

fn mcp_tool_result_content(result: &Value) -> String {
    let mut text = Vec::new();
    if let Some(items) = result.get("content").and_then(Value::as_array) {
        for item in items {
            if let Some(value) = item.get("text").and_then(Value::as_str) {
                text.push(value.to_string());
            }
        }
    }
    if text.is_empty() {
        result.to_string()
    } else {
        text.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
    use http::StatusCode;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    #[test]
    fn summarize_descriptor_source_counts_groups_by_source() {
        let descriptors = vec![
            json!({ "x_bears": { "source": "client_forwarded" } }),
            json!({ "x_bears": { "source": "client_forwarded" } }),
            json!({ "x_bears": { "source": "host_browser_bridge" } }),
        ];
        let counts = summarize_descriptor_source_counts(&descriptors);
        assert_eq!(counts["client_forwarded"], 2);
        assert_eq!(counts["host_browser_bridge"], 1);
        assert_eq!(count_browser_tools(&descriptors), 3);
        assert_eq!(
            active_browser_source_from_descriptors(&descriptors),
            "client_forwarded_mcp"
        );
    }

    async fn mock_streamable_mcp_handler(
        State(calls): State<Arc<Mutex<Vec<String>>>>,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        calls.lock().unwrap().push(method.clone());
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let response = match method.as_str() {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "mock-host-browser", "version": "0.1.0" }
                }
            }),
            "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "browser_snapshot",
                            "description": "Capture a browser snapshot.",
                            "inputSchema": { "type": "object", "properties": {} }
                        }
                    ]
                }
            }),
            "tools/call" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [
                        { "type": "text", "text": "snapshot ok" }
                    ],
                    "isError": false,
                    "structuredContent": { "ok": true, "snapshot": "snapshot ok" }
                }
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("unsupported method {method}") }
            }),
        };
        (StatusCode::OK, Json(response)).into_response()
    }

    async fn start_mock_streamable_mcp_server(
    ) -> Result<(String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>)> {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/mcp", post(mock_streamable_mcp_handler))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Ok((format!("http://{addr}/mcp"), calls, handle))
    }

    #[tokio::test]
    async fn host_browser_bridge_http_source_discovers_and_calls_tools() {
        let (url, calls, handle) = start_mock_streamable_mcp_server().await.unwrap();
        let registry = McpRegistry::default();
        let source = McpSourceConfig::HostBrowserBridge {
            name: "host-browser".to_string(),
            url,
            token: "secret-token".to_string(),
        };

        let context = registry
            .configure_session("session-1", vec![source])
            .await
            .unwrap();
        let tools = context["client_tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["x_bears"]["source"], "host_browser_bridge");
        assert_eq!(tools[0]["name"], "mcp__host_browser__browser_snapshot");

        let result = registry
            .call_tool(
                "session-1",
                "mcp__host_browser__browser_snapshot",
                Value::Null,
            )
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["content"], "snapshot ok");

        let methods = calls.lock().unwrap().clone();
        assert!(methods.iter().any(|method| method == "initialize"));
        assert!(methods.iter().any(|method| method == "tools/list"));
        assert!(methods.iter().any(|method| method == "tools/call"));

        handle.abort();
    }

    #[tokio::test]
    async fn host_browser_bridge_http_source_reports_discovery_errors_without_panicking() {
        let registry = McpRegistry::default();
        let source = McpSourceConfig::HostBrowserBridge {
            name: "host-browser".to_string(),
            url: "http://127.0.0.1:9/mcp".to_string(),
            token: "secret-token".to_string(),
        };

        let context = registry
            .configure_session("session-2", vec![source])
            .await
            .unwrap();
        assert_eq!(context["client_tools"].as_array().unwrap().len(), 0);
        let servers = context["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["status"], "error");
        assert_eq!(servers[0]["source"], "host_browser_bridge");
    }
}
