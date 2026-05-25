use std::sync::Arc;

use reqwest::Response;
use uuid::Uuid;

use crate::{
    api::service::ApiState,
    core::{
        acp_tool_turns::AcpToolTurnCoordinator,
        letta::{LettaClient, PendingApprovalDenialMode},
        pair_turn::{post_pair_turn_messages_streaming, PairTurnBoundaryLog, PairTurnRequest},
        runtime_contracts::RoleRuntimeBinding,
    },
    errors::CustomError,
};

pub const ACP_STALE_APPROVAL_RECOVERY_DENIAL_REASON: &str = "BEARS closed an expired ACP approval request during stale-approval recovery. This denial applies only to that stale request; it is not a user or web policy block. Retry the tool if it is still needed.";

pub struct AcpTurnStartRequest<'a> {
    pub state: &'a ApiState,
    pub request_id: Uuid,
    pub session_id: &'a str,
    pub bear_id: Uuid,
    pub binding: &'a RoleRuntimeBinding,
    pub upstream_target: &'a str,
    pub prompt: &'a str,
    pub client_tools: Option<serde_json::Value>,
    pub runtime_context_len: usize,
    pub stream_tokens: bool,
}

pub struct AcpStaleRuntimeCleanupParams {
    pub letta: Arc<LettaClient>,
    pub tool_turns: AcpToolTurnCoordinator,
    pub acp_session_id: String,
    pub bear_id: Uuid,
    pub pair_agent_id: String,
    pub run_ids: Vec<String>,
    pub reason: &'static str,
    pub request_id: Uuid,
}

pub fn looks_like_letta_waiting_for_approval_error(err: &CustomError) -> bool {
    let text = err.to_string();
    text.contains("waiting on an unresolved tool approval")
        || text.contains("waiting for approval")
}

async fn cancel_letta_runs_by_id_or_skip(
    _letta: &LettaClient,
    role_agent_id: &str,
    run_ids: &[String],
    reason: &str,
) -> String {
    if run_ids.is_empty() {
        tracing::warn!(
            pair_agent_id = role_agent_id,
            reason,
            "Skipping Letta run cancellation because no active run ids were recorded"
        );
        return "skipped:no_active_run_ids".to_string();
    }

    tracing::warn!(
        pair_agent_id = role_agent_id,
        reason,
        run_ids = ?run_ids,
        "Skipping Letta run-id cancellation because targeted per-run cancellation is not yet available in the runtime adapter"
    );
    format!("skipped:run_id_cancellation_unavailable:{}", run_ids.len())
}

async fn acp_preflight_runtime_hygiene(
    _state: &ApiState,
    _session_id: &str,
    _bear_id: Uuid,
    _role_agent_id: &str,
    _reason: &str,
) -> String {
    "skipped:session_turn_introspection_unavailable".to_string()
}

async fn post_turn(
    letta: &LettaClient,
    request_id: Uuid,
    session_id: &str,
    upstream_target: &str,
    role_agent_id: &str,
    prompt: &str,
    client_tools: Option<serde_json::Value>,
    runtime_context_len: usize,
    stream_tokens: bool,
) -> Result<Response, CustomError> {
    post_pair_turn_messages_streaming(
        letta,
        PairTurnRequest {
            conversation_id: upstream_target,
            role_agent_id,
            human_message: prompt,
            client_tools,
            stream_tokens,
            override_system: None,
            boundary: PairTurnBoundaryLog {
                request_id: &request_id.to_string(),
                channel_family: "acp",
                session_id,
                runtime_context_len,
            },
        },
    )
    .await
}

