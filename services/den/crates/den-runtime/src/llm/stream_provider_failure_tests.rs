use super::*;
use crate::runtime::bearwire_projection::wire::{
    runtime_stream_event_to_bearwire_events, runtime_stream_event_to_bearwire_notifications,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use crate::native_runtime::{
    openai_byte_stream_to_event_stream_with_telemetry,
    responses_byte_stream_to_event_stream_with_telemetry, ObservedPromptTokensSink,
};
use bytes::Bytes;
use futures::StreamExt;

const CANARY: &str = "UPSTREAM_PRIVATE_CANARY";

fn assert_safe_error(events: &[RuntimeStreamEvent], expected: ProviderFailureKind) {
    let [RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::Error {
        message,
        detail,
        error_type,
        request_id,
        context,
    })] = events
    else {
        panic!("expected one error: {events:?}");
    };
    assert_eq!(message, &ProviderFailure::new(expected).to_string());
    assert_eq!(detail, &None);
    assert_eq!(error_type.as_deref(), Some(expected.code()));
    assert_eq!(request_id, &None);
    assert_eq!(
        context.as_ref().unwrap()["provider_failure"],
        json!(ProviderFailure::new(expected))
    );

    let encoded = serde_json::to_string(events).unwrap();
    assert!(!encoded.contains(CANARY), "{encoded}");
    for event in events {
        let wire = runtime_stream_event_to_bearwire_events(event.clone());
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].event_type, "run.failed");
        let notifications = runtime_stream_event_to_bearwire_notifications(event.clone());
        for encoded in [
            serde_json::to_string(&wire).unwrap(),
            serde_json::to_string(&notifications).unwrap(),
        ] {
            assert!(!encoded.contains(CANARY), "{encoded}");
        }
    }
}

