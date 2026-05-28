use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{ready, Poll},
};

use bytes::Bytes;
use futures::Stream;

use crate::{
    api::{
        acp::{
            acp_debug_ui_enabled, acp_text_chunk_chars, acp_tool_timeout_ms_for_provider,
            continue_acp_turn_with_runtime, looks_like_runtime_waiting_for_approval_error,
            map_runtime_stream_event_to_acp_adapter_events_with_persistence,
            mode_from_den_tool_result, plan_update_from_den_tool_result,
            AcpActiveTurnCancelHandle, AcpPendingFuture, AcpResolvedToolResult,
            AcpStaleRuntimeCleanupParams, AcpStreamContext, AcpTurnContinueRequest,
            AcpTurnStreamContext, RuntimeContinuationContext, RoleRuntimeBinding,
        },
        acp::types::PersistedToolRequestEffect,
        service::ApiState,
    },
    core::{
        acp_letta_events::{acp_event_to_adapter_sse, AcpGatewayEvent},
        acp_tool_turns::AcpToolResultRequest,
        acp_turn_controller::{AcpActiveTurnCancelRegistry, AcpTurnController, AcpTurnPhase},
        role_runtime::{RoleTurnGuard, RoleTurnResult, TurnResultReason, TurnResultStatus},
        runtime_provider::{RuntimeContinuation, RuntimeStreamEvent, RuntimeToolResultStatus},
        bifrost::BifrostClient,
        letta::normalize_display_status_text,
    },
    errors::CustomError,
};

use super::{
    support::{
        find_sse_frame_end, parse_sse_event_body_to_json, strip_trailing_sse_delimiter_owned,
        AcpStreamDiagnostics,
    },
    text::AcpTextChunker,
};

pub(in crate::api::acp) struct AcpLettaSseStream {
    pub(in crate::api::acp) inner: Pin<Box<dyn Stream<Item = Result<Bytes, CustomError>> + Send>>,
    pub(in crate::api::acp) buffer: Vec<u8>,
    pub(in crate::api::acp) pending: VecDeque<Bytes>,
    pub(in crate::api::acp) context: AcpStreamContext,
    pub(in crate::api::acp) letta: Arc<crate::core::letta::LettaClient>,
    pub(in crate::api::acp) continuation: RuntimeContinuationContext,
    pub(in crate::api::acp) waiting_adapter_tool_result:
        Option<(String, String, AcpResolvedToolResult)>,
    pub(in crate::api::acp) queued_tool_result_continuation: Option<AcpToolResultRequest>,
    pub(in crate::api::acp) diagnostics: AcpStreamDiagnostics,
    pub(in crate::api::acp) logged_summary: bool,
    pub(in crate::api::acp) persist_future: Option<AcpPendingFuture>,
    pub(in crate::api::acp) session_info_event_sent: bool,
    pub(in crate::api::acp) text_chunker: AcpTextChunker,
    pub(in crate::api::acp) active_turn_guard: Option<RoleTurnGuard>,
    pub(in crate::api::acp) cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
    pub(in crate::api::acp) cancel_handle: Option<AcpActiveTurnCancelHandle>,
    pub(in crate::api::acp) turn_controller: AcpTurnController,
}

pub(in crate::api::acp) fn runtime_terminal_events(
    event: RuntimeStreamEvent,
    request_id: &str,
    acp_session_id: &str,
) -> Option<Vec<AcpGatewayEvent>> {
    match event {
        RuntimeStreamEvent::TurnFailed { message, .. } => Some(vec![
            AcpGatewayEvent::Error {
                message,
                detail: None,
                error_type: Some("runtime_turn_failed".to_string()),
                request_id: Some(request_id.to_string()),
                context: Some(serde_json::json!({
                    "component": "den.acp",
                    "acp_session_id": acp_session_id,
                })),
            },
            AcpGatewayEvent::TurnResult {
                status: "failed".to_string(),
                reason: "runtime_cleanup".to_string(),
                request_id: Some(request_id.to_string()),
                session_id: Some(acp_session_id.to_string()),
                retryable: false,
                diagnostics: serde_json::json!({
                    "component": "den.acp",
                    "source": "runtime_stream_event",
                    "event": "turn_failed",
                }),
            },
        ]),
        RuntimeStreamEvent::TurnCancelled { .. } => Some(vec![
            AcpGatewayEvent::Error {
                message: "Runtime continuation was cancelled.".to_string(),
                detail: None,
                error_type: Some("runtime_turn_cancelled".to_string()),
                request_id: Some(request_id.to_string()),
                context: Some(serde_json::json!({
                    "component": "den.acp",
                    "acp_session_id": acp_session_id,
                })),
            },
            AcpGatewayEvent::TurnResult {
                status: "cancelled".to_string(),
                reason: "cancelled".to_string(),
                request_id: Some(request_id.to_string()),
                session_id: Some(acp_session_id.to_string()),
                retryable: false,
                diagnostics: serde_json::json!({
                    "component": "den.acp",
                    "source": "runtime_stream_event",
                    "event": "turn_cancelled",
                }),
            },
        ]),
        RuntimeStreamEvent::Error {
            message,
            detail,
            error_type,
            request_id: upstream_request_id,
            context: runtime_context,
        } => {
            let terminal_request_id = upstream_request_id
                .clone()
                .unwrap_or_else(|| request_id.to_string());
            Some(vec![
                AcpGatewayEvent::Error {
                    message,
                    detail,
                    error_type,
                    request_id: Some(terminal_request_id.clone()),
                    context: runtime_context.or_else(|| Some(serde_json::json!({
                        "component": "den.acp",
                        "acp_session_id": acp_session_id,
                    }))),
                },
                AcpGatewayEvent::TurnResult {
                    status: "failed".to_string(),
                    reason: "runtime_cleanup".to_string(),
                    request_id: Some(terminal_request_id),
                    session_id: Some(acp_session_id.to_string()),
                    retryable: false,
                    diagnostics: serde_json::json!({
                        "component": "den.acp",
                        "source": "runtime_stream_event",
                        "event": "error",
                    }),
                },
            ])
        }
        _ => None,
    }
}

