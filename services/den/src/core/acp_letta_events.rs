use std::collections::BTreeMap;

use bytes::Bytes;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::{
    acp_tool_turns::AcpToolResultRequest,
    acp_tools::{AcpToolName, ACP_READ_TEXT_FILE_TOOL},
};

#[derive(Debug)]
pub enum AcpGatewayEvent {
    AssistantTextDelta {
        text: String,
    },
    StatusText {
        text: String,
    },
    TurnComplete {
        outcome: String,
    },
    Error {
        message: String,
        detail: Option<String>,
        error_type: Option<String>,
        request_id: Option<String>,
        context: Option<serde_json::Value>,
    },
    ToolRequest {
        request_id: String,
        turn_id: String,
        tool_call_id: String,
        approval_request_id: Option<String>,
        tool_name: String,
        title: String,
        kind: String,
        args: serde_json::Value,
        approval_required: bool,
        approval_reason: Option<String>,
        result_tx: Option<oneshot::Sender<AcpToolResultRequest>>,
        result_rx: Option<oneshot::Receiver<AcpToolResultRequest>>,
    },
    ConversationResolved {
        conversation_id: String,
    },
}

pub fn letta_inner(msg: &serde_json::Value) -> &serde_json::Value {
    match msg.get("contents") {
        Some(c) if c.get("message_type").is_some() => c,
        _ => msg,
    }
}

pub fn letta_stream_text_preserving_whitespace(inner: &serde_json::Value) -> Option<String> {
    let content = inner.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = content.as_object() {
        if let Some(t) = obj.get("text").and_then(|x| x.as_str()) {
            return Some(t.to_string());
        }
    }
    let parts = content.as_array()?;
    let mut out = String::new();
    let mut found_text = false;
    for part in parts {
        if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
            found_text = true;
            out.push_str(t);
        }
    }
    found_text.then_some(out)
}

