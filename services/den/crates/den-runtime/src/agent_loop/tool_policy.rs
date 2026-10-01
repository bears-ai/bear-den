use den_core::client_tools::{
    client_tool_policy, client_tool_policy_json_for_provider, ClientToolName,
};
use den_core::ids::{BearId, UserId};
use den_core::tools::{
    constants::DEN_WEB_FETCH, descriptor::builtin_den_tool_descriptor_for_provider_name,
};
use den_protocol::RuntimeSemanticEvent;
use den_service::{bears::hats::access, conversation::persistence};
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent_loop::approvals::{
    create_native_approval, decide_native_approval, NativeApprovalDecision,
};

/// Whether web chat (and other surfaces without interactive approval UI) may run this tool
/// without an explicit human approval step.
pub fn provider_tool_supports_unilateral_execution(provider_name: &str) -> bool {
    if let Some(descriptor) = builtin_den_tool_descriptor_for_provider_name(provider_name) {
        return descriptor.approval_policy == "never";
    }
    !provider_tool_requires_approval(provider_name)
}

pub fn provider_tool_is_den_web_fetch(provider_name: &str) -> bool {
    builtin_den_tool_descriptor_for_provider_name(provider_name)
        .is_some_and(|descriptor| descriptor.name == DEN_WEB_FETCH)
}

pub fn provider_tool_requires_approval(provider_name: &str) -> bool {
    if let Some(descriptor) = builtin_den_tool_descriptor_for_provider_name(provider_name) {
        return descriptor.approval_policy != "never";
    }
    client_tool_policy_json_for_provider(provider_name)
        .get("approval_required")
        .and_then(|value| value.as_bool())
        .unwrap_or_else(|| {
            ClientToolName::from_provider_alias(provider_name)
                .map(|tool| {
                    client_tool_policy(tool)
                        .approval_policy
                        .requires_unconditional_approval()
                })
                .unwrap_or(false)
        })
}

/// A native Den-hosted fetch may run without an ACP pause only when its
/// authenticated source still wears a hat with both the tool and exact host
/// grants. The fetcher independently rechecks both at the HTTP effect boundary.
pub async fn hat_web_fetch_preapproved(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
    user_id: Option<i32>,
    arguments: &serde_json::Value,
) -> Result<bool, den_core::DenError> {
    let (Some(user_id), Some(url)) = (
        user_id,
        arguments.get("url").and_then(|value| value.as_str()),
    ) else {
        return Ok(false);
    };
    let Some(conversation) =
        persistence::get_conversation_for_external_id(pool, bear_id, external_conversation_id)
            .await?
    else {
        return Ok(false);
    };
    access::has_web_fetch_grants_for_own_conversation(
        pool,
        BearId::new(bear_id),
        conversation.id,
        UserId::new(user_id),
        url,
    )
    .await
}

pub async fn maybe_pause_for_tool_approval(
    pool: &PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    client_session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Result<Option<RuntimeSemanticEvent>, den_core::DenError> {
    if !provider_tool_requires_approval(tool_name) {
        return Ok(None);
    }
    let approval_id = create_native_approval(
        pool,
        bear_id,
        conversation_id,
        client_session_id,
        tool_call_id,
        tool_name,
        arguments,
    )
    .await?;
    Ok(Some(RuntimeSemanticEvent::RunPaused {
        reason: "requires_approval".to_string(),
        resume_token: Some(approval_id),
        expires_at: None,
    }))
}

pub async fn record_approval_decision(
    pool: &PgPool,
    approval_id: &str,
    approve: bool,
    reason: Option<&str>,
) -> Result<(), den_core::DenError> {
    decide_native_approval(
        pool,
        approval_id,
        if approve {
            NativeApprovalDecision::Approve
        } else {
            NativeApprovalDecision::Deny
        },
        reason,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn den_capabilities_list_self_supports_unilateral_execution() {
        assert!(provider_tool_supports_unilateral_execution(
            "den_capabilities_list_self"
        ));
        assert!(provider_tool_supports_unilateral_execution(
            "den.capabilities.list_self"
        ));
        assert!(!provider_tool_requires_approval(
            "den_capabilities_list_self"
        ));
    }

    #[test]
    fn armature_read_only_fs_tools_support_unilateral_execution() {
        for tool in [
            "fs_read_text_file",
            "fs_list_directory",
            "fs_find_paths",
            "fs_search_files",
            "fs_stat",
        ] {
            assert!(
                provider_tool_supports_unilateral_execution(tool),
                "{tool} should not pause for permission"
            );
            assert!(
                !provider_tool_requires_approval(tool),
                "{tool} should execute without permission wait"
            );
        }
    }

    #[test]
    fn armature_mutating_tools_do_not_support_unilateral_execution() {
        assert!(!provider_tool_supports_unilateral_execution("fs_edit_file"));
        assert!(provider_tool_requires_approval("fs_edit_file"));
    }
}