#[derive(Clone)]
struct CapturedStreamEvents(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for CapturedStreamEvents {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "den.llm.stream"
    }
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(Vec<String>);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push(format!("{field}={value:?}"));
            }
        }
        let mut fields = Fields(Vec::new());
        event.record(&mut fields);
        self.0.lock().unwrap().extend(fields.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn capture_stream_traces<T>(observe: impl FnOnce() -> T) -> (T, String) {
    let fields = Arc::new(Mutex::new(Vec::new()));
    let observed = tracing::subscriber::with_default(CapturedStreamEvents(fields.clone()), observe);
    let traces = fields.lock().unwrap().join("\n");
    assert!(!traces.is_empty(), "stream diagnostics must be observed");
    assert!(!traces.contains(CANARY), "{traces}");
    (observed, traces)
}

fn assert_chat_error_diagnostics_safe(frame: &Value, expected: ProviderFailureKind) {
    let (snapshot, traces) = capture_stream_traces(|| {
        let mut accumulator = OpenAiStreamAccumulator::default();
        let mut diagnostics = OpenAiStreamDiagnostics::new(None);
        let parsed =
            accumulator.ingest_sse_data_line_with_diagnostics(frame, Some(&mut diagnostics));
        assert_safe_error(&parsed.events, expected);
        assert!(accumulator.has_provider_failure());
        assert!(accumulator.should_detach_upstream());
        let before = format!("{accumulator:?}\n{diagnostics:?}");
        for trailing in poisoned_trailing_frames() {
            let parsed = accumulator
                .ingest_sse_data_line_with_diagnostics(&trailing, Some(&mut diagnostics));
            assert!(parsed.events.is_empty());
            assert!(parsed.finish_reason.is_none());
        }
        assert!(accumulator.flush_end_of_stream().is_empty());
        assert_eq!(before, format!("{accumulator:?}\n{diagnostics:?}"));
        assert_eq!(diagnostics.json_frame_count, 1);
        assert_eq!(diagnostics.emitted_runtime_event_count, 1);
        assert_eq!(diagnostics.tool_delta_count, 0);
        assert!(diagnostics.tool_arg_lengths.is_empty());
        // The finish summary must not inherit the error frame's fake choice/tool fields.
        diagnostics.observe_finish_reason("stop", 0);
        format!("{accumulator:?}\n{diagnostics:?}")
    });
    assert!(traces.contains(expected.code()));
    assert!(!snapshot.contains(CANARY), "{snapshot}");
}

fn assert_responses_error_diagnostics_safe(frame: &Value, expected: ProviderFailureKind) {
    let (snapshot, traces) = capture_stream_traces(|| {
        let mut accumulator = ResponsesStreamAccumulator::default();
        let mut diagnostics = ResponsesStreamDiagnostics::new(None);
        let parsed =
            accumulator.ingest_sse_data_line_with_diagnostics(frame, Some(&mut diagnostics));
        assert_safe_error(&parsed.events, expected);
        assert!(accumulator.has_provider_failure());
        assert!(accumulator.should_detach_upstream());
        let before = format!("{accumulator:?}\n{diagnostics:?}");
        for trailing in poisoned_trailing_frames() {
            let parsed = accumulator
                .ingest_sse_data_line_with_diagnostics(&trailing, Some(&mut diagnostics));
            assert!(parsed.events.is_empty());
            assert!(parsed.finish_reason.is_none());
        }
        assert!(accumulator.flush_end_of_stream().is_empty());
        assert_eq!(before, format!("{accumulator:?}\n{diagnostics:?}"));
        assert_eq!(diagnostics.json_frame_count, 1);
        assert_eq!(diagnostics.emitted_runtime_event_count, 1);
        assert_eq!(
            diagnostics.event_type_counts,
            HashMap::from([("error".to_string(), 1)])
        );
        assert_eq!(diagnostics.last_event_type.as_deref(), Some("error"));
        assert_eq!(diagnostics.zero_runtime_event_frame_count, 0);
        assert_eq!(diagnostics.consecutive_zero_runtime_event_frame_count, 0);
        assert!(diagnostics.tool_arg_lengths.is_empty());
        assert!(diagnostics.tool_names.is_empty());
        assert!(diagnostics.tool_call_ids.is_empty());
        diagnostics.observe_incomplete_frame_on_end(1);
        format!("{accumulator:?}\n{diagnostics:?}")
    });
    assert!(traces.contains(expected.code()));
    assert!(!snapshot.contains(CANARY), "{snapshot}");
}

fn poisoned_trailing_frames() -> Vec<Value> {
    vec![
        json!({"type": CANARY, "item": {"type": "function_call", "id": CANARY, "name": CANARY, "arguments": CANARY}}),
        json!({
            "type": "response.output_item.done",
            "item": {"type": "function_call", "id": CANARY, "call_id": CANARY, "name": CANARY, "arguments": "{}"},
            "choices": [{"finish_reason": "tool_calls", "delta": {"tool_calls": [{
                "index": 0, "id": CANARY, "function": {"name": CANARY, "arguments": "{}"},
            }]}}],
        }),
        json!({
            "type": "response.completed", "response": {"usage": {"input_tokens": 777}},
            "choices": [{"finish_reason": "stop"}], "usage": {"prompt_tokens": 777},
        }),
        json!({"type": "error", "message": CANARY}),
    ]
}

fn provider_failure_with_buffered_trailing_frames() -> String {
    let error = json!({"type": "error", "message": CANARY});
    let mut body = format!("data: {error}\n\n");
    for frame in poisoned_trailing_frames() {
        body.push_str(&format!("data: {frame}\n\n"));
    }
    // Neither a parse error nor an incomplete-frame diagnostic may supersede the failure.
    body.push_str("data: [DONE]\n\ndata: not-json\n\ndata: incomplete");
    body
}

#[test]
fn framed_parsers_stop_at_failure_and_ignore_subsequent_frames_before_diagnostics() {
    let body = provider_failure_with_buffered_trailing_frames();
    let ((), _) = capture_stream_traces(|| {
        let mut chat = OpenAiStreamAccumulator::default();
        let mut chat_diagnostics = OpenAiStreamDiagnostics::new(None);
        let events = openai_sse_frame_to_runtime_events_with_diagnostics(
            &mut chat,
            body.as_bytes(),
            Some(&mut chat_diagnostics),
        )
        .unwrap();
        assert_safe_error(&events, ProviderFailureKind::Unknown);
        let before = format!("{chat:?}\n{chat_diagnostics:?}");
        for ignored in [b"data: [DONE]".as_slice(), &[0xff]] {
            assert!(openai_sse_frame_to_runtime_events_with_diagnostics(
                &mut chat,
                ignored,
                Some(&mut chat_diagnostics),
            )
            .unwrap()
            .is_empty());
        }
        assert_eq!(before, format!("{chat:?}\n{chat_diagnostics:?}"));
        assert_eq!(chat_diagnostics.sse_frame_count, 1);
        assert_eq!(chat_diagnostics.json_frame_count, 1);
        assert!(chat.take_observed_prompt_tokens().is_none());

        let mut responses = ResponsesStreamAccumulator::default();
        let mut diagnostics = ResponsesStreamDiagnostics::new(None);
        let events = responses_sse_frame_to_runtime_events_with_diagnostics(
            &mut responses,
            body.as_bytes(),
            Some(&mut diagnostics),
        )
        .unwrap();
        assert_safe_error(&events, ProviderFailureKind::Unknown);
        let before = format!("{responses:?}\n{diagnostics:?}");
        assert!(responses_sse_frame_to_runtime_events_with_diagnostics(
            &mut responses,
            &[0xff],
            Some(&mut diagnostics),
        )
        .unwrap()
        .is_empty());
        assert_eq!(before, format!("{responses:?}\n{diagnostics:?}"));
        assert_eq!(diagnostics.sse_frame_count, 1);
        assert_eq!(diagnostics.json_frame_count, 1);
        assert!(responses.take_observed_prompt_tokens().is_none());
        diagnostics.observe_incomplete_frame_on_end(1);
        assert!(!before.contains(CANARY), "{before}");
    });
    assert_safe_error(
        &openai_sse_chunk_to_runtime_events(body.as_bytes()).unwrap(),
        ProviderFailureKind::Unknown,
    );
}

#[test]
fn pending_chat_tools_then_provider_error_then_done_never_flushes_tools_or_completion() {
    capture_stream_traces(|| {
        let mut accumulator = OpenAiStreamAccumulator::default();
        let mut diagnostics = OpenAiStreamDiagnostics::new(None);
        let pending = json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0, "id": "call-safe", "function": {"name": "session_info", "arguments": "{}"},
        }]}}]});
        assert!(accumulator
            .ingest_sse_data_line_with_diagnostics(&pending, Some(&mut diagnostics))
            .events
            .is_empty());
        let error = json!({"error": {"message": CANARY}});
        let body = format!("data: {error}\n\ndata: [DONE]\n\n");
        let events = openai_sse_frame_to_runtime_events_with_diagnostics(
            &mut accumulator,
            body.as_bytes(),
            Some(&mut diagnostics),
        )
        .unwrap();
        assert_safe_error(&events, ProviderFailureKind::Unknown);
        assert!(accumulator.has_provider_failure());
        assert!(accumulator.should_detach_upstream());
        assert!(accumulator.flush_end_of_stream().is_empty());
        assert!(openai_sse_frame_to_runtime_events_with_diagnostics(
            &mut accumulator,
            b"data: [DONE]",
            Some(&mut diagnostics),
        )
        .unwrap()
        .is_empty());
        assert_eq!(diagnostics.json_frame_count, 2);
        assert_eq!(diagnostics.emitted_runtime_event_count, 1);
        diagnostics.observe_finish_reason("stop", 0);
    });
}

