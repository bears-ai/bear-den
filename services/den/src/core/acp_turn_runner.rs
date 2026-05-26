use std::sync::Arc;

use futures::{Stream, StreamExt};
use reqwest::Response;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    api::service::ApiState,
    core::{
        acp_tool_turns::AcpToolTurnCoordinator,
        letta::{LettaClient, PendingApprovalDenialMode},
        pair_turn::{post_pair_turn_messages_streaming, PairTurnBoundaryLog, PairTurnRequest},
        runtime_contracts::{
            AcpTurnRunner, CancelTurnRequest, CancelTurnResult, ContinueTurnRequest,
            ContinueTurnResult, RoleRuntimeBinding, RuntimeApprovalDecision,
            RuntimeContinuation, RuntimeConversationRef, RuntimeToolResultStatus,
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

pub struct AcpTurnContinueRequest<'a> {
    pub state: &'a ApiState,
    pub request_id: Uuid,
    pub acp_session_id: &'a str,
    pub binding: &'a RoleRuntimeBinding,
    pub continuation: RuntimeContinuation,
    pub stream_context: AcpTurnStreamContext,
}

#[derive(Debug, Clone)]
pub struct AcpTurnStreamContext {
    pub client_tools: Option<Value>,
    pub stream_tokens: bool,
    pub max_steps: u32,
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
    fn continuation_context(
        &self,
        conversation: &RuntimeConversationRef,
        binding: &RoleRuntimeBinding,
        stream: &AcpTurnStreamContext,
    ) -> crate::core::letta::LettaContinuationContext {
        crate::core::letta::LettaContinuationContext {
            conversation_id: conversation.id.clone(),
            agent_id: Some(binding.binding_id.clone()),
            client_tools: stream.client_tools.clone(),
            stream_tokens: stream.stream_tokens,
            max_steps: stream.max_steps,
        }
    }

    async fn continue_turn_response(
        &self,
        request: ContinueTurnRequest,
        stream: &AcpTurnStreamContext,
    ) -> Result<Response, CustomError> {
        let session_id = request.conversation.id.as_str();
        let context = self.continuation_context(&request.conversation, &request.binding, stream);
        match request.continuation {
            RuntimeContinuation::ToolResult {
                tool_call_id,
                approval_request_id,
                status,
                content,
            } => {
                let status = match status {
                    RuntimeToolResultStatus::Ok => "ok",
                    RuntimeToolResultStatus::Error => "error",
                    RuntimeToolResultStatus::Timeout => "timeout",
                };
                let response = self
                    .state
                    .letta
                    .post_conversation_tool_returns_streaming(
                        &context,
                        &tool_call_id,
                        approval_request_id.as_deref(),
                        status,
                        &content,
                    )
                    .await?;
                self.state.acp_tool_turns.cleanup_session(session_id);
                Ok(response)
            }
            RuntimeContinuation::ApprovalDecision {
                approval_request_id,
                tool_call_id,
                decision,
                reason,
            } => {
                let approve = matches!(decision, RuntimeApprovalDecision::Approve);
                let tool_call_id = tool_call_id.unwrap_or_default();
                let content = if approve {
                    reason.unwrap_or_else(|| "approved".to_string())
                } else {
                    reason.unwrap_or_else(|| "denied".to_string())
                };
                let status = if approve { "ok" } else { "error" };
                let response = self
                    .state
                    .letta
                    .post_conversation_tool_returns_streaming(
                        &context,
                        &tool_call_id,
                        Some(&approval_request_id),
                        status,
                        &content,
                    )
                    .await?;
                self.state.acp_tool_turns.cleanup_session(session_id);
                Ok(response)
            }
        }
    }

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
        request: ContinueTurnRequest,
    ) -> Result<ContinueTurnResult, CustomError> {
        let turn = request.turn.clone();
        let stream = AcpTurnStreamContext {
            client_tools: None,
            stream_tokens: false,
            max_steps: 2,
        };
        let _response = self.continue_turn_response(request, &stream).await?;
        Ok(ContinueTurnResult {
            turn,
            stream: crate::core::runtime_contracts::RuntimeStreamContinuation::BytesSse,
        })
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

fn find_sse_frame_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn strip_trailing_sse_delimiter_owned(mut frame: Vec<u8>) -> Vec<u8> {
    if frame.ends_with(b"\r\n\r\n") {
        frame.truncate(frame.len().saturating_sub(4));
    } else if frame.ends_with(b"\n\n") {
        frame.truncate(frame.len().saturating_sub(2));
    }
    frame
}

pub async fn continue_acp_turn_with_runtime(
    request: AcpTurnContinueRequest<'_>,
) -> Result<
    (
        crate::core::runtime_contracts::RuntimeStreamContinuation,
        crate::core::runtime_contracts::RuntimeEventStream,
    ),
    CustomError,
> {
    let runner = LettaAcpTurnRunner {
        state: request.state,
        request_id: request.request_id,
        runtime_context_len: 0,
    };
    let status = match request.continuation {
        RuntimeContinuation::ToolResult { .. }
        | RuntimeContinuation::ApprovalDecision { .. } => request.continuation,
    };
    let response = runner
        .continue_turn_response(
            ContinueTurnRequest {
                conversation: RuntimeConversationRef {
                    id: request.acp_session_id.to_string(),
                },
                turn: None,
                binding: request.binding.clone(),
                continuation: status,
            },
            &request.stream_context,
        )
        .await?;
    let mut parsed = response.bytes_stream().map(|item| item.map_err(CustomError::from));
    let mut buffer = Vec::new();
    let mut queued_events: std::collections::VecDeque<
        Result<crate::core::runtime_contracts::RuntimeStreamEvent, CustomError>,
    > = std::collections::VecDeque::new();
    let mut finished = false;
    let stream = futures::stream::poll_fn(move |cx| loop {
        if let Some(item) = queued_events.pop_front() {
            return std::task::Poll::Ready(Some(item));
        }
        if finished {
            return std::task::Poll::Ready(None);
        }
        match std::pin::Pin::new(&mut parsed).poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                buffer.extend_from_slice(&bytes);
                while let Some(end) = find_sse_frame_end(&buffer) {
                    let raw: Vec<u8> = buffer.drain(..end).collect();
                    let frame_body = strip_trailing_sse_delimiter_owned(raw);
                    queued_events.push_back(Ok(
                        crate::core::runtime_contracts::RuntimeStreamEvent::RawBytes {
                            bytes: frame_body,
                        },
                    ));
                }
            }
            std::task::Poll::Ready(Some(Err(err))) => {
                return std::task::Poll::Ready(Some(Err(err)));
            }
            std::task::Poll::Ready(None) => {
                finished = true;
                if buffer.is_empty() {
                    queued_events.push_back(Ok(
                        crate::core::runtime_contracts::RuntimeStreamEvent::TurnCompleted {
                            turn: None,
                        },
                    ));
                } else {
                    queued_events.push_back(Err(CustomError::System(format!(
                        "continuation SSE stream ended with incomplete frame ({} bytes)",
                        buffer.len()
                    ))));
                }
            }
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    });
    Ok((
        crate::core::runtime_contracts::RuntimeStreamContinuation::BytesSse,
        Box::pin(stream),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::header,
        response::{IntoResponse, Response},
        routing::post,
        Json, Router,
    };
    use sqlx::postgres::PgPoolOptions;
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    #[derive(Clone)]
    struct FakeState {
        captured: Arc<TokioMutex<Option<serde_json::Value>>>,
    }

    async fn fake_tool_return(
        State(state): State<FakeState>,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        *state.captured.lock().await = Some(body);
        (
            [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
            concat!(
                "data: {\"message_type\":\"assistant_message\",\"content\":\"continued\"}\n\n",
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            ),
        )
            .into_response()
    }

    fn test_api_state(letta: Arc<LettaClient>) -> ApiState {
        let config = Arc::new(crate::config::Config::test_stub());
        ApiState {
            sqlx_pool: PgPoolOptions::new()
                .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
                .unwrap(),
            config: config.clone(),
            letta,
            bifrost: Arc::new(crate::core::bifrost::BifrostClient::new(config.as_ref())),
            acp_tool_turns: AcpToolTurnCoordinator::new(),
            acp_turn_cancellations:
                crate::core::acp_turn_controller::AcpActiveTurnCancelRegistry::new(),
        }
    }

    #[tokio::test]
    async fn continue_turn_tool_result_without_approval_posts_tool_return_payload() {
        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::test_stub();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(LettaClient::new(&config));
        let state = test_api_state(letta);
        let runner = LettaAcpTurnRunner {
            state: &state,
            request_id: Uuid::new_v4(),
            runtime_context_len: 0,
        };

        let result = runner
            .continue_turn_response(
                ContinueTurnRequest {
                    conversation: RuntimeConversationRef {
                        id: "conv-test".to_string(),
                    },
                    turn: None,
                    binding: RoleRuntimeBinding {
                        binding_id: "agent-test".to_string(),
                        compatibility_backend: Some("letta".to_string()),
                    },
                    continuation: RuntimeContinuation::ToolResult {
                        tool_call_id: "call-1".to_string(),
                        approval_request_id: None,
                        status: RuntimeToolResultStatus::Ok,
                        content: "plain tool result".to_string(),
                    },
                },
                &AcpTurnStreamContext {
                    client_tools: Some(json!([{ "name": "fs_read_text_file" }])),
                    stream_tokens: false,
                    max_steps: 2,
                },
            )
            .await;
        assert!(result.is_ok());

        let body = captured.lock().await.clone().unwrap();
        assert_eq!(body["messages"][0]["type"], "tool_return");
        assert_eq!(body["messages"][0]["tool_returns"][0]["type"], "tool");
        assert_eq!(body["messages"][0]["tool_returns"][0]["status"], "success");
        assert_eq!(body["messages"][0]["tool_returns"][0]["tool_call_id"], "call-1");
        assert_eq!(
            body["messages"][0]["tool_returns"][0]["tool_return"],
            "plain tool result"
        );
    }

    #[tokio::test]
    async fn continue_turn_approval_decision_posts_approval_payload() {
        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::test_stub();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(LettaClient::new(&config));
        let state = test_api_state(letta);
        let runner = LettaAcpTurnRunner {
            state: &state,
            request_id: Uuid::new_v4(),
            runtime_context_len: 0,
        };

        let result = runner
            .continue_turn_response(
                ContinueTurnRequest {
                    conversation: RuntimeConversationRef {
                        id: "conv-test".to_string(),
                    },
                    turn: None,
                    binding: RoleRuntimeBinding {
                        binding_id: "agent-test".to_string(),
                        compatibility_backend: Some("letta".to_string()),
                    },
                    continuation: RuntimeContinuation::ApprovalDecision {
                        approval_request_id: "approval-1".to_string(),
                        tool_call_id: Some("call-2".to_string()),
                        decision: RuntimeApprovalDecision::Deny,
                        reason: Some("tool failed".to_string()),
                    },
                },
                &AcpTurnStreamContext {
                    client_tools: Some(json!([{ "name": "fs_read_text_file" }])),
                    stream_tokens: false,
                    max_steps: 2,
                },
            )
            .await;
        assert!(result.is_ok());

        let body = captured.lock().await.clone().unwrap();
        assert_eq!(body["messages"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approval_request_id"], "approval-1");
        assert_eq!(body["messages"][0]["approve"], false);
        assert_eq!(body["messages"][0]["approvals"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approvals"][0]["approve"], false);
        assert_eq!(body["messages"][0]["approvals"][0]["tool_call_id"], "call-2");
        assert_eq!(body["messages"][0]["approvals"][0]["reason"], "tool failed");
    }
}
