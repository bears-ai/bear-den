use std::str::FromStr;

use serde_json::{json, Value};

use crate::plan_mode;
use den_core::{client_tools::ResolvedSessionPolicy, profile::BearProfile};
use den_docket::{
    TaskListItem, TaskListItemStatus, TaskListLocalProjection, TaskListProjection,
    TaskListUpdateItem,
};

pub const TURN_STATE_SCHEMA: &str = "bears.turn_state/v1";
pub const TURN_STATE_VERSION: u32 = 1;
pub const TURN_STATE_AUTHORITY: &str = "current_turn_capabilities";

const AUTONOMOUS_CONTINUATION_POLICY: &str = "continue_until_complete_or_blocked";

fn json_null() -> Value {
    Value::Null
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousFinalResponseKind {
    CompletionFinal,
    BlockedFinal,
    ReasonedNonActionFinal,
    ScopeEscalationFinal,
    RuntimeLimitBlockedFinal,
    ProgressReport,
    ClarificationRequest,
    UnsafeActionPermissionRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomousExecutionGate {
    pub is_active_autonomous_task: bool,
    pub has_incomplete_unblocked_items: bool,
    pub acceptance_criteria_met: bool,
    pub has_hard_blocker: bool,
    pub may_stop: bool,
    pub next_incomplete_task_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFocusLoopDetection {
    pub detected: bool,
    pub continuation_nudges: usize,
    pub terminal_objections: usize,
    pub repeated_objection_kind: Option<AutonomousFinalResponseKind>,
}

pub fn workflow_state_label(policy: &ResolvedSessionPolicy) -> &'static str {
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
    policy: &ResolvedSessionPolicy,
    activity_plan: Option<&TaskListLocalProjection>,
) -> Value {
    turn_state_from_sources(policy, None, activity_plan)
}

pub fn turn_state_from_sources(
    policy: &ResolvedSessionPolicy,
    workplan_row: Option<&plan_mode::PlanModeSessionRow>,
    activity_plan: Option<&TaskListLocalProjection>,
) -> Value {
    let workplan = workplan_domain_json(policy, workplan_row);
    let activity = activity_domain_json(activity_plan);
    let autonomous_execution = autonomous_execution_domain_json(activity_plan);
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
        "autonomous_execution": autonomous_execution,
        "memory": memory_domain_json(),
        "execution": execution_domain_json(policy),
    })
}

pub fn autonomous_execution_gate_for_plan(
    profile: BearProfile,
    plan: Option<&TaskListLocalProjection>,
    final_response_kind: AutonomousFinalResponseKind,
) -> AutonomousExecutionGate {
    let Some(plan) = plan.filter(|plan| is_autonomous_implementation_plan(profile, plan)) else {
        return AutonomousExecutionGate {
            is_active_autonomous_task: false,
            has_incomplete_unblocked_items: false,
            acceptance_criteria_met: false,
            has_hard_blocker: false,
            may_stop: true,
            next_incomplete_task_title: None,
        };
    };

    let items = &plan.items;
    let acceptance_criteria_met = acceptance_criteria_met(plan);
    // A blocked sibling is not a global stop condition: continue any independent
    // pending work. Only the durable list-level blocked state represents a
    // structured execution gate for the whole focused plan.
    let has_hard_blocker = matches!(plan.status.as_str(), "blocked");
    let next_incomplete_task_title =
        next_incomplete_unblocked_item(items).map(|item| item.title.clone());
    let has_incomplete_unblocked_items = next_incomplete_task_title.is_some();
    // Runtime limits and prose-only scope claims do not settle active work.
    // They must lead to a continuation or a separately persisted structured
    // pause/blocker, never authorize a normal terminal response by themselves.
    let may_stop = if acceptance_criteria_met {
        final_response_kind == AutonomousFinalResponseKind::CompletionFinal
    } else if has_incomplete_unblocked_items {
        false
    } else if has_hard_blocker {
        matches!(
            final_response_kind,
            AutonomousFinalResponseKind::BlockedFinal
                | AutonomousFinalResponseKind::ReasonedNonActionFinal
                | AutonomousFinalResponseKind::UnsafeActionPermissionRequest
        )
    } else {
        matches!(
            final_response_kind,
            AutonomousFinalResponseKind::CompletionFinal
                | AutonomousFinalResponseKind::ReasonedNonActionFinal
        )
    };

    AutonomousExecutionGate {
        is_active_autonomous_task: true,
        has_incomplete_unblocked_items,
        acceptance_criteria_met,
        has_hard_blocker,
        may_stop,
        next_incomplete_task_title,
    }
}

