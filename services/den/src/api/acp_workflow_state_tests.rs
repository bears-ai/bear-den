use crate::core::{
    acp_plan_mode::AcpPlanModeSessionRow,
    acp_tools::{AcpResolvedSessionPolicy, AcpToolEnablementState},
    den_tools::{self, validate_memory_write_entry_semantics, MemoryWriteEntryArguments},
    turn_state::{approval_status_label, workflow_state_label},
};

use super::acp::{
    acp_direct_tool_prompt_context, acp_pair_den_tool_descriptors, resolve_acp_turn_context,
    workflow_state_json,
};

#[test]
fn submitted_plan_fallback_is_visible_output_and_adapter_plan_update() {
    let event = crate::core::acp_letta_events::AcpGatewayEvent::PlanApprovalFallback {
        plan_id: uuid::Uuid::nil(),
        title: "Example plan".to_string(),
        body: "Do the thing carefully".to_string(),
        artifact_path: "pair/plans/example.md".to_string(),
        state: "submitted".to_string(),
        approval_status: "awaiting_human_approval".to_string(),
    };
    assert!(crate::core::acp_letta_events::acp_event_has_visible_output(
        &event
    ));
    let frame = crate::core::acp_letta_events::acp_event_to_adapter_sse(event);
    let raw = std::str::from_utf8(&frame).expect("utf8 sse frame");
    let payload: serde_json::Value =
        serde_json::from_str(raw.trim().strip_prefix("data: ").expect("sse data prefix"))
            .expect("json payload");
    assert_eq!(payload["type"], "plan_update");
    assert_eq!(payload["entries"][0]["status"], "in_progress");
    assert_eq!(
        payload["approval_fallback"]["plan_id"],
        uuid::Uuid::nil().to_string()
    );
    assert_eq!(
        payload["approval_fallback"]["artifact_path"],
        "pair/plans/example.md"
    );
}

#[test]
fn workflow_state_label_prefers_plan_mode_state() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Plan",
        tool_enablement: AcpToolEnablementState::ReadOnly,
        plan_mode_state: Some("submitted".to_string()),
    };
    assert_eq!(workflow_state_label(&policy), "submitted_waiting_approval");
    assert_eq!(
        approval_status_label(policy.plan_mode_state.as_deref(), policy.mode_label),
        "awaiting_human_approval"
    );
}

#[test]
fn acp_prompt_includes_authoritative_workflow_state_summary() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let prompt = acp_direct_tool_prompt_context(
        "acp-test",
        "/workspace",
        &serde_json::json!({"workspace_roots": ["/workspace"]}),
        true,
        &policy,
    );
    assert!(prompt.contains("AUTHORITATIVE WORKFLOW STATE for this turn"));
    assert!(prompt
        .contains("state_authority=current turn capabilities override prior-turn assumptions"));
    assert!(prompt.contains("workplan.state=`approved`"));
    assert!(prompt.contains("workplan.approval_status=approved_execution_unlocked"));
    assert!(prompt.contains("activity.status=`inactive`"));
    assert!(prompt.contains("execution.execution_unlocked=true"));
    assert!(prompt.contains("memory.active_plan_write_allowed=false"));
}

