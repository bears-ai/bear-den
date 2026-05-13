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
    workboard_plan: Option<&WorkPlanProjection>,
) -> Value {
    let workflow = workflow_domain_json(policy);
    let workboard = workboard_domain_json(workboard_plan);
    let memory = memory_domain_json();
    let execution = execution_domain_json(policy);

    json!({
        "schema": TURN_STATE_SCHEMA,
        "state_version": TURN_STATE_VERSION,
        "state_authority": TURN_STATE_AUTHORITY,
        "focus": {
            "current_domain": if workboard_plan.is_some() { "workboard" } else { "workflow" },
            "current_activity_id": workboard["plan_id"].clone(),
            "current_workplan_id": workflow["plan_id"].clone(),
            "root_workplan_id": workflow["root_id"].clone(),
        },
        "workflow": workflow,
        "workboard": workboard,
        "memory": memory,
        "execution": execution,
        // Backward-compatible aliases for callers/tests that still use the earlier
        // workplan/activity labels while the canonical ontology moves to
        // workflow/workboard.
        "workplan": legacy_workplan_domain_json(policy),
        "activity": legacy_activity_domain_json(workboard_plan),
    })
}

fn workflow_domain_json(policy: &AcpResolvedSessionPolicy) -> Value {
    let state = workflow_state_label(policy);
    let approval_status =
        approval_status_label(policy.plan_mode_state.as_deref(), policy.mode_label);
    json!({
        "domain": "workflow",
        "state": state,
        "approval_status": approval_status,
        "plan_id": Value::Null,
        "root_id": Value::Null,
        "parent_id": Value::Null,
        "relation": if state == "inactive" { "none" } else { "root" },
        "mode_label": policy.mode_label,
        "title": Value::Null,
        "summary": Value::Null,
        "execution_unlocked_when_approved": policy.tool_enablement.enables_non_read_tools(),
    })
}

fn legacy_workplan_domain_json(policy: &AcpResolvedSessionPolicy) -> Value {
    let state = workflow_state_label(policy);
    let approval_status =
        approval_status_label(policy.plan_mode_state.as_deref(), policy.mode_label);
    json!({
        "domain": "workplan",
        "id": Value::Null,
        "root_id": Value::Null,
        "parent_id": Value::Null,
        "relation": if state == "inactive" { "none" } else { "root" },
        "state": state,
        "approval_status": approval_status,
        "mode_label": policy.mode_label,
        "title": Value::Null,
        "summary": Value::Null,
    })
}

fn workboard_domain_json(plan: Option<&WorkPlanProjection>) -> Value {
    match plan {
        Some(plan) => json!({
            "domain": "workboard",
            "plan_id": plan.id,
            "root_id": plan.id,
            "parent_id": Value::Null,
            "relation": "root",
            "status": plan.status,
            "title": plan.title,
            "summary": plan.summary,
            "current_item": plan.current_item.as_ref().map(workboard_item_json).unwrap_or(Value::Null),
            "counts": workboard_item_counts(plan),
            "toward_workflow_id": Value::Null,
            "handoff_requested": plan.handoff_intent_path.is_some() || plan.handoff_task_id.is_some(),
            "visibility": plan.visibility,
            "owner_role": plan.owner_role,
            "version": plan.version,
        }),
        None => json!({
            "domain": "workboard",
            "plan_id": Value::Null,
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
            "toward_workflow_id": Value::Null,
            "handoff_requested": false
        }),
    }
}

fn legacy_activity_domain_json(plan: Option<&WorkPlanProjection>) -> Value {
    let workboard = workboard_domain_json(plan);
    json!({
        "domain": "activity",
        "id": workboard["plan_id"].clone(),
        "root_id": workboard["root_id"].clone(),
        "parent_id": workboard["parent_id"].clone(),
        "relation": workboard["relation"].clone(),
        "status": workboard["status"].clone(),
        "title": workboard["title"].clone(),
        "summary": workboard["summary"].clone(),
        "current_item": workboard["current_item"].clone(),
        "counts": workboard["counts"].clone(),
        "toward_workplan_id": Value::Null,
        "handoff_requested": workboard["handoff_requested"].clone(),
    })
}

fn workboard_item_json(item: &crate::core::work_plans::WorkPlanItem) -> Value {
    json!({
        "id": item.id,
        "title": item.title,
        "summary": item.summary,
        "status": item.status.as_str(),
        "blocked_reason": item.blocked_reason,
        "source_refs": item.source_refs,
    })
}

fn workboard_item_counts(plan: &WorkPlanProjection) -> Value {
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
