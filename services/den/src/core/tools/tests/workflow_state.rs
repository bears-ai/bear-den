use serde_json::json;
use uuid::Uuid;
fn pair_context() -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        binding_id: "agent".to_string(),
        profile: Some(den_service::bears::BearProfile::Pair),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("client-test".to_string()),
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: vec!["/workspace".to_string()],
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: None,
        channel: Default::default(),
    }
}

use crate::core::tools::{
    activity_payloads::{activity_payload, no_active_workplan_payload, plan_mode_workplan_payload},
    descriptor::builtin_den_tool_descriptor_for_provider_name,
    memory_write::MemoryWriteEntryArguments,
    session::{invoke_den_tool, DenToolInvocationContext},
    support::validate_memory_write_entry_semantics,
};
use den_core::client_tools::{provider_tool_descriptor, ClientToolName};
use den_service::bears::BearProfile;
use den_core::tools::preflight::{tool_warning_payload, ToolSemanticWarning};
use den_docket::{TaskListItemStatus, TaskListLocalProjection, TaskListUpdateItem};
use den_runtime::plan_mode::PlanModeSessionRow;

#[test]
fn descriptor_exposes_turn_state_domain_metadata() {
    let descriptor = builtin_den_tool_descriptor_for_provider_name("exit_plan_mode").unwrap();
    assert_eq!(descriptor.domain, "workplan");
    assert_eq!(descriptor.content_class, Some("workplan_artifact"));

    let descriptor = builtin_den_tool_descriptor_for_provider_name("update_task_list").unwrap();
    assert_eq!(descriptor.domain, "activity");
    assert_eq!(descriptor.content_class, Some("activity_status"));

    let descriptor = builtin_den_tool_descriptor_for_provider_name("memory_write_entry").unwrap();
    assert_eq!(descriptor.domain, "memory");
    assert_eq!(descriptor.content_class, Some("semantic_memory"));
}

#[test]
fn armature_client_descriptors_expose_execution_domain_metadata() {
    let descriptor = provider_tool_descriptor(ClientToolName::ReadTextFile);
    assert_eq!(descriptor["name"], "fs_read_text_file");
    assert!(descriptor["description"]
        .as_str()
        .is_some_and(|text| text.contains("armature.fs.read_text_file")));
}

#[test]
fn plan_mode_payload_is_workplan_native() {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    let row = PlanModeSessionRow {
        id: uuid::Uuid::nil(),
        user_id: 1,
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        client_session_id: "client-test".to_string(),
        state: "submitted".to_string(),
        reason: "test".to_string(),
        requested_by: "pair".to_string(),
        previous_permission_mode: Some("ask".to_string()),
        plan_artifact_path: Some("pair/plans/plan.md".to_string()),
        plan_title: Some("Test plan".to_string()),
        plan_body: Some("Do the implementation.".to_string()),
        approval_request_id: None,
        approved_by_user_id: None,
        approved_at: None,
        rejected_at: None,
        closed_at: None,
        created_at: now,
        updated_at: now,
    };

    let payload = plan_mode_workplan_payload(&row);
    assert_eq!(payload["domain"], "workplan");
    assert_eq!(payload["state"], "submitted_waiting_approval");
    assert_eq!(payload["approval_status"], "awaiting_human_approval");
    assert_eq!(payload["submitted_plan_present"], true);

    let inactive = no_active_workplan_payload();
    assert_eq!(inactive["domain"], "workplan");
    assert_eq!(inactive["state"], "inactive");
}

#[test]
fn work_plan_payload_is_activity_native() {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    let item = TaskListUpdateItem {
        id: "item-1".to_string(),
        title: "Implement".to_string(),
        summary: None,
        status: TaskListItemStatus::InProgress,
        blocked_reason: None,
        source_refs: Vec::new(),
    };
    let plan = TaskListLocalProjection {
        id: uuid::Uuid::nil(),
        bear_id: uuid::Uuid::nil(),
        title: "Activity".to_string(),
        summary: "Current work".to_string(),
        owner_profile: "pair".to_string(),
        visibility: "same_user".to_string(),
        status: "active".to_string(),
        version: 1,
        items: vec![item.clone()],
        current_item: Some(item),
        source_conversation_id: Some("conv".to_string()),
        source_client_session_id: Some("armature".to_string()),
        handoff_intent_path: None,
        handoff_task_id: None,
        created_at: now,
        updated_at: now,
    };

    let payload = activity_payload(Some(&plan));
    assert_eq!(payload["domain"], "activity");
    assert_eq!(payload["status"], "active");
    assert_eq!(payload["current_item"]["title"], "Implement");
}