pub fn classify_autonomous_final_response(text: &str) -> AutonomousFinalResponseKind {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("runtime limit")
        || lower.contains("runtime limits")
        || lower.contains("tool budget")
        || lower.contains("write budget")
        || lower.contains("wall-clock")
        || lower.contains("wall clock")
        || lower.contains("loop-ko")
        || lower.contains("further write")
        || lower.contains("fresh turn")
    {
        return AutonomousFinalResponseKind::RuntimeLimitBlockedFinal;
    }
    if lower.contains("need approval")
        || lower.contains("requires approval")
        || lower.contains("permission")
    {
        return AutonomousFinalResponseKind::UnsafeActionPermissionRequest;
    }
    if lower.contains("scope escalation")
        || lower.contains("requires scope escalation")
        || lower.contains("requires a separate")
        || lower.contains("separate api migration")
        || lower.contains("separate migration plan")
        || lower.contains("public api migration")
        || lower.contains("out of scope")
        || lower.contains("outside scope")
        || lower.contains("public tool protocol")
        || lower.contains("external tool contract")
        || lower.contains("external tool contracts")
    {
        return AutonomousFinalResponseKind::ScopeEscalationFinal;
    }
    if lower.contains("not applicable")
        || lower.contains("waived")
        || lower.contains("intentionally skipped")
        || lower.contains("skipped because")
        || lower.contains("not appropriate")
        || lower.contains("no relevant changes")
        || lower.contains("do not commit")
        || lower.contains("did not commit")
    {
        return AutonomousFinalResponseKind::ReasonedNonActionFinal;
    }
    if lower.contains("blocked") || lower.contains("cannot continue") {
        return AutonomousFinalResponseKind::BlockedFinal;
    }
    if lower.contains("what i changed")
        || lower.contains("remaining work")
        || lower.contains("next useful steps")
        || lower.contains("next i would")
        || lower.contains("i can continue if")
    {
        // heuristic progress-report detection only catches common partial-summary shapes;
        // replace with structured assistant turn intent once final responses are explicitly typed.
        return AutonomousFinalResponseKind::ProgressReport;
    }
    if lower.ends_with('?') {
        return AutonomousFinalResponseKind::ClarificationRequest;
    }
    AutonomousFinalResponseKind::CompletionFinal
}

pub fn detect_task_focus_loop(recent_texts: &[impl AsRef<str>]) -> TaskFocusLoopDetection {
    let continuation_nudges = recent_texts
        .iter()
        .filter(|text| looks_like_task_focus_continuation_nudge(text.as_ref()))
        .count();
    let mut terminal_objections = 0;
    let mut previous_objection_kind = None;
    let mut previous_objection_fingerprint = None;
    let mut repeated_objection_kind = None;

    for text in recent_texts {
        let text = text.as_ref();
        if looks_like_task_focus_continuation_nudge(text) {
            continue;
        }
        let kind = classify_autonomous_final_response(text);
        if is_terminal_objection_kind(kind) {
            terminal_objections += 1;
            let fingerprint = terminal_objection_fingerprint(text);
            if previous_objection_kind == Some(kind)
                && previous_objection_fingerprint.as_deref() == Some(fingerprint.as_str())
            {
                repeated_objection_kind = Some(kind);
            }
            previous_objection_kind = Some(kind);
            previous_objection_fingerprint = Some(fingerprint);
        }
    }

    // ponytail: phrase-based loop detection is intentionally conservative;
    // replace with structured assistant terminal-state events once final answers are typed.
    let detected =
        continuation_nudges >= 2 && terminal_objections >= 2 && repeated_objection_kind.is_some();

    TaskFocusLoopDetection {
        detected,
        continuation_nudges,
        terminal_objections,
        repeated_objection_kind,
    }
}

