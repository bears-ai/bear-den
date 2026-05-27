use crate::core::{acp_sessions, den_tools};

use super::ToolExecutionRoute;

pub(crate) fn is_acp_archive_target(conversation_id: &str) -> bool {
    conversation_id.starts_with("conv-")
}

pub(crate) fn acp_archive_target_for_session(
    session: &acp_sessions::AcpSessionRow,
) -> Option<&str> {
    session
        .resolved_conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|s| is_acp_archive_target(s))
        .or_else(|| {
            let selection = session.conversation_id.trim();
            is_acp_archive_target(selection).then_some(selection)
        })
}

pub(crate) fn acp_den_provider_to_canonical_tool_name(
    provider_name: &str,
) -> Option<&'static str> {
    den_tools::builtin_den_tool_descriptor_for_provider_name(provider_name)
        .map(|descriptor| descriptor.name)
}

pub(in crate::api::acp) fn tool_execution_route(
    tool_name: &str,
    args: &serde_json::Value,
) -> ToolExecutionRoute {
    if args.get("_unsupported_detail").is_some() {
        ToolExecutionRoute::Unsupported
    } else if acp_den_provider_to_canonical_tool_name(tool_name).is_some() {
        ToolExecutionRoute::DenServer
    } else {
        ToolExecutionRoute::AdapterLocal
    }
}
