use crate::{
    adapter_capabilities_context, browser_tool_source_summary, direct_tools_context, session_context,
    AdapterState, Config,
};
use anyhow::Result;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};

use super::mcp::host_browser_bridge_env_summary;

pub(crate) async fn collect_bear_environment(
    adapter_state: &AdapterState,
    session_id: &str,
    config: Option<&Config>,
    http: Option<&reqwest::Client>,
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
    let inspect_den = args
        .get("inspect_den")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let adapter = adapter_capabilities_context();
    let direct_tools = direct_tools_context();
    let browser = browser_tool_source_summary(context);
    let host_bridge_env = host_browser_bridge_env_summary();

    let mut den_service = json!({
        "available": false,
        "configured": false,
        "status": "not_inspected",
    });
    let mut warnings = Vec::<Value>::new();
    let mut errors = Vec::<Value>::new();

    if inspect_den {
        if let (Some(config), Some(http)) = (config, http) {
            den_service["configured"] = Value::Bool(true);
            match fetch_den_runtime_state(http, config, session_id).await {
                Ok(runtime) => {
                    den_service = json!({
                        "available": true,
                        "configured": true,
                        "reachable": true,
                        "status": "ok",
                        "runtime": runtime,
                    });
                }
                Err(err) => {
                    den_service = json!({
                        "available": false,
                        "configured": true,
                        "reachable": false,
                        "status": "unreachable",
                        "error": format!("{err:#}"),
                    });
                    warnings.push(json!("Den runtime is unreachable from the adapter"));
                    errors.push(json!(format!("Den runtime inspection failed: {err:#}")));
                }
            }
        } else {
            den_service["status"] = json!("not_available_in_this_runtime");
        }
    }

    let diagnostics_status = if !errors.is_empty() {
        "degraded"
    } else {
        "ok"
    };

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
            "den": den_service
        },
        "environment_variants": {
            "acp_adapter": {
                "adapter": adapter,
                "host_browser_bridge_env": host_bridge_env,
            }
        },
        "diagnostics": {
            "warnings": warnings,
            "errors": errors,
            "status": diagnostics_status
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

pub(crate) async fn handle_bear_environment(
    adapter_state: &AdapterState,
    session_id: &str,
    config: Option<&Config>,
    http: Option<&reqwest::Client>,
    args: &Value,
) -> Result<Value> {
    collect_bear_environment(adapter_state, session_id, config, http, args).await
}

pub(crate) async fn fetch_den_runtime_state(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
) -> Result<Value> {
    let url = format!(
        "{}/acp/bears/{}/sessions/{}/runtime",
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
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(crate::den_status_error_message(status, body.trim()));
    }
    Ok(serde_json::from_str(&body).unwrap_or_else(|_| json!({ "raw": body })))
}
