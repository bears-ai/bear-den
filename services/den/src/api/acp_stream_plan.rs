use serde_json::Value;
use uuid::Uuid;

use crate::{
    core::{
        acp_letta_events::AcpGatewayEvent, acp_plan_mode,
        acp_tool_turns::AcpToolResultRequest, turn_state,
    },
};

pub(super) fn mode_from_den_tool_result(result: &AcpToolResultRequest) -> Option<&str> {
    result
        .structured_content
        .get("mode_update")
        .and_then(Value::as_str)
        .filter(|mode| matches!(*mode, "ask" | "plan" | "write"))
}

fn work_plan_item_to_acp_plan_entry(item: &Value) -> Option<Value> {
    let title = item.get("title").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }
    let raw_status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending");
    let blocked_reason = item
        .get("blocked_reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let summary = item
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let content = match (raw_status, blocked_reason, summary) {
        ("blocked", Some(reason), _) => format!("Blocked: {title} — {reason}"),
        ("blocked", None, _) => format!("Blocked: {title}"),
        ("cancelled", _, _) => format!("Cancelled: {title}"),
        (_, _, Some(summary)) => format!("{title} — {summary}"),
        _ => title.to_string(),
    };
    let status = match raw_status {
        "in_progress" => "in_progress",
        "completed" | "cancelled" => "completed",
        _ => "pending",
    };
    let priority = if raw_status == "in_progress" {
        "high"
    } else {
        "medium"
    };
    Some(serde_json::json!({
        "content": content,
        "priority": priority,
        "status": status,
        "_meta": {
            "bears": {
                "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                "status": raw_status,
                "blocked_reason": item.get("blocked_reason").cloned().unwrap_or(Value::Null),
                "source_refs": item.get("source_refs").cloned().unwrap_or_else(|| serde_json::json!([])),
            }
        }
    }))
}

pub(super) fn plan_update_from_den_tool_result(
    result: &AcpToolResultRequest,
) -> Option<AcpGatewayEvent> {
    if let Some(plan) = result.structured_content.get("plan") {
        let items = plan.get("items").and_then(Value::as_array)?;
        let entries = items
            .iter()
            .filter_map(work_plan_item_to_acp_plan_entry)
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return None;
        }
        return Some(AcpGatewayEvent::PlanUpdateJson { entries });
    }

    plan_approval_fallback_from_tool_result(result)
}

pub(super) fn plan_approval_fallback_payload(
    row: &acp_plan_mode::AcpPlanModeSessionRow,
) -> Value {
    serde_json::json!({
        "kind": "submitted_plan_approval",
        "plan_id": row.id,
        "title": row.plan_title.as_deref().unwrap_or("Submitted implementation plan"),
        "body": row.plan_body.as_deref().unwrap_or(""),
        "artifact_path": row.plan_artifact_path.as_deref().unwrap_or("not_submitted"),
        "state": row.state,
        "approval_status": turn_state::approval_status_label(Some(row.state.as_str()), "Plan"),
    })
}

fn plan_approval_fallback_from_tool_result(
    result: &AcpToolResultRequest,
) -> Option<AcpGatewayEvent> {
    let workplan = result.structured_content.get("workplan")?;
    let raw_state = workplan
        .get("raw_state")
        .and_then(Value::as_str)
        .or_else(|| workplan.get("state").and_then(Value::as_str))?
        .trim();
    if raw_state != "submitted" {
        return None;
    }
    let plan_id = workplan
        .get("plan_id")
        .or_else(|| workplan.get("id"))
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())?;
    let title = workplan
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| {
            result
                .structured_content
                .get("submitted_plan")
                .and_then(|submitted| submitted.get("title"))
                .and_then(Value::as_str)
        })
        .unwrap_or("Submitted implementation plan")
        .trim()
        .to_string();
    let body = result
        .structured_content
        .get("submitted_plan")
        .and_then(|submitted| submitted.get("body"))
        .and_then(Value::as_str)
        .or_else(|| workplan.get("body").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let artifact_path = result
        .structured_content
        .get("artifact")
        .and_then(|artifact| artifact.get("path"))
        .and_then(Value::as_str)
        .or_else(|| {
            result
                .structured_content
                .get("submitted_plan")
                .and_then(|submitted| submitted.get("artifact_path"))
                .and_then(Value::as_str)
        })
        .or_else(|| workplan.get("artifact_path").and_then(Value::as_str))
        .unwrap_or("not_submitted")
        .trim()
        .to_string();

    Some(AcpGatewayEvent::PlanApprovalFallback {
        plan_id,
        title,
        body,
        artifact_path,
        state: raw_state.to_string(),
        approval_status: workplan
            .get("approval_status")
            .and_then(Value::as_str)
            .unwrap_or("awaiting_human_approval")
            .to_string(),
    })
}