#[test]
fn native_same_chunk_failure_discards_trailing_frames_and_never_polls_upstream_again() {
    for responses in [false, true] {
        let ((), traces) = capture_stream_traces(|| {
            let chunks_read = Arc::new(AtomicU64::new(0));
            let observed_reads = chunks_read.clone();
            let source = futures::stream::iter(vec![
                Ok::<Bytes, den_core::DenError>(Bytes::from(
                    provider_failure_with_buffered_trailing_frames(),
                )),
                Ok(Bytes::from_static(b"data: not-json\n\n")),
            ])
            .inspect(move |_| {
                observed_reads.fetch_add(1, Ordering::SeqCst);
            });
            let observed_usage = Arc::new(AtomicU64::new(0));
            let usage = observed_usage.clone();
            let sink: ObservedPromptTokensSink = Arc::new(move |tokens| {
                usage.store(tokens, Ordering::SeqCst);
            });
            let stream = if responses {
                responses_byte_stream_to_event_stream_with_telemetry(source, None, Some(sink))
            } else {
                openai_byte_stream_to_event_stream_with_telemetry(source, None, Some(sink))
            };
            let events = futures::executor::block_on(stream.collect::<Vec<_>>())
                .into_iter()
                .map(|event| event.expect("only the safe error is emitted"))
                .collect::<Vec<_>>();
            assert!(matches!(
                events.as_slice(),
                [
                    RuntimeStreamEvent::ProviderActivity,
                    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::Error { .. })
                ]
            ));
            assert_safe_error(&events[1..], ProviderFailureKind::Unknown);
            assert_eq!(chunks_read.load(Ordering::SeqCst), 1);
            assert_eq!(observed_usage.load(Ordering::SeqCst), 0);
        });
        assert!(!traces.contains("sse_frame_count=2"), "{traces}");
        assert!(!traces.contains("json_frame_count=2"), "{traces}");
    }
}

