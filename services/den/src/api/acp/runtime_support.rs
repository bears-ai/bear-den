use crate::errors::CustomError;

pub(crate) fn looks_like_runtime_waiting_for_approval_error(err: &CustomError) -> bool {
    let message = format!("{err:#}").to_ascii_lowercase();
    message.contains("waiting for approval")
        || message.contains("please approve or deny")
        || message.contains("requires_approval")
}

pub(crate) fn looks_like_runtime_no_active_runs_error(err: &CustomError) -> bool {
    let message = format!("{err:#}").to_ascii_lowercase();
    message.contains("no active runs to cancel")
}

pub(crate) async fn cancel_runtime_runs_by_id_or_skip(
    letta: &crate::core::letta::LettaClient,
    pair_agent_id: &str,
    run_ids: &[String],
    reason: &str,
) -> serde_json::Value {
    if run_ids.is_empty() {
        return serde_json::json!({
            "ok": true,
            "skipped": true,
            "attempted": false,
            "run_ids": run_ids,
            "reason": "no_run_ids",
            "requested_reason": reason,
            "message": "Skipped runtime run cancellation because no run IDs were known; refusing agent-wide cancel for concurrent ACP safety.",
        });
    }
    match letta.cancel_agent_runs(pair_agent_id, run_ids).await {
        Ok(value) => serde_json::json!({
            "ok": true,
            "skipped": false,
            "attempted": true,
            "run_ids": run_ids,
            "result": value,
        }),
        Err(err) if looks_like_runtime_no_active_runs_error(&err) => serde_json::json!({
            "ok": true,
            "skipped": false,
            "attempted": true,
            "run_ids": run_ids,
            "result": "no_active_runs",
        }),
        Err(err) => serde_json::json!({
            "ok": false,
            "skipped": false,
            "attempted": true,
            "run_ids": run_ids,
            "error": err.to_string(),
        }),
    }
}
