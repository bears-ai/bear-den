use crate::{
    client_tools::{provider_tool_descriptor, ClientToolName},
    tools::{
        constants::{
            DEN_DOCKET_ENTRY_APPEND_PROVIDER, DEN_DOCKET_ENTRY_LIST_PROVIDER,
            DEN_JOB_CREATE_PROVIDER, DEN_JOB_RECONCILE, DEN_JOB_RECONCILE_PROVIDER,
            DEN_JOB_SETTLE_TASK, DEN_JOB_SETTLE_TASK_PROVIDER, DEN_MEMORY_WRITE_ENTRY_PROVIDER,
            DEN_PROMPT_MEMORY_UPSERT_PROVIDER, DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER,
            DEN_SITUATION_GET_PROVIDER, DEN_TASK_CREATE_PROVIDER, DEN_TASK_FOCUS_PROVIDER,
            DEN_TASK_LISTS_REQUEST_HANDOFF_PROVIDER, DEN_TASK_LISTS_UPDATE_PROVIDER,
            DEN_TASK_LIST_CHECKOUT_PROVIDER, DEN_TASK_LIST_PROVIDER, DEN_TASK_SELECT_PROVIDER,
            DEN_TASK_UPDATE_CURRENT_STATUS_PROVIDER,
        },
        descriptor::{builtin_den_tool_descriptors, builtin_den_tool_descriptors_for_profile},
    },
    BearProfile,
};

#[test]
fn apply_patch_schema_explains_required_diff_and_edit_file_fallback() {
    let schema = provider_tool_descriptor(ClientToolName::ApplyPatch)["parameters"].clone();
    let description = schema["properties"]["patch"]["description"]
        .as_str()
        .expect("patch description");

    assert!(description.contains("Standard unified diff"));
    assert!(description.contains("--- a/path"));
    assert!(description.contains("@@"));
    assert!(description.contains("Markdown fences"));
    assert!(description.contains("fs_edit_file"));
}

#[test]
fn session_info_descriptor_keeps_explicit_orientation_language() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_SITUATION_GET_PROVIDER)
        .expect("session_info descriptor");

    assert!(
        descriptor
            .description
            .contains("Trusted Den orientation tool"),
        "unexpected description: {}",
        descriptor.description
    );
    assert!(
        descriptor.description.contains("Read-only"),
        "unexpected description: {}",
        descriptor.description
    );
}

#[test]
fn memory_write_descriptor_includes_shared_guidance() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_MEMORY_WRITE_ENTRY_PROVIDER)
        .expect("memory_write_entry descriptor");

    assert!(descriptor.description.contains("Scope:"));
    assert!(descriptor.description.contains("Side effect:"));
    assert!(descriptor.description.contains("session_info"));
    assert!(descriptor
        .description
        .contains("writes role-local semantic memory"));
}

#[test]
fn task_list_update_descriptor_includes_active_work_state_guidance() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_TASK_LISTS_UPDATE_PROVIDER)
        .expect("update_task_list descriptor");

    assert!(descriptor.description.contains("Scope:"));
    assert!(descriptor
        .description
        .contains("Side effect: updates active work state"));
    assert!(descriptor.description.contains("session_info"));
    assert!(descriptor
        .description
        .contains("Use durable Docket task tools"));
    assert!(descriptor.description.contains("current Pair task"));
    assert!(descriptor
        .description
        .contains("does not start a background job"));
    assert!(descriptor
        .description
        .contains("create a Docket Job instead"));
}