#[test]
fn native_successful_finishes_still_drain_same_chunk_usage_and_tools() {
    for responses in [false, true] {
        let frames = if responses {
            concat!(
                "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"item-1\",\"call_id\":\"call-1\",\"name\":\"session_info\",\"arguments\":\"{}\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":123}}}\n\n",
            )
        } else {
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":123}}\n\n",
            )
        };
        let usage = Arc::new(AtomicU64::new(0));
        let observed_usage = usage.clone();
        let sink: ObservedPromptTokensSink = Arc::new(move |tokens| {
            observed_usage.store(tokens, Ordering::SeqCst);
        });
        let source = futures::stream::iter(vec![Ok::<Bytes, den_core::DenError>(
            Bytes::from_static(frames.as_bytes()),
        )]);
        let stream = if responses {
            responses_byte_stream_to_event_stream_with_telemetry(source, None, Some(sink))
        } else {
            openai_byte_stream_to_event_stream_with_telemetry(source, None, Some(sink))
        };
        let events = futures::executor::block_on(stream.collect::<Vec<_>>())
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(usage.load(Ordering::SeqCst), 123);
        if responses {
            assert!(matches!(
                events.as_slice(),
                [
                    RuntimeStreamEvent::ProviderActivity,
                    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested { .. })
                ]
            ));
        } else {
            assert!(matches!(
                events.as_slice(),
                [
                    RuntimeStreamEvent::ProviderActivity,
                    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::AssistantTextDelta { .. }),
                    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted { .. })
                ]
            ));
        }
    }
}

#[test]
fn flat_declared_errors_are_selected_before_diagnostics_for_both_apis() {
    for (code, expected) in [
        (
            "context_length_exceeded",
            ProviderFailureKind::ContextOverflow,
        ),
        (CANARY, ProviderFailureKind::Unknown),
    ] {
        let frame = json!({
            "type": "error", "code": code, "message": CANARY, "param": CANARY,
            "request": {"code": "rate_limit_error", "message": CANARY},
            "choices": [{"finish_reason": CANARY, "delta": {"tool_calls": [{
                "index": CANARY, "id": CANARY,
                "function": {"name": CANARY, "arguments": CANARY},
            }]}}],
            "item": {"type": "function_call", "id": CANARY, "call_id": CANARY, "name": CANARY, "arguments": CANARY},
        });
        assert_chat_error_diagnostics_safe(&frame, expected);
        assert_responses_error_diagnostics_safe(&frame, expected);
    }
}

#[test]
fn nested_errors_cannot_seed_diagnostic_labels_or_tool_summaries() {
    for outer_type in [CANARY, "response.failed", "response.incomplete"] {
        let mut frame = json!({
            "type": outer_type, "code": "rate_limit_error",
            "error": {"code": "context_length_exceeded", "message": CANARY, "type": CANARY},
            "choices": [{"finish_reason": CANARY, "delta": {"tool_calls": [{
                "index": CANARY, "id": CANARY,
                "function": {"name": CANARY, "arguments": CANARY},
            }]}}],
            "item": {"type": "function_call", "id": CANARY, "call_id": CANARY, "name": CANARY, "arguments": CANARY},
        });
        assert_chat_error_diagnostics_safe(&frame, ProviderFailureKind::ContextOverflow);
        assert_responses_error_diagnostics_safe(&frame, ProviderFailureKind::ContextOverflow);
        frame["response"] = json!({"error": frame["error"]});
        frame["error"] = Value::Null;
        assert_responses_error_diagnostics_safe(&frame, ProviderFailureKind::ContextOverflow);
    }
}

