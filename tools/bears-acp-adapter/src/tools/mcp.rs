use anyhow::{anyhow, Context, Result};
use rmcp::{
    model::CallToolRequestParams,
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceExt,
};
use serde_json::{json, Map, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex as TokioMutex;

#[derive(Clone, Debug)]
pub(crate) struct AcpMcpServerConfig {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
}

#[derive(Clone, Default)]
pub(crate) struct McpRegistry {
    sessions: Arc<TokioMutex<HashMap<String, McpSession>>>,
}

#[derive(Default)]
struct McpSession {
    servers: Vec<AcpMcpServerConfig>,
    tools: HashMap<String, McpToolRoute>,
}

#[derive(Clone)]
struct McpToolRoute {
    server: AcpMcpServerConfig,
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

pub(crate) fn parse_acp_mcp_servers(params: &Value) -> Result<Vec<AcpMcpServerConfig>> {
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
        return Err(anyhow!("ACP mcpServers must be an array"));
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
            // MCP-over-ACP is currently only a draft ACP RFD. When it stabilizes,
            // this parser should accept `type: "acp"` and route MCP messages over
            // the existing ACP channel instead of spawning stdio child processes.
            return Err(anyhow!(
                "ACP MCP server {:?} uses unsupported transport {transport_type:?}; BEARS currently supports stdio MCP servers forwarded by Zed",
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
            "bears-acp-adapter: acp_mcp_parse accepted_stdio name={} command={} args_count={} env_names={:?}",
            name,
            command,
            args.len(),
            env.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>()
        );
        servers.push(AcpMcpServerConfig {
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
        servers: Vec<AcpMcpServerConfig>,
    ) -> Result<Value> {
        let mut tools = HashMap::new();
        let mut descriptors = Vec::new();
        let mut server_summaries = Vec::new();
        eprintln!(
            "bears-acp-adapter: acp_mcp_configure session_id={} server_count={}",
            session_id,
            servers.len()
        );
        for server in &servers {
            eprintln!(
                "bears-acp-adapter: acp_mcp_discovery_start session_id={} server={} command={} args_count={} env_count={}",
                session_id,
                server.name,
                server.command,
                server.args.len(),
                server.env.len()
            );
            match discover_server_tools(server).await {
                Ok(server_tools) => {
                    let tool_names = server_tools
                        .iter()
                        .filter_map(|tool| {
                            tool.get("name").and_then(Value::as_str).map(str::to_string)
                        })
                        .collect::<Vec<_>>();
                    eprintln!(
                        "bears-acp-adapter: acp_mcp_discovery_ok session_id={} server={} tool_count={} tool_names={:?}",
                        session_id,
                        server.name,
                        server_tools.len(),
                        tool_names
                    );
                    server_summaries.push(json!({
                        "name": server.name,
                        "transport": "stdio",
                        "command": server.command,
                        "tool_count": server_tools.len(),
                        "status": "ok",
                    }));
                    for tool in server_tools {
                        let original_name = tool
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let provider_name = mcp_provider_name(&server.name, &original_name);
                        tools.insert(
                            provider_name.clone(),
                            McpToolRoute {
                                server: server.clone(),
                                original_tool_name: original_name,
                            },
                        );
                        descriptors.push(mcp_client_tool_descriptor(
                            &provider_name,
                            &server.name,
                            tool,
                        ));
                    }
                }
                Err(err) => {
                    eprintln!(
                        "bears-acp-adapter: acp_mcp_discovery_error session_id={} server={} error={err:#}",
                        session_id,
                        server.name
                    );
                    server_summaries.push(json!({
                        "name": server.name,
                        "transport": "stdio",
                        "command": server.command,
                        "tool_count": 0,
                        "status": "error",
                        "error": format!("{err:#}"),
                    }));
                }
            }
        }
        let mut sessions = self.sessions.lock().await;
        let tool_count = tools.len();
        let tool_names = tools.keys().cloned().collect::<Vec<_>>();
        sessions.insert(session_id.to_string(), McpSession { servers, tools });
        eprintln!(
            "bears-acp-adapter: acp_mcp_configure_complete session_id={} dynamic_tool_count={} dynamic_tool_names={:?}",
            session_id,
            tool_count,
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
        call_server_tool(&route.server, &route.original_tool_name, args).await
    }
}

async fn discover_server_tools(server: &AcpMcpServerConfig) -> Result<Vec<Value>> {
    with_server_client(server, |client| async move {
        let tools = client.peer().list_all_tools().await?;
        let values = tools
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(values)
    })
    .await
}

async fn call_server_tool(
    server: &AcpMcpServerConfig,
    tool_name: &str,
    args: Value,
) -> Result<Value> {
    eprintln!(
        "bears-acp-adapter: acp_mcp_call_start server={} tool={} args_keys={:?}",
        server.name,
        tool_name,
        args.as_object()
            .map(|map| map.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    );
    with_server_client(server, |client| async move {
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
        eprintln!(
            "bears-acp-adapter: acp_mcp_call_ok server={} tool={} is_error={:?} content_items={} structured={}",
            server.name,
            tool_name,
            result.is_error,
            result.content.len(),
            result.structured_content.is_some()
        );
        let structured = serde_json::to_value(&result)?;
        let content = mcp_tool_result_content(&structured);
        Ok(json!({
            "ok": result.is_error != Some(true),
            "content": content,
            "mcp_result": structured,
        }))
    })
    .await
}

// Zed may wrap remote context-server commands as `docker exec -it ...`.
// Stdio MCP transports require clean pipes, not a TTY; Docker exits with
// "the input device is not a TTY" when `-t` is used from this adapter.
// Preserve stdin (`-i`) but remove TTY allocation. This is intentionally
// scoped to Docker exec wrappers and should remain until Zed stops adding
// `-t` for ACP-forwarded stdio MCP servers, or until we deliberately stop
// supporting such forwarded stdio MCP servers in remote/container sessions.
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

async fn with_server_client<F, Fut, T>(server: &AcpMcpServerConfig, f: F) -> Result<T>
where
    F: FnOnce(rmcp::service::RunningService<rmcp::RoleClient, ()>) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut command = tokio::process::Command::new(&server.command);
    let args = stdio_safe_command_args(&server.command, &server.args, &server.name);
    command.args(&args);
    for (name, value) in &server.env {
        command.env(name, value);
    }
    eprintln!(
        "bears-acp-adapter: acp_mcp_spawn server={} command={} args={:?} env_names={:?}",
        server.name,
        server.command,
        args,
        server
            .env
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
    );
    let transport = TokioChildProcess::new(command.configure(|cmd| {
        cmd.kill_on_drop(true);
    }))
    .with_context(|| format!("spawn MCP stdio server {}", server.name))?;
    let client = ().serve(transport).await?;
    let result = f(client).await;
    result
}

fn mcp_client_tool_descriptor(provider_name: &str, server_name: &str, tool: Value) -> Value {
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("MCP tool forwarded by Zed over ACP.");
    let input_schema = tool
        .get("inputSchema")
        .or_else(|| tool.get("input_schema"))
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    json!({
        "name": provider_name,
        "description": format!("MCP server `{server_name}`: {description}"),
        "input_schema": input_schema,
        "scope": "client_mcp_server",
        "side_effect_class": "unknown_external_tool",
        "approval_sensitivity": "request_client_permission_unless_policy_allows",
        "orientation": "This tool comes from a Zed context server forwarded to BEARS over ACP. Use it when the server name and tool description match the user's request.",
        "x_bears": {
            "source": "acp_mcp_server",
            "server": server_name,
            "original_tool": tool.get("name").cloned().unwrap_or(Value::Null)
        }
    })
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