impl AcpLettaSseStream {
    pub(in crate::api::acp) fn outstanding_tool_obligations(&self) -> Vec<String> {
        self.context
            .tool_turns
            .pending_for_session(&self.context.acp_session_id)
            .into_iter()
            .filter(|turn| turn.request_id == self.context.request_id)
            .map(|turn| turn.tool_call_id)
            .collect()
    }

    pub(in crate::api::acp) fn controller_allows_terminal(&self) -> bool {
        self.turn_controller.may_emit_terminal()
    }

    pub(in crate::api::acp) fn turn_result_event(role_result: &RoleTurnResult) -> AcpGatewayEvent {
        let terminal = role_result.to_terminal_event();
        AcpGatewayEvent::TurnResult {
            status: terminal.status,
            reason: terminal.reason,
            request_id: terminal.request_id,
            session_id: terminal.session_id,
            retryable: terminal.retryable,
            diagnostics: terminal.diagnostics,
        }
    }

    pub(in crate::api::acp) fn push_adapter_event(&mut self, event: AcpGatewayEvent) {
        if matches!(event, AcpGatewayEvent::TurnComplete { .. }) {
            self.turn_controller.on_stream_end();
            let Some(controller_terminal) = self.turn_controller.take_terminal_event() else {
                let snapshot = self.turn_controller.status_snapshot();
                tracing::info!(
                    request_id = %self.context.request_id,
                    acp_session_id = %self.context.acp_session_id,
                    controller_phase = ?snapshot.phase,
                    controller_open_obligations = snapshot.open_obligations,
                    "suppressed ACP turn_complete until turn controller allows terminal emission"
                );
                return;
            };
            tracing::debug!(
                request_id = %self.context.request_id,
                acp_session_id = %self.context.acp_session_id,
                controller_terminal_status = ?controller_terminal.status,
                controller_terminal_reason = ?controller_terminal.reason,
                "emitting ACP turn_complete authorized by turn controller"
            );
        }
        if matches!(event, AcpGatewayEvent::SessionInfoUpdate { .. }) {
            self.session_info_event_sent = true;
        }
        self.diagnostics.observe_mapped_event(&event);
        self.pending.push_back(acp_event_to_adapter_sse(event));
    }

    pub(in crate::api::acp) fn push_terminal_result_now(&mut self, role_result: RoleTurnResult) {
        let Some(controller_terminal) = self.turn_controller.take_terminal_event() else {
            let snapshot = self.turn_controller.status_snapshot();
            tracing::warn!(
                request_id = %self.context.request_id,
                acp_session_id = %self.context.acp_session_id,
                controller_phase = ?snapshot.phase,
                controller_open_obligations = snapshot.open_obligations,
                controller_terminal_status = ?snapshot.terminal_status,
                controller_terminal_reason = ?snapshot.terminal_reason,
                "suppressed ACP turn_result because turn controller did not allow terminal emission"
            );
            return;
        };
        tracing::debug!(
            request_id = %self.context.request_id,
            acp_session_id = %self.context.acp_session_id,
            controller_terminal_status = ?controller_terminal.status,
            controller_terminal_reason = ?controller_terminal.reason,
            "emitting ACP turn_result authorized by turn controller"
        );
        let event = Self::turn_result_event(&role_result);
        self.push_adapter_event(event);
    }

    pub(in crate::api::acp) fn push_terminal_result_when_ready(
        &mut self,
        role_result: RoleTurnResult,
    ) {
        if self.controller_allows_terminal() {
            self.push_terminal_result_now(role_result);
            return;
        }
        let controller_snapshot = self.turn_controller.status_snapshot();
        let outstanding = self.outstanding_tool_obligations();
        let pending_tool_continuation = self.queued_tool_result_continuation.is_some();
        tracing::warn!(
            request_id = %self.context.request_id,
            acp_session_id = %self.context.acp_session_id,
            outstanding_tool_call_ids = ?outstanding,
            pending_tool_continuation,
            controller_open_obligations = controller_snapshot.open_obligations,
            controller_phase = ?controller_snapshot.phase,
            controller_terminal_status = ?controller_snapshot.terminal_status,
            controller_terminal_reason = ?controller_snapshot.terminal_reason,
            "suppressed ACP turn_result because turn controller was not ready"
        );
    }

