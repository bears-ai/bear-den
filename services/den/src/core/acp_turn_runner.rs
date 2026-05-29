use std::sync::Arc;

use futures::{Stream, StreamExt};
use reqwest::Response;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    api::service::ApiState,
    core::{
        acp_tool_turns::AcpToolTurnCoordinator,
        letta::LettaClient,
        pair_turn::{post_pair_turn_messages_streaming, PairTurnBoundaryLog, PairTurnRequest},
        runtime_contracts::{
            AcpTurnRunner, CancelTurnRequest, CancelTurnResult, ContinueTurnRequest,
            ContinueTurnResult, RoleRuntimeBinding, RuntimeApprovalDecision, RuntimeCancellationBackend,
            RuntimeContinuation, RuntimeConversationRef, RuntimeStreamContinuation,
            RuntimeToolResultStatus, RuntimeTurnBackend, StartTurnRequest, StartTurnResult,
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

pub struct DenRuntimeAcpTurnRunner<'a> {
    pub state: &'a ApiState,
    pub request_id: Uuid,
    pub runtime_context_len: usize,
}

pub fn looks_like_runtime_waiting_for_approval_error(err: &CustomError) -> bool {
    let text = err.to_string();
    text.contains("waiting on an unresolved tool approval") || text.contains("waiting for approval")
}

pub struct LettaRuntimeCancellationBackend<'a> {
    letta: &'a LettaClient,
}

impl<'a> LettaRuntimeCancellationBackend<'a> {
    pub fn new(letta: &'a LettaClient) -> Self {
        Self { letta }
    }
}

#[allow(async_fn_in_trait)]
impl RuntimeCancellationBackend for LettaRuntimeCancellationBackend<'_> {
    async fn cancel_turn(
        &self,
        request: CancelTurnRequest,
    ) -> Result<CancelTurnResult, CustomError> {
        let role_agent_id = request
            .binding
            .as_ref()
            .map(|binding| binding.binding_id.as_str())
            .unwrap_or("unknown-binding");
        let reason = request.reason.as_deref().unwrap_or("runtime_cancel");
        let run_ids = request.run_ids;
        if run_ids.is_empty() {
            tracing::warn!(
                pair_agent_id = role_agent_id,
                reason,
                "Skipping runtime run cancellation because no active run ids were recorded"
            );
            return Ok(CancelTurnResult {
                skipped: true,
                detail: "skipped:no_active_run_ids".to_string(),
            });
        }

        let url = format!(
            "{}/v1/agents/{role_agent_id}/messages/cancel",
            self.letta.base_url()
        );
        let body = serde_json::json!({ "run_ids": run_ids });
        let detail = match self.letta.http().post(url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => format!("cancelled:{}", body["run_ids"].as_array().map(|ids| ids.len()).unwrap_or(0)),
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                tracing::warn!(
                    pair_agent_id = role_agent_id,
                    reason,
                    run_ids = ?body["run_ids"],
                    %status,
                    body = %text,
                    "Failed runtime run cancellation request"
                );
                format!("failed:{status}:{text}")
            }
            Err(err) => {
                tracing::warn!(
                    pair_agent_id = role_agent_id,
                    reason,
                    run_ids = ?body["run_ids"],
                    error = %err,
                    "Failed runtime run cancellation request"
                );
                format!("failed:reqwest:{err}")
            }
        };
        Ok(CancelTurnResult {
            skipped: detail.starts_with("skipped:"),
            detail,
        })
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

pub struct LettaRuntimeTurnBackend<'a> {
    letta: &'a LettaClient,
    request_id: Uuid,
    runtime_context_len: usize,
}

impl<'a> LettaRuntimeTurnBackend<'a> {
    pub fn new(letta: &'a LettaClient, request_id: Uuid, runtime_context_len: usize) -> Self {
        Self {
            letta,
            request_id,
            runtime_context_len,
        }
    }

    async fn post_turn_response(&self, request: &StartTurnRequest) -> Result<Response, CustomError> {
        let session_id = request
            .acp_session_id
            .as_deref()
            .ok_or_else(|| CustomError::ValidationError("missing acp_session_id".to_string()))?;
        post_pair_turn_messages_streaming(
            self.letta,
            PairTurnRequest {
                conversation_id: &request.conversation.id,
                role_agent_id: &request.binding.binding_id,
                human_message: &request.human_message,
                client_tools: request.client_tools.clone(),
                stream_tokens: request.stream_tokens,
                override_system: None,
                boundary: PairTurnBoundaryLog {
                    request_id: &self.request_id.to_string(),
                    channel_family: "acp",
                    session_id,
                    runtime_context_len: self.runtime_context_len,
                },
            },
        )
        .await
    }

