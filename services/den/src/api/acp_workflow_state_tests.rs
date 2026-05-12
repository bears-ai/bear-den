use crate::core::acp_tools::{AcpResolvedSessionPolicy, AcpToolEnablementState};

use super::acp::{acp_direct_tool_prompt_context, workflow_state_label};

#[test]
fn workflow_state_label_prefers_plan_mode_state() {
    let policy = AcpResolvedSessionPolicy {
        mode_label: "Plan",
        tool_enablement: AcpToolEnablementState::ReadOnly,
        plan_mode_state: Some("submitted".to_string()),
    };
    assert_eq!(workflow_state_label(&policy), "submitted_waiting_approval");
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