#[test]
fn terminal_responses_without_error_objects_also_bypass_raw_diagnostics() {
    for kind in ["response.failed", "response.incomplete"] {
        let (snapshot, _) = capture_stream_traces(|| {
            let mut accumulator = ResponsesStreamAccumulator::default();
            let mut diagnostics = ResponsesStreamDiagnostics::new(None);
            let parsed = accumulator.ingest_sse_data_line_with_diagnostics(&json!({
                "type": kind, "response": {"error": null},
                "item": {"type": "function_call", "id": CANARY, "name": CANARY, "arguments": CANARY},
            }), Some(&mut diagnostics));
            assert!(matches!(
                parsed.events.as_slice(),
                [RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::TurnFailed { .. }
                )]
            ));
            assert_eq!(diagnostics.json_frame_count, 1);
            assert_eq!(diagnostics.emitted_runtime_event_count, 1);
            assert_eq!(diagnostics.last_event_type.as_deref(), Some("error"));
            diagnostics.observe_incomplete_frame_on_end(1);
            format!("{parsed:?}\n{accumulator:?}\n{diagnostics:?}")
        });
        assert!(!snapshot.contains(CANARY), "{snapshot}");
    }
}

#[test]
fn successful_frames_keep_existing_diagnostics() {
    let mut chat = OpenAiStreamAccumulator::default();
    let mut chat_diagnostics = OpenAiStreamDiagnostics::new(None);
    chat.ingest_sse_data_line_with_diagnostics(
        &json!({
            "choices": [{"delta": {"content": "hello"}, "finish_reason": "stop"}],
        }),
        Some(&mut chat_diagnostics),
    );
    assert_eq!(chat_diagnostics.json_frame_count, 1);
    assert_eq!(chat_diagnostics.emitted_runtime_event_count, 2);

    let mut responses = ResponsesStreamAccumulator::default();
    let mut diagnostics = ResponsesStreamDiagnostics::new(None);
    responses.ingest_sse_data_line_with_diagnostics(&json!({
        "type": "response.output_item.done",
        "item": {"type": "function_call", "id": "item-1", "call_id": "call-1", "name": "session_info", "arguments": "{}"},
    }), Some(&mut diagnostics));
    assert_eq!(diagnostics.json_frame_count, 1);
    assert_eq!(diagnostics.emitted_runtime_event_count, 1);
    assert_eq!(
        diagnostics.last_event_type.as_deref(),
        Some("response.output_item.done")
    );
    assert_eq!(
        diagnostics
            .event_type_counts
            .get("response.output_item.done"),
        Some(&1)
    );
    assert_eq!(
        diagnostics.tool_names.get("item-1").map(String::as_str),
        Some("session_info")
    );
    assert_eq!(
        diagnostics.tool_call_ids.get("item-1").map(String::as_str),
        Some("call-1")
    );
    assert_eq!(diagnostics.tool_arg_lengths.get("item-1"), Some(&2));
}

#[test]
fn selected_sse_error_fields_are_classified_but_never_forwarded() {
    for (field, value, expected) in [
        (
            "code",
            "context_length_exceeded",
            ProviderFailureKind::ContextOverflow,
        ),
        ("type", "rate_limit_error", ProviderFailureKind::RateLimited),
        ("message", "upstream timeout", ProviderFailureKind::Timeout),
        ("code", CANARY, ProviderFailureKind::Unknown),
        ("type", CANARY, ProviderFailureKind::Unknown),
        ("message", CANARY, ProviderFailureKind::Unknown),
    ] {
        let mut error = json!({
            "code": CANARY,
            "type": CANARY,
            "message": CANARY,
            "detail": CANARY,
            "request_id": CANARY,
            "request": { "code": "context_length_exceeded", "messages": [CANARY] },
            "extra": { "error": { "code": "rate_limit_error" }, "secret": CANARY }
        });
        error[field] = json!(value);
        let frames = [
            json!({"error": error, "request": {"code": "rate_limit_error"}}),
            json!({"type": "response.created", "error": error}),
            json!({"type": "response.failed", "response": {"error": error}}),
            json!({"type": "response.incomplete", "error": null, "response": {"error": error}}),
        ];
        for (index, frame) in frames.into_iter().enumerate() {
            let body = format!("data: {frame}\n\n");
            let events = if index == 0 {
                openai_sse_frame_to_runtime_events(
                    &mut OpenAiStreamAccumulator::default(),
                    body.as_bytes(),
                )
                .unwrap()
            } else {
                let mut accumulator = ResponsesStreamAccumulator::default();
                let events =
                    responses_sse_frame_to_runtime_events(&mut accumulator, body.as_bytes())
                        .unwrap();
                assert!(accumulator.should_detach_upstream());
                events
            };
            assert_safe_error(&events, expected);
        }
    }
}