    fn continuation_context(
        &self,
        conversation: &RuntimeConversationRef,
        binding: &RoleRuntimeBinding,
    ) -> crate::core::letta::RuntimeContinuationContext {
        crate::core::letta::RuntimeContinuationContext {
            conversation_id: conversation.id.clone(),
            agent_id: Some(binding.binding_id.clone()),
            client_tools: None,
            stream_tokens: false,
            max_steps: 2,
        }
    }

    async fn continue_turn_response(
        &self,
        request: &ContinueTurnRequest,
    ) -> Result<Response, CustomError> {
        let session_id = request.conversation.id.as_str();
        let context = self.continuation_context(&request.conversation, &request.binding);
        match &request.continuation {
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
                    .letta
                    .post_conversation_tool_returns_streaming(
                        &context,
                        tool_call_id,
                        approval_request_id.as_deref(),
                        status,
                        content,
                    )
                    .await?;
                Ok(response)
            }
            RuntimeContinuation::ApprovalDecision {
                approval_request_id,
                tool_call_id,
                decision,
                reason,
            } => {
                let approve = matches!(decision, RuntimeApprovalDecision::Approve);
                let tool_call_id = tool_call_id.clone().unwrap_or_default();
                let content = if approve {
                    reason.clone().unwrap_or_else(|| "approved".to_string())
                } else {
                    reason.clone().unwrap_or_else(|| "denied".to_string())
                };
                let status = if approve { "ok" } else { "error" };
                let response = self
                    .letta
                    .post_conversation_tool_returns_streaming(
                        &context,
                        &tool_call_id,
                        Some(approval_request_id),
                        status,
                        &content,
                    )
                    .await?;
                let _ = session_id;
                Ok(response)
            }
        }
    }
}

#[allow(async_fn_in_trait)]
impl RuntimeTurnBackend for LettaRuntimeTurnBackend<'_> {
    async fn start_turn(&self, request: StartTurnRequest) -> Result<StartTurnResult, CustomError> {
        let _response = self.post_turn_response(&request).await?;
        Ok(StartTurnResult {
            turn: None,
            stream: RuntimeStreamContinuation::BytesSse,
        })
    }

    async fn continue_turn(
        &self,
        request: ContinueTurnRequest,
    ) -> Result<ContinueTurnResult, CustomError> {
        let turn = request.turn.clone();
        let _response = self.continue_turn_response(&request).await?;
        Ok(ContinueTurnResult {
            turn,
            stream: RuntimeStreamContinuation::BytesSse,
        })
    }

    async fn start_turn_stream(
        &self,
        request: StartTurnRequest,
    ) -> Result<crate::core::runtime_contracts::RuntimeByteStream, CustomError> {
        let response = self.post_turn_response(&request).await?;
        Ok(Box::pin(response.bytes_stream().map(|item| item.map_err(Into::into))))
    }

    async fn continue_turn_stream(
        &self,
        request: ContinueTurnRequest,
    ) -> Result<crate::core::runtime_contracts::RuntimeByteStream, CustomError> {
        let response = self.continue_turn_response(&request).await?;
        Ok(Box::pin(response.bytes_stream().map(|item| item.map_err(Into::into))))
    }
}

impl<'a> DenRuntimeAcpTurnRunner<'a> {
    async fn continue_turn_response(
        &self,
        request: ContinueTurnRequest,
        _stream: &AcpTurnStreamContext,
    ) -> Result<Response, CustomError> {
        let session_id = request.conversation.id.as_str().to_string();
        let tool_call_id_to_remove = match &request.continuation {
            RuntimeContinuation::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            RuntimeContinuation::ApprovalDecision { tool_call_id, .. } => tool_call_id.clone(),
        };
        let backend = LettaRuntimeTurnBackend::new(
            self.state.letta.as_ref(),
            self.request_id,
            self.runtime_context_len,
        );
        let response = backend.continue_turn_response(&request).await?;
        if let Some(tool_call_id) = tool_call_id_to_remove.filter(|id| !id.is_empty()) {
            self.state.acp_tool_turns.remove(&session_id, &tool_call_id);
        }
        Ok(response)
    }

}

#[allow(async_fn_in_trait)]
impl AcpTurnRunner for DenRuntimeAcpTurnRunner<'_> {
    async fn preflight_hygiene(
        &self,
        binding: &RoleRuntimeBinding,
        conversation: Option<&RuntimeConversationRef>,
        reason: &str,
    ) -> Result<(), CustomError> {
        let session_id = conversation
            .map(|c| c.id.as_str())
            .unwrap_or("unknown-session");
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
        LettaRuntimeTurnBackend::new(
            self.state.letta.as_ref(),
            self.request_id,
            self.runtime_context_len,
        )
        .start_turn(request)
        .await
    }

    async fn continue_turn(
        &self,
        request: ContinueTurnRequest,
    ) -> Result<ContinueTurnResult, CustomError> {
        LettaRuntimeTurnBackend::new(
            self.state.letta.as_ref(),
            self.request_id,
            self.runtime_context_len,
        )
        .continue_turn(request)
        .await
    }

    async fn cancel_turn(
        &self,
        request: CancelTurnRequest,
    ) -> Result<CancelTurnResult, CustomError> {
        LettaRuntimeCancellationBackend::new(self.state.letta.as_ref())
            .cancel_turn(request)
            .await
    }
}

