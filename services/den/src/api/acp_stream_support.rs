use base64::Engine as _;
use serde_json::json;
use std::collections::BTreeMap;

use crate::{
    api::acp::{acp_debug_event_sample_chars, AcpStreamContext},
    core::{
        acp_letta_events::{
            acp_event_adapter_type, acp_event_has_visible_output, AcpGatewayEvent,
            LettaToolCallAccumulator,
        },
        acp_turn_controller::AcpTurnController,
    },
};

#[derive(Default)]
pub(super) struct AcpStreamDiagnostics {
    pub(super) upstream_frames: usize,
    pub(super) parsed_events: usize,
    pub(super) mapped_events: usize,
    pub(super) unmapped_events: usize,
    pub(super) native_message_types: BTreeMap<String, usize>,
    pub(super) native_event_types: BTreeMap<String, usize>,
    pub(super) adapter_event_types: BTreeMap<String, usize>,
    pub(super) tool_request_counts: BTreeMap<String, usize>,
    pub(super) tool_call_accumulator: LettaToolCallAccumulator,
    pub(super) unmapped_event_samples: Vec<String>,
    pub(super) run_ids: Vec<String>,
    pub(super) saw_visible_output: bool,
    pub(super) saw_error: bool,
    pub(super) saw_turn_complete: bool,
    pub(super) saw_tool_return_ack: bool,
    pub(super) saw_requires_approval_stop: bool,
    pub(super) emitted_empty_turn_error: bool,
    pub(super) emitted_runtime_cleanup: bool,
}

impl AcpStreamDiagnostics {
    pub(super) fn merge_from(&mut self, other: Self) {
        self.upstream_frames += other.upstream_frames;
        self.parsed_events += other.parsed_events;
        self.mapped_events += other.mapped_events;
        self.unmapped_events += other.unmapped_events;
        for (key, value) in other.native_message_types {
            *self.native_message_types.entry(key).or_insert(0) += value;
        }
        for (key, value) in other.native_event_types {
            *self.native_event_types.entry(key).or_insert(0) += value;
        }
        for (key, value) in other.adapter_event_types {
            *self.adapter_event_types.entry(key).or_insert(0) += value;
        }
        for (key, value) in other.tool_request_counts {
            *self.tool_request_counts.entry(key).or_insert(0) += value;
        }
        for sample in other.unmapped_event_samples {
            if self.unmapped_event_samples.len() < 5 {
                self.unmapped_event_samples.push(sample);
            }
        }
        for run_id in other.run_ids {
            if !self.run_ids.iter().any(|known| known == &run_id) {
                self.run_ids.push(run_id);
            }
        }
        self.saw_visible_output |= other.saw_visible_output;
        self.saw_error |= other.saw_error;
        self.saw_turn_complete |= other.saw_turn_complete;
        self.saw_tool_return_ack |= other.saw_tool_return_ack;
        self.saw_requires_approval_stop |= other.saw_requires_approval_stop;
        self.emitted_empty_turn_error |= other.emitted_empty_turn_error;
        self.emitted_runtime_cleanup |= other.emitted_runtime_cleanup;
    }

