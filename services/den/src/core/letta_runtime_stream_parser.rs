use crate::{core::runtime_contracts::RuntimeStreamEvent, errors::CustomError};

pub fn find_sse_frame_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

pub fn strip_trailing_sse_delimiter_owned(mut frame: Vec<u8>) -> Vec<u8> {
    if frame.ends_with(b"\r\n\r\n") {
        frame.truncate(frame.len().saturating_sub(4));
    } else if frame.ends_with(b"\n\n") {
        frame.truncate(frame.len().saturating_sub(2));
    }
    frame
}

pub fn parse_sse_event_body_to_json(body: &[u8]) -> Result<Option<serde_json::Value>, CustomError> {
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

pub fn runtime_stream_event_from_letta_json(event: &serde_json::Value) -> Option<RuntimeStreamEvent> {
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
            Some(RuntimeStreamEvent::AssistantTextDelta { text })
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
            Some(RuntimeStreamEvent::RunProgress {
                kind: "status_text".to_string(),
                text: Some(text),
                phase: None,
                detail: None,
            })
        }
        "error_message" => Some(RuntimeStreamEvent::Error {
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
                Some(RuntimeStreamEvent::TurnCompleted { turn: None })
            } else if stop_reason == "requires_approval" {
                Some(RuntimeStreamEvent::RunPaused {
                    reason: "awaiting_approval".to_string(),
                    resume_token: None,
                    expires_at: None,
                })
            } else {
                Some(RuntimeStreamEvent::TurnFailed {
                    turn: None,
                    category: crate::core::runtime_contracts::RuntimeErrorCategory::BackendProtocol,
                    message: format!(
                        "Letta stopped before producing assistant output: {stop_reason}"
                    ),
                })
            }
        }
        "tool_call_message" | "approval_request_message" | "function_call" => Some(
            RuntimeStreamEvent::ToolCallRequested {
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
                } => RuntimeStreamEvent::ConversationResolved {
                    conversation: crate::core::runtime_contracts::RuntimeConversationRef {
                        id: conversation_id,
                    },
                },
                _ => unreachable!(),
            },
        ),
    }
}