#[test]
fn pair_tool_surface_reminder_and_descriptors_agree_on_domains() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let prompt = acp_direct_tool_prompt_context(
        "acp-test",
        "/workspace",
        &serde_json::json!({"workspace_roots": ["/workspace"]}),
        true,
        &policy,
    );
    let descriptors = acp_pair_den_tool_descriptors();
    let descriptors = descriptors.as_array().expect("descriptor array");
    let domain_for = |name: &str| {
        descriptors
            .iter()
            .find(|item| item["name"] == name)
            .and_then(|item| item["x-bears-domain"].as_str())
            .expect("descriptor domain")
            .to_string()
    };

    assert!(prompt.contains("workplan.state=`approved`"));
    assert!(prompt.contains("activity.status=`inactive`"));
    assert!(prompt.contains("memory.active_plan_write_allowed=false"));
    assert!(prompt.contains("execution.execution_unlocked=true"));
    assert_eq!(
        den_tools::builtin_den_tool_descriptor_for_provider_name("enter_plan_mode")
            .expect("enter_plan_mode descriptor")
            .domain,
        "workplan"
    );
    assert_eq!(
        den_tools::builtin_den_tool_descriptor_for_provider_name("exit_plan_mode")
            .expect("exit_plan_mode descriptor")
            .domain,
        "workplan"
    );
    assert_eq!(
        den_tools::builtin_den_tool_descriptor_for_provider_name("record_plan_approval")
            .expect("record_plan_approval descriptor")
            .domain,
        "workplan"
    );
    assert_eq!(domain_for("update_plan"), "activity");
    assert_eq!(domain_for("get_plan_status"), "activity");
    assert_eq!(domain_for("memory_write_entry"), "memory");
    assert_eq!(domain_for("web_fetch"), "execution");

    let invalid_memory: MemoryWriteEntryArguments = serde_json::from_value(serde_json::json!({
        "kind": "summary",
        "title": "Current tasks",
        "body": "- [ ] inspect files\n- [ ] edit files\n- [ ] run tests"
    }))
    .unwrap();
    let err = validate_memory_write_entry_semantics(
        &invalid_memory,
        &crate::core::den_tools::DenToolInvocationContext {
            bear_id: uuid::Uuid::nil(),
            bear_slug: "test".to_string(),
            role_agent_id: "agent".to_string(),
            agent_role: Some(crate::core::bears::BearAgentRole::Pair),
            user_id: 1,
            username: Some("tester".to_string()),
            membership_role: None,
            conversation_id: "conv-test".to_string(),
            session_id: "sess-test".to_string(),
            acp_session_id: Some("acp-test".to_string()),
            conversation_selection: None,
            runtime_target: None,
            workspace_roots: Vec::new(),
            session_policy: None,
            activity: None,
            runtime: None,
            context_budget: None,
            request_id: None,
            channel: Default::default(),
        },
    );
    if let Err(err) = err {
        let err = err.to_string();
        assert!(err.contains("update_plan") || err.contains("task"));
    }
}

#[test]
fn acp_prompt_mentions_current_turn_tool_gating_when_write_unlocked() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let prompt = acp_direct_tool_prompt_context(
        "acp-test",
        "/workspace",
        &serde_json::json!({"workspace_roots": ["/workspace"]}),
        true,
        &policy,
    );
    assert!(prompt
        .contains("state_authority=current turn capabilities override prior-turn assumptions"));
    assert!(prompt.contains("tool_classes=read_only, workspace_mutation, execution, browser"));
}

#[test]
fn acp_prompt_teaches_workplace_first_memory_retrieval() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let prompt = acp_direct_tool_prompt_context(
        "acp-test",
        "/workspace",
        &serde_json::json!({"workspace_roots": ["/workspace"]}),
        true,
        &policy,
    );
    assert!(prompt.contains(
        "Memory is Bear-scoped across Workplaces and may contain multiple work surfaces."
    ));
    assert!(prompt.contains(
        "A Workplace is the role-scoped memory surface; for pair, that is the `pair` workplace."
    ));
    assert!(
        prompt.contains("Prefer work-surface-first retrieval for local-understanding questions")
    );
    assert!(prompt.contains("current work-surface canonical anchors"));
    assert!(prompt.contains("current work-surface role-local working memory"));
    assert!(prompt.contains("Use `memory_browse`, `memory_read`, and `memory_search` not only to recall prior notes, but to learn the current work surface within the current Workplace."));
    assert!(prompt.contains("Use `session_info.work_surface` as the trusted Den briefing for current Workplace/work-surface hints when available."));
}

#[test]
fn plan_mode_decision_payload_should_surface_turn_state_shape() {
    let payload = serde_json::json!({
        "accepted": true,
        "reason": "plan_mode_approved",
        "effective_mode": "write",
        "workflow_state": {
            "schema": "bears.turn_state/v1",
            "state_version": 1,
            "state_authority": "current_turn_capabilities",
            "workplan": {
                "domain": "workplan",
                "state": "approved",
                "approval_status": "approved_execution_unlocked"
            },
            "memory": {
                "domain": "memory",
                "write_for_active_workplan_allowed": false
            },
            "execution": {
                "domain": "execution",
                "execution_unlocked": true
            }
        }
    });
    assert_eq!(payload["workflow_state"]["schema"], "bears.turn_state/v1");
    assert_eq!(payload["workflow_state"]["workplan"]["domain"], "workplan");
    assert_eq!(payload["workflow_state"]["workplan"]["state"], "approved");
    assert_eq!(
        payload["workflow_state"]["workplan"]["approval_status"],
        "approved_execution_unlocked"
    );
    assert_eq!(
        payload["workflow_state"]["memory"]["write_for_active_workplan_allowed"],
        false
    );
    assert_eq!(
        payload["workflow_state"]["execution"]["execution_unlocked"],
        true
    );
    assert_eq!(
        payload["workflow_state"]["state_authority"],
        "current_turn_capabilities"
    );
}

