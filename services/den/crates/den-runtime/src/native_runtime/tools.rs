use den_core::{
    config::Config,
    tools::constants::{
        DEN_CABINET_CREATE_PROVIDER, DEN_CABINET_HISTORY_PROVIDER, DEN_CABINET_LIFECYCLE_PROVIDER,
        DEN_CABINET_READ_PROVIDER, DEN_CABINET_SEARCH_PROVIDER, DEN_CABINET_SOURCE_LINK_PROVIDER,
        DEN_CABINET_UPDATE_PROVIDER, DEN_JOB_ARCHIVE_PROVIDER, DEN_JOB_CANCEL_PROVIDER,
        DEN_JOB_CANCEL_RUN_PROVIDER, DEN_JOB_CREATE_PROVIDER, DEN_JOB_EVALUATE_CRITERION_PROVIDER,
        DEN_JOB_EXECUTE_PROVIDER, DEN_JOB_GET_PROVIDER, DEN_JOB_LIST_PROVIDER,
        DEN_JOB_RECONCILE_PROVIDER, DEN_JOB_SETTLE_TASK_PROVIDER, DEN_JOB_UPDATE_PROVIDER,
        DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER, DEN_TASK_CREATE_PROVIDER, DEN_TASK_FOCUS_PROVIDER,
        DEN_TASK_LISTS_GET_STATUS_PROVIDER, DEN_TASK_LISTS_LIST_PROVIDER,
        DEN_TASK_LISTS_REQUEST_HANDOFF_PROVIDER, DEN_TASK_LISTS_UPDATE_PROVIDER,
        DEN_TASK_LIST_CHECKOUT_PROVIDER, DEN_TASK_LIST_PROVIDER, DEN_TASK_LIST_SYNC_PROVIDER,
        DEN_TASK_SELECT_PROVIDER, DEN_TASK_UPDATE_CURRENT_STATUS_PROVIDER,
        DEN_TASK_UPDATE_PROVIDER, DEN_WEB_SEARCH, DEN_WORK_CATALOG_PROVIDER,
        DEN_WORK_DISPATCH_PROVIDER, DEN_WORK_PREPARE_RUST_DEPENDENCIES,
        DEN_WORK_RUN_CANCEL_PROVIDER, DEN_WORK_RUN_GET_PROVIDER, DEN_WORK_RUN_LIST_PROVIDER,
    },
    DenError, TurnExecutionOrigin,
};
use serde_json::Value;

use crate::llm::LlmToolDefinition;
use den_core::tools::descriptor::{
    builtin_den_tool_descriptor_for_provider_name, builtin_den_tool_descriptors_for_origin,
    builtin_den_tool_descriptors_for_pair_acp_origin, DenToolDescriptor,
};

use super::legacy_memory_tools::{
    filter_client_tools_for_native_runtime, is_legacy_memory_client_tool_name,
};

fn den_tool_to_llm_definition(descriptor: &DenToolDescriptor, compact: bool) -> LlmToolDefinition {
    let compact = compact && descriptor.name != den_core::tools::constants::DEN_REPOSITORY_HEAD;
    LlmToolDefinition {
        name: descriptor.provider_name.clone(),
        description: Some(if compact {
            descriptor.label.to_string()
        } else {
            descriptor.description.to_string()
        }),
        parameters: descriptor.input_schema.clone(),
    }
}

fn den_tools_for_origin(
    origin: TurnExecutionOrigin,
    capabilities: &den_core::CapabilitySet,
) -> Vec<LlmToolDefinition> {
    let descriptors = if capabilities.contains(den_core::BearCapability::OwnSessionTasks) {
        builtin_den_tool_descriptors_for_pair_acp_origin(origin)
    } else {
        builtin_den_tool_descriptors_for_origin(origin)
    };
    descriptors
        .into_iter()
        .map(|descriptor| den_tool_to_llm_definition(&descriptor, true))
        .collect()
}

pub(crate) fn omit_unbounded_cargo_helper(tools: &mut Vec<LlmToolDefinition>) {
    tools.retain(|tool| {
        builtin_den_tool_descriptor_for_provider_name(&tool.name)
            .is_none_or(|descriptor| descriptor.name != DEN_WORK_PREPARE_RUST_DEPENDENCIES)
    });
}

