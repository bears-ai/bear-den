use std::sync::Arc;

use reqwest::Response;
use uuid::Uuid;

use crate::{
    api::service::ApiState,
    core::{
        acp_tool_turns::AcpToolTurnCoordinator,
        letta::{LettaClient, PendingApprovalDenialMode},
        pair_turn::{post_pair_turn_messages_streaming, PairTurnBoundaryLog, PairTurnRequest},
        runtime_contracts::{
            AcpTurnRunner, CancelTurnRequest, CancelTurnResult, ContinueTurnRequest,
            RoleRuntimeBinding, RuntimeConversationRef, RuntimeStreamEvent, RuntimeTurnRef,
            StartTurnRequest, StartTurnResult,
        },
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

pub struct LettaAcpTurnRunner<'a> {
    pub state: &'a ApiState,
    pub request_id: Uuid,
    pub runtime_context_len: usize,
}

pub fn looks_like_letta_waiting_for_approval_error(err: &CustomError) -> bool {
    let text = err.to_string();
    text.contains("waiting on an unresolved tool approval")
        || text.contains("waiting for approval")
}

async fn cancel_letta_runs_by_id_or_skip(
    letta: &LettaClient,
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

    let url = format!("{}/v1/agents/{role_agent_id}/messages/cancel", letta.base_url());
    let body = serde_json::json!({ "run_ids": run_ids });
    match letta.http().post(url).json(&body).send().await {
        Ok(resp) if resp.status().is_success() => format!("cancelled:{}", run_ids.len()),
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(
                pair_agent_id = role_agent_id,
                reason,
                run_ids = ?run_ids,
                %status,
                body = %text,
                "Failed Letta run cancellation request"
            );
            format!("failed:{status}:{text}")
        }
        Err(err) => {
            tracing::warn!(
                pair_agent_id = role_agent_id,
                reason,
                run_ids = ?run_ids,
                error = %err,
                "Failed Letta run cancellation request"
            );
            format!("failed:reqwest:{err}")
        }
    }
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

impl<'a> LettaAcpTurnRunner<'a> {
    async fn start_turn_response(
        &self,
        request: StartTurnRequest,
    ) -> Result<Response, CustomError> {
        let session_id = request
            .acp_session_id
            .as_deref()
            .ok_or_else(|| CustomError::ValidationError("missing acp_session_id".to_string()))?;
        let upstream_target = request.conversation.id.as_str();
        if upstream_target != request.binding.binding_id {
            let _preflight_hygiene = acp_preflight_runtime_hygiene(
                self.state,
                session_id,
                Uuid::nil(),
                &request.binding.binding_id,
                "before_new_acp_prompt",
            )
            .await;
        }

        let first_attempt = post_turn(
            self.state.letta.as_ref(),
            self.request_id,
            session_id,
            upstream_target,
            &request.binding.binding_id,
            &request.human_message,
            request.client_tools.clone(),
            self.runtime_context_len,
            request.stream_tokens,
        )
        .await;

        match first_attempt {
            Ok(upstream) => Ok(upstream),
            Err(err) if looks_like_letta_waiting_for_approval_error(&err) => {
                tracing::warn!(
                    %self.request_id,
                    acp_session_id = %session_id,
                    compatibility_binding_id = %request.binding.binding_id,
                    error = %err,
                    "Letta conversation is waiting for stale approval; skipping agent-wide cancel before retry"
                );
                self.state.acp_tool_turns.cleanup_session(session_id);
                let cancel_result = cancel_letta_runs_by_id_or_skip(
                    self.state.letta.as_ref(),
                    &request.binding.binding_id,
                    &[],
                    "stale_approval_retry",
                )
                .await;
                tracing::info!(
                    %self.request_id,
                    acp_session_id = %session_id,
                    compatibility_binding_id = %request.binding.binding_id,
                    cancel_result = %cancel_result,
                    "ACP stale-approval retry cleaned process-local state without agent-wide cancellation"
                );
                match post_turn(
                    self.state.letta.as_ref(),
                    self.request_id,
                    session_id,
                    upstream_target,
                    &request.binding.binding_id,
                    &request.human_message,
                    request.client_tools.clone(),
                    self.runtime_context_len,
                    request.stream_tokens,
                )
                .await
                {
                    Ok(upstream) => Ok(upstream),
                    Err(retry_err) if looks_like_letta_waiting_for_approval_error(&retry_err) => {
                        tracing::warn!(
                            %self.request_id,
                            acp_session_id = %session_id,
                            compatibility_binding_id = %request.binding.binding_id,
                            conversation_id = %upstream_target,
                            active_tool_call_id = tracing::field::Empty,
                            error = %retry_err,
                            "Stale approval persisted after run cleanup; denying pending Letta approvals before final ACP prompt retry"
                        );
                        let denied = self
                            .state
                            .letta
                            .deny_pending_conversation_approvals(
                                upstream_target,
                                Some(&request.binding.binding_id),
                                ACP_STALE_APPROVAL_RECOVERY_DENIAL_REASON,
                                PendingApprovalDenialMode::InspectOnly,
                            )
                            .await?;
                        tracing::warn!(
                            %self.request_id,
                            acp_session_id = %session_id,
                            compatibility_binding_id = %request.binding.binding_id,
                            conversation_id = %upstream_target,
                            denied_count = denied.len(),
                            denied_tool_call_ids = ?denied.iter().map(|p| p.tool_call_id.as_str()).collect::<Vec<_>>(),
                            denied_source_message_ids = ?denied.iter().filter_map(|p| p.source_message_id.as_deref()).collect::<Vec<_>>(),
                            active_tool_call_id = tracing::field::Empty,
                            "Detected stale pending Letta approvals after retry failure; suppressed conversation-posted denial to avoid contaminating later turns"
                        );
                        post_turn(
                            self.state.letta.as_ref(),
                            self.request_id,
                            session_id,
                            upstream_target,
                            &request.binding.binding_id,
                            &request.human_message,
                            request.client_tools,
                            self.runtime_context_len,
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
}

#[allow(async_fn_in_trait)]
impl AcpTurnRunner for LettaAcpTurnRunner<'_> {
    async fn preflight_hygiene(
        &self,
        binding: &RoleRuntimeBinding,
        conversation: Option<&RuntimeConversationRef>,
        reason: &str,
    ) -> Result<(), CustomError> {
        let session_id = conversation.map(|c| c.id.as_str()).unwrap_or("unknown-session");
        let _ = acp_preflight_runtime_hygiene(
            self.state,
            session_id,
            Uuid::nil(),
            &binding.binding_id,
            reason,
        )
        .await;
        Ok(())
    }

    async fn start_turn(&self, request: StartTurnRequest) -> Result<StartTurnResult, CustomError> {
        let _response = self.start_turn_response(request).await?;
        Ok(StartTurnResult { turn: None })
    }

    async fn continue_turn(
        &self,
        _request: ContinueTurnRequest,
    ) -> Result<Vec<RuntimeStreamEvent>, CustomError> {
        Ok(vec![RuntimeStreamEvent::WaitingForContinuation {
            turn: Some(RuntimeTurnRef {
                id: "letta-continuation-unimplemented".to_string(),
            }),
        }])
    }

    async fn cancel_turn(
        &self,
        request: CancelTurnRequest,
    ) -> Result<CancelTurnResult, CustomError> {
        let detail = cancel_letta_runs_by_id_or_skip(
            self.state.letta.as_ref(),
            request
                .binding
                .as_ref()
                .map(|binding| binding.binding_id.as_str())
                .unwrap_or("unknown-binding"),
            &request.run_ids,
            request.reason.as_deref().unwrap_or("runtime_cancel"),
        )
        .await;
        Ok(CancelTurnResult {
            skipped: detail.starts_with("skipped:"),
            detail,
        })
    }
}

pub async fn start_acp_turn_with_retries(
    request: AcpTurnStartRequest<'_>,
) -> Result<Response, CustomError> {
    let runner = LettaAcpTurnRunner {
        state: request.state,
        request_id: request.request_id,
        runtime_context_len: request.runtime_context_len,
    };
    runner
        .start_turn_response(StartTurnRequest {
            conversation: RuntimeConversationRef {
                id: request.upstream_target.to_string(),
            },
            binding: request.binding.clone(),
            human_message: request.prompt.to_string(),
            runtime_context: None,
            acp_session_id: Some(request.session_id.to_string()),
            client_tools: request.client_tools,
            stream_tokens: request.stream_tokens,
        })
        .await
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
    let cancel_result = cancel_letta_runs_by_id_or_skip(letta.as_ref(), &pair_agent_id, &run_ids, reason).await;
    serde_json::json!({
        "ok": cancel_result.starts_with("cancelled:") || cancel_result.starts_with("skipped:"),
        "reason": reason,
        "run_ids": run_ids,
        "cancel_result": cancel_result,
    })
}