    pub(super) fn observe_runtime_event(
        &mut self,
        event: &crate::core::runtime_provider::RuntimeStreamEvent,
    ) {
        self.parsed_events += 1;
        let runtime_type = match event {
            crate::core::runtime_provider::RuntimeStreamEvent::JsonValue { .. } => "json_value",
            crate::core::runtime_provider::RuntimeStreamEvent::AssistantTextDelta { .. } => {
                self.saw_visible_output = true;
                "assistant_text_delta"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::RunProgress { .. } => {
                self.saw_visible_output = true;
                "run_progress"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::RunPaused { reason, .. } => {
                if reason == "awaiting_approval" {
                    self.saw_requires_approval_stop = true;
                }
                "run_paused"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::ToolCallRequested { tool_call_id, .. } => {
                let count = self.tool_request_counts.entry(tool_call_id.clone()).or_insert(0);
                *count += 1;
                "tool_call_requested"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::Error { .. } => {
                self.saw_error = true;
                self.saw_visible_output = true;
                "error"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::ConversationResolved { conversation } => {
                let run_id = conversation.id.clone();
                if !self.run_ids.iter().any(|known| known == &run_id) {
                    self.run_ids.push(run_id);
                }
                "conversation_resolved"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::TurnCompleted { .. } => {
                self.saw_turn_complete = true;
                "turn_completed"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::TurnFailed { .. } => {
                self.saw_error = true;
                self.saw_visible_output = true;
                "turn_failed"
            }
            crate::core::runtime_provider::RuntimeStreamEvent::TurnCancelled { .. } => {
                self.saw_error = true;
                self.saw_visible_output = true;
                "turn_cancelled"
            }
        };
        Self::increment(&mut self.native_event_types, runtime_type);
    }

    fn increment(map: &mut BTreeMap<String, usize>, key: &str) {
        let key = if key.trim().is_empty() { "<missing>" } else { key };
        *map.entry(key.to_string()).or_insert(0) += 1;
    }

    pub(super) fn observe_parsed_event(&mut self, value: &serde_json::Value) -> Vec<String> {
        self.parsed_events += 1;
        let mut newly_observed_run_ids = Vec::new();
        let message_type = value.get("message_type").and_then(|v| v.as_str()).unwrap_or("");
        let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        for run_id in Self::extract_run_ids(value) {
            if self.observe_run_id(&run_id) {
                newly_observed_run_ids.push(run_id);
            }
        }
        Self::increment(&mut self.native_message_types, message_type);
        if message_type == "tool_return_message" {
            self.saw_tool_return_ack = true;
        }
        let stop_reason = value
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .or_else(|| value.pointer("/message/stop_reason").and_then(|v| v.as_str()))
            .or_else(|| value.pointer("/data/stop_reason").and_then(|v| v.as_str()));
        if stop_reason == Some("requires_approval") {
            self.saw_requires_approval_stop = true;
        }
        Self::increment(&mut self.native_event_types, event_type);
        newly_observed_run_ids
    }

    fn extract_run_ids(value: &serde_json::Value) -> Vec<String> {
        let mut run_ids = Vec::new();
        for pointer in ["/run_id", "/message/run_id", "/data/run_id", "/run/id", "/message/run/id", "/data/run/id"] {
            if let Some(run_id) = value.pointer(pointer).and_then(serde_json::Value::as_str).map(str::trim).filter(|run_id| !run_id.is_empty()) {
                let run_id = run_id.to_string();
                if !run_ids.iter().any(|known| known == &run_id) {
                    run_ids.push(run_id);
                }
            }
        }
        for pointer in ["/run_ids", "/message/run_ids", "/data/run_ids"] {
            if let Some(items) = value.pointer(pointer).and_then(serde_json::Value::as_array) {
                for run_id in items.iter().filter_map(serde_json::Value::as_str).map(str::trim).filter(|run_id| !run_id.is_empty()) {
                    let run_id = run_id.to_string();
                    if !run_ids.iter().any(|known| known == &run_id) {
                        run_ids.push(run_id);
                    }
                }
            }
        }
        run_ids
    }

    fn observe_run_id(&mut self, run_id: &str) -> bool {
        let run_id = run_id.trim();
        if run_id.is_empty() || self.run_ids.iter().any(|known| known == run_id) {
            return false;
        }
        self.run_ids.push(run_id.to_string());
        true
    }

    pub(super) fn observe_mapped_event(&mut self, event: &AcpGatewayEvent) {
        self.mapped_events += 1;
        Self::increment(&mut self.adapter_event_types, acp_event_adapter_type(event));
        self.saw_visible_output |= acp_event_has_visible_output(event);
        self.saw_error |= matches!(event, AcpGatewayEvent::Error { .. });
        self.saw_turn_complete |= matches!(event, AcpGatewayEvent::TurnComplete { .. } | AcpGatewayEvent::TurnResult { .. });
    }

    pub(super) fn observe_unmapped_event(&mut self, value: &serde_json::Value) {
        self.unmapped_events += 1;
        if self.unmapped_event_samples.len() < 5 {
            self.unmapped_event_samples.push(summarize_letta_event_for_log(value).to_string());
        }
    }

    pub(super) fn empty_turn_error_event(&mut self, context: &AcpStreamContext) -> Option<AcpGatewayEvent> {
        if self.emitted_empty_turn_error || self.saw_visible_output || self.saw_error || self.saw_tool_return_ack || self.emitted_runtime_cleanup {
            return None;
        }
        self.emitted_empty_turn_error = true;
        let detail = format!(
            "Letta stream ended without displayable assistant/status/error output. upstream_frames={}, parsed_events={}, mapped_events={}, unmapped_events={}, message_types={:?}, event_types={:?}",
            self.upstream_frames, self.parsed_events, self.mapped_events, self.unmapped_events, self.native_message_types, self.native_event_types,
        );
        Some(AcpGatewayEvent::Error {
            message: "Letta completed the turn without producing displayable ACP output.".to_string(),
            detail: Some(detail),
            error_type: Some("empty_mapped_turn".to_string()),
            request_id: Some(context.request_id.to_string()),
            context: Some(json!({
                "acp_session_id": context.acp_session_id,
                "unmapped_event_samples": self.unmapped_event_samples,
                "run_ids": self.run_ids,
            })),
        })
    }

    pub(super) fn mark_runtime_cleanup_emitted(&mut self) {
        self.emitted_runtime_cleanup = true;
    }

    pub(super) fn diagnostic_json_with_turn_controller(&self, context: &AcpStreamContext, turn_controller: Option<&AcpTurnController>) -> serde_json::Value {
        json!({
            "request_id": context.request_id,
            "acp_session_id": context.acp_session_id,
            "upstream_frames": self.upstream_frames,
            "parsed_events": self.parsed_events,
            "mapped_events": self.mapped_events,
            "unmapped_events": self.unmapped_events,
            "native_message_types": self.native_message_types,
            "native_event_types": self.native_event_types,
            "adapter_event_types": self.adapter_event_types,
            "tool_request_counts": self.tool_request_counts,
            "run_ids": self.run_ids,
            "saw_visible_output": self.saw_visible_output,
            "saw_error": self.saw_error,
            "saw_turn_complete": self.saw_turn_complete,
            "saw_tool_return_ack": self.saw_tool_return_ack,
            "saw_requires_approval_stop": self.saw_requires_approval_stop,
            "turn_controller": turn_controller.map(|controller| {
                let snapshot = controller.status_snapshot();
                json!({
                    "phase": format!("{:?}", snapshot.phase),
                    "open_obligations": snapshot.open_obligations,
                    "pending_adapter_tools": snapshot.pending_adapter_tools,
                    "pending_den_tools": snapshot.pending_den_tools,
                    "pending_permissions": snapshot.pending_permissions,
                    "terminal_status": snapshot.terminal_status.map(|status| format!("{:?}", status)),
                    "terminal_reason": snapshot.terminal_reason.map(|reason| format!("{:?}", reason)),
                    "orphaned_requires_approval": snapshot.orphaned_requires_approval,
                    "late_results_ignored": snapshot.late_results_ignored,
                })
            }),
        })
    }

    pub(super) fn log_summary(&self, context: &AcpStreamContext) {
        let turn_result_count = self.adapter_event_types.get("turn_result").copied().unwrap_or(0);
        if turn_result_count > 1 {
            tracing::warn!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                turn_result_count,
                "ACP stream emitted more than one terminal turn_result"
            );
        }
        tracing::info!(
            request_id = %context.request_id,
            acp_session_id = %context.acp_session_id,
            upstream_frames = self.upstream_frames,
            parsed_events = self.parsed_events,
            mapped_events = self.mapped_events,
            unmapped_events = self.unmapped_events,
            saw_visible_output = self.saw_visible_output,
            saw_error = self.saw_error,
            saw_turn_complete = self.saw_turn_complete,
            saw_tool_return_ack = self.saw_tool_return_ack,
            native_message_types = ?self.native_message_types,
            native_event_types = ?self.native_event_types,
            adapter_event_types = ?self.adapter_event_types,
            tool_request_counts = ?self.tool_request_counts,
            pending_tool_argument_buffers = self.tool_call_accumulator.pending_argument_buffers(),
            pending_tool_name_buffers = self.tool_call_accumulator.pending_name_buffers(),
            unmapped_event_samples = ?self.unmapped_event_samples,
            run_ids = ?self.run_ids,
            "ACP Letta stream summary"
        );
    }
}

/// Byte offset after the first complete SSE frame delimiter (`\n\n` or `\r\n\r\n`).
pub(super) fn find_sse_frame_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

pub(super) fn strip_trailing_sse_delimiter_owned(mut frame: Vec<u8>) -> Vec<u8> {
    if frame.ends_with(b"\r\n\r\n") {
        frame.truncate(frame.len().saturating_sub(4));
    } else if frame.ends_with(b"\n\n") {
        frame.truncate(frame.len().saturating_sub(2));
    }
    frame
}

#[cfg(test)]
#[allow(dead_code)]
pub(super) fn strip_trailing_sse_delimiter(frame: &[u8]) -> &[u8] {
    if frame.ends_with(b"\r\n\r\n") {
        &frame[..frame.len().saturating_sub(4)]
    } else if frame.ends_with(b"\n\n") {
        &frame[..frame.len().saturating_sub(2)]
    } else {
        frame
    }
}

const SSE_JSON_PREVIEW_MAX: usize = 192;

fn sha256_short(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest).chars().take(16).collect()
}

fn summarize_large_text_field(value: &str, allow_preview: bool) -> serde_json::Value {
    let mut summary = json!({
        "redacted": true,
        "bytes": value.len(),
        "chars": value.chars().count(),
        "sha256": sha256_short(value),
    });
    if allow_preview && !value.is_empty() {
        summary["preview"] = json!(preview_str_truncated(value, acp_debug_event_sample_chars().min(512)));
        summary["truncated"] = json!(value.len() > acp_debug_event_sample_chars().min(512));
    }
    summary
}

fn summarize_tool_arguments(value: &str) -> serde_json::Value {
    let mut summary = summarize_large_text_field(value, false);
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(object) = parsed.as_object() {
            summary["json_keys"] = json!(object.keys().cloned().collect::<Vec<_>>());
            for key in ["path", "destination_path", "source_path", "root", "glob", "pattern", "query", "line", "limit", "recursive", "include_hidden", "command", "cwd"] {
                if let Some(value) = object.get(key) {
                    summary[key] = value.clone();
                }
            }
        }
    }
    summary
}

pub(super) fn summarize_letta_event_for_log(value: &serde_json::Value) -> serde_json::Value {
    let mut event = value.clone();
    let allow_preview = cfg!(debug_assertions) || std::env::var("BEARS_ACP_DEBUG_EVENT_SAMPLES").ok().is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));
    if let Some(object) = event.as_object_mut() {
        if let Some(args) = object.get("args").and_then(serde_json::Value::as_str) {
            let summarized = summarize_tool_arguments(args);
            object.insert("args".to_string(), summarized);
        }
        for key in ["tool_return", "content", "reasoning", "text", "message"] {
            if let Some(value) = object.get(key).and_then(serde_json::Value::as_str) {
                if value.len() > 256 {
                    object.insert(key.to_string(), summarize_large_text_field(value, allow_preview));
                }
            }
        }
        if let Some(message) = object.get_mut("message").and_then(serde_json::Value::as_object_mut) {
            for key in ["content", "reasoning", "text", "tool_return"] {
                if let Some(value) = message.get(key).and_then(serde_json::Value::as_str) {
                    if value.len() > 256 {
                        message.insert(key.to_string(), summarize_large_text_field(value, allow_preview));
                    }
                }
            }
        }
        let keys = object.keys().cloned().collect::<Vec<_>>();
        object.insert("keys".to_string(), json!(keys));
    }
    event
}

fn preview_str_truncated(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

pub(super) fn parse_sse_event_body_to_json(body: &[u8]) -> Result<Option<serde_json::Value>, String> {
    let body = String::from_utf8_lossy(body);
    let mut data_lines = Vec::new();
    for raw_line in body.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            data_lines.push(rest);
        }
    }
    if data_lines.is_empty() {
        return Ok(None);
    }
    let joined = data_lines.join("\n");
    serde_json::from_str::<serde_json::Value>(&joined)
        .map(Some)
        .map_err(|err| format!("{err}; body_preview={}", preview_str_truncated(&joined, SSE_JSON_PREVIEW_MAX)))
}
