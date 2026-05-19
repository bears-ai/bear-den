use std::collections::BTreeMap;

use bytes::Bytes;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::{
    acp_tool_turns::AcpToolResultRequest,
    acp_tools::{
        acp_diag_phase, acp_tool_policy_json_for_provider, supported_provider_tool_names,
        AcpToolName,
    },
    den_tools,
    work_plans::{WorkPlanItemStatus, WorkPlanProjection},
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
    TurnResult {
        status: String,
        reason: String,
        request_id: Option<String>,
        session_id: Option<String>,
        retryable: bool,
        diagnostics: serde_json::Value,
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
    PermissionRequest {
        request_id: String,
        permission_id: String,
        tool_call_id: String,
        tool_name: String,
        title: String,
        reason: String,
        target: serde_json::Value,
        options: Vec<String>,
    },
    PlanUpdate(WorkPlanProjection),
    PlanUpdateJson {
        entries: Vec<serde_json::Value>,
    },
    PlanApprovalFallback {
        plan_id: Uuid,
        title: String,
        body: String,
        artifact_path: String,
        state: String,
        approval_status: String,
    },
    ModeUpdate {
        mode: String,
    },
    ConversationResolved {
        conversation_id: String,
    },
    SessionInfoUpdate {
        title: Option<String>,
        updated_at: Option<String>,
        meta: Option<serde_json::Value>,
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
        "ping" => None,
        "assistant_message" => {
            let text = letta_stream_text_preserving_whitespace(inner)
                .or_else(|| letta_stream_text_preserving_whitespace(event))
                .unwrap_or_default();
            if let Some(tool_name) = pseudo_tool_call_name(&text) {
                Some(AcpGatewayEvent::Error {
                    message: format!(
                        "Model emitted textual pseudo tool call for {tool_name} instead of a native tool call."
                    ),
                    detail: Some("The tool was advertised, but the model emitted text instead of a native tool call. This can happen when the continuation tool surface is too large, tool schema handling drifted inside Letta/model provider, or the run hit a continuation budget. Check `Posting Letta ACP tool return continuation` for client_tools_count/client_tools_bytes/max_steps.".to_string()),
                    error_type: Some("pseudo_tool_call_text".to_string()),
                    request_id: None,
                    context: Some(serde_json::json!({
                        "tool_name": tool_name,
                        "preview": preview_str_truncated(&text, 500),
                    })),
                })
            } else {
                Some(AcpGatewayEvent::AssistantTextDelta { text })
            }
        }
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
        "tool_return_message" => None,
        _ => native_letta_conversation_resolved_event(event)
            .or_else(|| extract_stream_text_delta(event)),
    }
}

fn extract_stream_text_delta(event: &serde_json::Value) -> Option<AcpGatewayEvent> {
    let kind = stream_text_delta_kind(event)?;
    let text = stream_text_delta_text(event)?;
    if text.is_empty() {
        return None;
    }
    match kind {
        StreamTextDeltaKind::Assistant => Some(AcpGatewayEvent::AssistantTextDelta { text }),
        StreamTextDeltaKind::Reasoning => Some(AcpGatewayEvent::StatusText { text }),
    }
}

#[derive(Clone, Copy)]
enum StreamTextDeltaKind {
    Assistant,
    Reasoning,
}

fn stream_text_delta_kind(event: &serde_json::Value) -> Option<StreamTextDeltaKind> {
    let candidates = [
        event.get("kind").and_then(|v| v.as_str()),
        event.get("role").and_then(|v| v.as_str()),
        event.get("type").and_then(|v| v.as_str()),
        event.get("message_type").and_then(|v| v.as_str()),
        event.pointer("/delta/kind").and_then(|v| v.as_str()),
        event.pointer("/delta/role").and_then(|v| v.as_str()),
        event
            .pointer("/choices/0/delta/role")
            .and_then(|v| v.as_str()),
    ];
    for candidate in candidates.into_iter().flatten() {
        let candidate = candidate.to_ascii_lowercase();
        if candidate.contains("reasoning") || candidate.contains("thought") {
            return Some(StreamTextDeltaKind::Reasoning);
        }
        if candidate.contains("assistant")
            || candidate.contains("text_delta")
            || candidate.contains("message_delta")
        {
            return Some(StreamTextDeltaKind::Assistant);
        }
    }
    None
}

fn stream_text_delta_text(event: &serde_json::Value) -> Option<String> {
    for pointer in [
        "/text",
        "/delta/text",
        "/delta/content",
        "/content_delta",
        "/content/text",
        "/choices/0/delta/content",
        "/message/delta/content",
    ] {
        if let Some(text) = event.pointer(pointer).and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }
    if let Some(delta) = event.get("delta") {
        if let Some(text) = letta_stream_text_preserving_whitespace(delta) {
            return Some(text);
        }
    }
    None
}

fn pseudo_tool_call_name(text: &str) -> Option<String> {
    for name in supported_provider_tool_names() {
        if text.contains(&format!("to=functions.{name}"))
            || text.contains(&format!("functions.{name}"))
            || text.contains(&format!("<tool_call>{name}"))
        {
            return Some(name.to_string());
        }
    }
    None
}

fn native_letta_tool_request_event(
    event: &serde_json::Value,
    inner: &serde_json::Value,
    has_letta_approval_request: bool,
) -> Option<AcpGatewayEvent> {
    native_letta_tool_request_event_with_args(event, inner, has_letta_approval_request, None, None)
}

fn native_letta_tool_request_event_with_args(
    event: &serde_json::Value,
    inner: &serde_json::Value,
    has_letta_approval_request: bool,
    args_override: Option<serde_json::Value>,
    tool_name_override: Option<&str>,
) -> Option<AcpGatewayEvent> {
    let tool_call = tool_call_value(inner, event);
    let tool_name = tool_name_override.or_else(|| tool_call_name(tool_call, inner, event))?;
    let acp_tool = AcpToolName::from_provider_alias(tool_name);
    let den_server_tool =
        den_tools::builtin_den_tool_descriptor_for_provider_name(tool_name).is_some();
    let unsupported_tool_detail = if acp_tool.is_none() && !den_server_tool {
        let mut supported = supported_provider_tool_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        for descriptor in den_tools::builtin_den_tool_descriptors() {
            supported.push(descriptor.provider_name);
            supported.extend(
                descriptor
                    .provider_aliases
                    .iter()
                    .map(|name| name.to_string()),
            );
        }
        Some(format!(
            "Unsupported ACP/Den tool: {tool_name}. Supported ACP/Den tools: {}.",
            supported.join(", ")
        ))
    } else {
        None
    };
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
    if let Some(tool) = acp_tool {
        let descriptor = tool.descriptor();
        if let Some(missing) = tool.missing_required_string_arg(&args) {
            if !args.is_object() || args.as_object().is_some_and(|m| m.is_empty()) {
                return None;
            }
            return Some(AcpGatewayEvent::Error {
                message: format!(
                    "Letta requested {} without a {missing} argument.",
                    descriptor.provider_name
                ),
                detail: Some(format!(
                    "Parsed arguments did not contain required string field `{missing}`; args={}",
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
                    "missing": missing,
                })),
            });
        }
    }
    let tool_call_id =
        tool_call_id(tool_call, inner, event).unwrap_or_else(|| format!("call-{}", Uuid::new_v4()));
    let client_approval_required = has_letta_approval_request;
    let letta_approval_request_id = has_letta_approval_request.then(|| {
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
        approval_request_id: letta_approval_request_id,
        tool_name: tool_name.to_string(),
        title: acp_tool
            .map(|tool| tool.descriptor().title.to_string())
            .unwrap_or_else(|| tool_name.to_string()),
        kind: if unsupported_tool_detail.is_some() {
            "unsupported".to_string()
        } else {
            acp_tool
                .map(|tool| tool.descriptor().kind.to_string())
                .unwrap_or_else(|| "server_tool".to_string())
        },
        args: if let Some(detail) = unsupported_tool_detail.as_ref() {
            let mut args = args;
            args["_unsupported_detail"] = serde_json::json!(detail);
            args
        } else {
            args
        },
        approval_required: client_approval_required
            && !den_server_tool
            && unsupported_tool_detail.is_none(),
        approval_reason: (!den_server_tool && unsupported_tool_detail.is_none()).then(|| {
            "BEARS requires client approval before running this local ACP tool.".to_string()
        }),
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
        AcpGatewayEvent::TurnResult { .. } => "turn_result",
        AcpGatewayEvent::Error { .. } => "error",
        AcpGatewayEvent::ToolRequest { .. } => "tool_request",
        AcpGatewayEvent::PermissionRequest { .. } => "permission_request",
        AcpGatewayEvent::PlanUpdate { .. }
        | AcpGatewayEvent::PlanUpdateJson { .. }
        | AcpGatewayEvent::PlanApprovalFallback { .. } => "plan_update",
        AcpGatewayEvent::ModeUpdate { .. } => "mode_update",
        AcpGatewayEvent::SessionInfoUpdate { .. } => "session_info_update",
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
        | AcpGatewayEvent::TurnResult { .. }
        | AcpGatewayEvent::ToolRequest { .. }
        | AcpGatewayEvent::PermissionRequest { .. }
        | AcpGatewayEvent::PlanApprovalFallback { .. } => true,
        AcpGatewayEvent::PlanUpdate { .. }
        | AcpGatewayEvent::PlanUpdateJson { .. }
        | AcpGatewayEvent::ModeUpdate { .. }
        | AcpGatewayEvent::ConversationResolved { .. }
        | AcpGatewayEvent::SessionInfoUpdate { .. } => false,
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
        AcpGatewayEvent::TurnResult {
            status,
            reason,
            request_id,
            session_id,
            retryable,
            diagnostics,
        } => serde_json::json!({
            "type": "turn_result",
            "status": status,
            "reason": reason,
            "request_id": request_id,
            "session_id": session_id,
            "retryable": retryable,
            "diagnostics": diagnostics,
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
        } => {
            let display = den_tools::builtin_den_tool_descriptor_for_provider_name(&tool_name)
                .map(|descriptor| descriptor.display)
                .unwrap_or_else(|| {
                    crate::core::acp_tools::acp_tool_display_for_provider(&tool_name, &args)
                });
            serde_json::json!({
                "type": "tool_request",
                "request_id": request_id,
                "turn_id": turn_id,
                "tool_call_id": tool_call_id,
                "approval_request_id": approval_request_id,
                "tool_name": tool_name,
                "title": title,
                "kind": kind,
                "args": args,
                "display": display,
                "approval": {
                    "required": approval_required,
                    "reason": approval_reason,
                },
                "policy": acp_tool_policy_json_for_provider(&tool_name),
                "diagnostic": {
                    "component": "den.acp",
                    "phase": acp_diag_phase::LETTA_TOOL_CALL_MAPPED,
                    "transport_version": 4,
                },
            })
        }
        AcpGatewayEvent::PermissionRequest {
            request_id,
            permission_id,
            tool_call_id,
            tool_name,
            title,
            reason,
            target,
            options,
        } => serde_json::json!({
            "type": "permission_request",
            "request_id": request_id,
            "permission_id": permission_id,
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "title": title,
            "reason": reason,
            "target": target,
            "options": options,
            "diagnostic": {
                "component": "den.acp",
                "phase": "permission_request_mapped",
                "transport_version": 3,
            }
        }),
        AcpGatewayEvent::SessionInfoUpdate {
            title,
            updated_at,
            meta,
        } => {
            let mut mapped = serde_json::json!({
                "type": "session_info_update",
                "title": title,
                "updated_at": updated_at,
                "diagnostic": {
                "component": "den.acp",
                    "phase": "session_info_update"
                }
            });
            if let Some(meta) = meta {
                mapped["meta"] = meta;
            }
            mapped
        }
        AcpGatewayEvent::PlanUpdate(plan) => serde_json::json!({
            "type": "plan_update",
            "plan_id": plan.id,
            "version": plan.version,
            "title": plan.title,
            "entries": plan.items.iter().map(|item| {
                let blocked_reason = item.blocked_reason.as_deref().unwrap_or("").trim();
                let summary = item.summary.as_deref().unwrap_or("").trim();
                let content = match item.status {
                    WorkPlanItemStatus::Blocked if !blocked_reason.is_empty() => format!("Blocked: {} — {}", item.title, blocked_reason),
                    WorkPlanItemStatus::Blocked => format!("Blocked: {}", item.title),
                    WorkPlanItemStatus::Cancelled => format!("Cancelled: {}", item.title),
                    _ if !summary.is_empty() => format!("{} — {}", item.title, summary),
                    _ => item.title.clone(),
                };
                let status = match item.status {
                    WorkPlanItemStatus::InProgress => "in_progress",
                    WorkPlanItemStatus::Completed | WorkPlanItemStatus::Cancelled => "completed",
                    _ => "pending",
                };
                let priority = if item.status == WorkPlanItemStatus::InProgress { "high" } else { "medium" };
                serde_json::json!({
                    "content": content,
                    "priority": priority,
                    "status": status,
                    "_meta": {
                        "bears": {
                            "item_id": item.id,
                            "status": item.status.as_str(),
                            "blocked_reason": item.blocked_reason,
                            "source_refs": item.source_refs,
                        }
                    }
                })
            }).collect::<Vec<_>>(),
            "diagnostic": {
                "component": "den.acp",
                "phase": "plan_update_mapped",
                "transport_version": 3,
            }
        }),
        AcpGatewayEvent::PlanUpdateJson { entries } => serde_json::json!({
            "type": "plan_update",
            "entries": entries,
            "diagnostic": {
                "component": "den.acp",
                "phase": "plan_update_mapped",
                "transport_version": 3,
            }
        }),
        AcpGatewayEvent::PlanApprovalFallback {
            plan_id,
            title,
            body,
            artifact_path,
            state,
            approval_status,
        } => serde_json::json!({
            "type": "plan_update",
            "entries": [{
                "content": format!("Review submitted implementation plan: {title}"),
                "priority": "high",
                "status": "in_progress",
                "_meta": {
                    "bears": {
                        "kind": "submitted_plan_approval",
                        "plan_id": plan_id,
                        "state": state,
                        "approval_status": approval_status,
                        "artifact_path": artifact_path,
                        "title": title,
                    }
                }
            }],
            "approval_fallback": {
                "kind": "submitted_plan_approval",
                "plan_id": plan_id,
                "title": title,
                "body": body,
                "artifact_path": artifact_path,
                "state": state,
                "approval_status": approval_status,
            },
            "diagnostic": {
                "component": "den.acp",
                "phase": "plan_approval_fallback_mapped",
                "transport_version": 3,
            }
        }),
        AcpGatewayEvent::ModeUpdate { mode } => serde_json::json!({
            "type": "mode_update",
            "mode": mode,
            "diagnostic": {
                "component": "den.acp",
                "phase": "mode_update_mapped",
                "transport_version": 3,
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call_event(name: &str, args: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "message_type": "tool_call_message",
            "tool_call": {
                "name": name,
                "tool_call_id": "call-test",
                "arguments": args.to_string(),
            }
        })
    }

    #[test]
    fn maps_list_directory_tool_call() {
        let event = tool_call_event(
            "fs_list_directory",
            serde_json::json!({ "path": "/workspace" }),
        );
        let mapped = map_native_letta_stream_event_to_acp_event(&event).expect("mapped event");
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_name,
                kind,
                args,
                ..
            } => {
                assert_eq!(tool_name, "fs_list_directory");
                assert_eq!(kind, "read");
                assert_eq!(args["path"], "/workspace");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_search_files_tool_call() {
        let event = tool_call_event(
            "fs_search_files",
            serde_json::json!({ "path": "/workspace", "query": "needle" }),
        );
        let mapped = map_native_letta_stream_event_to_acp_event(&event).expect("mapped event");
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_name,
                kind,
                args,
                ..
            } => {
                assert_eq!(tool_name, "fs_search_files");
                assert_eq!(kind, "search");
                assert_eq!(args["path"], "/workspace");
                assert_eq!(args["query"], "needle");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn tool_call_message_requires_adapter_result_without_letta_approval() {
        let event = tool_call_event(
            "fs_edit_file",
            serde_json::json!({
                "path": "/workspace/a.txt",
                "old_text": "before",
                "new_text": "after"
            }),
        );
        let mapped = map_native_letta_stream_event_to_acp_event(&event).expect("mapped event");
        match mapped {
            AcpGatewayEvent::ToolRequest {
                approval_required,
                approval_request_id,
                approval_reason,
                ..
            } => {
                assert!(!approval_required);
                assert!(approval_request_id.is_none());
                assert!(approval_reason.is_none());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_replace_text_tool_call() {
        let event = tool_call_event(
            "fs_edit_file",
            serde_json::json!({
                "path": "/workspace/a.txt",
                "old_text": "before",
                "new_text": "after"
            }),
        );
        let mapped = map_native_letta_stream_event_to_acp_event(&event).expect("mapped event");
        match mapped {
            AcpGatewayEvent::ToolRequest {
                tool_name,
                kind,
                args,
                ..
            } => {
                assert_eq!(tool_name, "fs_edit_file");
                assert_eq!(kind, "edit");
                assert_eq!(args["old_text"], "before");
                assert_eq!(args["new_text"], "after");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn search_files_requires_query() {
        let event = tool_call_event(
            "fs_search_files",
            serde_json::json!({ "path": "/workspace" }),
        );
        let mapped = map_native_letta_stream_event_to_acp_event(&event).expect("mapped event");
        match mapped {
            AcpGatewayEvent::Error {
                error_type,
                message,
                context,
                ..
            } => {
                assert_eq!(error_type.as_deref(), Some("invalid_tool_arguments"));
                assert!(message.contains("fs_search_files"));
                assert_eq!(context.unwrap()["missing"], "query");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn replace_text_requires_new_text() {
        let event = tool_call_event(
            "fs_edit_file",
            serde_json::json!({ "path": "/workspace/a.txt", "old_text": "before" }),
        );
        let mapped = map_native_letta_stream_event_to_acp_event(&event).expect("mapped event");
        match mapped {
            AcpGatewayEvent::Error {
                error_type,
                message,
                context,
                ..
            } => {
                assert_eq!(error_type.as_deref(), Some("invalid_tool_arguments"));
                assert!(message.contains("fs_edit_file"));
                assert_eq!(context.unwrap()["missing"], "new_text");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_openai_style_assistant_delta() {
        let event = serde_json::json!({
            "type": "message_delta",
            "choices": [{ "delta": { "role": "assistant", "content": "hello" } }]
        });
        match map_native_letta_stream_event_to_acp_event(&event) {
            Some(AcpGatewayEvent::AssistantTextDelta { text }) => assert_eq!(text, "hello"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_reasoning_delta_fallback() {
        let event = serde_json::json!({
            "type": "reasoning_delta",
            "delta": { "text": "thinking" }
        });
        match map_native_letta_stream_event_to_acp_event(&event) {
            Some(AcpGatewayEvent::StatusText { text }) => assert_eq!(text, "thinking"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn detects_pseudo_tool_call_text() {
        let event = serde_json::json!({
            "message_type": "assistant_message",
            "content": "to=functions.fs_edit_file {\"path\":\"/workspace/README.md\"}"
        });
        let mapped = map_native_letta_stream_event_to_acp_event(&event).expect("mapped event");
        match mapped {
            AcpGatewayEvent::Error {
                error_type,
                context,
                ..
            } => {
                assert_eq!(error_type.as_deref(), Some("pseudo_tool_call_text"));
                assert_eq!(context.unwrap()["tool_name"], "fs_edit_file");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn tool_return_message_is_diagnostic_only() {
        let event = serde_json::json!({
            "message_type": "tool_return_message",
            "tool_call_id": "call-1",
            "status": "success",
            "tool_return": "hello"
        });
        assert!(map_native_letta_stream_event_to_acp_event(&event).is_none());
    }

    #[test]
    fn maps_tool_call_to_adapter_sse_without_database() {
        let event = tool_call_event(
            "fs_edit_file",
            serde_json::json!({
                "path": "/workspace/a.txt",
                "old_text": "before",
                "new_text": "after"
            }),
        );
        let mapped = map_native_letta_stream_event_to_acp_event(&event).expect("mapped event");
        let bytes = acp_event_to_adapter_sse(mapped);
        let raw = std::str::from_utf8(&bytes).expect("utf8 sse");
        assert!(raw.contains("\"type\":\"tool_request\""));
        assert!(raw.contains("\"tool_name\":\"fs_edit_file\""));
        assert!(raw.contains("\"required\":true"));
        assert!(raw.contains("\"risk\":\"writes_workspace\""));
        assert!(raw.contains("\"phase\":\"letta_tool_call_mapped\""));
    }

    #[test]
    fn list_directory_sse_policy_includes_entry_limit() {
        let event = AcpGatewayEvent::ToolRequest {
            request_id: "request-1".to_string(),
            turn_id: "turn-1".to_string(),
            tool_call_id: "call-1".to_string(),
            approval_request_id: None,
            tool_name: "fs_list_directory".to_string(),
            title: "List directory".to_string(),
            kind: "read".to_string(),
            args: serde_json::json!({ "path": "/workspace" }),
            approval_required: false,
            approval_reason: None,
            result_tx: None,
            result_rx: None,
        };
        let bytes = acp_event_to_adapter_sse(event);
        let raw = std::str::from_utf8(&bytes).expect("utf8 sse");
        assert!(raw.contains("\"max_entries\":1000"));
        assert!(raw.contains("\"risk\":\"read_only\""));
    }
}
