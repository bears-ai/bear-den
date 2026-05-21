use crate::{
    adapter_capabilities_context, browser_tool_source_summary, direct_tools_context, session_context,
    AdapterState,
};
use anyhow::Result;
use serde_json::{json, Value};

use super::mcp::host_browser_bridge_env_summary;

pub(crate) async fn handle_bear_environment(
    adapter_state: &AdapterState,
    session_id: &str,
    args: &Value,
) -> Result<Value> {
    let context = session_context(adapter_state, session_id)?;
    let include_client_capabilities = args
        .get("include_client_capabilities")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_session_mcp = args
        .get("include_session_mcp")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let include_raw_context = args
        .get("include_raw_context")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let adapter = adapter_capabilities_context();
    let direct_tools = direct_tools_context();
    let browser = browser_tool_source_summary(context);
    let host_bridge_env = host_browser_bridge_env_summary();

    let mut response = json!({
        "bear": {
            "role": "pair",
            "identity": "Builder Bear",
            "implementation": "bears-acp-adapter"
        },
        "runtime": {
            "kind": "acp_adapter",
            "name": adapter.get("name").cloned().unwrap_or(Value::Null),
            "version": adapter.get("version").cloned().unwrap_or(Value::Null),
            "git_sha": adapter.get("git_sha").cloned().unwrap_or(Value::Null),
            "built_at_utc": adapter.get("built_at_utc").cloned().unwrap_or(Value::Null),
            "api_contract": adapter.get("api_contract").cloned().unwrap_or(Value::Null),
        },
        "session": {
            "id": session_id,
            "cwd": context.cwd,
            "workspace_roots": context.roots,
            "conversation_id": context.conversation_id,
            "resolved_conversation_id": context.resolved_conversation_id,
        },
        "tools": {
            "direct": direct_tools,
            "dynamic_mcp_sources": context
                .mcp_sources
                .iter()
                .map(|source| source.safe_summary_for_session_context())
                .collect::<Vec<_>>(),
        },
        "browser": browser,
        "services": {
            "den": {
                "available": false,
                "status": "not_inspected_by_this_tool"
            }
        },
        "environment_variants": {
            "acp_adapter": {
                "adapter": adapter,
                "host_browser_bridge_env": host_bridge_env,
            }
        },
        "diagnostics": {
            "warnings": [],
            "status": "ok"
        }
    });

    if include_session_mcp {
        response["environment_variants"]["acp_adapter"]["session_mcp"] =
            context.raw.get("mcp").cloned().unwrap_or(Value::Null);
    }
    if include_client_capabilities {
        response["environment_variants"]["acp_adapter"]["client_capabilities"] =
            adapter_state.client_capabilities.clone();
    }
    if include_raw_context {
        response["environment_variants"]["acp_adapter"]["raw_session_context"] =
            context.raw.clone();
    }

    Ok(response)
}