pub fn is_work_tool_provider_name(name: &str) -> bool {
    matches!(
        name,
        DEN_TASK_LISTS_LIST_PROVIDER
            | DEN_TASK_LISTS_GET_STATUS_PROVIDER
            | DEN_TASK_LISTS_UPDATE_PROVIDER
            | DEN_TASK_LISTS_REQUEST_HANDOFF_PROVIDER
            | DEN_JOB_CREATE_PROVIDER
            | DEN_JOB_LIST_PROVIDER
            | DEN_JOB_GET_PROVIDER
            | DEN_JOB_UPDATE_PROVIDER
            | DEN_JOB_CANCEL_PROVIDER
            | DEN_JOB_CANCEL_RUN_PROVIDER
            | DEN_JOB_ARCHIVE_PROVIDER
            | DEN_JOB_EXECUTE_PROVIDER
            | DEN_JOB_RECONCILE_PROVIDER
            | DEN_JOB_SETTLE_TASK_PROVIDER
            | DEN_JOB_EVALUATE_CRITERION_PROVIDER
            | DEN_TASK_CREATE_PROVIDER
            | DEN_TASK_LIST_PROVIDER
            | DEN_TASK_UPDATE_PROVIDER
            | DEN_TASK_SELECT_PROVIDER
            | DEN_TASK_FOCUS_PROVIDER
            | DEN_TASK_UPDATE_CURRENT_STATUS_PROVIDER
            | DEN_TASK_LIST_SYNC_PROVIDER
            | DEN_TASK_LIST_CHECKOUT_PROVIDER
            | DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER
            | DEN_WORK_DISPATCH_PROVIDER
            | DEN_WORK_RUN_LIST_PROVIDER
            | DEN_WORK_RUN_GET_PROVIDER
            | DEN_WORK_RUN_CANCEL_PROVIDER
            | DEN_WORK_CATALOG_PROVIDER
    )
}

pub fn is_task_definition_or_delegation_tool_provider_name(name: &str) -> bool {
    matches!(
        name,
        DEN_TASK_LISTS_UPDATE_PROVIDER
            | DEN_TASK_LISTS_REQUEST_HANDOFF_PROVIDER
            | DEN_WORK_DISPATCH_PROVIDER
    )
}

pub fn is_cabinet_tool_provider_name(name: &str) -> bool {
    matches!(
        name,
        DEN_CABINET_SEARCH_PROVIDER
            | DEN_CABINET_READ_PROVIDER
            | DEN_CABINET_CREATE_PROVIDER
            | DEN_CABINET_UPDATE_PROVIDER
            | DEN_CABINET_HISTORY_PROVIDER
            | DEN_CABINET_SOURCE_LINK_PROVIDER
            | DEN_CABINET_LIFECYCLE_PROVIDER
    )
}

/// Collapse duplicate forwarded MCP tools that share the same action suffix, e.g.
/// `mcp__chrome_devtools_mcp_zed__click` and `mcp__chrome_devtools_custom__click`.
fn mcp_client_tool_dedup_key(name: &str) -> Option<&str> {
    if !name.starts_with("mcp__") {
        return None;
    }
    name.rsplit_once("__").map(|(_, action)| action)
}

fn compact_client_tool_description(description: Option<&str>) -> Option<String> {
    let description = description?.trim();
    if description.is_empty() {
        return None;
    }
    let first_sentence = description
        .split_once(". ")
        .map(|(head, _)| head)
        .unwrap_or(description);
    let compact = if first_sentence.len() > 96 {
        // Back off to a UTF-8 char boundary so multi-byte input can't panic.
        let mut end = 96;
        while end > 0 && !first_sentence.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &first_sentence[..end])
    } else {
        first_sentence.to_string()
    };
    Some(compact)
}

pub(crate) struct ToolSurfacePolicy {
    pub work_enabled: bool,
    pub cabinet_enabled: bool,
    pub may_define_task: bool,
}

pub fn merge_den_and_client_tools(
    config: &Config,
    origin: TurnExecutionOrigin,
    work_enabled: bool,
    cabinet_enabled: bool,
    may_define_task: bool,
    client_tools: Option<&Value>,
    pair_turn_prompt: Option<&str>,
) -> Result<Vec<LlmToolDefinition>, DenError> {
    merge_den_and_client_tools_with_search(
        config,
        origin,
        ToolSurfacePolicy {
            work_enabled,
            cabinet_enabled,
            may_define_task,
        },
        client_tools,
        pair_turn_prompt,
        true,
    )
}

