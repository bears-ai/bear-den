use crate::core::tools::{
    aliases::is_builtin_den_tool,
    constants::*,
    descriptor::{
        builtin_den_tool_descriptor_for_provider_name, builtin_den_tool_descriptors,
        builtin_den_tool_descriptors_for_profile,
    },
};
use den_service::bears::RuntimeContextLabel;
use std::collections::HashSet;

#[test]
fn provider_names_are_safe_and_unique() {
    let descriptors = builtin_den_tool_descriptors();
    let mut provider_names = HashSet::new();
    for descriptor in descriptors {
        assert!(
            descriptor
                .provider_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "provider name must be provider/provider-safe: {}",
            descriptor.provider_name
        );
        assert!(!descriptor.provider_name.contains('.'));
        assert!(!descriptor.provider_name.contains('/'));
        assert!(
            provider_names.insert(descriptor.provider_name.clone()),
            "duplicate provider name: {}",
            descriptor.provider_name
        );
    }
}

#[test]
fn canonical_dotted_names_map_to_provider_safe_aliases() {
    let descriptors = builtin_den_tool_descriptors();
    let task = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_TASK_WRITE_INTENT)
        .expect("task intent descriptor exists");
    assert_eq!(task.provider_name, "den_task_write_intent");

    let skill = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_SKILL_PROPOSE)
        .expect("skill proposal descriptor exists");
    assert_eq!(skill.provider_name, "den_skill_propose");

    let conversation_title = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_CONVERSATION_SET_TITLE)
        .expect("conversation title descriptor exists");
    assert_eq!(
        conversation_title.provider_name,
        DEN_CONVERSATION_SET_TITLE_PROVIDER
    );

    let web_fetch = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_WEB_FETCH)
        .expect("web fetch descriptor exists");
    assert_eq!(web_fetch.provider_name, DEN_WEB_FETCH_PROVIDER);

    let web_search = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_WEB_SEARCH)
        .expect("web search descriptor exists");
    assert_eq!(web_search.provider_name, DEN_WEB_SEARCH_PROVIDER);

    let bear_environment = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_BEAR_ENVIRONMENT)
        .expect("bear environment descriptor exists");
    assert_eq!(
        bear_environment.provider_name,
        DEN_BEAR_ENVIRONMENT_PROVIDER
    );

    let situation = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_SITUATION_GET)
        .expect("situation descriptor exists");
    assert_eq!(situation.provider_name, DEN_SITUATION_GET_PROVIDER);
    assert_eq!(situation.provider_name, "session_info");
    assert_ne!(situation.provider_name, "situation_get");
    assert_ne!(situation.provider_name, "den_situation_get");

    let memory_browse = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_MEMORY_TREE)
        .expect("memory browse descriptor exists");
    assert_eq!(memory_browse.provider_name, DEN_MEMORY_TREE_PROVIDER);
    assert_eq!(memory_browse.provider_name, "memory_browse");
    assert_ne!(memory_browse.provider_name, "memory_tree");

    let memory = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_MEMORY_WRITE_ENTRY)
        .expect("memory write descriptor exists");
    assert_eq!(memory.provider_name, DEN_MEMORY_WRITE_ENTRY_PROVIDER);

    let entity_browse = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_ENTITY_BROWSE)
        .expect("entity browse descriptor exists");
    assert_eq!(entity_browse.provider_name, DEN_ENTITY_BROWSE_PROVIDER);
    assert_eq!(entity_browse.provider_name, "entity_browse");

    let entity_resolve = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_ENTITY_RESOLVE)
        .expect("entity resolve descriptor exists");
    assert_eq!(entity_resolve.provider_name, DEN_ENTITY_RESOLVE_PROVIDER);
    assert_eq!(entity_resolve.provider_name, "entity_resolve");

    let entity_link = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_ENTITY_LINK_MEMORY)
        .expect("entity link descriptor exists");
    assert_eq!(entity_link.provider_name, DEN_ENTITY_LINK_MEMORY_PROVIDER);
    assert_eq!(entity_link.provider_name, "entity_link_memory");

    for retired in [
        DEN_ENTITY_MERGE,
        DEN_ENTITY_SPLIT,
        DEN_ENTITY_WRITE_ACCESS_RULE,
        DEN_ENTITY_WRITE_ANCHOR,
    ] {
        assert!(descriptors
            .iter()
            .all(|descriptor| descriptor.name != retired));
    }

    let update_task_list = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_TASK_LISTS_UPDATE)
        .expect("task-list update descriptor exists");
    assert_eq!(
        update_task_list.provider_name,
        DEN_TASK_LISTS_UPDATE_PROVIDER
    );
    assert_eq!(update_task_list.provider_name, "update_task_list");
    for retired_alias in [
        "list_plans",
        "get_plan_status",
        "update_plan",
        "request_work_handoff",
    ] {
        assert!(
            builtin_den_tool_descriptor_for_provider_name(retired_alias).is_none(),
            "retired task-list alias should not resolve: {retired_alias}"
        );
    }

    let enter_plan_mode = descriptors
        .iter()
        .find(|descriptor| descriptor.name == DEN_PLAN_MODE_ENTER)
        .expect("enter plan mode descriptor exists");
    assert_eq!(enter_plan_mode.provider_name, DEN_PLAN_MODE_ENTER_PROVIDER);
    assert_eq!(enter_plan_mode.provider_name, "enter_plan_mode");
}