fn terminal_objection_fingerprint(text: &str) -> String {
    // ponytail: phrase fingerprint keeps loop detection conservative; upgrade to
    // structured terminal-state IDs when assistant responses are typed.
    text.to_ascii_lowercase()
        .split_whitespace()
        .filter(|word| word.len() >= 4)
        .take(16)
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_terminal_objection_kind(kind: AutonomousFinalResponseKind) -> bool {
    matches!(
        kind,
        AutonomousFinalResponseKind::BlockedFinal
            | AutonomousFinalResponseKind::ReasonedNonActionFinal
            | AutonomousFinalResponseKind::ScopeEscalationFinal
            | AutonomousFinalResponseKind::RuntimeLimitBlockedFinal
            | AutonomousFinalResponseKind::UnsafeActionPermissionRequest
    )
}

fn looks_like_task_focus_continuation_nudge(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("active task") && lower.contains("incomplete"))
        || lower.contains("do not final-answer yet")
        || lower.contains("continue with:")
        || lower.contains("resume execution from the next incomplete")
        || lower.contains("you still have incomplete")
}

pub fn should_allow_terminal_response(
    profile: BearProfile,
    cached_activity_plan_projection: Option<&TaskListLocalProjection>,
    assistant_text: &str,
) -> bool {
    let kind = classify_autonomous_final_response(assistant_text);
    autonomous_execution_gate_for_plan(profile, cached_activity_plan_projection, kind).may_stop
}

pub fn autonomous_execution_gate_for_task_list(
    profile: BearProfile,
    task_list: Option<&TaskListProjection>,
    final_response_kind: AutonomousFinalResponseKind,
) -> AutonomousExecutionGate {
    let Some(task_list) = task_list.filter(|task_list| is_autonomous_task_list(profile, task_list))
    else {
        return AutonomousExecutionGate {
            is_active_autonomous_task: false,
            has_incomplete_unblocked_items: false,
            acceptance_criteria_met: false,
            has_hard_blocker: false,
            may_stop: true,
            next_incomplete_task_title: None,
        };
    };

    let acceptance_criteria_met = task_list_acceptance_criteria_met(task_list);
    // A blocked sibling is not a global stop condition: continue any independent
    // pending work. Only the durable list-level blocked state represents a
    // structured execution gate for the whole focused plan.
    let has_hard_blocker = matches!(task_list.status.as_str(), "blocked");
    let next_incomplete_task_title =
        next_incomplete_unblocked_task_list_item(&task_list.items).map(|item| item.title.clone());
    let has_incomplete_unblocked_items = next_incomplete_task_title.is_some();
    // Runtime limits and prose-only scope claims do not settle active work.
    // They must lead to a continuation or a separately persisted structured
    // pause/blocker, never authorize a normal terminal response by themselves.
    let may_stop = if acceptance_criteria_met {
        final_response_kind == AutonomousFinalResponseKind::CompletionFinal
    } else if has_incomplete_unblocked_items {
        false
    } else if has_hard_blocker {
        matches!(
            final_response_kind,
            AutonomousFinalResponseKind::BlockedFinal
                | AutonomousFinalResponseKind::ReasonedNonActionFinal
                | AutonomousFinalResponseKind::UnsafeActionPermissionRequest
        )
    } else {
        matches!(
            final_response_kind,
            AutonomousFinalResponseKind::CompletionFinal
                | AutonomousFinalResponseKind::ReasonedNonActionFinal
        )
    };

    AutonomousExecutionGate {
        is_active_autonomous_task: true,
        has_incomplete_unblocked_items,
        acceptance_criteria_met,
        has_hard_blocker,
        may_stop,
        next_incomplete_task_title,
    }
}

pub fn should_allow_terminal_response_for_task_list(
    profile: BearProfile,
    active_task_list: Option<&TaskListProjection>,
    assistant_text: &str,
) -> bool {
    let kind = classify_autonomous_final_response(assistant_text);
    autonomous_execution_gate_for_task_list(profile, active_task_list, kind).may_stop
}