pub fn map_native_letta_stream_event_to_acp_event(
    event: &serde_json::Value,
) -> Option<AcpGatewayEvent> {
    let inner = letta_inner(event);
    let message_type = inner
        .get("message_type")
        .and_then(|v| v.as_str())
        .or_else(|| event.get("message_type").and_then(|v| v.as_str()))
        .unwrap_or("");
    match message_type {
        "assistant_message" => Some(AcpGatewayEvent::AssistantTextDelta {
            text: letta_stream_text_preserving_whitespace(inner)
                .or_else(|| letta_stream_text_preserving_whitespace(event))
                .unwrap_or_default(),
        }),
        "reasoning_message" => Some(AcpGatewayEvent::StatusText {
            text: inner
                .get("reasoning")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| {
                    event
                        .get("reasoning")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .or_else(|| letta_stream_text_preserving_whitespace(inner))
                .or_else(|| letta_stream_text_preserving_whitespace(event))
                .unwrap_or_default(),
        }),
        "error_message" => Some(AcpGatewayEvent::Error {
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
                Some(AcpGatewayEvent::TurnComplete {
                    outcome: "ok".to_string(),
                })
            } else if stop_reason == "requires_approval" {
                None
            } else {
                Some(AcpGatewayEvent::Error {
                    message: format!(
                        "Letta stopped before producing assistant output: {stop_reason}"
                    ),
                    detail: None,
                    error_type: Some(stop_reason.to_string()),
                    request_id: None,
                    context: None,
                })
            }
        }
        "tool_call_message" | "approval_request_message" | "function_call" => {
            native_letta_tool_request_event(
                event,
                inner,
                message_type == "approval_request_message",
            )
        }
        _ => native_letta_conversation_resolved_event(event),
    }
}

fn native_letta_tool_request_event(
    event: &serde_json::Value,
    inner: &serde_json::Value,
    approval_required: bool,
) -> Option<AcpGatewayEvent> {
    native_letta_tool_request_event_with_args(event, inner, approval_required, None, None)
}

fn native_letta_tool_request_event_with_args(
    event: &serde_json::Value,
    inner: &serde_json::Value,
    approval_required: bool,
    args_override: Option<serde_json::Value>,
    tool_name_override: Option<&str>,
) -> Option<AcpGatewayEvent> {
    let tool_call = tool_call_value(inner, event);
    let tool_name = tool_name_override.or_else(|| tool_call_name(tool_call, inner, event))?;
    let Some(tool) = AcpToolName::from_provider_alias(tool_name) else {
        return Some(AcpGatewayEvent::Error {
            message: format!("Letta requested unsupported ACP local tool: {tool_name}"),
            detail: Some(format!(
                "Only {} is wired for the current ACP local tool slice.",
                ACP_READ_TEXT_FILE_TOOL.provider_name
            )),
            error_type: Some("unsupported_tool".to_string()),
            request_id: None,
            context: Some(serde_json::json!({ "tool_name": tool_name })),
        });
    };
    let descriptor = tool.descriptor();
    let args = if let Some(args) = args_override {
        args
    } else {
        match tool_call_args_raw(tool_call, inner, event) {
            Some(v) if !v.is_null() => {
                if let Some(s) = v.as_str() {
                    let parsed = serde_json::from_str::<serde_json::Value>(s).ok()?;
                    if parsed.is_null() {
                        return None;
                    }
                    parsed
                } else {
                    v.clone()
                }
            }
            _ => return None,
        }
    };
    if tool == AcpToolName::ReadTextFile && args.get("path").and_then(|v| v.as_str()).is_none() {
        if !args.is_object() || args.as_object().is_some_and(|m| m.is_empty()) {
            return None;
        }
        return Some(AcpGatewayEvent::Error {
            message: "Letta requested fs_read_text_file without a path argument.".to_string(),
            detail: Some(format!(
                "Parsed arguments did not contain string field `path`; args={}",
                preview_str_truncated(&args.to_string(), 240)
            )),
            error_type: Some("invalid_tool_arguments".to_string()),
            request_id: None,
            context: Some(serde_json::json!({
                "tool_name": tool_name,
                "tool_call_id": tool_call
                    .and_then(|v| v.get("tool_call_id"))
                    .or_else(|| tool_call.and_then(|v| v.get("id")))
                    .and_then(|v| v.as_str()),
                "args": args,
            })),
        });
    }
    let tool_call_id =
        tool_call_id(tool_call, inner, event).unwrap_or_else(|| format!("call-{}", Uuid::new_v4()));
    let approval_request_id = (approval_required).then(|| {
        event
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("approval-{}", Uuid::new_v4()))
    });
    let request_id = event
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let turn_id = event
        .get("turn_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let (result_tx, result_rx) = oneshot::channel();
    Some(AcpGatewayEvent::ToolRequest {
        request_id,
        turn_id,
        tool_call_id,
        approval_request_id,
        tool_name: descriptor.provider_name.to_string(),
        title: descriptor.title.to_string(),
        kind: descriptor.kind.to_string(),
        args,
        approval_required,
        approval_reason: approval_required
            .then(|| "Letta requested approval before running this local ACP tool.".to_string()),
        result_tx: Some(result_tx),
        result_rx: Some(result_rx),
    })
}

/// Defensive compatibility layer for Letta tool-call streaming.
///
/// The preferred ACP path uses the conversation-scoped Letta messages endpoint with
/// `streaming=true` and `stream_tokens=false`, which should normally yield coherent
/// step-level tool events. Older/deployed Letta builds and some provider paths may
/// still surface tool calls as repeated delta-like `approval_request_message` events:
/// the tool name can appear in one event, arguments can arrive later as string
/// fragments, and duplicate events for the same `tool_call_id` may be emitted.
///
/// Keep this accumulator even if it looks vestigial in the clean/native case. It is a
/// low-cost guardrail that reconstructs partial tool-call deltas into exactly one
/// `AcpGatewayEvent::ToolRequest` and prevents early/duplicate local tool execution.
#[derive(Debug, Default)]
pub struct LettaToolCallAccumulator {
    names: BTreeMap<String, String>,
    argument_buffers: BTreeMap<String, String>,
    emitted: BTreeMap<String, usize>,
}

impl LettaToolCallAccumulator {
    pub fn pending_argument_buffers(&self) -> usize {
        self.argument_buffers.len()
    }

    pub fn pending_name_buffers(&self) -> usize {
        self.names.len()
    }

    pub fn observe(&mut self, event: &serde_json::Value) -> Option<AcpGatewayEvent> {
        let inner = letta_inner(event);
        let message_type = inner
            .get("message_type")
            .and_then(|v| v.as_str())
            .or_else(|| event.get("message_type").and_then(|v| v.as_str()))
            .unwrap_or("");
        if !matches!(
            message_type,
            "tool_call_message" | "approval_request_message" | "function_call"
        ) {
            return None;
        }
        let tool_call = tool_call_value(inner, event);
        let tool_call_id = tool_call_id(tool_call, inner, event)
            .unwrap_or_else(|| format!("unknown-{}", Uuid::new_v4()));
        if self.emitted.contains_key(&tool_call_id) {
            return None;
        }
        if let Some(name) = tool_call_name(tool_call, inner, event) {
            self.names.insert(tool_call_id.clone(), name.to_string());
        }
        let args = self.parse_args_fragment(&tool_call_id, tool_call, inner, event)?;
        let tool_name = self.names.get(&tool_call_id).map(String::as_str)?;
        let mapped = native_letta_tool_request_event_with_args(
            event,
            inner,
            message_type == "approval_request_message",
            Some(args),
            Some(tool_name),
        );
        if mapped.is_some() {
            self.names.remove(&tool_call_id);
            self.argument_buffers.remove(&tool_call_id);
            *self.emitted.entry(tool_call_id).or_insert(0) += 1;
        }
        mapped
    }

    fn parse_args_fragment(
        &mut self,
        tool_call_id: &str,
        tool_call: Option<&serde_json::Value>,
        inner: &serde_json::Value,
        event: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let args_raw = tool_call_args_raw(tool_call, inner, event)?;
        if args_raw.is_null() {
            return None;
        }
        if let Some(fragment) = args_raw.as_str() {
            let buffer = self
                .argument_buffers
                .entry(tool_call_id.to_string())
                .or_default();
            buffer.push_str(fragment);
            match serde_json::from_str::<serde_json::Value>(buffer) {
                Ok(value) if !value.is_null() => Some(value),
                _ => None,
            }
        } else {
            Some(args_raw.clone())
        }
    }
}

fn tool_call_value<'a>(
    inner: &'a serde_json::Value,
    event: &'a serde_json::Value,
) -> Option<&'a serde_json::Value> {
    inner
        .get("tool_call")
        .or_else(|| event.get("tool_call"))
        .or_else(|| {
            inner
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .and_then(|items| items.first())
        })
        .or_else(|| {
            event
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .and_then(|items| items.first())
        })
}

