use crate::core::{
    acp_tools::{AcpResolvedSessionPolicy, AcpToolEnablementState},
    den_tools::{validate_memory_write_entry_semantics, MemoryWriteEntryArguments},
    turn_state::{approval_status_label, workflow_state_label},
};

use super::acp::{
    acp_direct_tool_prompt_context, acp_pair_den_tool_descriptors, workflow_state_json,
};

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
    assert_eq!(domain_for("enter_plan_mode"), "workplan");
    assert_eq!(domain_for("exit_plan_mode"), "workplan");
    assert_eq!(domain_for("record_plan_approval"), "workplan");
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
    let err = validate_memory_write_entry_semantics(&invalid_memory)
        .unwrap_err()
        .to_string();
    assert!(err.contains("update_plan") || err.contains("task"));
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
fn workflow_state_json_preserves_approved_state_for_unwedge_reconciliation() {
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