pub fn autonomous_resume_obligation_text(plan: &TaskListLocalProjection) -> Option<String> {
    if !matches!(plan.owner_profile.as_str(), "pair" | "work") {
        return None;
    }
    let items = plan
        .items
        .iter()
        .map(|item| {
            let marker = match item.status {
                TaskListItemStatus::Completed => "[x]",
                _ => "[ ]",
            };
            format!("- {marker} {}", item.title)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let next = next_incomplete_unblocked_item(&plan.items)
        .map(|item| item.title.as_str())
        .unwrap_or("resolve blocker before proceeding");
    Some(format!(
        "Previous assistant turn ended before completing an autonomous implementation task.\n\nActive goal:\n{}\n\nContinuation policy:\nContinue the next incomplete, unblocked task. Do not provide a progress-only final answer unless the work is complete, blocked, not applicable, waived, or permission-gated. If the next planned action is inappropriate, mark that item blocked or cancelled with evidence and report the reason.\n\nCurrent task state:\n{}\n\nRequired action:\nResume execution from the next incomplete actionable item: {}.",
        plan.title, items, next
    ))
}

fn workplan_domain_json(
    policy: &ResolvedSessionPolicy,
    workplan_row: Option<&plan_mode::PlanModeSessionRow>,
) -> Value {
    let state = workflow_state_label(policy);
    let approval_status =
        approval_status_label(policy.plan_mode_state.as_deref(), policy.mode_label);
    // `plan_id`, `id`, and `root_id` intentionally share the same root workplan id in the
    // v1 wire shape: older clients read different aliases for the same root plan identity.
    json!({
        "domain": "workplan",
        "state": state,
        "approval_status": approval_status,
        "plan_id": workplan_row.map(|row| Value::from(row.id.to_string())).unwrap_or_else(json_null),
        "id": workplan_row.map(|row| Value::from(row.id.to_string())).unwrap_or_else(json_null),
        "root_id": workplan_row.map(|row| Value::from(row.id.to_string())).unwrap_or_else(json_null),
        "parent_id": json_null(),
        "relation": if state == "inactive" { "none" } else { "root" },
        "mode_label": policy.mode_label,
        "raw_state": workplan_row.map(|row| Value::from(row.state.clone())).unwrap_or_else(json_null),
        "title": workplan_row.and_then(|row| row.plan_title.clone()).map(Value::from).unwrap_or_else(json_null),
        "summary": workplan_row
            .and_then(|row| row.plan_body.as_ref().map(|body| summarize_text(body, 240)))
            .map(Value::from)
            .unwrap_or_else(json_null),
        "artifact_path": workplan_row
            .and_then(|row| row.plan_artifact_path.clone())
            .map(Value::from)
            .unwrap_or_else(json_null),
        "submitted_plan_present": workplan_row
            .map(|row| row.plan_artifact_path.is_some())
            .unwrap_or(false),
        "execution_unlocked": approval_status == "approved_execution_unlocked",
        "execution_unlocked_when_approved": policy.tool_enablement.enables_non_read_tools(),
        "approved_at": workplan_row
            .and_then(|row| row.approved_at.map(|t| Value::from(t.to_string())))
            .unwrap_or_else(json_null),
        "closed_at": workplan_row
            .and_then(|row| row.closed_at.map(|t| Value::from(t.to_string())))
            .unwrap_or_else(json_null),
        "updated_at": workplan_row
            .map(|row| Value::from(row.updated_at.to_string()))
            .unwrap_or_else(json_null),
    })
}

fn activity_domain_json(plan: Option<&TaskListLocalProjection>) -> Value {
    match plan {
        Some(plan) => {
            let counts = activity_item_counts(plan);
            let status_sync_required =
                !matches!(plan.status.as_str(), "completed" | "cancelled" | "archived");
            json!({
                "domain": "activity",
                "plan_id": plan.id,
                "id": plan.id,
                "root_id": plan.id,
                "parent_id": json_null(),
                "relation": "root",
                "frontmost": true,
                "status": plan.status,
                "title": plan.title,
                "summary": plan.summary,
                "current_item": plan.current_item.as_ref().map(activity_item_json).unwrap_or_else(json_null),
                "counts": counts,
                "status_sync_required": status_sync_required,
                "completion_claim_requires_status_update": status_sync_required,
                "status_update_tool": if status_sync_required { Value::from("update_task_list") } else { json_null() },
                "toward_workplan_id": json_null(),
                "handoff_requested": plan.handoff_intent_path.is_some() || plan.handoff_task_id.is_some(),
                "visibility": plan.visibility,
                "owner_profile": plan.owner_profile,
                "version": plan.version,
            })
        }
        None => json!({
            "domain": "activity",
            "plan_id": json_null(),
            "id": json_null(),
            "root_id": json_null(),
            "parent_id": json_null(),
            "relation": "none",
            "frontmost": false,
            "status": "inactive",
            "title": json_null(),
            "summary": json_null(),
            "current_item": json_null(),
            "counts": {
                "pending": 0,
                "in_progress": 0,
                "blocked": 0,
                "completed": 0,
                "cancelled": 0
            },
            "status_sync_required": false,
            "completion_claim_requires_status_update": false,
            "status_update_tool": json_null(),
            "toward_workplan_id": json_null(),
            "handoff_requested": false
        }),
    }
}

fn autonomous_execution_domain_json(plan: Option<&TaskListLocalProjection>) -> Value {
    let profile = plan
        .and_then(|plan| BearProfile::from_str(&plan.owner_profile).ok())
        .unwrap_or(BearProfile::Pair);
    let Some(plan) = plan.filter(|plan| is_autonomous_implementation_plan(profile, plan)) else {
        return json!({
            "mode": json_null(),
            "active": false,
        });
    };
    let gate = autonomous_execution_gate_for_plan(
        profile,
        Some(plan),
        AutonomousFinalResponseKind::ProgressReport,
    );
    let last_verified_completed_step = plan
        .items
        .iter()
        .rev()
        .find(|item| item.status == TaskListItemStatus::Completed)
        .map(|item| Value::from(item.title.clone()))
        .unwrap_or_else(json_null);
    json!({
        "mode": "autonomous_implementation",
        "active": true,
        "goal": plan.title,
        "acceptance_criteria": plan.summary,
        "continuation_policy": AUTONOMOUS_CONTINUATION_POLICY,
        "stop_conditions": [
            "acceptance_criteria_met",
            "hard_blocker",
            "unsafe_or_external_action_required"
        ],
        "tasks": plan.items.iter().map(autonomous_task_json).collect::<Vec<_>>(),
        "current_in_progress_item": plan.current_item.as_ref().map(|item| item.title.clone()),
        "known_blockers": plan.items.iter().filter(|item| item.status == TaskListItemStatus::Blocked).filter_map(|item| item.blocked_reason.clone()).collect::<Vec<_>>(),
        "last_verified_completed_step": last_verified_completed_step,
        "has_incomplete_unblocked_items": gate.has_incomplete_unblocked_items,
        "next_incomplete_task_title": gate.next_incomplete_task_title,
    })
}

fn autonomous_task_json(item: &TaskListUpdateItem) -> Value {
    json!({
        "title": item.title,
        "status": item.status.as_str(),
        "evidence": item.summary,
        "blocked_reason": item.blocked_reason,
    })
}

fn can_execute_focused_tasks(profile: BearProfile) -> bool {
    den_core::EffectivePolicy::compile(
        profile,
        den_core::Governance::Interactive,
        den_core::ArmatureAvailability::Absent,
    )
    .capabilities
    .contains(den_core::BearCapability::ExecuteFocusedTask)
}

fn is_autonomous_implementation_plan(profile: BearProfile, plan: &TaskListLocalProjection) -> bool {
    can_execute_focused_tasks(profile)
        && matches!(plan.owner_profile.as_str(), "pair" | "work")
        && matches!(
            plan.status.as_str(),
            "active" | "blocked" | "completed" | "cancelled"
        )
}

fn is_autonomous_task_list(profile: BearProfile, task_list: &TaskListProjection) -> bool {
    can_execute_focused_tasks(profile)
        && matches!(task_list.owner_profile.as_str(), "pair" | "work")
        && matches!(
            task_list.status.as_str(),
            "active" | "ready" | "running" | "blocked" | "completed" | "cancelled"
        )
}

fn acceptance_criteria_met(plan: &TaskListLocalProjection) -> bool {
    !plan.items.is_empty()
        && plan.items.iter().all(|item| {
            matches!(
                item.status,
                TaskListItemStatus::Completed | TaskListItemStatus::Cancelled
            )
        })
        && matches!(plan.status.as_str(), "completed" | "cancelled")
}

fn next_incomplete_unblocked_item(items: &[TaskListUpdateItem]) -> Option<&TaskListUpdateItem> {
    items.iter().find(|item| {
        matches!(
            item.status,
            TaskListItemStatus::Pending | TaskListItemStatus::InProgress
        )
    })
}

fn task_list_acceptance_criteria_met(task_list: &TaskListProjection) -> bool {
    !task_list.items.is_empty()
        && task_list.items.iter().all(|item| {
            matches!(
                item.status,
                TaskListItemStatus::Completed | TaskListItemStatus::Cancelled
            )
        })
        && matches!(task_list.status.as_str(), "completed" | "cancelled")
}

fn next_incomplete_unblocked_task_list_item(items: &[TaskListItem]) -> Option<&TaskListItem> {
    items.iter().find(|item| {
        matches!(
            item.status,
            TaskListItemStatus::Pending | TaskListItemStatus::InProgress
        )
    })
}

fn activity_item_json(item: &den_docket::TaskListUpdateItem) -> Value {
    json!({
        "id": item.id,
        "title": item.title,
        "summary": item.summary,
        "status": item.status.as_str(),
        "blocked_reason": item.blocked_reason,
        "source_refs": item.source_refs,
    })
}

fn activity_item_counts(plan: &TaskListLocalProjection) -> Value {
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

fn execution_domain_json(policy: &ResolvedSessionPolicy) -> Value {
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

fn summarize_text(body: &str, max_chars: usize) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let mut summary = trimmed.chars().take(max_chars).collect::<String>();
        summary.push('…');
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use den_docket::{
        TaskListItem, TaskListLocalProjection, TaskListProjection, TaskListSourceRef,
        TaskListSyncState,
    };
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn item(title: &str, status: TaskListItemStatus) -> TaskListUpdateItem {
        TaskListUpdateItem {
            id: title.to_string(),
            title: title.to_string(),
            summary: Some(format!("evidence: {title}")),
            status,
            blocked_reason: (status == TaskListItemStatus::Blocked).then(|| "waiting".to_string()),
            source_refs: Vec::new(),
        }
    }

    fn plan(status: &str, items: Vec<TaskListUpdateItem>) -> TaskListLocalProjection {
        TaskListLocalProjection {
            id: Uuid::nil(),
            bear_id: Uuid::nil(),
            title: "Complete Docket relational work management".to_string(),
            summary: "Acceptance criteria".to_string(),
            owner_profile: "pair".to_string(),
            visibility: "bear_visible".to_string(),
            status: status.to_string(),
            version: 1,
            current_item: items
                .iter()
                .find(|item| item.status == TaskListItemStatus::InProgress)
                .cloned(),
            items,
            source_conversation_id: None,
            source_client_session_id: None,
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn task_list_item(title: &str, status: TaskListItemStatus) -> TaskListItem {
        TaskListItem {
            id: title.to_string(),
            title: title.to_string(),
            summary: Some(format!("evidence: {title}")),
            status,
            blocked_reason: (status == TaskListItemStatus::Blocked)
                .then(|| "permission needed".to_string()),
            source_ref: TaskListSourceRef::local(Vec::new()),
            sync_state: TaskListSyncState::LocalOnly,
        }
    }

    fn task_list(status: &str, items: Vec<TaskListItem>) -> TaskListProjection {
        TaskListProjection {
            id: Uuid::nil(),
            bear_id: Uuid::nil(),
            title: "Implementation".to_string(),
            summary: "Acceptance criteria".to_string(),
            owner_profile: "pair".to_string(),
            visibility: "bear_visible".to_string(),
            status: status.to_string(),
            version: 1,
            source_ref: TaskListSourceRef::local(Vec::new()),
            current_item: items
                .iter()
                .find(|item| item.status == TaskListItemStatus::InProgress)
                .cloned(),
            items,
            source_conversation_id: None,
            source_client_session_id: None,
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn autonomous_resume_obligation_lists_next_incomplete_item() {
        let plan = plan(
            "active",
            vec![
                item(
                    "Inventory schema and Docket API coupling",
                    TaskListItemStatus::Completed,
                ),
                item(
                    "Add lifecycle/dispatcher tests",
                    TaskListItemStatus::Pending,
                ),
            ],
        );
        let text = autonomous_resume_obligation_text(&plan).expect("autonomous reminder");
        assert!(text.contains("Add lifecycle/dispatcher tests"));
        assert!(text.contains("Do not provide a progress-only final answer"));
    }

    #[test]
    fn autonomous_gate_blocks_progress_report_while_work_remains() {
        let plan = plan(
            "active",
            vec![
                item("done", TaskListItemStatus::Completed),
                item("remaining", TaskListItemStatus::InProgress),
            ],
        );
        let gate = autonomous_execution_gate_for_plan(
            BearProfile::Pair,
            Some(&plan),
            classify_autonomous_final_response(
                "What I changed: added one test. Remaining work: gate final answers.",
            ),
        );
        assert!(gate.is_active_autonomous_task);
        assert!(gate.has_incomplete_unblocked_items);
        assert!(!gate.may_stop);
    }

    #[test]
    fn autonomous_gate_allows_completion_only_when_plan_complete() {
        let plan = plan(
            "completed",
            vec![item("done", TaskListItemStatus::Completed)],
        );
        let gate = autonomous_execution_gate_for_plan(
            BearProfile::Pair,
            Some(&plan),
            AutonomousFinalResponseKind::CompletionFinal,
        );
        assert!(gate.acceptance_criteria_met);
        assert!(gate.may_stop);
    }

    #[test]
    fn autonomous_gate_allows_blocked_final_when_no_safe_path_remains() {
        let plan = plan(
            "blocked",
            vec![item("blocked", TaskListItemStatus::Blocked)],
        );
        let gate = autonomous_execution_gate_for_plan(
            BearProfile::Pair,
            Some(&plan),
            AutonomousFinalResponseKind::BlockedFinal,
        );
        assert!(gate.has_hard_blocker);
        assert!(gate.may_stop);
    }

    #[test]
    fn pair_without_active_task_list_does_not_trigger_terminal_gate() {
        assert!(should_allow_terminal_response(
            BearProfile::Pair,
            None,
            "What I changed: added one test. Remaining work: more later."
        ));
    }

    #[test]
    fn cancelled_remaining_task_allows_reasoned_non_action_final() {
        let task_list = task_list(
            "active",
            vec![
                task_list_item("Implement change", TaskListItemStatus::Completed),
                task_list_item("Commit changes", TaskListItemStatus::Cancelled),
            ],
        );

        let gate = autonomous_execution_gate_for_task_list(
            BearProfile::Pair,
            Some(&task_list),
            classify_autonomous_final_response(
                "I did not commit because there are no relevant changes to commit.",
            ),
        );

        assert!(gate.is_active_autonomous_task);
        assert!(!gate.has_incomplete_unblocked_items);
        assert!(gate.may_stop);
    }

    #[test]
    fn blocked_list_state_allows_blocker_final() {
        let task_list = task_list(
            "blocked",
            vec![
                task_list_item("Implement change", TaskListItemStatus::Completed),
                task_list_item("Commit changes", TaskListItemStatus::Blocked),
            ],
        );

        let gate = autonomous_execution_gate_for_task_list(
            BearProfile::Pair,
            Some(&task_list),
            classify_autonomous_final_response(
                "I am blocked because committing requires explicit permission.",
            ),
        );

        assert!(gate.has_hard_blocker);
        assert!(gate.may_stop);
    }

    #[test]
    fn scope_escalation_prose_does_not_allow_terminal_response_with_remaining_work() {
        let task_list = task_list(
            "active",
            vec![
                task_list_item(
                    "Rename internal Docket model names",
                    TaskListItemStatus::Completed,
                ),
                task_list_item(
                    "Rename public den.work_plan tools",
                    TaskListItemStatus::Pending,
                ),
            ],
        );

        let gate = autonomous_execution_gate_for_task_list(
            BearProfile::Pair,
            Some(&task_list),
            classify_autonomous_final_response(
                "Terminal status: requires scope escalation. Remaining work is a public API migration for public tool protocol names and needs a separate migration plan.",
            ),
        );

        assert!(gate.has_incomplete_unblocked_items);
        assert!(!gate.may_stop);
    }

    #[test]
    fn scope_escalation_classifier_beats_progress_report_language() {
        assert_eq!(
            classify_autonomous_final_response(
                "Remaining work exists, but it is out of scope because it changes external tool contracts.",
            ),
            AutonomousFinalResponseKind::ScopeEscalationFinal
        );
    }

    #[test]
    fn runtime_limit_blocked_final_forces_continuation_with_remaining_work() {
        let task_list = task_list(
            "active",
            vec![
                task_list_item(
                    "Add runtime-limit terminal state",
                    TaskListItemStatus::Completed,
                ),
                task_list_item("Commit task-focus batch", TaskListItemStatus::Pending),
            ],
        );

        let gate = autonomous_execution_gate_for_task_list(
            BearProfile::Pair,
            Some(&task_list),
            classify_autonomous_final_response(
                "Terminal status: blocked by runtime limits. The write budget is exhausted; continuing requires a fresh turn.",
            ),
        );

        assert!(gate.has_incomplete_unblocked_items);
        assert!(!gate.may_stop);
    }

    #[test]
    fn runtime_limit_blocked_classifier_beats_progress_report_language() {
        assert_eq!(
            classify_autonomous_final_response(
                "Remaining work exists, but the tool budget and write budget are exhausted; resume in a fresh turn.",
            ),
            AutonomousFinalResponseKind::RuntimeLimitBlockedFinal
        );
    }

    #[test]
    fn task_focus_loop_detects_repeated_scope_objections_after_nudges() {
        let recent = [
            "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.",
            "Terminal status: requires scope escalation. Remaining public tool protocol names need a separate API migration plan.",
            "Continue with: finish the active task list.",
            "Terminal status: requires scope escalation. Remaining public tool protocol names need a separate API migration plan.",
        ];

        let detection = detect_task_focus_loop(&recent);

        assert!(detection.detected);
        assert_eq!(detection.continuation_nudges, 2);
        assert_eq!(detection.terminal_objections, 2);
        assert_eq!(
            detection.repeated_objection_kind,
            Some(AutonomousFinalResponseKind::ScopeEscalationFinal)
        );
    }

    #[test]
    fn task_focus_loop_ignores_substantially_different_scope_objections() {
        let recent = [
            "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.",
            "Terminal status: requires scope escalation. Remaining public tool protocol names need a separate API migration plan.",
            "Continue with: finish the active task list.",
            "Terminal status: requires scope escalation. Database migration ownership is outside scope for this plan.",
        ];

        let detection = detect_task_focus_loop(&recent);

        assert!(!detection.detected);
        assert_eq!(detection.continuation_nudges, 2);
        assert_eq!(detection.terminal_objections, 2);
        assert_eq!(detection.repeated_objection_kind, None);
    }

    #[test]
    fn task_focus_loop_requires_substantially_same_terminal_objection() {
        let recent = [
            "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.",
            "Terminal status: blocked by runtime limits. The write budget is exhausted; continuing requires a fresh turn.",
            "Continue with: finish the active task list.",
            "Terminal status: requires scope escalation. Remaining public tool protocol names need a separate API migration plan.",
        ];

        let detection = detect_task_focus_loop(&recent);

        assert!(!detection.detected);
        assert_eq!(detection.continuation_nudges, 2);
        assert_eq!(detection.terminal_objections, 2);
        assert_eq!(detection.repeated_objection_kind, None);
    }

    #[test]
    fn task_focus_loop_ignores_single_progress_report() {
        let recent = [
            "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.",
            "What I changed: updated one file. Remaining work: tests.",
        ];

        let detection = detect_task_focus_loop(&recent);

        assert!(!detection.detected);
        assert_eq!(detection.terminal_objections, 0);
    }
}