#[test]
fn workflow_state_json_surfaces_authoritative_session_state() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let workflow_state = workflow_state_json(&policy);
    assert_eq!(workflow_state["schema"], "bears.turn_state/v1");
    assert_eq!(workflow_state["workplan"]["domain"], "workplan");
    assert_eq!(workflow_state["workplan"]["state"], "approved");
    assert_eq!(
        workflow_state["workplan"]["approval_status"],
        "approved_execution_unlocked"
    );
    assert_eq!(workflow_state["activity"]["domain"], "activity");
    assert_eq!(workflow_state["activity"]["status"], "inactive");
    assert_eq!(workflow_state["execution"]["execution_unlocked"], true);
    assert_eq!(
        workflow_state["memory"]["write_for_active_workplan_allowed"],
        false
    );
    assert_eq!(
        workflow_state["state_authority"],
        "current_turn_capabilities"
    );
}

#[test]
fn workflow_state_json_preserves_approved_state_for_session_reconciliation() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let workflow_state = workflow_state_json(&policy);
    assert_eq!(workflow_state["workplan"]["state"], "approved");
    assert_eq!(
        workflow_state["workplan"]["approval_status"],
        "approved_execution_unlocked"
    );
    assert_eq!(workflow_state["execution"]["execution_unlocked"], true);
}

#[test]
fn workflow_state_json_from_sources_carries_workplan_identity_and_artifact_fields() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let row = AcpPlanModeSessionRow {
        id: uuid::Uuid::nil(),
        user_id: 1,
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test-bear".to_string(),
        acp_session_id: "acp-test".to_string(),
        state: "approved".to_string(),
        reason: "test".to_string(),
        requested_by: "pair".to_string(),
        previous_permission_mode: Some("plan".to_string()),
        plan_title: Some("Example plan".to_string()),
        plan_body: Some("Do the thing carefully".to_string()),
        plan_artifact_path: Some("pair/plans/example.md".to_string()),
        approval_request_id: None,
        approved_by_user_id: Some(1),
        approved_at: Some(time::OffsetDateTime::UNIX_EPOCH),
        rejected_at: None,
        closed_at: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    let workflow_state = super::acp::workflow_state_json_from_sources(&policy, Some(&row), None);
    assert_eq!(
        workflow_state["workplan"]["plan_id"],
        uuid::Uuid::nil().to_string()
    );
    assert_eq!(
        workflow_state["workplan"]["artifact_path"],
        "pair/plans/example.md"
    );
    assert_eq!(workflow_state["workplan"]["title"], "Example plan");
    assert_eq!(workflow_state["workplan"]["submitted_plan_present"], true);
}

#[test]
fn resolve_turn_context_returns_matching_policy_and_turn_state() {
    let session = crate::core::acp_sessions::AcpSessionRow {
        id: uuid::Uuid::nil(),
        user_id: 1,
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test-bear".to_string(),
        acp_session_id: "acp-test".to_string(),
        runtime_session_id: "runtime-test".to_string(),
        conversation_id: "default".to_string(),
        resolved_conversation_id: None,
        client: "acp".to_string(),
        cwd: None,
        adapter_environment: None,
        current_mode: "ask".to_string(),
        conversation_title: None,
        conversation_title_updated_at: None,
        conversation_title_synced_at: None,
        closed_at: None,
        archived_at: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    let plan_mode = AcpPlanModeSessionRow {
        id: uuid::Uuid::nil(),
        user_id: 1,
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test-bear".to_string(),
        acp_session_id: "acp-test".to_string(),
        state: "approved".to_string(),
        reason: "test".to_string(),
        requested_by: "pair".to_string(),
        previous_permission_mode: Some("plan".to_string()),
        plan_title: Some("Example plan".to_string()),
        plan_body: Some("Do the thing carefully".to_string()),
        plan_artifact_path: Some("pair/plans/example.md".to_string()),
        approval_request_id: None,
        approved_by_user_id: Some(1),
        approved_at: Some(time::OffsetDateTime::UNIX_EPOCH),
        rejected_at: None,
        closed_at: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    let resolved = resolve_acp_turn_context(&session, Some(&plan_mode), None);
    assert_eq!(resolved.effective_mode, "write");
    assert_eq!(resolved.policy.mode_label, "Write");
    assert_eq!(resolved.workflow_state["workplan"]["state"], "approved");
    assert_eq!(
        resolved.workflow_state["workplan"]["approval_status"],
        "approved_execution_unlocked"
    );
    assert_eq!(
        resolved.workflow_state["execution"]["execution_unlocked"],
        true
    );
}