#[test]
fn memory_write_entry_semantics_reject_non_memory_domain_before_db_access() {
    let args: MemoryWriteEntryArguments = serde_json::from_value(json!({
        "kind": "note",
        "title": "workflow-ish",
        "body": "do thing",
        "domain": "workplan"
    }))
    .unwrap();

    let err = validate_memory_write_entry_semantics(&args, &pair_context())
        .unwrap_err()
        .to_string();
    assert!(err.contains("workplan") || err.contains("plan-mode"));
}

#[test]
fn memory_write_entry_semantics_reject_activity_domain_before_db_access() {
    let args: MemoryWriteEntryArguments = serde_json::from_value(json!({
        "kind": "summary",
        "title": "activity status",
        "body": "item one is in progress",
        "domain": "activity"
    }))
    .unwrap();

    let err = validate_memory_write_entry_semantics(&args, &pair_context())
        .unwrap_err()
        .to_string();
    assert!(err.contains("activity") || err.contains("update_task_list"));
}

#[test]
fn memory_write_entry_semantics_reject_unlabeled_plan_task_result_and_observation_content() {
    let cases = [
        (
            "plan-like",
            json!({
                "kind": "note",
                "title": "Implementation plan",
                "body": "Phase 1: inspect\nPhase 2: edit\nPhase 3: test"
            }),
            "workplan",
        ),
        (
            "task-like",
            json!({
                "kind": "summary",
                "title": "Current tasks",
                "body": "- [ ] inspect files\n- [ ] edit implementation\n- [ ] run tests"
            }),
            "task",
        ),
        (
            "run-result-like",
            json!({
                "kind": "log",
                "title": "cargo test result",
                "body": "cargo test exited with exit code 101; stderr contained failed tests"
            }),
            "run result",
        ),
        (
            "observation-like",
            json!({
                "kind": "note",
                "title": "Observation",
                "body": "API latency alert detected during telemetry review",
                "domain": "execution"
            }),
            "observation",
        ),
    ];

    for (label, value, expected) in cases {
        let args: MemoryWriteEntryArguments = serde_json::from_value(value).unwrap();
        let result = validate_memory_write_entry_semantics(&args, &pair_context());
        let err = match result {
            Err(err) => err.to_string(),
            Ok(kind) => panic!("{label} unexpectedly allowed with kind {kind}"),
        };
        assert!(
            err.to_ascii_lowercase().contains(expected),
            "{label} should mention {expected}, got {err}"
        );
    }
}

#[test]
fn memory_write_entry_semantics_allows_plain_semantic_memory() {
    let args: MemoryWriteEntryArguments = serde_json::from_value(json!({
        "kind": "decision",
        "title": "Prefer descriptor-owned naming",
        "body": "Provider-facing tool names should stay concise, while descriptor metadata carries ontology and permission information."
    }))
    .unwrap();

    let kind = validate_memory_write_entry_semantics(&args, &pair_context()).unwrap();
    assert_eq!(kind, "decision");
}

#[tokio::test]
async fn memory_write_entry_returns_warning_payload_for_ambiguous_plan_like_memory() {
    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let config = crate::config::Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let result = invoke_den_tool(
        &pool,
        &config,
        &stores,
        "den.memory.write_entry",
        json!({
            "kind": "note",
            "title": "Plan concepts",
            "body": "High-level understanding of the architecture: how plan artifacts differ from live progress tracking and why the distinction matters for durable memory."
        }),
        pair_context(),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "warning");
    assert_eq!(result["warning"]["code"], "semantic_confirmation_required");
    assert_eq!(result["warning"]["category"], "plan_like_memory");
    assert!(
        result["warning"]["confirmation_token"]
            .as_str()
            .unwrap()
            .len()
            > 10
    );
}