#[test]
fn den_server_tools_advertise_semantic_aliases_not_legacy_den_prefixes() {
    let provider_names =
        builtin_den_tool_descriptors_for_profile(RuntimeContextLabel::ArmatureConversation)
            .into_iter()
            .map(|descriptor| descriptor.provider_name)
            .collect::<HashSet<_>>();
    assert!(provider_names.contains("session_info"));
    assert!(provider_names.contains("bear_environment"));
    assert!(provider_names.contains("set_conversation_title"));
    assert!(provider_names.contains("web_search"));
    assert!(provider_names.contains("memory_browse"));
    assert!(provider_names.contains("memory_read"));
    assert!(provider_names.contains("entity_browse"));
    assert!(provider_names.contains("entity_resolve"));
    assert!(provider_names.contains("entity_link_memory"));
    assert!(provider_names.contains("update_task_list"));
    assert!(provider_names.contains("enter_plan_mode"));
    assert!(provider_names.contains("record_plan_approval"));
    assert!(provider_names.contains("exit_plan_mode"));
    assert!(provider_names.contains("cancel_plan_mode"));
    assert!(!provider_names.contains("situation_get"));
    assert!(!provider_names.contains("memory_tree"));
    assert!(!provider_names.contains("list_plans"));
    assert!(!provider_names.contains("get_plan_status"));
    assert!(!provider_names.contains("update_plan"));
    assert!(!provider_names.contains("request_work_handoff"));
    assert!(!provider_names.contains("den_situation_get"));
    assert!(!provider_names.contains("den_web_search"));
    assert!(!provider_names.contains("den_memory_read"));
    assert!(!provider_names.contains("den_work_plan_update"));
    assert!(!provider_names.contains("den_plan_mode_enter"));
}

#[test]
fn all_descriptors_are_known_tools() {
    let unknown = builtin_den_tool_descriptors()
        .into_iter()
        .map(|descriptor| descriptor.name)
        .filter(|name| !is_builtin_den_tool(name))
        .collect::<Vec<_>>();
    assert!(
        unknown.is_empty(),
        "descriptors without canonical alias resolution: {unknown:?}"
    );
}

#[test]
fn descriptor_resolver_accepts_advertised_names() {
    // Models invoke provider names, while execution uses canonical names. Both
    // forms must resolve from descriptors; unadvertised legacy aliases and
    // unknown names must not.
    assert!(is_builtin_den_tool(DEN_JOB_RECONCILE));
    assert!(is_builtin_den_tool(DEN_JOB_SETTLE_TASK));
    assert!(is_builtin_den_tool(DEN_JOB_RECONCILE_PROVIDER));
    assert!(is_builtin_den_tool(DEN_JOB_SETTLE_TASK_PROVIDER));
    assert!(!is_builtin_den_tool(DEN_MEMORY_TREE_LEGACY_PROVIDER));
    assert!(!is_builtin_den_tool("den.job.does_not_exist"));
}

#[test]
fn den_tool_display_json_includes_memory_titles() {
    use crate::core::tools::descriptor::{
        den_tool_display_json_for_provider, den_tool_policy_json_for_provider,
    };

    let read = den_tool_display_json_for_provider(
        "memory_read",
        &serde_json::json!({ "path": "pair/notes/example.md" }),
    )
    .expect("memory_read display");
    assert_eq!(read["title"], "Reading memory pair/notes/example.md");
    assert_eq!(read["progress"], "Reading memory");

    let write = den_tool_display_json_for_provider(
        "memory_write_entry",
        &serde_json::json!({ "title": "Saved fact", "path": "pair/notes/mem_1.md" }),
    )
    .expect("memory_write_entry display");
    assert!(write["title"]
        .as_str()
        .unwrap_or("")
        .starts_with("Writing memory entry"));
    assert_eq!(write["progress"], "Writing memory entry");

    let policy = den_tool_policy_json_for_provider("memory_read").expect("memory_read policy");
    assert_eq!(policy["execution_target"], "den");
}