#[test]
fn top_level_error_precedes_nested_error_without_merging_classification() {
    let frame = json!({
        "type": "response.failed",
        "error": {"message": CANARY},
        "response": {"error": {"code": "context_length_exceeded", "message": CANARY}},
    });
    let events = ResponsesStreamAccumulator::default()
        .ingest_sse_data_line(&frame)
        .events;
    assert_safe_error(&events, ProviderFailureKind::Unknown);
}

#[test]
fn failed_and_incomplete_without_selected_error_keep_safe_terminal_kind() {
    for kind in ["response.failed", "response.incomplete"] {
        let mut accumulator = ResponsesStreamAccumulator::default();
        let parsed = accumulator.ingest_sse_data_line(&json!({
            "type": kind,
            "message": CANARY,
            "response": {
                "error": null,
                "incomplete_details": {"reason": CANARY},
                "request": {"code": "context_length_exceeded", "messages": [CANARY]},
            },
        }));
        assert!(accumulator.should_detach_upstream());
        assert!(
            matches!(parsed.events.as_slice(), [RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::TurnFailed { category: RuntimeErrorCategory::BackendProtocol, message, .. }
        )] if message == &ProviderFailure::new(ProviderFailureKind::Unknown).to_string())
        );
        let wire = runtime_stream_event_to_bearwire_events(parsed.events[0].clone());
        assert_eq!(wire[0].data["reason"], "runtime_backend_protocol");
        assert!(!serde_json::to_string(&wire).unwrap().contains(CANARY));
    }
}

#[test]
fn nested_request_codes_never_control_success_streams() {
    let request = json!({"code": "context_length_exceeded", "error": {"type": "rate_limit_error"}});
    let mut chat = OpenAiStreamAccumulator::default();
    let parsed = chat.ingest_sse_data_line(&json!({
        "error": null,
        "request": request,
        "choices": [{"delta": {"content": CANARY}, "finish_reason": null}],
    }));
    assert!(
        matches!(parsed.events.as_slice(), [RuntimeStreamEvent::Semantic(
        RuntimeSemanticEvent::AssistantTextDelta { text }
    )] if text == CANARY)
    );
    chat.ingest_sse_data_line(
        &json!({"request": request, "usage": {"prompt_tokens": 123}, "choices": []}),
    );
    assert_eq!(chat.take_observed_prompt_tokens(), Some(123));
    let completed = chat.ingest_sse_data_line(
        &json!({"request": request, "choices": [{"delta": {}, "finish_reason": "stop"}]}),
    );
    assert!(matches!(
        completed.events.as_slice(),
        [RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::TurnCompleted { .. }
        )]
    ));

    let mut responses = ResponsesStreamAccumulator::default();
    let control = responses.ingest_sse_data_line(
        &json!({"type": "response.created", "request": request, "response": {"error": null}}),
    );
    assert!(control.events.is_empty());
    assert!(!responses.should_detach_upstream());
    let text = responses.ingest_sse_data_line(
        &json!({"type": "response.output_text.delta", "delta": CANARY, "request": request}),
    );
    assert!(
        matches!(text.events.as_slice(), [RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::AssistantTextDelta { text })] if text == CANARY)
    );
    let tool = responses.ingest_sse_data_line(&json!({
        "type": "response.output_item.done",
        "request": request,
        "item": {"type": "function_call", "id": "item-1", "call_id": "call-1", "name": "session_info", "arguments": "{}"},
    }));
    assert!(
        matches!(tool.events.as_slice(), [RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested { tool_name, .. })] if tool_name == "session_info")
    );
    responses.ingest_sse_data_line(&json!({"type": "response.completed", "request": request, "response": {"usage": {"input_tokens": 456}}}));
    assert_eq!(responses.take_observed_prompt_tokens(), Some(456));
    assert!(responses.should_detach_upstream());
}
