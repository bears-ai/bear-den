use crate::{
    core::{
        acp_turn_runner::LettaRuntimeCancellationBackend,
        runtime_contracts::{CancelTurnRequest, RoleRuntimeBinding, RuntimeCancellationBackend},
    },
    errors::CustomError,
};

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
    let request = CancelTurnRequest {
        conversation: crate::core::runtime_contracts::RuntimeConversationRef {
            id: "unknown-conversation".to_string(),
        },
        turn: None,
        reason: Some(reason.to_string()),
        binding: Some(RoleRuntimeBinding {
            binding_id: pair_agent_id.to_string(),
            compatibility_backend: Some("runtime:letta".to_string()),
        }),
        run_ids: run_ids.to_vec(),
    };
    match LettaRuntimeCancellationBackend::new(letta)
        .cancel_turn(request)
        .await
    {
        Ok(result) => serde_json::json!({
            "ok": true,
            "skipped": result.skipped,
            "attempted": !result.skipped,
            "run_ids": run_ids,
            "result": result.detail,
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