#[test]
fn docket_recovery_descriptors_have_callable_provider_names() {
    let descriptors = builtin_den_tool_descriptors();

    for (name, provider_name) in [
        (DEN_JOB_RECONCILE, DEN_JOB_RECONCILE_PROVIDER),
        (DEN_JOB_SETTLE_TASK, DEN_JOB_SETTLE_TASK_PROVIDER),
    ] {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.name == name)
            .expect("Docket recovery descriptor");
        assert_eq!(descriptor.provider_name, provider_name);
    }
}
#[test]
fn docket_descriptors_distinguish_session_task_trees_from_work_jobs() {
    let descriptors = builtin_den_tool_descriptors();

    let create_job = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == DEN_JOB_CREATE_PROVIDER)
        .expect("create_job descriptor");
    assert!(create_job.description.contains("durable Docket work job"));
    assert!(create_job.description.contains("acceptance criteria"));
    assert!(create_job
        .description
        .contains("Creating a Job does not execute or dispatch it"));

    let create_task = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == DEN_TASK_CREATE_PROVIDER)
        .expect("create_task descriptor");
    assert!(create_task.description.contains("durable/resumable plans"));
    assert!(create_task
        .description
        .contains("authenticated current session"));
    assert!(create_task.description.contains("exactly one owner"));
    assert!(create_task.description.contains("does not execute work"));
    assert!(create_task
        .input_schema
        .get("properties")
        .is_some_and(|properties| properties.get("session_anchor_id").is_none()));

    let list_tasks = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == DEN_TASK_LIST_PROVIDER)
        .expect("list_tasks descriptor");
    assert!(list_tasks
        .description
        .contains("current session's implied Docket objective"));

    let checkout = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == DEN_TASK_LIST_CHECKOUT_PROVIDER)
        .expect("checkout_task_list descriptor");
    assert!(checkout
        .description
        .contains("current conversation's implied Docket objective"));
    assert!(checkout.description.contains("does not execute tasks"));
    assert!(!checkout
        .input_schema
        .get("required")
        .is_some_and(|required| {
            required
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item == "job_id"))
        }));
}

#[test]
fn select_current_task_descriptor_requires_confirmation_for_redirection() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_TASK_SELECT_PROVIDER)
        .expect("select_current_task descriptor");

    assert!(descriptor
        .description
        .contains("first ask the user to confirm the proposed task switch"));
    assert!(descriptor
        .description
        .contains("If several eligible tasks could match, ask which one to select"));
    assert!(descriptor
        .description
        .contains("If none matches, ask whether to create a new session task or continue with no selected task"));
    assert!(descriptor
        .description
        .contains("Never silently select, clear, replace, complete, or create a Pair task"));
}

#[test]
fn focus_current_task_is_pair_only_and_does_not_select() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_TASK_FOCUS_PROVIDER)
        .expect("focus_current_task descriptor");

    assert_eq!(descriptor.input_schema["properties"], serde_json::json!({}));
    assert!(descriptor
        .description
        .contains("already-selected executable task"));
    assert!(descriptor
        .description
        .contains("does not select, create, replace, or settle"));
    assert!(builtin_den_tool_descriptors_for_profile(BearProfile::Pair)
        .iter()
        .any(|descriptor| descriptor.provider_name == DEN_TASK_FOCUS_PROVIDER));
    assert!(!builtin_den_tool_descriptors_for_profile(BearProfile::Work)
        .iter()
        .any(|descriptor| descriptor.provider_name == DEN_TASK_FOCUS_PROVIDER));
}

#[test]
fn current_task_status_descriptor_exposes_compatible_outcomes() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_TASK_UPDATE_CURRENT_STATUS_PROVIDER)
        .expect("update_current_task_status descriptor");

    assert_eq!(
        descriptor.input_schema["properties"]["outcome_disposition"]["enum"],
        serde_json::json!([
            "completed",
            "no_change",
            "delegated",
            "blocked",
            "failed",
            "cancelled"
        ])
    );
    assert!(descriptor.description.contains("done accepts completed"));
    assert!(descriptor
        .description
        .contains("blocked accepts blocked or failed"));
    assert!(builtin_den_tool_descriptors_for_profile(BearProfile::Pair)
        .iter()
        .any(|descriptor| descriptor.provider_name == DEN_TASK_UPDATE_CURRENT_STATUS_PROVIDER));
    assert!(!builtin_den_tool_descriptors_for_profile(BearProfile::Work)
        .iter()
        .any(|descriptor| descriptor.provider_name == DEN_TASK_UPDATE_CURRENT_STATUS_PROVIDER));
}