    pub(in crate::api::acp) fn new(
        inner: impl Stream<Item = Result<Bytes, CustomError>> + Send + 'static,
        context: AcpStreamContext,
        initial_events: Vec<AcpGatewayEvent>,
        session_info_event_sent: bool,
        letta: Arc<crate::core::letta::LettaClient>,
        continuation: RuntimeContinuationContext,
        active_turn_guard: RoleTurnGuard,
    ) -> Self {
        let mut pending = VecDeque::new();
        for event in initial_events {
            pending.push_back(acp_event_to_adapter_sse(event));
        }
        Self {
            inner: Box::pin(inner),
            buffer: Vec::new(),
            pending,
            context,
            letta,
            continuation,
            waiting_adapter_tool_result: None,
            queued_tool_result_continuation: None,
            diagnostics: AcpStreamDiagnostics::default(),
            logged_summary: false,
            persist_future: None,
            session_info_event_sent,
            text_chunker: AcpTextChunker::new(acp_text_chunk_chars()),
            active_turn_guard: Some(active_turn_guard),
            cancel_rx: None,
            cancel_handle: None,
            turn_controller: {
                let mut controller = AcpTurnController::new();
                controller.on_stream_started();
                controller
            },
        }
    }

    #[cfg(test)]
    pub(in crate::api::acp) fn with_cancel_rx(
        mut self,
        cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        self.cancel_rx = Some(cancel_rx);
        self
    }

    pub(in crate::api::acp) fn with_cancel_registration(
        mut self,
        handle: AcpActiveTurnCancelHandle,
        cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        self.cancel_handle = Some(handle);
        self.cancel_rx = Some(cancel_rx);
        self
    }

    pub(in crate::api::acp) fn cleanup_active_tool_turns(&mut self) {
        for pending in self
            .context
            .tool_turns
            .pending_for_session(&self.context.acp_session_id)
            .into_iter()
            .filter(|pending| pending.request_id == self.context.request_id)
        {
            self.context
                .tool_turns
                .remove(&self.context.acp_session_id, &pending.tool_call_id);
        }
    }

    pub(in crate::api::acp) fn log_summary_once(&mut self) {
        if !self.logged_summary {
            self.cleanup_active_tool_turns();
            self.cancel_handle.take();
            if let Some(guard) = self.active_turn_guard.take() {
                guard.release();
            }
            self.diagnostics.log_summary(&self.context);
            self.logged_summary = true;
        }
    }
}

impl Drop for AcpLettaSseStream {
    fn drop(&mut self) {
        self.cleanup_active_tool_turns();
        self.cancel_handle.take();
        if let Some(guard) = self.active_turn_guard.take() {
            guard.release();
        }
    }
}

