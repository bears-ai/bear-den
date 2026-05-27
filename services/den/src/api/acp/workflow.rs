use crate::{
    api::acp::workflow_guidance::render_turn_state_summary,
    core::{acp_tools::AcpResolvedSessionPolicy, turn_state, work_plans::WorkPlanProjection},
};

pub(crate) fn workflow_state_json(policy: &AcpResolvedSessionPolicy) -> serde_json::Value {
    workflow_state_json_from_sources(policy, None, None)
}

pub(crate) fn workflow_state_json_with_activity(
    policy: &AcpResolvedSessionPolicy,
    activity_plan: Option<&WorkPlanProjection>,
) -> serde_json::Value {
    workflow_state_json_from_sources(policy, None, activity_plan)
}

pub(crate) fn workflow_state_json_from_sources(
    policy: &AcpResolvedSessionPolicy,
    workplan_row: Option<&crate::core::acp_plan_mode::AcpPlanModeSessionRow>,
    activity_plan: Option<&WorkPlanProjection>,
) -> serde_json::Value {
    turn_state::turn_state_from_sources(policy, workplan_row, activity_plan)
}

pub(super) fn render_turn_state_summary_with_activity(
    session_id: &str,
    roots: &[String],
    local_tool_names: &[&str],
    den_tool_names: &[&str],
    policy: &AcpResolvedSessionPolicy,
    activity_plan: Option<&WorkPlanProjection>,
) -> String {
    let execution_unlocked = policy.tool_enablement.enables_non_read_tools();
    let turn_state = workflow_state_json_with_activity(policy, activity_plan);
    let activity_status = turn_state["activity"]["status"]
        .as_str()
        .unwrap_or("inactive");
    let activity_plan_id = turn_state["activity"]["plan_id"].as_str().unwrap_or("none");
    let current_item = turn_state["activity"]["current_item"]["title"]
        .as_str()
        .unwrap_or("none");
    render_turn_state_summary(
        session_id,
        roots,
        local_tool_names,
        den_tool_names,
        policy.mode_label,
        &policy.allowed_tool_classes(),
        turn_state["workplan"]["state"].as_str().unwrap_or("inactive"),
        turn_state["workplan"]["approval_status"]
            .as_str()
            .unwrap_or("inactive"),
        activity_status,
        activity_plan_id,
        current_item,
        execution_unlocked,
    )
}