#[test]
fn docket_entry_descriptors_keep_outcomes_settlement_owned() {
    let descriptors = builtin_den_tool_descriptors();
    let append = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == DEN_DOCKET_ENTRY_APPEND_PROVIDER)
        .expect("append_docket_entry descriptor");
    assert!(append.description.contains("Outcomes are settlement-owned"));
    assert_eq!(
        append.input_schema["properties"]["kind"]["enum"],
        serde_json::json!([
            "finding",
            "decision",
            "obstacle",
            "follow_up",
            "milestone",
            "question"
        ])
    );
    assert!(!append.input_schema["properties"]["kind"]["enum"]
        .as_array()
        .expect("entry kind enum")
        .iter()
        .any(|kind| kind == "outcome"));

    let list = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == DEN_DOCKET_ENTRY_LIST_PROVIDER)
        .expect("list_docket_entries descriptor");
    assert!(list.description.contains("settlement outcomes"));
    assert_eq!(list.input_schema["properties"]["limit"]["maximum"], 500);
}

#[test]
fn prompt_memory_upsert_descriptor_mentions_runtime_prompt_memory() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_PROMPT_MEMORY_UPSERT_PROVIDER)
        .expect("upsert_prompt_memory descriptor");

    assert!(descriptor
        .description
        .contains("editable runtime prompt memory"));
    assert!(descriptor.description.contains("semantic memory"));
}

#[test]
fn docket_work_descriptors_keep_execution_evidence_and_surfaces_explicit() {
    let descriptors = builtin_den_tool_descriptors();
    let dispatch = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == "dispatch_work")
        .expect("dispatch_work descriptor");
    assert!(dispatch
        .description
        .contains("isolated background execution in a sandbox"));
    assert!(dispatch
        .description
        .contains("never modifies Pair's attached checkout"));
    assert_eq!(
        dispatch.input_schema["required"],
        serde_json::json!(["job_id"])
    );
    assert!(dispatch.input_schema["properties"]["root"].is_object());
    assert!(dispatch.input_schema["properties"]["image"].is_object());
    assert!(builtin_den_tool_descriptors_for_profile(BearProfile::Pair)
        .iter()
        .any(|descriptor| descriptor.provider_name == "dispatch_work"));
    assert!(!builtin_den_tool_descriptors_for_profile(BearProfile::Work)
        .iter()
        .any(|descriptor| descriptor.provider_name == "dispatch_work"));

    let work_run = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == "get_work_run")
        .expect("get_work_run descriptor");
    assert!(work_run
        .description
        .contains("terminal result and durable evidence"));
    assert!(work_run
        .description
        .contains("not implicitly accessible to Pair"));
    assert!(work_run.description.contains("work run e4e4797b"));

    let diagnostics = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER)
        .expect("list_runtime_diagnostics descriptor");
    assert!(diagnostics.description.contains("not raw server logs"));
    assert_eq!(
        diagnostics.input_schema["properties"]["severity"]["enum"],
        serde_json::json!(["warning", "error"])
    );
    assert!(builtin_den_tool_descriptors_for_profile(BearProfile::Pair)
        .iter()
        .any(|descriptor| descriptor.provider_name == DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER));

    for (provider_name, expected_handle) in [
        ("create_job", "job e4e4797b"),
        (DEN_TASK_CREATE_PROVIDER, "task e4e4797b"),
        (DEN_TASK_LIST_PROVIDER, "short task handles"),
        ("list_work_runs", "short work-run handles"),
    ] {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.provider_name == provider_name)
            .unwrap_or_else(|| panic!("{provider_name} descriptor"));
        assert!(descriptor.description.contains(expected_handle));
    }
}

#[test]
fn task_list_handoff_descriptor_does_not_require_a_server_owned_projection() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_TASK_LISTS_REQUEST_HANDOFF_PROVIDER)
        .expect("request_task_list_handoff descriptor");

    let properties = descriptor.input_schema["properties"]
        .as_object()
        .expect("handoff properties");
    assert!(
        !properties.contains_key("task_list"),
        "the runtime must derive the task-list projection from session context"
    );
    assert!(
        !properties.contains_key("title") && !properties.contains_key("summary"),
        "the request should not repeat metadata already held by the task list"
    );
    assert_eq!(
        descriptor.input_schema["required"],
        serde_json::json!(["requested_outcome"]),
        "only the desired handoff outcome belongs in an ordinary request"
    );
}
