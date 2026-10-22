use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use den_llm::LlmRequestTelemetry;
use den_protocol::{RuntimeEventStream, RuntimeSemanticEvent, RuntimeStreamEvent};
use futures::Stream;

use crate::{
    llm::{
        openai_sse_frame_to_runtime_events_with_diagnostics,
        responses_sse_frame_to_runtime_events_with_diagnostics, OpenAiStreamAccumulator,
        OpenAiStreamDiagnostics, ResponsesStreamAccumulator, ResponsesStreamDiagnostics,
    },
    runtime_stream_parser::{find_sse_frame_end, strip_trailing_sse_delimiter_owned},
};
use den_core::DenError;

/// Best-effort receiver for provider-reported prompt-token usage observed on a
/// stream (ADR-0047 §7 calibration input). Invoked at most once per stream;
/// implementations must not block.
pub type ObservedPromptTokensSink = Arc<dyn Fn(u64) + Send + Sync>;

fn is_terminal_or_pause(event: &RuntimeStreamEvent) -> bool {
    matches!(
        event,
        RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::RunPaused { .. }
                | RuntimeSemanticEvent::ToolCallRequested { .. }
                | RuntimeSemanticEvent::TurnCompleted { .. }
                | RuntimeSemanticEvent::TurnFailed { .. }
                | RuntimeSemanticEvent::TurnCancelled { .. }
                | RuntimeSemanticEvent::Error { .. }
        )
    )
}

pub fn responses_byte_stream_to_event_stream(
    parsed: impl Stream<Item = Result<bytes::Bytes, DenError>> + Send + Unpin + 'static,
) -> RuntimeEventStream {
    responses_byte_stream_to_event_stream_with_telemetry(parsed, None, None)
}

pub fn responses_byte_stream_to_event_stream_with_telemetry(
    parsed: impl Stream<Item = Result<bytes::Bytes, DenError>> + Send + Unpin + 'static,
    telemetry: Option<LlmRequestTelemetry>,
    usage_sink: Option<ObservedPromptTokensSink>,
) -> RuntimeEventStream {
    let mut buffer = Vec::new();
    let mut queued_events: VecDeque<Result<RuntimeStreamEvent, DenError>> = VecDeque::new();
    let mut finished = false;
    let mut saw_terminal_or_pause = false;
    let mut accumulator = ResponsesStreamAccumulator::default();
    let mut diagnostics = ResponsesStreamDiagnostics::new(telemetry);
    let mut pinned = Box::pin(parsed);
    let stream = futures::stream::poll_fn(move |cx| loop {
        if let Some(item) = queued_events.pop_front() {
            return Poll::Ready(Some(item));
        }
        if finished {
            return Poll::Ready(None);
        }
        match Pin::new(&mut pinned).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                queued_events.push_back(Ok(RuntimeStreamEvent::ProviderActivity));
                buffer.extend_from_slice(&bytes);
                while let Some(end) = find_sse_frame_end(&buffer) {
                    let raw: Vec<u8> = buffer.drain(..end).collect();
                    let frame_body = strip_trailing_sse_delimiter_owned(raw);
                    match responses_sse_frame_to_runtime_events_with_diagnostics(
                        &mut accumulator,
                        &frame_body,
                        Some(&mut diagnostics),
                    ) {
                        Ok(events) => {
                            for event in events {
                                if is_terminal_or_pause(&event) {
                                    saw_terminal_or_pause = true;
                                }
                                queued_events.push_back(Ok(event));
                            }
                        }
                        Err(err) => queued_events.push_back(Err(err)),
                    }
                    if accumulator.has_provider_failure() {
                        buffer.clear();
                        finished = true;
                        break;
                    }
                }
                if let (Some(prompt_tokens), Some(sink)) = (
                    accumulator.take_observed_prompt_tokens(),
                    usage_sink.as_ref(),
                ) {
                    sink(prompt_tokens);
                }
                if accumulator.should_detach_upstream() {
                    finished = true;
                }
            }
            Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err))),
            Poll::Ready(None) => {
                finished = true;
                if buffer.is_empty() && !saw_terminal_or_pause {
                    for event in accumulator.flush_end_of_stream() {
                        diagnostics.observe_emitted_events(std::slice::from_ref(&event));
                        queued_events.push_back(Ok(event));
                    }
                } else if !buffer.is_empty() {
                    diagnostics.observe_incomplete_frame_on_end(buffer.len());
                    queued_events.push_back(Err(DenError::System(format!(
                        "Responses SSE stream ended with incomplete frame ({} bytes)",
                        buffer.len()
                    ))));
                }
            }
            Poll::Pending => return Poll::Pending,
        }
    });
    Box::pin(stream)
}

