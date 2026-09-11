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
        DEN_TASK_UPDATE_PROVIDER, DEN_WORK_CATALOG_PROVIDER, DEN_WORK_DISPATCH_PROVIDER,
        DEN_WORK_RUN_CANCEL_PROVIDER, DEN_WORK_RUN_GET_PROVIDER, DEN_WORK_RUN_LIST_PROVIDER,
    },
    DenError,
};
use serde_json::Value;

use crate::llm::LlmToolDefinition;
use den_core::tools::descriptor::{
    builtin_den_tool_descriptors_for_pair_acp_surface, builtin_den_tool_descriptors_for_profile,
    DenToolDescriptor,
};
use den_service::bears::BearProfile;

use super::legacy_memory_tools::{
    filter_client_tools_for_native_runtime, is_legacy_memory_client_tool_name,
};

fn den_tool_to_llm_definition(descriptor: &DenToolDescriptor, compact: bool) -> LlmToolDefinition {
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

fn den_tools_for_profile(
    role: BearProfile,
    capabilities: &den_core::CapabilitySet,
) -> Vec<LlmToolDefinition> {
    let descriptors = if capabilities.contains(den_core::BearCapability::OwnSessionTasks) {
        builtin_den_tool_descriptors_for_pair_acp_surface()
    } else {
        builtin_den_tool_descriptors_for_profile(role)
    };
    descriptors
        .into_iter()
        .map(|descriptor| den_tool_to_llm_definition(&descriptor, true))
        .collect()
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

/// Browser chat turns omit the full Den tool surface unless the prompt suggests tool-relevant work.
pub fn chat_turn_needs_full_tool_surface(prompt: Option<&str>) -> bool {
    let Some(prompt) = prompt.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    if chat_turn_is_capabilities_meta_query(prompt) {
        return false;
    }
    let lower = prompt.to_ascii_lowercase();
    const PHRASES: &[&str] = &["work plan", "rename conversation", "plan mode", "https://"];
    const WORDS: &[&str] = &[
        "memory",
        "remember",
        "recall",
        "search",
        "browse",
        "workboard",
        "handoff",
        "fetch",
        "http",
        "url",
        "web",
        "title",
        "policy",
        "members",
        "proposal",
        "review",
        "curate",
        "write",
        "save",
        "update",
        "file",
        "code",
        "workspace",
    ];
    PHRASES.iter().any(|phrase| lower.contains(phrase))
        || WORDS.iter().any(|word| contains_ascii_word(&lower, word))
}

fn contains_ascii_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(start, matched)| {
        let end = start + matched.len();
        is_word_boundary(haystack[..start].chars().next_back())
            && is_word_boundary(haystack[end..].chars().next())
    })
}

fn is_word_boundary(ch: Option<char>) -> bool {
    ch.is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_')
}

pub fn chat_turn_is_capabilities_meta_query(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    const PHRASES: &[&str] = &[
        "list capabilities",
        "list your capabilities",
        "list tools",
        "list your tools",
        "what tools",
        "what capabilities",
        "which tools",
        "which capabilities",
        "what can you do",
        "what do you have access",
        "show me your tools",
        "show your tools",
        "available tools",
        "available capabilities",
        "tools do you have",
        "capabilities do you have",
    ];
    PHRASES.iter().any(|phrase| lower.contains(phrase))
}

pub fn merge_den_and_client_tools(
    _config: &Config,
    role: BearProfile,
    work_enabled: bool,
    cabinet_enabled: bool,
    may_define_task: bool,
    client_tools: Option<&Value>,
    pair_turn_prompt: Option<&str>,
) -> Result<Vec<LlmToolDefinition>, DenError> {
    let effective_policy = den_core::EffectivePolicy::compile(
        role,
        den_core::Governance::Interactive,
        if client_tools.is_some() {
            den_core::ArmatureAvailability::Connected
        } else {
            den_core::ArmatureAvailability::Absent
        },
    );
    let mut merged = if role == BearProfile::Chat
        && !chat_turn_needs_full_tool_surface(pair_turn_prompt)
    {
        tracing::info!(
            role = %role.as_str(),
            "native chat turn using empty tool surface (informational prompt; tool list is in system context)"
        );
        Vec::new()
    } else {
        den_tools_for_profile(role, &effective_policy.capabilities)
    };
    if !work_enabled {
        merged.retain(|tool| !is_work_tool_provider_name(&tool.name));
    }
    if !cabinet_enabled {
        merged.retain(|tool| !is_cabinet_tool_provider_name(&tool.name));
    }
    if !may_define_task {
        merged.retain(|tool| !is_task_definition_or_delegation_tool_provider_name(&tool.name));
    }
    if role == BearProfile::Chat {
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
        if is_legacy_memory_client_tool_name(name) {
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
    use den_core::config::Config;

    fn native_test_config() -> Config {
        Config::test_stub()
    }

    #[test]
    fn pair_surface_includes_docket_recovery_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            BearProfile::Pair,
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
            BearProfile::Pair,
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
            BearProfile::Pair,
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
            BearProfile::Pair,
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
            BearProfile::Pair,
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
            BearProfile::Pair,
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
    fn curate_profile_includes_proposal_and_core_tools() {
        let config = native_test_config();
        let merged =
            merge_den_and_client_tools(&config, BearProfile::Curate, true, true, true, None, None)
                .unwrap();
        let names: Vec<_> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"memory_list_proposals"));
        assert!(names.contains(&"memory_read_proposal"));
        assert!(names.contains(&"memory_apply_core_update"));
        assert!(names.contains(&"memory_read"));
        assert!(!names.contains(&"enter_plan_mode"));
    }

    #[test]
    fn closed_freeform_policy_keeps_docket_planning_but_omits_work_delegation_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            BearProfile::Pair,
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
            BearProfile::Pair,
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
    fn chat_capabilities_query_omits_den_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            BearProfile::Chat,
            true,
            true,
            true,
            None,
            Some("list your capabilities"),
        )
        .unwrap();
        assert!(merged.is_empty());
    }

    #[test]
    fn chat_memory_prompt_includes_memory_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            BearProfile::Chat,
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
    fn chat_prompt_keyword_matching_avoids_substring_false_positives() {
        assert!(!chat_turn_needs_full_tool_surface(Some(
            "research preview webhook ideas"
        )));
        assert!(chat_turn_needs_full_tool_surface(Some(
            "please search memory"
        )));
        assert!(chat_turn_needs_full_tool_surface(Some(
            "fetch https://example.com"
        )));
    }

    #[test]
    fn chat_memory_prompt_includes_den_tools() {
        let config = native_test_config();
        let merged = merge_den_and_client_tools(
            &config,
            BearProfile::Chat,
            true,
            true,
            true,
            None,
            Some("search memory for deployment notes"),
        )
        .unwrap();
        assert!(!merged.is_empty());
    }

    #[test]
    fn chat_turn_is_capabilities_meta_query_matches_common_phrases() {
        assert!(chat_turn_is_capabilities_meta_query("list your tools"));
        assert!(chat_turn_is_capabilities_meta_query(
            "What capabilities do you have?"
        ));
        assert!(!chat_turn_is_capabilities_meta_query(
            "search memory for onboarding"
        ));
    }
}