fn tool_call_id(
    tool_call: Option<&serde_json::Value>,
    inner: &serde_json::Value,
    event: &serde_json::Value,
) -> Option<String> {
    tool_call
        .and_then(|v| v.get("tool_call_id"))
        .or_else(|| tool_call.and_then(|v| v.get("id")))
        .or_else(|| {
            tool_call
                .and_then(|v| v.get("function"))
                .and_then(|f| f.get("tool_call_id"))
        })
        .or_else(|| inner.get("tool_call_id"))
        .or_else(|| inner.get("id"))
        .or_else(|| event.get("tool_call_id"))
        .or_else(|| event.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn tool_call_name<'a>(
    tool_call: Option<&'a serde_json::Value>,
    inner: &'a serde_json::Value,
    event: &'a serde_json::Value,
) -> Option<&'a str> {
    tool_call
        .and_then(|v| v.get("name"))
        .or_else(|| {
            tool_call
                .and_then(|v| v.get("function"))
                .and_then(|f| f.get("name"))
        })
        .or_else(|| inner.get("tool_name"))
        .or_else(|| inner.get("name"))
        .or_else(|| event.get("tool_name"))
        .or_else(|| event.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn tool_call_args_raw<'a>(
    tool_call: Option<&'a serde_json::Value>,
    inner: &'a serde_json::Value,
    event: &'a serde_json::Value,
) -> Option<&'a serde_json::Value> {
    tool_call
        .and_then(|v| v.get("input"))
        .or_else(|| tool_call.and_then(|v| v.get("arguments")))
        .or_else(|| tool_call.and_then(|v| v.get("args")))
        .or_else(|| {
            tool_call
                .and_then(|v| v.get("function"))
                .and_then(|f| f.get("arguments"))
        })
        .or_else(|| inner.get("input"))
        .or_else(|| inner.get("args"))
        .or_else(|| inner.get("arguments"))
        .or_else(|| event.get("input"))
        .or_else(|| event.get("args"))
        .or_else(|| event.get("arguments"))
}

