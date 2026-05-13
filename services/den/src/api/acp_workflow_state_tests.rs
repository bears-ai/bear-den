use crate::core::acp_tools::{AcpResolvedSessionPolicy, AcpToolEnablementState};

use super::acp::{acp_direct_tool_prompt_context, approval_status_label, workflow_state_json, workflow_state_label};

#[test]
fn workflow_state_label_prefers_plan_mode_state() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Plan",
        tool_enablement: AcpToolEnablementState::ReadOnly,
        plan_mode_state: Some("submitted".to_string()),
    };
    assert_eq!(workflow_state_label(&policy), "submitted_waiting_approval");
    assert_eq!(approval_status_label(policy.plan_mode_state.as_deref(), policy.mode_label), "awaiting_human_approval");
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
    assert!(prompt.contains("state_authority=current turn capabilities override prior-turn assumptions"));
    assert!(prompt.contains("workflow_state=`approved`"));
    assert!(prompt.contains("approval_status=approved_execution_unlocked"));
    assert!(prompt.contains("execution_unlocked=true"));
    assert!(prompt.contains("memory_for_active_plan_allowed=false"));
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
    assert!(prompt.contains("state_authority=current turn capabilities override prior-turn assumptions"));
    assert!(prompt.contains("tool_classes=read_only, workspace_mutation, execution, browser"));
}

#[test]
fn plan_mode_decision_payload_should_surface_full_workflow_state_shape() {
    let payload = serde_json::json!({
        "accepted": true,
        "reason": "plan_mode_approved",
        "effective_mode": "write",
        "workflow_state": {
            "workflow_state": "approved",
            "approval_status": "approved_execution_unlocked",
            "execution_unlocked": true,
            "memory_for_active_plan_allowed": false,
            "state_authority": "session_policy_and_current_turn_tools"
        }
    });
    assert_eq!(payload["workflow_state"]["workflow_state"], "approved");
    assert_eq!(payload["workflow_state"]["approval_status"], "approved_execution_unlocked");
    assert_eq!(payload["workflow_state"]["execution_unlocked"], true);
    assert_eq!(payload["workflow_state"]["memory_for_active_plan_allowed"], false);
    assert_eq!(payload["workflow_state"]["state_authority"], "session_policy_and_current_turn_tools");
}

#[test]
fn workflow_state_json_surfaces_authoritative_session_state() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let workflow_state = workflow_state_json(&policy);
    assert_eq!(workflow_state["workflow_state"], "approved");
    assert_eq!(workflow_state["approval_status"], "approved_execution_unlocked");
    assert_eq!(workflow_state["execution_unlocked"], true);
    assert_eq!(workflow_state["memory_for_active_plan_allowed"], false);
    assert_eq!(workflow_state["state_authority"], "session_policy_and_current_turn_tools");
}

#[test]
fn workflow_state_json_preserves_approved_state_for_unwedge_reconciliation() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: AcpToolEnablementState::AllTools,
        plan_mode_state: Some("approved".to_string()),
    };
    let workflow_state = workflow_state_json(&policy);
    assert_eq!(workflow_state["workflow_state"], "approved");
    assert_eq!(workflow_state["approval_status"], "approved_execution_unlocked");
    assert_eq!(workflow_state["execution_unlocked"], true);
}