pub fn openai_byte_stream_to_event_stream(
    parsed: impl Stream<Item = Result<bytes::Bytes, DenError>> + Send + Unpin + 'static,
) -> RuntimeEventStream {
    openai_byte_stream_to_event_stream_with_telemetry(parsed, None, None)
}

pub fn openai_byte_stream_to_event_stream_with_telemetry(
    parsed: impl Stream<Item = Result<bytes::Bytes, DenError>> + Send + Unpin + 'static,
    telemetry: Option<LlmRequestTelemetry>,
    usage_sink: Option<ObservedPromptTokensSink>,
) -> RuntimeEventStream {
    let mut buffer = Vec::new();
    let mut queued_events: VecDeque<Result<RuntimeStreamEvent, DenError>> = VecDeque::new();
    let mut finished = false;
    let mut saw_terminal_or_pause = false;
    let mut accumulator = OpenAiStreamAccumulator::default();
    let mut diagnostics = OpenAiStreamDiagnostics::new(telemetry);
    let mut pinned = Box::pin(parsed);
    let stream = futures::stream::poll_fn(move |cx| {
        loop {
            if let Some(item) = queued_events.pop_front() {
                return Poll::Ready(Some(item));
            }
            if finished {
                return Poll::Ready(None);
            }
            match Pin::new(&mut pinned).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    queued_events.push_back(Ok(RuntimeStreamEvent::ProviderActivity));
                    buffer.extend_from_slice(&bytes);
                    while let Some(end) = find_sse_frame_end(&buffer) {
                        let raw: Vec<u8> = buffer.drain(..end).collect();
                        let frame_body = strip_trailing_sse_delimiter_owned(raw);
                        match openai_sse_frame_to_runtime_events_with_diagnostics(
                            &mut accumulator,
                            &frame_body,
                            Some(&mut diagnostics),
                        ) {
                            Ok(events) => {
                                for event in events {
                                    // Tool-call finishes are not turn terminals: the client adapter layer must
                                    // park for tool results and continue. A synthetic TurnCompleted
                                    // here preempts that continuation (same class of provider bug
                                    // requires_approval pauses in client_turn_runner_stream_tests).
                                    if is_terminal_or_pause(&event) {
                                        saw_terminal_or_pause = true;
                                    }
                                    queued_events.push_back(Ok(event));
                                }
                            }
                            Err(err) => queued_events.push_back(Err(err)),
                        }
                        if accumulator.has_provider_failure() {
                            buffer.clear();
                            finished = true;
                            break;
                        }
                    }
                    if let (Some(prompt_tokens), Some(sink)) = (
                        accumulator.take_observed_prompt_tokens(),
                        usage_sink.as_ref(),
                    ) {
                        sink(prompt_tokens);
                    }
                    if accumulator.should_detach_upstream() {
                        finished = true;
                    }
                }
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err))),
                Poll::Ready(None) => {
                    finished = true;
                    if buffer.is_empty() && !saw_terminal_or_pause {
                        for event in accumulator.flush_end_of_stream() {
                            if is_terminal_or_pause(&event) {
                                saw_terminal_or_pause = true;
                            }
                            queued_events.push_back(Ok(event));
                        }
                        if !saw_terminal_or_pause {
                            tracing::warn!(
                                "OpenAI SSE stream ended with no frames; synthesizing TurnCompleted"
                            );
                            queued_events.push_back(Ok(RuntimeStreamEvent::Semantic(
                                RuntimeSemanticEvent::TurnCompleted { turn: None },
                            )));
                        }
                    } else if !buffer.is_empty() {
                        queued_events.push_back(Err(DenError::System(format!(
                            "OpenAI SSE stream ended with incomplete frame ({} bytes)",
                            buffer.len()
                        ))));
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    });
    Box::pin(stream)
}