#[tokio::test]
async fn confirm_work_surface_requires_capability_before_database_access() {
    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let mut context = pair_context();
    context.profile = Some(BearProfile::Chat);

    let err = super::super::confirm_work_surface(
        &pool,
        &context,
        BearProfile::Chat,
        json!({"work_surface_id": Uuid::new_v4()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(err.contains("UseWorkSurfaces"), "got: {err}");
}

#[tokio::test]
async fn confirm_work_surface_requires_client_session_before_database_access() {
    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let mut context = pair_context();
    context.client_session_id = None;

    let err = super::super::confirm_work_surface(
        &pool,
        &context,
        BearProfile::Pair,
        json!({"work_surface_id": Uuid::new_v4()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(err.contains("active client session"), "got: {err}");
}

#[tokio::test]
async fn create_task_without_job_requires_current_session_before_database_access() {
    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let mut context = pair_context();
    context.client_session_id = None;
    let args: super::super::DocketTaskCreateArguments = serde_json::from_value(json!({
        "title": "Session task",
        "body": "Track the slice.",
        "completion_criteria": ["task is tracked"]
    }))
    .unwrap();

    let err = super::super::resolve_task_session_anchor_id(&pool, &context, args.job_id)
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("current client session"));
}

#[test]
fn work_run_surface_projection_keeps_pair_report_only() {
    assert_eq!(
        super::super::execution_surface_kind("sandbox"),
        "work_sandbox"
    );
    assert!(super::super::work_run_may_contain_partial_changes(
        "blocked"
    ));
    assert!(super::super::work_run_may_contain_partial_changes("failed"));
    assert!(!super::super::work_run_may_contain_partial_changes(
        "succeeded"
    ));
}

#[test]
fn tool_warning_payload_has_expected_shape() {
    let payload = tool_warning_payload(
        "den.memory.write_entry",
        ToolSemanticWarning {
            code: "semantic_confirmation_required",
            category: "plan_like_memory",
            message: "warning".to_string(),
            confirmation_token: "token".to_string(),
        },
    );
    assert_eq!(payload["status"], "warning");
    assert_eq!(payload["tool_name"], "den.memory.write_entry");
    assert_eq!(payload["warning"]["confirmation_token"], "token");
}

#[tokio::test]
async fn memory_write_entry_rejects_non_memory_domain_without_db_access() {
    let context = DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        binding_id: "agent".to_string(),
        profile: Some(den_service::bears::BearProfile::Pair),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("client-test".to_string()),
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: Vec::new(),
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: None,
        channel: Default::default(),
    };

    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let config = crate::config::Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let result = invoke_den_tool(
        &pool,
        &config,
        &stores,
        "den.memory.write_entry",
        json!({
            "kind": "note",
            "title": "workflow-ish",
            "body": "do thing",
            "domain": "workplan"
        }),
        context,
    )
    .await;

    let err = result.unwrap_err().to_string();
    assert!(err.contains("workplan") || err.contains("plan-mode"));
}

#[test]
fn memory_write_entry_semantics_reject_activity_content_class_before_db_access() {
    let args: MemoryWriteEntryArguments = serde_json::from_value(json!({
        "kind": "summary",
        "title": "activity-ish",
        "body": "status changed",
        "content_class": "activity_status"
    }))
    .unwrap();

    let err = validate_memory_write_entry_semantics(&args, &pair_context())
        .unwrap_err()
        .to_string();
    assert!(err.contains("activity") || err.contains("update_task_list"));
}

#[tokio::test]
async fn memory_write_entry_rejects_activity_content_class_without_db_access() {
    let context = DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        binding_id: "agent".to_string(),
        profile: Some(den_service::bears::BearProfile::Pair),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("client-test".to_string()),
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: Vec::new(),
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: None,
        channel: Default::default(),
    };

    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let config = crate::config::Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let result = invoke_den_tool(
        &pool,
        &config,
        &stores,
        "den.memory.write_entry",
        json!({
            "kind": "summary",
            "title": "activity-ish",
            "body": "status changed",
            "content_class": "activity_status"
        }),
        context,
    )
    .await;

    let err = result.unwrap_err().to_string();
    assert!(err.contains("activity") || err.contains("update_task_list"));
}
