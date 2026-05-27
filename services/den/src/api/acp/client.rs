use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use uuid::Uuid;

use crate::{
    api::acp::AcpPromptRequest,
    core::{bears::BearAgentRole, den_tools},
    errors::CustomError,
};

pub(crate) fn tools_enabled_for_client(client: &str) -> bool {
    let normalized = normalize_acp_client(Some(client));
    matches!(
        normalized.as_str(),
        "zed" | "cursor" | "vscode" | "windsurf"
    )
}

pub(crate) fn normalize_acp_requested_mode(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ask" => Some("ask"),
        "plan" => Some("plan"),
        "write" => Some("write"),
        _ => None,
    }
}

pub(crate) fn requested_mode_from_prompt(
    body: &AcpPromptRequest,
) -> Result<Option<&'static str>, CustomError> {
    let Some(raw) = body
        .requested_mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    normalize_acp_requested_mode(raw).map(Some).ok_or_else(|| {
        CustomError::ValidationError("requested_mode must be one of ask, plan, write".to_string())
    })
}

pub(crate) fn normalize_acp_client(raw: Option<&str>) -> String {
    let value = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("acp_adapter");
    match value.to_ascii_lowercase().as_str() {
        "zed" => "zed".to_string(),
        "opencode" => "opencode".to_string(),
        _ => "acp_adapter".to_string(),
    }
}

pub(crate) fn new_acp_conversation_id(client: &str) -> String {
    let uuid = Uuid::new_v4();
    format!(
        "new-acp-{client}-{}",
        URL_SAFE_NO_PAD.encode(uuid.as_bytes())
    )
}

pub(crate) fn acp_pair_den_tool_descriptors() -> serde_json::Value {
    let descriptors = den_tools::builtin_den_tool_descriptors_for_role(BearAgentRole::Pair)
        .into_iter()
        .filter(|descriptor| {
            matches!(
                descriptor.name,
                den_tools::DEN_CONVERSATION_SET_TITLE
                    | den_tools::DEN_WEB_FETCH
                    | den_tools::DEN_WEB_SEARCH
                    | den_tools::DEN_SITUATION_GET
                    | den_tools::DEN_MEMORY_WRITE_ENTRY
                    | den_tools::DEN_MEMORY_STATUS
                    | den_tools::DEN_MEMORY_TREE
                    | den_tools::DEN_MEMORY_READ
                    | den_tools::DEN_MEMORY_SEARCH
                    | den_tools::DEN_MEMORY_REQUEST_REVIEW
                    | den_tools::DEN_WORK_PLAN_LIST
                    | den_tools::DEN_WORK_PLAN_GET_STATUS
                    | den_tools::DEN_WORK_PLAN_UPDATE
                    | den_tools::DEN_WORK_PLAN_REQUEST_HANDOFF
            )
        })
        .map(|descriptor| {
            serde_json::json!({
                "name": descriptor.provider_name,
                "description": format!(
                    "Den server tool ({}). {}",
                    descriptor.name, descriptor.description
                ),
                "parameters": descriptor.input_schema,
                "x-bears-domain": descriptor.domain,
                "x-bears-content-class": descriptor.content_class,
                "x-bears-display": descriptor.display,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!(descriptors)
}

pub(crate) fn merge_acp_pair_tool_descriptors(client_tools: serde_json::Value) -> serde_json::Value {
    let mut merged = client_tools.as_array().cloned().unwrap_or_default();
    if let Some(server_tools) = acp_pair_den_tool_descriptors().as_array() {
        merged.extend(server_tools.iter().cloned());
    }
    serde_json::json!(merged)
}

