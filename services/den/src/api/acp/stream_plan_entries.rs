use serde_json::Value;

pub(super) fn work_plan_item_to_acp_plan_entry(item: &Value) -> Option<Value> {
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