pub async fn start_acp_turn_with_retries(
    request: AcpTurnStartRequest<'_>,
) -> Result<Response, CustomError> {
    if request.upstream_target != request.binding.binding_id {
        let _preflight_hygiene = acp_preflight_runtime_hygiene(
            request.state,
            request.session_id,
            request.bear_id,
            &request.binding.binding_id,
            "before_new_acp_prompt",
        )
        .await;
    }

    let first_attempt = post_turn(
        request.state.letta.as_ref(),
        request.request_id,
        request.session_id,
        request.upstream_target,
        &request.binding.binding_id,
        request.prompt,
        request.client_tools.clone(),
        request.runtime_context_len,
        request.stream_tokens,
    )
    .await;

    match first_attempt {
        Ok(upstream) => Ok(upstream),
        Err(err) if looks_like_letta_waiting_for_approval_error(&err) => {
            tracing::warn!(
                %request.request_id,
                acp_session_id = %request.session_id,
                compatibility_binding_id = %request.binding.binding_id,
                error = %err,
                "Letta conversation is waiting for stale approval; skipping agent-wide cancel before retry"
            );
            request.state.acp_tool_turns.cleanup_session(request.session_id);
            let cancel_result = cancel_letta_runs_by_id_or_skip(
                request.state.letta.as_ref(),
                &request.binding.binding_id,
                &[],
                "stale_approval_retry",
            )
            .await;
            tracing::info!(
                %request.request_id,
                acp_session_id = %request.session_id,
                compatibility_binding_id = %request.binding.binding_id,
                cancel_result = %cancel_result,
                "ACP stale-approval retry cleaned process-local state without agent-wide cancellation"
            );
            match post_turn(
                request.state.letta.as_ref(),
                request.request_id,
                request.session_id,
                request.upstream_target,
                &request.binding.binding_id,
                request.prompt,
                request.client_tools.clone(),
                request.runtime_context_len,
                request.stream_tokens,
            )
            .await
            {
                Ok(upstream) => Ok(upstream),
                Err(retry_err) if looks_like_letta_waiting_for_approval_error(&retry_err) => {
                    tracing::warn!(
                        %request.request_id,
                        acp_session_id = %request.session_id,
                        compatibility_binding_id = %request.binding.binding_id,
                        conversation_id = %request.upstream_target,
                        active_tool_call_id = tracing::field::Empty,
                        error = %retry_err,
                        "Stale approval persisted after run cleanup; denying pending Letta approvals before final ACP prompt retry"
                    );
                    let denied = request
                        .state
                        .letta
                        .deny_pending_conversation_approvals(
                            request.upstream_target,
                            Some(&request.binding.binding_id),
                            ACP_STALE_APPROVAL_RECOVERY_DENIAL_REASON,
                            PendingApprovalDenialMode::InspectOnly,
                        )
                        .await?;
                    tracing::warn!(
                        %request.request_id,
                        acp_session_id = %request.session_id,
                        compatibility_binding_id = %request.binding.binding_id,
                        conversation_id = %request.upstream_target,
                        denied_count = denied.len(),
                        denied_tool_call_ids = ?denied.iter().map(|p| p.tool_call_id.as_str()).collect::<Vec<_>>(),
                        denied_source_message_ids = ?denied.iter().filter_map(|p| p.source_message_id.as_deref()).collect::<Vec<_>>(),
                        active_tool_call_id = tracing::field::Empty,
                        "Detected stale pending Letta approvals after retry failure; suppressed conversation-posted denial to avoid contaminating later turns"
                    );
                    post_turn(
                        request.state.letta.as_ref(),
                        request.request_id,
                        request.session_id,
                        request.upstream_target,
                        &request.binding.binding_id,
                        request.prompt,
                        request.client_tools,
                        request.runtime_context_len,
                        request.stream_tokens,
                    )
                    .await
                }
                Err(retry_err) => Err(retry_err),
            }
        }
        Err(err) => Err(err),
    }
}

pub async fn acp_cleanup_stale_runtime_state(
    params: AcpStaleRuntimeCleanupParams,
) -> serde_json::Value {
    let AcpStaleRuntimeCleanupParams {
        letta,
        tool_turns,
        acp_session_id,
        bear_id,
        pair_agent_id,
        run_ids,
        reason,
        request_id,
    } = params;
    tool_turns.cleanup_session(&acp_session_id);
    if run_ids.is_empty() {
        tracing::warn!(
            request_id = %request_id,
            acp_session_id = %acp_session_id,
            bear_id = %bear_id,
            pair_agent_id = %pair_agent_id,
            reason,
            "ACP stale runtime cleanup had no Letta run_ids; skipped upstream cancel to avoid agent-wide cancellation"
        );
    }
    let cancel_result =
        cancel_letta_runs_by_id_or_skip(letta.as_ref(), &pair_agent_id, &run_ids, reason).await;
    serde_json::json!({
        "ok": cancel_result.starts_with("cancelled:") || cancel_result.starts_with("skipped:"),
        "reason": reason,
        "run_ids": run_ids,
        "cancel_result": cancel_result,
    })
}
