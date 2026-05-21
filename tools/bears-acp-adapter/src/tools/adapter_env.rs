use crate::{
    adapter_capabilities_context, browser_tool_source_summary, direct_tools_context, session_context,
    AdapterState,
};
use anyhow::Result;
use serde_json::{json, Value};

use super::mcp::host_browser_bridge_env_summary;

pub(crate) async fn handle_adapter_env_inspect(
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

    let mut response = json!({
        "adapter": adapter_capabilities_context(),
        "host_browser_bridge_env": host_browser_bridge_env_summary(),
        "browser_tool_source": browser_tool_source_summary(context),
        "direct_tools": direct_tools_context(),
        "session": {
            "id": session_id,
            "cwd": context.cwd,
            "roots": context.roots,
            "conversation_id": context.conversation_id,
            "resolved_conversation_id": context.resolved_conversation_id,
            "mcp_sources": context
                .mcp_sources
                .iter()
                .map(|source| source.safe_summary_for_session_context())
                .collect::<Vec<_>>(),
        },
    });

    if include_session_mcp {
        response["session"]["mcp"] = context.raw.get("mcp").cloned().unwrap_or(Value::Null);
    }
    if include_client_capabilities {
        response["client_capabilities"] = adapter_state.client_capabilities.clone();
    }
    if include_raw_context {
        response["session"]["raw_context"] = context.raw.clone();
    }

    Ok(response)
}