pub fn map_native_letta_stream_event_to_acp_event_with_accumulator(
    event: &serde_json::Value,
    accumulator: &mut LettaToolCallAccumulator,
) -> Option<AcpGatewayEvent> {
    if let Some(mapped) = accumulator.observe(event) {
        return Some(mapped);
    }
    map_native_letta_stream_event_to_acp_event(event)
}

pub fn native_letta_conversation_resolved_event(
    event: &serde_json::Value,
) -> Option<AcpGatewayEvent> {
    let conversation_id = event
        .get("conversation_id")
        .or_else(|| event.get("conversationId"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| s.starts_with("conv-"))?;
    let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let message_type = event
        .get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if ty == "conversation_resolved" || message_type == "conversation_resolved" {
        Some(AcpGatewayEvent::ConversationResolved {
            conversation_id: conversation_id.to_string(),
        })
    } else {
        None
    }
}

pub fn acp_event_adapter_type(event: &AcpGatewayEvent) -> &'static str {
    match event {
        AcpGatewayEvent::AssistantTextDelta { .. } => "assistant_text_delta",
        AcpGatewayEvent::StatusText { .. } => "status_text",
        AcpGatewayEvent::TurnComplete { .. } => "turn_complete",
        AcpGatewayEvent::Error { .. } => "error",
        AcpGatewayEvent::ToolRequest { .. } => "tool_request",
        AcpGatewayEvent::ConversationResolved { .. } => "conversation_resolved",
    }
}

pub fn acp_event_has_visible_output(event: &AcpGatewayEvent) -> bool {
    match event {
        AcpGatewayEvent::AssistantTextDelta { text } | AcpGatewayEvent::StatusText { text } => {
            !text.is_empty()
        }
        AcpGatewayEvent::Error { .. } => true,
        AcpGatewayEvent::TurnComplete { .. }
        | AcpGatewayEvent::ToolRequest { .. }
        | AcpGatewayEvent::ConversationResolved { .. } => false,
    }
}

pub fn acp_event_to_adapter_sse(event: AcpGatewayEvent) -> Bytes {
    let mapped = match event {
        AcpGatewayEvent::AssistantTextDelta { text } => serde_json::json!({
            "type": "assistant_text_delta",
            "text": text,
        }),
        AcpGatewayEvent::StatusText { text } => serde_json::json!({
            "type": "status_text",
            "text": text,
        }),
        AcpGatewayEvent::TurnComplete { outcome } => serde_json::json!({
            "type": "turn_complete",
            "outcome": outcome,
        }),
        AcpGatewayEvent::Error {
            message,
            detail,
            error_type,
            request_id,
            context,
        } => {
            let mut mapped = serde_json::json!({
                "type": "error",
                "message": message,
                "detail": detail,
                "error_type": error_type,
            });
            if let Some(context) = context {
                mapped["context"] = context;
            }
            if let Some(request_id) = request_id {
                mapped["request_id"] = serde_json::json!(request_id);
            }
            mapped
        }
        AcpGatewayEvent::ToolRequest {
            request_id,
            turn_id,
            tool_call_id,
            approval_request_id,
            tool_name,
            title,
            kind,
            args,
            approval_required,
            approval_reason,
            result_tx: _,
            result_rx: _,
        } => serde_json::json!({
            "type": "tool_request",
            "request_id": request_id,
            "turn_id": turn_id,
            "tool_call_id": tool_call_id,
            "approval_request_id": approval_request_id,
            "tool_name": tool_name,
            "title": title,
            "kind": kind,
            "args": args,
            "approval": {
                "required": approval_required,
                "reason": approval_reason,
            },
            "policy": {
                "scope_basis": "acp:tools",
                "risk": ACP_READ_TEXT_FILE_TOOL.risk,
                "max_lines": 2000,
                "sensitive_path_policy": "client_permission_required",
            },
            "diagnostic": {
                "component": "den.acp",
                "transport_version": 3,
            },
        }),
        AcpGatewayEvent::ConversationResolved { conversation_id } => serde_json::json!({
            "type": "conversation_resolved",
            "conversation_id": conversation_id,
        }),
    };
    Bytes::from(format!("data: {}\n\n", mapped))
}

fn preview_str_truncated(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