pub(crate) fn merge_den_and_client_tools_with_search(
    _config: &Config,
    origin: TurnExecutionOrigin,
    policy: ToolSurfacePolicy,
    client_tools: Option<&Value>,
    _pair_turn_prompt: Option<&str>,
    search_available: bool,
) -> Result<Vec<LlmToolDefinition>, DenError> {
    let effective_policy =
        den_core::EffectivePolicy::compile_for_origin(origin, den_core::Governance::Interactive);
    let role = effective_policy.context_label;
    let mut merged = den_tools_for_origin(origin, &effective_policy.capabilities);
    if !search_available {
        merged.retain(|tool| {
            builtin_den_tool_descriptor_for_provider_name(&tool.name)
                .is_none_or(|descriptor| descriptor.name != DEN_WEB_SEARCH)
        });
    }
    if !policy.work_enabled {
        merged.retain(|tool| !is_work_tool_provider_name(&tool.name));
    }
    if !policy.cabinet_enabled {
        merged.retain(|tool| !is_cabinet_tool_provider_name(&tool.name));
    }
    if !policy.may_define_task {
        merged.retain(|tool| !is_task_definition_or_delegation_tool_provider_name(&tool.name));
    }
    // A client-supplied descriptor list cannot turn an internal curation or
    // channel run into a trusted armature. Effective policy is the grant, not
    // the mere presence of `client_tools` or the hat's identity text.
    if !effective_policy
        .capabilities
        .contains(den_core::BearCapability::UseArmatureTools)
    {
        return Ok(merged);
    }
    let filtered_client_tools = filter_client_tools_for_native_runtime(client_tools);
    let Some(client_tools) = filtered_client_tools.as_ref().and_then(|v| v.as_array()) else {
        return Ok(merged);
    };
    let compact = true;
    let mut seen = std::collections::HashSet::<String>::new();
    let mut seen_mcp_actions = std::collections::HashSet::<String>::new();
    for tool in &merged {
        seen.insert(tool.name.clone());
    }
    let mut skipped_mcp_duplicates = 0usize;
    for item in client_tools {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(name) = name else {
            continue;
        };
        if is_legacy_memory_client_tool_name(name)
            || (!search_available
                && builtin_den_tool_descriptor_for_provider_name(name)
                    .is_some_and(|descriptor| descriptor.name == DEN_WEB_SEARCH))
        {
            continue;
        }
        if let Some(action) = mcp_client_tool_dedup_key(name) {
            if !seen_mcp_actions.insert(action.to_string()) {
                skipped_mcp_duplicates += 1;
                continue;
            }
        }
        if !seen.insert(name.to_string()) {
            continue;
        }
        merged.push(LlmToolDefinition {
            name: name.to_string(),
            description: if compact {
                compact_client_tool_description(item.get("description").and_then(|v| v.as_str()))
            } else {
                item.get("description")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            },
            parameters: item
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}})),
        });
    }
    if skipped_mcp_duplicates > 0 {
        tracing::info!(
            skipped_mcp_duplicates,
            merged_tool_count = merged.len(),
            "deduplicated forwarded MCP client tools with identical action suffixes"
        );
    }
    tracing::info!(
        role = %role.as_str(),
        den_tool_count = merged.len(),
        client_tool_count = client_tools.len(),
        "merged native turn tool surface"
    );
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use den_core::{config::Config, ArmatureAvailability, RuntimeContextLabel};

    // Exercise the compatibility profiles through the origin-owned production
    // roster; the profile is never passed to the actual policy compiler.
    fn merge_den_and_client_tools(
        config: &Config,
        role: RuntimeContextLabel,
        work_enabled: bool,
        cabinet_enabled: bool,
        may_define_task: bool,
        client_tools: Option<&Value>,
        prompt: Option<&str>,
    ) -> Result<Vec<LlmToolDefinition>, DenError> {
        let armature = if client_tools.is_some() {
            ArmatureAvailability::Connected
        } else {
            ArmatureAvailability::Absent
        };
        let origin = match role {
            RuntimeContextLabel::ChannelConversation => TurnExecutionOrigin::ChannelConversation,
            RuntimeContextLabel::ArmatureConversation => {
                TurnExecutionOrigin::ArmatureConversation(armature)
            }
            RuntimeContextLabel::JobRun => TurnExecutionOrigin::AuthorizedWorkRun(armature),
            RuntimeContextLabel::Curation => TurnExecutionOrigin::InternalCuration,
            RuntimeContextLabel::Observation => TurnExecutionOrigin::InboundObservation,
        };
        super::merge_den_and_client_tools(
            config,
            origin,
            work_enabled,
            cabinet_enabled,
            may_define_task,
            client_tools,
            prompt,
        )
    }

    fn native_test_config() -> Config {
        Config::test_stub()
    }

    #[test]
    fn hat_work_roster_can_remove_the_unbounded_cargo_helper() {
        let config = native_test_config();
        let mut tools = super::merge_den_and_client_tools(
            &config,
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
            true,
            true,
            true,
            None,
            None,
        )
        .unwrap();
        let before = tools.len();
        assert!(tools.iter().any(|tool| {
            builtin_den_tool_descriptor_for_provider_name(&tool.name)
                .is_some_and(|descriptor| descriptor.name == DEN_WORK_PREPARE_RUST_DEPENDENCIES)
        }));
        omit_unbounded_cargo_helper(&mut tools);
        assert_eq!(tools.len(), before - 1);
        assert!(!tools.iter().any(|tool| {
            builtin_den_tool_descriptor_for_provider_name(&tool.name)
                .is_some_and(|descriptor| descriptor.name == DEN_WORK_PREPARE_RUST_DEPENDENCIES)
        }));
    }

    #[test]
    fn verified_origin_controls_native_tool_roster_even_with_forwarded_client_descriptors() {
        let config = native_test_config();
        let fake = serde_json::json!([
            {"name": "fs_read_text_file", "parameters": {"type": "object"}},
            {"name": "mcp__outside__send", "parameters": {"type": "object"}}
        ]);
        for origin in [
            TurnExecutionOrigin::ChannelConversation,
            TurnExecutionOrigin::BrowserTaskSession,
            TurnExecutionOrigin::InternalCuration,
            TurnExecutionOrigin::InboundObservation,
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        ] {
            let tools = super::merge_den_and_client_tools(
                &config,
                origin,
                true,
                true,
                true,
                Some(&fake),
                Some("use a tool"),
            )
            .unwrap();
            assert!(
                !tools.iter().any(|tool| tool.name == "fs_read_text_file"),
                "{origin:?}"
            );
            assert!(
                !tools.iter().any(|tool| tool.name == "mcp__outside__send"),
                "{origin:?}"
            );
        }
        let work = super::merge_den_and_client_tools(
            &config,
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            true,
            true,
            true,
            Some(&fake),
            None,
        )
        .unwrap();
        assert!(work.iter().any(|tool| tool.name == "fs_read_text_file"));
        assert!(work.iter().any(|tool| tool.name == "mcp__outside__send"));
        assert!(!work.iter().any(|tool| tool.name == "create_job"));
    }

    #[test]
    fn denied_search_is_not_advertised_or_resurrected_by_client_descriptors() {
        let config = native_test_config();
        let client = serde_json::json!([
            {"name": "web_search", "parameters": {"type": "object"}},
            {"name": "den.web.search", "parameters": {"type": "object"}},
            {"name": "fs_read_text_file", "parameters": {"type": "object"}}
        ]);
        let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
        let denied = super::merge_den_and_client_tools_with_search(
            &config,
            origin,
            ToolSurfacePolicy {
                work_enabled: true,
                cabinet_enabled: true,
                may_define_task: true,
            },
            Some(&client),
            None,
            false,
        )
        .unwrap();
        assert!(!denied
            .iter()
            .any(
                |tool| builtin_den_tool_descriptor_for_provider_name(&tool.name)
                    .is_some_and(|descriptor| descriptor.name == DEN_WEB_SEARCH)
            ));
        assert!(denied.iter().any(|tool| tool.name == "session_info"));
        assert!(denied.iter().any(|tool| tool.name == "fs_read_text_file"));
        let permitted = super::merge_den_and_client_tools_with_search(
            &config,
            origin,
            ToolSurfacePolicy {
                work_enabled: true,
                cabinet_enabled: true,
                may_define_task: true,
            },
            Some(&client),
            None,
            true,
        )
        .unwrap();
        assert!(permitted.iter().any(|tool| tool.name == "web_search"));
    }

    #[test]
    fn pair_surface_includes_docket_recovery_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ArmatureConversation,
            true,
            true,
            true,
            None,
            Some("work on the current task"),
        )
        .unwrap();
        let names: std::collections::HashSet<_> =
            merged.iter().map(|tool| tool.name.as_str()).collect();

        assert!(names.contains(DEN_TASK_SELECT_PROVIDER));
        assert!(names.contains(DEN_TASK_FOCUS_PROVIDER));
        assert!(is_work_tool_provider_name(DEN_TASK_FOCUS_PROVIDER));
        assert!(names.contains(DEN_JOB_RECONCILE_PROVIDER));
        assert!(names.contains(DEN_JOB_SETTLE_TASK_PROVIDER));
        assert!(names.contains(DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER));
        assert!(is_work_tool_provider_name(
            DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER
        ));
    }

    #[test]
    fn mcp_dedup_key_uses_action_suffix() {
        assert_eq!(
            mcp_client_tool_dedup_key("mcp__chrome_devtools_mcp_zed__click"),
            Some("click")
        );
    }

    #[test]
    fn merge_skips_duplicate_mcp_action_suffixes() {
        let config = native_test_config();
        let client_tools = serde_json::json!([
            {"name": "mcp__chrome_devtools_mcp_zed__click", "parameters": {"type": "object"}},
            {"name": "mcp__chrome_devtools_custom__click", "parameters": {"type": "object"}},
            {"name": "fs_read_text_file", "parameters": {"type": "object"}},
        ]);
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ArmatureConversation,
            true,
            true,
            true,
            Some(&client_tools),
            Some("click the browser page button"),
        )
        .unwrap();
        let names: Vec<_> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"mcp__chrome_devtools_mcp_zed__click"));
        assert!(!names.contains(&"mcp__chrome_devtools_custom__click"));
        assert!(names.contains(&"fs_read_text_file"));
        assert!(names.contains(&"session_info"));
    }

    #[test]
    fn pair_memory_question_keeps_stable_den_and_client_tool_surface() {
        let config = native_test_config();
        let client_tools = serde_json::json!([
            {"name": "fs_read_text_file", "parameters": {"type": "object"}},
            {"name": "mcp__chrome_devtools_mcp_zed__click", "parameters": {"type": "object"}},
        ]);
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ArmatureConversation,
            true,
            true,
            true,
            Some(&client_tools),
            Some("what do you know about me?"),
        )
        .unwrap();
        let names: Vec<_> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"session_info"));
        assert!(names.contains(&"fs_read_text_file"));
        assert!(names.iter().any(|name| name.starts_with("mcp__")));
    }

    #[test]
    fn pair_read_prompt_keeps_stable_den_and_client_tool_surface() {
        let config = native_test_config();
        let client_tools = serde_json::json!([
            {"name": "fs_read_text_file", "parameters": {"type": "object"}},
            {"name": "fs_find_paths", "parameters": {"type": "object"}},
            {"name": "fs_edit_file", "parameters": {"type": "object"}},
            {"name": "terminal_run_command", "parameters": {"type": "object"}},
            {"name": "mcp__chrome_devtools_mcp_zed__click", "parameters": {"type": "object"}}
        ]);
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ArmatureConversation,
            true,
            true,
            true,
            Some(&client_tools),
            Some("please read README.md"),
        )
        .unwrap();
        let names: Vec<_> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"fs_read_text_file"));
        assert!(names.contains(&"fs_find_paths"));
        assert!(names.contains(&"fs_edit_file"));
        assert!(names.contains(&"terminal_run_command"));
        assert!(names.iter().any(|name| name.starts_with("mcp__")));
        assert!(names.contains(&"session_info"));
    }

    #[test]
    fn pair_workspace_edit_prompt_includes_write_client_tools() {
        let config = native_test_config();
        let client_tools = serde_json::json!([
            {"name": "fs_read_text_file", "parameters": {"type": "object"}},
            {"name": "fs_edit_file", "parameters": {"type": "object"}},
        ]);
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ArmatureConversation,
            true,
            true,
            true,
            Some(&client_tools),
            Some("please edit the file src/lib.rs"),
        )
        .unwrap();
        let names: Vec<_> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"fs_read_text_file"));
        assert!(names.contains(&"fs_edit_file"));
        assert!(names.contains(&"session_info"));
    }

    #[test]
    fn pair_workspace_build_prompt_includes_terminal_client_and_den_tools() {
        let config = native_test_config();
        let client_tools = serde_json::json!([
            {"name": "fs_read_text_file", "parameters": {"type": "object"}},
            {"name": "terminal_run_command", "parameters": {"type": "object"}},
            {"name": "process_run", "parameters": {"type": "object"}},
            {"name": "mcp__chrome_devtools_mcp_zed__click", "parameters": {"type": "object"}}
        ]);
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ArmatureConversation,
            true,
            true,
            true,
            Some(&client_tools),
            Some("please build the project and inspect errors"),
        )
        .unwrap();
        let names: Vec<_> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"terminal_run_command"));
        assert!(names.contains(&"process_run"));
        assert!(names.contains(&"fs_read_text_file"));
        assert!(names.contains(&"session_info"));
        assert!(names.iter().any(|name| name.starts_with("mcp__")));
    }

    #[test]
    fn system_operations_have_no_generic_model_tool_roster() {
        let config = native_test_config();
        for profile in [
            RuntimeContextLabel::Curation,
            RuntimeContextLabel::Observation,
        ] {
            let merged =
                merge_den_and_client_tools(&config, profile, true, true, true, None, None).unwrap();
            assert!(merged.is_empty(), "{profile:?}");
        }
    }

    #[test]
    fn untrusted_client_descriptors_cannot_grant_armature_tools_to_chat_curate_or_watch() {
        let config = native_test_config();
        let fake = serde_json::json!([
            {"name": "fs_edit_file", "parameters": {"type": "object"}},
            {"name": "terminal_run_command", "parameters": {"type": "object"}},
            {"name": "mcp__outside__send", "parameters": {"type": "object"}},
        ]);
        for profile in [
            RuntimeContextLabel::ChannelConversation,
            RuntimeContextLabel::Curation,
            RuntimeContextLabel::Observation,
        ] {
            let tools = merge_den_and_client_tools(
                &config,
                profile,
                true,
                true,
                true,
                Some(&fake),
                Some("edit the workspace"),
            )
            .unwrap();
            let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
            assert!(!names.contains(&"fs_edit_file"), "{profile:?}");
            assert!(!names.contains(&"terminal_run_command"), "{profile:?}");
            assert!(!names.contains(&"mcp__outside__send"), "{profile:?}");
        }
    }

    #[test]
    fn closed_freeform_policy_keeps_docket_planning_but_omits_work_delegation_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ArmatureConversation,
            true,
            true,
            false,
            None,
            Some("hello"),
        )
        .unwrap();
        let names: Vec<_> = merged.iter().map(|t| t.name.as_str()).collect();

        assert!(names.contains(&"list_jobs"));
        assert!(names.contains(&"get_task_list_status"));
        assert!(names.contains(&"create_job"));
        assert!(names.contains(&"update_job"));
        assert!(names.contains(&"cancel_job"));
        assert!(names.contains(&"cancel_job_run"));
        assert!(names.contains(&"archive_job"));
        assert!(names.contains(&"execute_job"));
        assert!(names.contains(&"create_task"));
        assert!(names.contains(&"update_task"));
        assert!(names.contains(&"sync_task_list"));
        assert!(names.contains(&"checkout_task_list"));
        assert!(!names.contains(&"dispatch_work"));
        assert!(!names.contains(&"update_task_list"));
        assert!(!names.contains(&"request_task_list_handoff"));
    }

    #[test]
    fn disabled_work_bear_omits_task_job_and_work_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ArmatureConversation,
            false,
            true,
            true,
            None,
            Some("please create a job"),
        )
        .unwrap();
        let names: Vec<_> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"session_info"));
        assert!(!names.contains(&"create_job"));
        assert!(!names.contains(&"list_task_lists"));
        assert!(!names.contains(&"dispatch_work"));
    }

    #[test]
    fn chat_capabilities_query_retains_den_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ChannelConversation,
            true,
            true,
            true,
            None,
            Some("list your capabilities"),
        )
        .unwrap();
        assert!(merged.iter().any(|tool| tool.name == "session_info"));
    }

    #[test]
    fn chat_memory_prompt_includes_memory_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ChannelConversation,
            true,
            true,
            true,
            None,
            Some("search memory for deployment notes"),
        )
        .unwrap();
        let names: Vec<_> = merged.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains(&"memory_search"));
        assert!(names.contains(&"session_info"));
    }

    #[test]
    fn chat_tool_roster_does_not_change_with_prompt_phrasing() {
        let config = native_test_config();
        let rosters = [
            None,
            Some("list your capabilities"),
            Some("search memory"),
            Some("hello"),
        ]
        .into_iter()
        .map(|prompt| {
            merge_den_and_client_tools(
                &config,
                RuntimeContextLabel::ChannelConversation,
                true,
                true,
                true,
                None,
                prompt,
            )
            .unwrap()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
        assert!(!rosters[0].is_empty());
        assert!(rosters.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn chat_memory_prompt_includes_den_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            RuntimeContextLabel::ChannelConversation,
            true,
            true,
            true,
            None,
            Some("search memory for deployment notes"),
        )
        .unwrap();
        assert!(!merged.is_empty());
    }
}