pub async fn start_acp_turn_with_retries(
    request: AcpTurnStartRequest<'_>,
) -> Result<Response, CustomError> {
    LettaRuntimeTurnBackend::new(
        request.state.letta.as_ref(),
        request.request_id,
        request.runtime_context_len,
    )
    .post_turn_response(&StartTurnRequest {
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

pub async fn start_acp_turn_stream_with_retries(
    request: AcpTurnStartRequest<'_>,
) -> Result<crate::core::runtime_contracts::RuntimeByteStream, CustomError> {
    LettaRuntimeTurnBackend::new(
        request.state.letta.as_ref(),
        request.request_id,
        request.runtime_context_len,
    )
    .start_turn_stream(StartTurnRequest {
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

fn parse_sse_event_body_to_json(body: &[u8]) -> Result<Option<serde_json::Value>, CustomError> {
    let text = std::str::from_utf8(body).map_err(|_| {
        CustomError::System(format!(
            "invalid UTF-8 in continuation SSE event body ({} bytes)",
            body.len()
        ))
    })?;
    let mut chunks: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        chunks.push(rest);
    }
    let joined = chunks.join("\n");
    let joined = joined.trim();
    if joined.is_empty() || joined == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str::<serde_json::Value>(joined)
        .map(Some)
        .map_err(|e| CustomError::System(format!("invalid continuation SSE JSON: {e}")))
}

fn runtime_stream_event_from_letta_json(
    event: &serde_json::Value,
) -> Option<crate::core::runtime_contracts::RuntimeStreamEvent> {
    let inner = match event.get("contents") {
        Some(contents) if contents.get("message_type").is_some() => contents,
        _ => event,
    };
    let message_type = inner
        .get("message_type")
        .and_then(|v| v.as_str())
        .or_else(|| event.get("message_type").and_then(|v| v.as_str()))
        .unwrap_or("");
    match message_type {
        "ping" => None,
        "assistant_message" => {
            let text =
                crate::core::acp_letta_events::letta_stream_text_preserving_whitespace(inner)
                    .or_else(|| {
                        crate::core::acp_letta_events::letta_stream_text_preserving_whitespace(
                            event,
                        )
                    })
                    .unwrap_or_default();
            Some(crate::core::runtime_contracts::RuntimeStreamEvent::AssistantTextDelta { text })
        }
        "reasoning_message" => {
            let text = inner
                .get("reasoning")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| {
                    event
                        .get("reasoning")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .or_else(|| {
                    crate::core::acp_letta_events::letta_stream_text_preserving_whitespace(inner)
                })
                .or_else(|| {
                    crate::core::acp_letta_events::letta_stream_text_preserving_whitespace(event)
                })
                .unwrap_or_default();
            Some(crate::core::runtime_contracts::RuntimeStreamEvent::RunProgress {
                kind: "status_text".to_string(),
                text: Some(text),
                phase: None,
                detail: None,
            })
        }
        "error_message" => Some(crate::core::runtime_contracts::RuntimeStreamEvent::Error {
            message: event
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Upstream error")
                .to_string(),
            detail: event
                .get("detail")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            error_type: event
                .get("error_type")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            request_id: event
                .get("request_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            context: event.get("context").cloned(),
        }),
        "stop_reason" => {
            let stop_reason = inner
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .or_else(|| event.get("stop_reason").and_then(|v| v.as_str()))
                .unwrap_or("unknown");
            if stop_reason == "end_turn" {
                Some(
                    crate::core::runtime_contracts::RuntimeStreamEvent::TurnCompleted {
                        turn: None,
                    },
                )
            } else if stop_reason == "requires_approval" {
                Some(crate::core::runtime_contracts::RuntimeStreamEvent::RunPaused {
                    reason: "awaiting_approval".to_string(),
                    resume_token: None,
                    expires_at: None,
                })
            } else {
                Some(
                    crate::core::runtime_contracts::RuntimeStreamEvent::TurnFailed {
                        turn: None,
                        category:
                            crate::core::runtime_contracts::RuntimeErrorCategory::BackendProtocol,
                        message: format!(
                            "Letta stopped before producing assistant output: {stop_reason}"
                        ),
                    },
                )
            }
        }
        "tool_call_message" | "approval_request_message" | "function_call" => Some(
            crate::core::runtime_contracts::RuntimeStreamEvent::ToolCallRequested {
                tool_call_id: event
                    .get("tool_call_id")
                    .or_else(|| inner.get("tool_call_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                tool_name: event
                    .get("tool_name")
                    .or_else(|| inner.get("tool_name"))
                    .or_else(|| event.pointer("/tool_call/name"))
                    .or_else(|| inner.pointer("/tool_call/name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                title: event
                    .get("tool_title")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                kind: event
                    .get("tool_kind")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                arguments: event
                    .get("args")
                    .cloned()
                    .or_else(|| inner.get("args").cloned())
                    .or_else(|| event.get("arguments").cloned())
                    .or_else(|| inner.get("arguments").cloned())
                    .unwrap_or_else(|| serde_json::json!({})),
                approval_request_id: event
                    .get("approval_request_id")
                    .or_else(|| inner.get("approval_request_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                approval_required: message_type == "approval_request_message",
                approval_reason: event
                    .get("approval_reason")
                    .or_else(|| inner.get("approval_reason"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            },
        ),
        _ => crate::core::acp_letta_events::native_letta_conversation_resolved_event(event).map(
            |evt| match evt {
                crate::core::acp_letta_events::AcpGatewayEvent::ConversationResolved {
                    conversation_id,
                } => crate::core::runtime_contracts::RuntimeStreamEvent::ConversationResolved {
                    conversation: crate::core::runtime_contracts::RuntimeConversationRef {
                        id: conversation_id,
                    },
                },
                _ => unreachable!(),
            },
        ),
    }
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
    let runner = DenRuntimeAcpTurnRunner {
        state: request.state,
        request_id: request.request_id,
        runtime_context_len: 0,
    };
    let status = match request.continuation {
        RuntimeContinuation::ToolResult { .. } | RuntimeContinuation::ApprovalDecision { .. } => {
            request.continuation
        }
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
    let mut parsed = response
        .bytes_stream()
        .map(|item| item.map_err(CustomError::from));
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
                    match parse_sse_event_body_to_json(&frame_body) {
                        Ok(Some(value)) => {
                            if let Some(event) = runtime_stream_event_from_letta_json(&value) {
                                queued_events.push_back(Ok(event));
                            } else {
                                queued_events.push_back(Ok(
                                    crate::core::runtime_contracts::RuntimeStreamEvent::JsonValue {
                                        value,
                                    },
                                ));
                            }
                        }
                        Ok(None) => {}
                        Err(err) => queued_events.push_back(Err(err)),
                    }
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
    let tool_turn_cleanup = tool_turns.cleanup_request_tool_turns(&acp_session_id, request_id);
    if run_ids.is_empty() {
        tracing::warn!(
            request_id = %request_id,
            acp_session_id = %acp_session_id,
            bear_id = %bear_id,
            pair_agent_id = %pair_agent_id,
            reason,
            "ACP stale runtime cleanup had no runtime run_ids; skipped upstream cancel to avoid agent-wide cancellation"
        );
    }
    let cancel_result = match LettaRuntimeCancellationBackend::new(letta.as_ref())
        .cancel_turn(CancelTurnRequest {
            conversation: RuntimeConversationRef {
                id: acp_session_id.clone(),
            },
            turn: None,
            binding: Some(RoleRuntimeBinding {
                binding_id: pair_agent_id.clone(),
                compatibility_backend: Some("runtime:letta".to_string()),
            }),
            reason: Some(reason.to_string()),
            run_ids: run_ids.clone(),
        })
        .await
    {
        Ok(result) => result,
        Err(err) => {
            return serde_json::json!({
                "ok": false,
                "reason": reason,
                "run_ids": run_ids,
                "cancel_result": format!("failed:{err}"),
                "tool_turn_cleanup": tool_turn_cleanup.to_json(),
                "cleanup_scope": {
                    "kind": "request",
                    "request_id": request_id,
                },
            });
        }
    };
    serde_json::json!({
        "ok": !cancel_result.detail.starts_with("failed:"),
        "reason": reason,
        "run_ids": run_ids,
        "cancel_result": cancel_result.detail,
        "tool_turn_cleanup": tool_turn_cleanup.to_json(),
        "cleanup_scope": {
            "kind": "request",
            "request_id": request_id,
        },
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
    use serde_json::json;
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
        let runner = DenRuntimeAcpTurnRunner {
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
        assert_eq!(
            body["messages"][0]["tool_returns"][0]["tool_call_id"],
            "call-1"
        );
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
        let runner = DenRuntimeAcpTurnRunner {
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
        assert_eq!(
            body["messages"][0]["approvals"][0]["tool_call_id"],
            "call-2"
        );
        assert_eq!(body["messages"][0]["approvals"][0]["reason"], "tool failed");
    }
}
