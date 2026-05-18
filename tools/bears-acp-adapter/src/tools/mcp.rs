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

pub(crate) fn parse_acp_mcp_servers(params: &Value) -> Result<Vec<AcpMcpServerConfig>> {
    let Some(raw) = params.get("mcpServers").or_else(|| params.get("mcp_servers")) else {
        return Ok(Vec::new());
    };
    let Some(items) = raw.as_array() else {
        return Err(anyhow!("ACP mcpServers must be an array"));
    };
    let mut servers = Vec::new();
    for item in items {
        let transport_type = item.get("type").and_then(Value::as_str).unwrap_or("stdio");
        if transport_type != "stdio" {
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
        servers.push(AcpMcpServerConfig {
            name,
            command,
            args,
            env,
        });
    }
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
        let normalized = if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '_' };
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
        for server in &servers {
            match discover_server_tools(server).await {
                Ok(server_tools) => {
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
                        descriptors.push(mcp_client_tool_descriptor(&provider_name, &server.name, tool));
                    }
                }
                Err(err) => {
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
        sessions.insert(
            session_id.to_string(),
            McpSession {
                servers,
                tools,
            },
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
            .ok_or_else(|| anyhow!("MCP tool {provider_name:?} is not registered for ACP session {session_id}"))?;
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

async fn call_server_tool(server: &AcpMcpServerConfig, tool_name: &str, args: Value) -> Result<Value> {
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

async fn with_server_client<F, Fut, T>(server: &AcpMcpServerConfig, f: F) -> Result<T>
where
    F: FnOnce(rmcp::service::RunningService<rmcp::RoleClient, ()>) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut command = tokio::process::Command::new(&server.command);
    command.args(&server.args);
    for (name, value) in &server.env {
        command.env(name, value);
    }
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
