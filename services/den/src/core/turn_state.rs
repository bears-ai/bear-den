use serde_json::{json, Value};

use crate::core::{acp_tools::AcpResolvedSessionPolicy, work_plans::WorkPlanProjection};

pub const TURN_STATE_SCHEMA: &str = "bears.turn_state/v1";
pub const TURN_STATE_VERSION: u32 = 1;
pub const TURN_STATE_AUTHORITY: &str = "current_turn_capabilities";

pub fn workflow_state_label(policy: &AcpResolvedSessionPolicy) -> &'static str {
    match policy.plan_mode_state.as_deref() {
        Some("submitted") => "submitted_waiting_approval",
        Some("approved") => "approved",
        Some("active") => "drafting",
        Some("rejected") => "cancelled",
        _ if policy.mode_label == "Write" => "approved",
        _ => "inactive",
    }
}

pub fn approval_status_label(plan_mode_state: Option<&str>, mode_label: &str) -> &'static str {
    match plan_mode_state {
        Some("approved") => "approved_execution_unlocked",
        Some("submitted") => "awaiting_human_approval",
        Some("active") => "drafting",
        Some("rejected") => "cancelled",
        _ if mode_label == "Write" => "approved_execution_unlocked",
        _ => "inactive",
    }
}

pub fn turn_state_json(
    policy: &AcpResolvedSessionPolicy,
    activity_plan: Option<&WorkPlanProjection>,
) -> Value {
    let workplan = workplan_domain_json(policy);
    let activity = activity_domain_json(activity_plan);
    json!({
        "schema": TURN_STATE_SCHEMA,
        "state_version": TURN_STATE_VERSION,
        "state_authority": TURN_STATE_AUTHORITY,
        "focus": {
            "current_domain": if activity_plan.is_some() { "activity" } else { "workplan" },
            "current_activity_id": activity["plan_id"].clone(),
            "current_workplan_id": workplan["plan_id"].clone(),
            "root_workplan_id": workplan["root_id"].clone(),
        },
        "workplan": workplan,
        "activity": activity,
        "memory": memory_domain_json(),
        "execution": execution_domain_json(policy),
    })
}

fn workplan_domain_json(policy: &AcpResolvedSessionPolicy) -> Value {
    let state = workflow_state_label(policy);
    let approval_status =
        approval_status_label(policy.plan_mode_state.as_deref(), policy.mode_label);
    json!({
        "domain": "workplan",
        "state": state,
        "approval_status": approval_status,
        "plan_id": Value::Null,
        "id": Value::Null,
        "root_id": Value::Null,
        "parent_id": Value::Null,
        "relation": if state == "inactive" { "none" } else { "root" },
        "mode_label": policy.mode_label,
        "title": Value::Null,
        "summary": Value::Null,
        "execution_unlocked_when_approved": policy.tool_enablement.enables_non_read_tools(),
    })
}

fn activity_domain_json(plan: Option<&WorkPlanProjection>) -> Value {
    match plan {
        Some(plan) => json!({
            "domain": "activity",
            "plan_id": plan.id,
            "id": plan.id,
            "root_id": plan.id,
            "parent_id": Value::Null,
            "relation": "root",
            "status": plan.status,
            "title": plan.title,
            "summary": plan.summary,
            "current_item": plan.current_item.as_ref().map(activity_item_json).unwrap_or(Value::Null),
            "counts": activity_item_counts(plan),
            "toward_workplan_id": Value::Null,
            "handoff_requested": plan.handoff_intent_path.is_some() || plan.handoff_task_id.is_some(),
            "visibility": plan.visibility,
            "owner_role": plan.owner_role,
            "version": plan.version,
        }),
        None => json!({
            "domain": "activity",
            "plan_id": Value::Null,
            "id": Value::Null,
            "root_id": Value::Null,
            "parent_id": Value::Null,
            "relation": "none",
            "status": "inactive",
            "title": Value::Null,
            "summary": Value::Null,
            "current_item": Value::Null,
            "counts": {
                "pending": 0,
                "in_progress": 0,
                "blocked": 0,
                "completed": 0,
                "cancelled": 0
            },
            "toward_workplan_id": Value::Null,
            "handoff_requested": false
        }),
    }
}

fn activity_item_json(item: &crate::core::work_plans::WorkPlanItem) -> Value {
    json!({
        "id": item.id,
        "title": item.title,
        "summary": item.summary,
        "status": item.status.as_str(),
        "blocked_reason": item.blocked_reason,
        "source_refs": item.source_refs,
    })
}

fn activity_item_counts(plan: &WorkPlanProjection) -> Value {
    let mut pending = 0;
    let mut in_progress = 0;
    let mut blocked = 0;
    let mut completed = 0;
    let mut cancelled = 0;
    for item in &plan.items {
        match item.status.as_str() {
            "pending" => pending += 1,
            "in_progress" => in_progress += 1,
            "blocked" => blocked += 1,
            "completed" => completed += 1,
            "cancelled" => cancelled += 1,
            _ => {}
        }
    }
    json!({
        "pending": pending,
        "in_progress": in_progress,
        "blocked": blocked,
        "completed": completed,
        "cancelled": cancelled,
    })
}

fn memory_domain_json() -> Value {
    json!({
        "domain": "memory",
        "write_allowed": true,
        "active_plan_write_allowed": false,
        "write_for_active_workplan_allowed": false,
        "review_requested": false,
        "active_scope": "role-local"
    })
}

fn execution_domain_json(policy: &AcpResolvedSessionPolicy) -> Value {
    let execution_unlocked = policy.tool_enablement.enables_non_read_tools();
    json!({
        "domain": "execution",
        "permission_mode": policy.mode_label,
        "tool_classes": policy.allowed_tool_classes(),
        "execution_unlocked": execution_unlocked,
        "local_tools_available": true,
        "approval_required_for_mutation": execution_unlocked
    })
}