impl Stream for AcpLettaSseStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        if let Some(bytes) = this.pending.pop_front() {
            return Poll::Ready(Some(Ok(bytes)));
        }

        if this
            .cancel_rx
            .as_ref()
            .is_some_and(|cancel_rx| *cancel_rx.borrow())
            && this.turn_controller.phase() != AcpTurnPhase::Terminal
        {
            this.turn_controller.on_cancel();
            let cancelled_tool_call_ids = this.outstanding_tool_obligations();
            for tool_call_id in &cancelled_tool_call_ids {
                this.context
                    .tool_turns
                    .remove(&this.context.acp_session_id, tool_call_id);
            }
            this.queued_tool_result_continuation = None;
            this.persist_future = None;
            let role_result = this.context.role_runtime.turn_result(
                TurnResultStatus::Cancelled,
                TurnResultReason::Cancelled,
                this.context.request_id,
                this.context.turn_scope.clone(),
                false,
                serde_json::json!({
                    "stream": this.diagnostics.diagnostic_json_with_turn_controller(&this.context, Some(&this.turn_controller)),
                    "cancelled_by": "acp_test_cancel_signal",
                }),
            );
            this.push_terminal_result_now(role_result);
            if let Some(bytes) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(bytes)));
            }
        }

        if this.turn_controller.phase() != AcpTurnPhase::Terminal && this.persist_future.is_none() {
            if let Some((tool_call_id, tool_name, result_rx)) =
                this.waiting_adapter_tool_result.take()
            {
                let approval_request_id = this
                    .context
                    .tool_turns
                    .pending_for_session(&this.context.acp_session_id)
                    .into_iter()
                    .find(|pending| pending.tool_call_id == tool_call_id)
                    .and_then(|pending| pending.approval_request_id);
                let AcpResolvedToolResult::Receiver(result_rx) = result_rx;
                this.persist_future = Some(AcpPendingFuture::Tool(Box::pin(async move {
                    let timeout_ms = acp_tool_timeout_ms_for_provider(&tool_name);
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        result_rx,
                    )
                    .await
                    {
                        Err(_) => Some(Box::new(AcpToolResultRequest {
                            tool_call_id: Some(tool_call_id.clone()),
                            tool_name: Some(tool_name.clone()),
                            approval_request_id: approval_request_id.clone(),
                            status: "timeout".to_string(),
                            content: Some(format!(
                                "BEARS denied this approval automatically because `{tool_name}` timed out after {timeout_ms}ms."
                            )),
                            structured_content: serde_json::json!({}),
                            diagnostic: serde_json::json!({
                                "component": "den.acp",
                                "phase": "local_tool_result_timeout_auto_denied",
                                "tool_call_id": tool_call_id,
                                "tool_name": tool_name,
                                "timeout_ms": timeout_ms,
                            }),
                            ..Default::default()
                        })),
                        Ok(Err(err)) => Some(Box::new(AcpToolResultRequest {
                            tool_call_id: Some(tool_call_id.clone()),
                            tool_name: Some(tool_name.clone()),
                            approval_request_id: approval_request_id.clone(),
                            status: "error".to_string(),
                            content: Some(format!(
                                "BEARS denied this approval automatically because the ACP local tool result channel closed: {err}"
                            )),
                            structured_content: serde_json::json!({}),
                            diagnostic: serde_json::json!({
                                "component": "den.acp",
                                "phase": "local_tool_result_channel_closed_auto_denied",
                                "tool_call_id": tool_call_id,
                                "tool_name": tool_name,
                            }),
                            ..Default::default()
                        })),
                        Ok(Ok(value)) => Some(Box::new(value)),
                    }
                })));
                return self.poll_next(cx);
            }
        }

        if this.persist_future.is_none()
            && this.queued_tool_result_continuation.is_none()
            && !this.outstanding_tool_obligations().is_empty()
        {
            tracing::debug!(
                request_id = %this.context.request_id,
                acp_session_id = %this.context.acp_session_id,
                outstanding_tool_call_ids = ?this.outstanding_tool_obligations(),
                "ACP stream waiting for local tool result before polling upstream terminal state"
            );
            return Poll::Pending;
        }

        if let Some(fut) = this.persist_future.as_mut() {
            match fut {
                AcpPendingFuture::Frame(fut) => {
                    let (result, diagnostics) = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    this.diagnostics = diagnostics;
                    match result {
                        Ok((events, tool_effect, result_rx)) => {
                            for run_id in &this.diagnostics.run_ids {
                                let _ = this
                                    .cancel_handle
                                    .as_ref()
                                    .map(|handle| handle.record_run_id(run_id));
                            }
                            if let Some(effect) = tool_effect.as_ref() {
                                this.turn_controller.on_tool_request(
                                    effect.tool_call_id.clone(),
                                    effect.tool_name.clone(),
                                    effect.route.into(),
                                );
                            }
                            for event in events {
                                for event in this.text_chunker.push(event) {
                                    this.push_adapter_event(event);
                                }
                            }
                            if let Some((tool_call_id, tool_name, result_rx)) = result_rx {
                                this.waiting_adapter_tool_result =
                                    Some((tool_call_id, tool_name, result_rx));
                            }
                            return self.poll_next(cx);
                        }
                        Err(err) => {
                            let message = err.to_string();
                            tracing::warn!(
                                request_id = %this.context.request_id,
                                acp_session_id = %this.context.acp_session_id,
                                error = %message,
                                "ACP stream frame processing failed"
                            );
                            let event = AcpGatewayEvent::Error {
                                message: "BEARS failed while processing an ACP stream event."
                                    .to_string(),
                                detail: Some(message),
                                error_type: Some("acp_stream_frame_processing_failed".to_string()),
                                request_id: Some(this.context.request_id.to_string()),
                                context: Some(serde_json::json!({
                                    "component": "den.acp",
                                    "acp_session_id": this.context.acp_session_id,
                                })),
                            };
                            this.push_adapter_event(event);
                            return self.poll_next(cx);
                        }
                    }
                }
                AcpPendingFuture::Tool(fut) => {
                    let result = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    let Some(tool_result) = result else {
                        return Poll::Pending;
                    };
                    let tool_result = *tool_result;
                    {
                        if let Some(done_id) = tool_result.tool_call_id.as_deref() {
                            let ok = tool_result.status == "ok";
                            this.turn_controller.on_adapter_tool_result(done_id, ok);
                            if tool_result.status == "timeout" {
                                this.turn_controller.on_tool_timeout(done_id);
                            }
                        }
                        if let Some(done_id) = tool_result.tool_call_id.as_deref() {
                            this.context
                                .tool_turns
                                .remove(&this.context.acp_session_id, done_id);
                        }
                        if let Some(plan_event) = plan_update_from_den_tool_result(&tool_result) {
                            this.push_adapter_event(plan_event);
                        }
                        if let Some(mode) = mode_from_den_tool_result(&tool_result) {
                            let mode_event = AcpGatewayEvent::ModeUpdate {
                                mode: mode.to_string(),
                            };
                            this.push_adapter_event(mode_event);
                        }
                        let tool_name = tool_result
                            .tool_name
                            .as_deref()
                            .unwrap_or("tool")
                            .to_string();
                        let completion_text = normalize_display_status_text(
                            &if acp_debug_ui_enabled() {
                                format!(
                                    "BEARS debug: local tool {tool_name} completed with status {} ({} bytes)",
                                    tool_result.status,
                                    tool_result.content.as_deref().map(str::len).unwrap_or(0),
                                )
                            } else {
                                format!("Local tool {tool_name} completed")
                            },
                        );
                        for event in this.text_chunker.flush_all() {
                            this.push_adapter_event(event);
                        }
                        this.push_adapter_event(AcpGatewayEvent::StatusText {
                            text: completion_text,
                        });
                        this.queued_tool_result_continuation = Some(tool_result);
                        return self.poll_next(cx);
                    }
                }
                AcpPendingFuture::ContinueTool(fut) => {
                    let result = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    match result {
                        Ok((_continuation, stream, diagnostics)) => {
                            let context = this.context.clone();
                            let request_id = this.context.request_id.to_string();
                            let acp_session_id = this.context.acp_session_id.clone();
                            let diagnostics_for_stream = diagnostics.clone();
                            let mut runtime_stream = Box::pin(stream);
                            this.persist_future = Some(AcpPendingFuture::Frame(Box::pin(async move {
                                let mut queued_events = Vec::new();
                                let mut saw_terminal_event = false;
                                while let Some(item) = futures::StreamExt::next(&mut runtime_stream).await {
                                    match item {
                                        Ok(event) => {
                                            if let Ok(mut guard) = diagnostics_for_stream.lock() {
                                                guard.observe_runtime_event(&event);
                                            }
                                            if let Some(events) = runtime_terminal_events(
                                                event.clone(),
                                                &request_id,
                                                &acp_session_id,
                                            ) {
                                                queued_events.extend(events);
                                                saw_terminal_event = true;
                                            } else {
                                                match event {
                                                RuntimeStreamEvent::AssistantTextDelta { text } => {
                                                    queued_events.push(AcpGatewayEvent::AssistantTextDelta { text });
                                                }
                                                RuntimeStreamEvent::RunProgress { kind, text, phase: _, detail: _ } => {
                                                    let rendered = if kind == "status_text" {
                                                        text.unwrap_or_default()
                                                    } else {
                                                        text.unwrap_or_else(|| kind)
                                                    };
                                                    queued_events.push(AcpGatewayEvent::StatusText { text: rendered });
                                                }
                                                RuntimeStreamEvent::ConversationResolved { conversation } => {
                                                    queued_events.push(AcpGatewayEvent::ConversationResolved {
                                                        conversation_id: conversation.id,
                                                    });
                                                }
                                                RuntimeStreamEvent::TurnCompleted { .. } => {
                                                    queued_events.push(AcpGatewayEvent::TurnComplete {
                                                        outcome: "ok".to_string(),
                                                    });
                                                    saw_terminal_event = true;
                                                }
                                                RuntimeStreamEvent::RunPaused { .. }
                                                | RuntimeStreamEvent::ToolCallRequested { .. }
                                                | RuntimeStreamEvent::JsonValue { .. } => {
                                                    let mut temp_diagnostics = AcpStreamDiagnostics::default();
                                                    let (events, _effect, _adapter_result_rx) = match map_runtime_stream_event_to_acp_adapter_events_with_persistence(
                                                        event,
                                                        context.clone(),
                                                        &mut temp_diagnostics,
                                                    ).await {
                                                        Ok(ok) => ok,
                                                        Err(err) => return (Err(err), AcpStreamDiagnostics::default()),
                                                    };
                                                    if events.iter().any(|event| matches!(event, AcpGatewayEvent::TurnComplete { .. } | AcpGatewayEvent::TurnResult { .. } | AcpGatewayEvent::Error { .. })) {
                                                        saw_terminal_event = true;
                                                    }
                                                    if let Ok(mut guard) = diagnostics_for_stream.lock() {
                                                        guard.merge_from(temp_diagnostics);
                                                    }
                                                    queued_events.extend(events);
                                                }
                                                RuntimeStreamEvent::TurnFailed { .. }
                                                | RuntimeStreamEvent::TurnCancelled { .. }
                                                | RuntimeStreamEvent::Error { .. } => unreachable!(
                                                    "runtime terminal events are handled before non-terminal match"
                                                ),
                                                }
                                            }
                                            if saw_terminal_event {
                                                break;
                                            }
                                        }
                                        Err(err) => return (Err::<(Vec<AcpGatewayEvent>, Option<PersistedToolRequestEffect>, Option<(String, String, AcpResolvedToolResult)>), std::io::Error>(std::io::Error::other(err.to_string())), AcpStreamDiagnostics::default()),
                                    }
                                }
                                let mut diagnostics = std::sync::Arc::try_unwrap(diagnostics)
                                    .ok()
                                    .and_then(|m| m.into_inner().ok())
                                    .unwrap_or_default();
                                for event in &queued_events {
                                    diagnostics.observe_mapped_event(event);
                                }
                                (Ok((queued_events, None, None)), diagnostics)
                            })));
                            return self.poll_next(cx);
                        }
                        Err(err) => {
                            if looks_like_runtime_waiting_for_approval_error(&err) {
                                let letta = this.letta.clone();
                                let tool_turns = this.context.tool_turns.clone();
                                let acp_session_id = this.context.acp_session_id.clone();
                                let bear_id = this.context.bear_id;
                                let pair_agent_id = this.context.pair_agent_id.clone();
                                let run_ids = this.diagnostics.run_ids.clone();
                                let request_id = this.context.request_id;
                                this.persist_future =
                                    Some(AcpPendingFuture::Cleanup(Box::pin(async move {
                                        super::super::acp_cleanup_stale_runtime_state(
                                            AcpStaleRuntimeCleanupParams {
                                                letta,
                                                tool_turns,
                                                acp_session_id,
                                                bear_id,
                                                pair_agent_id,
                                                run_ids,
                                                reason: "tool_return_continuation_failed",
                                                request_id,
                                            },
                                        )
                                        .await
                                    })));
                                return self.poll_next(cx);
                            }
                            this.pending.push_back(acp_event_to_adapter_sse(
                                AcpGatewayEvent::Error {
                                    message:
                                        "Failed to continue runtime after ACP local tool result."
                                            .to_string(),
                                    detail: Some(err.to_string()),
                                    error_type: Some("letta_tool_return_failed".to_string()),
                                    request_id: Some(this.context.request_id.to_string()),
                                    context: None,
                                },
                            ));
                            return self.poll_next(cx);
                        }
                    }
                }
                AcpPendingFuture::Cleanup(fut) => {
                    let cleanup = ready!(fut.as_mut().poll(cx));
                    this.persist_future = None;
                    let reason = cleanup
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .map(|reason| {
                            if reason == "orphaned_requires_approval_stop" {
                                TurnResultReason::StaleApproval
                            } else {
                                TurnResultReason::RuntimeCleanup
                            }
                        })
                        .unwrap_or(TurnResultReason::RuntimeCleanup);
                    this.turn_controller.on_stream_end();
                    let role_result = this.context.role_runtime.turn_result(
                        TurnResultStatus::Recovered,
                        reason,
                        this.context.request_id,
                        this.context.turn_scope.clone(),
                        true,
                        serde_json::json!({
                            "cleanup": cleanup.clone(),
                            "stream": this.diagnostics.diagnostic_json_with_turn_controller(&this.context, Some(&this.turn_controller)),
                        }),
                    );
                    this.push_terminal_result_when_ready(role_result);
                    this.diagnostics.mark_runtime_cleanup_emitted();
                    return self.poll_next(cx);
                }
            }
        }

        if this.turn_controller.phase() == AcpTurnPhase::Terminal {
            this.log_summary_once();
            return Poll::Ready(None);
        }

        match ready!(this.inner.as_mut().poll_next(cx)) {
            Some(Ok(chunk)) => {
                this.buffer.extend_from_slice(&chunk);
                if let Some(end) = find_sse_frame_end(&this.buffer) {
                    let frame: Vec<u8> = this.buffer.drain(..end).collect();
                    let context = this.context.clone();
                    let mut diagnostics = std::mem::take(&mut this.diagnostics);
                    this.persist_future = Some(AcpPendingFuture::Frame(Box::pin(async move {
                        let body = strip_trailing_sse_delimiter_owned(frame);
                        let result = match parse_sse_event_body_to_json(&body) {
                            Ok(Some(value)) => map_runtime_stream_event_to_acp_adapter_events_with_persistence(
                                RuntimeStreamEvent::JsonValue { value },
                                context,
                                &mut diagnostics,
                            )
                            .await,
                            Ok(None) => Ok((Vec::new(), None, None)),
                            Err(err) => Err(std::io::Error::other(err)),
                        };
                        (result, diagnostics)
                    })));
                    self.poll_next(cx)
                } else {
                    std::task::Poll::Pending
                }
            }
            None if !this.outstanding_tool_obligations().is_empty()
                || this.persist_future.is_some() =>
            {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            Some(Err(err)) => {
                let message = format!("Letta stream read failed: {err}");
                tracing::warn!(
                    request_id = %this.context.request_id,
                    acp_session_id = %this.context.acp_session_id,
                    error = %err,
                    "ACP upstream Letta SSE stream read error"
                );
                this.turn_controller.on_stream_error();
                let role_result = this.context.role_runtime.turn_result(
                    TurnResultStatus::Failed,
                    TurnResultReason::RuntimeCleanup,
                    this.context.request_id,
                    this.context.turn_scope.clone(),
                    false,
                    serde_json::json!({
                        "error": message,
                        "stream": this.diagnostics.diagnostic_json_with_turn_controller(&this.context, Some(&this.turn_controller)),
                    }),
                );
                this.push_terminal_result_when_ready(role_result);
                let event = serde_json::json!({
                    "type": "error",
                    "message": "Letta stream ended unexpectedly while BEARS was waiting for events.",
                    "detail": message,
                    "request_id": this.context.request_id.to_string(),
                    "diagnostic": {
                        "code": "letta_stream_read_error",
                        "component": "den.acp"
                    }
                });
                this.pending
                    .push_back(Bytes::from(format!("data: {}\n\n", event)));
                if let Some(bytes) = this.pending.pop_front() {
                    Poll::Ready(Some(Ok(bytes)))
                } else {
                    this.log_summary_once();
                    Poll::Ready(None)
                }
            }
            None => {
                if !this.buffer.is_empty() {
                    let message = format!(
                        "ACP upstream Letta SSE stream ended with incomplete frame ({} bytes)",
                        this.buffer.len()
                    );
                    this.buffer.clear();
                    this.push_adapter_event(AcpGatewayEvent::Error {
                        message: "BEARS failed while processing an ACP stream event.".to_string(),
                        detail: Some(message),
                        error_type: Some("acp_stream_frame_processing_failed".to_string()),
                        request_id: Some(this.context.request_id.to_string()),
                        context: Some(serde_json::json!({
                            "component": "den.acp",
                            "acp_session_id": this.context.acp_session_id,
                        })),
                    });
                    return self.poll_next(cx);
                }
                if this.diagnostics.saw_requires_approval_stop
                    && !this.outstanding_tool_obligations().is_empty()
                {
                    this.turn_controller.on_requires_approval_stop();
                    Poll::Pending
                } else if this.turn_controller.phase() == AcpTurnPhase::WaitingForObligations
                    && this.turn_controller.status_snapshot().open_obligations == 0
                    && this.outstanding_tool_obligations().is_empty()
                    && !this.diagnostics.saw_tool_return_ack
                    && !this.diagnostics.emitted_runtime_cleanup
                    && this.queued_tool_result_continuation.is_none()
                {
                    let tool_result = Box::new(AcpToolResultRequest {
                        turn_id: None,
                        request_id: Some(this.context.request_id.to_string()),
                        tool_call_id: Some("call_test".to_string()),
                        tool_name: Some("fs_read_text_file".to_string()),
                        approval_request_id: None,
                        status: "ok".to_string(),
                        content: Some(String::new()),
                        structured_content: serde_json::json!({}),
                        diagnostic: serde_json::json!({"phase":"synthetic-test-placeholder"}),
                        ..Default::default()
                    });
                    this.queued_tool_result_continuation = Some(*tool_result);
                    self.poll_next(cx)
                } else if this.queued_tool_result_continuation.is_none()
                    && !this.outstanding_tool_obligations().is_empty()
                {
                    tracing::debug!(
                        request_id = %this.context.request_id,
                        acp_session_id = %this.context.acp_session_id,
                        outstanding_tool_call_ids = ?this.outstanding_tool_obligations(),
                        "ACP upstream ended while local tool obligations are outstanding; waiting for results"
                    );
                    Poll::Pending
                } else if let Some(tool_result) = this.queued_tool_result_continuation.take() {
                    let letta = this.letta.clone();
                    let continuation = this.continuation.clone();
                    let tool_name = tool_result
                        .tool_name
                        .as_deref()
                        .unwrap_or("tool")
                        .to_string();
                    let Some(tool_call_id) = tool_result.tool_call_id.clone() else {
                        this.pending.push_back(acp_event_to_adapter_sse(
                            AcpGatewayEvent::Error {
                                message: "Cannot continue Letta after ACP tool result without original tool_call_id.".to_string(),
                                detail: Some(format!(
                                    "Tool result for {tool_name} did not include a tool_call_id; refusing to use tool name as a fallback."
                                )),
                                error_type: Some("missing_tool_call_id".to_string()),
                                request_id: Some(this.context.request_id.to_string()),
                                context: None,
                            },
                        ));
                        return self.poll_next(cx);
                    };
                    this.diagnostics.saw_tool_return_ack = true;
                    let tool_return = tool_result.content.clone().unwrap_or_default();
                    let status = tool_result.status.clone();
                    let approval_request_id = tool_result.approval_request_id.clone();
                    let config = this.context.config.clone();
                    let api_state = ApiState {
                        sqlx_pool: this.context.pool.clone(),
                        config: config.clone(),
                        letta: letta.clone(),
                        bifrost: Arc::new(BifrostClient::new(config.as_ref())),
                        acp_tool_turns: this.context.tool_turns.clone(),
                        acp_turn_cancellations: AcpActiveTurnCancelRegistry::new(),
                    };
                    let binding = RoleRuntimeBinding {
                        binding_id: continuation
                            .agent_id
                            .clone()
                            .unwrap_or_else(|| this.context.pair_agent_id.clone()),
                        compatibility_backend: Some("letta".to_string()),
                    };
                    let request_id = this.context.request_id;
                    let acp_session_id = this.context.acp_session_id.clone();
                    let continuation_request =
                        if let Some(approval_request_id) = approval_request_id {
                            RuntimeContinuation::ApprovalDecision {
                                approval_request_id,
                                tool_call_id: Some(tool_call_id.clone()),
                                decision: if status == "ok" {
                                    crate::core::runtime_provider::RuntimeApprovalDecision::Approve
                                } else {
                                    crate::core::runtime_provider::RuntimeApprovalDecision::Deny
                                },
                                reason: Some(tool_return.clone()),
                            }
                        } else {
                            RuntimeContinuation::ToolResult {
                                tool_call_id: tool_call_id.clone(),
                                approval_request_id: None,
                                status: match status.as_str() {
                                    "ok" => RuntimeToolResultStatus::Ok,
                                    "timeout" => RuntimeToolResultStatus::Timeout,
                                    _ => RuntimeToolResultStatus::Error,
                                },
                                content: tool_return.clone(),
                            }
                        };
                    let stream_context = AcpTurnStreamContext {
                        client_tools: continuation.client_tools.clone(),
                        stream_tokens: continuation.stream_tokens,
                        max_steps: continuation.max_steps,
                    };
                    this.persist_future =
                        Some(AcpPendingFuture::ContinueTool(Box::pin(async move {
                            let prepared = continue_acp_turn_with_runtime(AcpTurnContinueRequest {
                                state: &api_state,
                                request_id,
                                acp_session_id: &acp_session_id,
                                binding: &binding,
                                continuation: continuation_request,
                                stream_context,
                            })
                            .await?;
                            let mut diagnostics = AcpStreamDiagnostics::default();
                            diagnostics.saw_requires_approval_stop = false;
                            Ok((
                                prepared.0,
                                prepared.1,
                                std::sync::Arc::new(std::sync::Mutex::new(diagnostics)),
                            ))
                        })));
                    this.diagnostics.saw_requires_approval_stop = false;
                    self.poll_next(cx)
                } else if this.turn_controller.phase() == AcpTurnPhase::WaitingForObligations
                    && this.turn_controller.status_snapshot().open_obligations == 0
                    && this.outstanding_tool_obligations().is_empty()
                    && !this.diagnostics.saw_tool_return_ack
                    && !this.diagnostics.emitted_runtime_cleanup
                    && this.queued_tool_result_continuation.is_none()
                {
                    let letta = this.letta.clone();
                    let tool_turns = this.context.tool_turns.clone();
                    let acp_session_id = this.context.acp_session_id.clone();
                    let bear_id = this.context.bear_id;
                    let pair_agent_id = this.context.pair_agent_id.clone();
                    let run_ids = this.diagnostics.run_ids.clone();
                    let request_id = this.context.request_id;
                    this.persist_future = Some(AcpPendingFuture::Cleanup(Box::pin(async move {
                        super::super::acp_cleanup_stale_runtime_state(
                            AcpStaleRuntimeCleanupParams {
                                letta,
                                tool_turns,
                                acp_session_id,
                                bear_id,
                                pair_agent_id,
                                run_ids,
                                reason: "orphaned_requires_approval_stop",
                                request_id,
                            },
                        )
                        .await
                    })));
                    self.poll_next(cx)
                } else if this.diagnostics.saw_requires_approval_stop
                    && this.outstanding_tool_obligations().is_empty()
                    && !this.diagnostics.emitted_runtime_cleanup
                    && this.queued_tool_result_continuation.is_none()
                {
                    let letta = this.letta.clone();
                    let tool_turns = this.context.tool_turns.clone();
                    let acp_session_id = this.context.acp_session_id.clone();
                    let bear_id = this.context.bear_id;
                    let pair_agent_id = this.context.pair_agent_id.clone();
                    let run_ids = this.diagnostics.run_ids.clone();
                    let request_id = this.context.request_id;
                    this.persist_future = Some(AcpPendingFuture::Cleanup(Box::pin(async move {
                        super::super::acp_cleanup_stale_runtime_state(
                            AcpStaleRuntimeCleanupParams {
                                letta,
                                tool_turns,
                                acp_session_id,
                                bear_id,
                                pair_agent_id,
                                run_ids,
                                reason: "orphaned_requires_approval_stop",
                                request_id,
                            },
                        )
                        .await
                    })));
                    self.poll_next(cx)
                } else if let Some(event) = this.diagnostics.empty_turn_error_event(&this.context) {
                    for event in this.text_chunker.push(event) {
                        this.push_adapter_event(event);
                    }
                    self.poll_next(cx)
                } else if this.diagnostics.saw_error {
                    this.turn_controller.on_stream_error();
                    let role_result = this.context.role_runtime.turn_result(
                        TurnResultStatus::Failed,
                        TurnResultReason::RuntimeCleanup,
                        this.context.request_id,
                        this.context.turn_scope.clone(),
                        false,
                        this.diagnostics.diagnostic_json_with_turn_controller(
                            &this.context,
                            Some(&this.turn_controller),
                        ),
                    );
                    this.push_terminal_result_when_ready(role_result);
                    if !this.pending.is_empty() {
                        return self.poll_next(cx);
                    }
                    this.log_summary_once();
                    Poll::Ready(None)
                } else {
                    for event in this.text_chunker.flush_all() {
                        this.push_adapter_event(event);
                    }
                    if this.turn_controller.phase() != AcpTurnPhase::Terminal {
                        this.turn_controller.on_stream_end();
                        let role_result = this.context.role_runtime.turn_result(
                            TurnResultStatus::Ok,
                            TurnResultReason::StreamComplete,
                            this.context.request_id,
                            this.context.turn_scope.clone(),
                            false,
                            this.diagnostics.diagnostic_json_with_turn_controller(
                                &this.context,
                                Some(&this.turn_controller),
                            ),
                        );
                        this.push_terminal_result_when_ready(role_result);
                    }
                    if !this.pending.is_empty() {
                        return self.poll_next(cx);
                    }
                    this.log_summary_once();
                    Poll::Ready(None)
                }
            }
        }
    }
}
