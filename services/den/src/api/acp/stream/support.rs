use serde_json::json;
use std::collections::BTreeMap;

use crate::{
    api::acp::{
        AcpStreamContext,
    },
    core::{
        acp_letta_events::{
            acp_event_adapter_type, acp_event_has_visible_output, AcpGatewayEvent,
            LettaToolCallAccumulator,
        },
        acp_turn_controller::AcpTurnController,
    },
};

pub(in crate::api::acp) use super::support_sse::{
    find_sse_frame_end, parse_sse_event_body_to_json, strip_trailing_sse_delimiter_owned,
};

#[derive(Default)]
pub(in crate::api::acp) struct AcpStreamDiagnostics {
    pub(in crate::api::acp) upstream_frames: usize,
    pub(in crate::api::acp) parsed_events: usize,
    pub(in crate::api::acp) mapped_events: usize,
    pub(in crate::api::acp) unmapped_events: usize,
    pub(in crate::api::acp) native_message_types: BTreeMap<String, usize>,
    pub(in crate::api::acp) native_event_types: BTreeMap<String, usize>,
    pub(in crate::api::acp) adapter_event_types: BTreeMap<String, usize>,
    pub(in crate::api::acp) tool_request_counts: BTreeMap<String, usize>,
    pub(in crate::api::acp) tool_call_accumulator: LettaToolCallAccumulator,
    pub(in crate::api::acp) unmapped_event_samples: Vec<String>,
    pub(in crate::api::acp) run_ids: Vec<String>,
    pub(in crate::api::acp) saw_visible_output: bool,
    pub(in crate::api::acp) saw_error: bool,
    pub(in crate::api::acp) saw_turn_complete: bool,
    pub(in crate::api::acp) saw_tool_return_ack: bool,
    pub(in crate::api::acp) saw_requires_approval_stop: bool,
    pub(in crate::api::acp) emitted_empty_turn_error: bool,
    pub(in crate::api::acp) emitted_runtime_cleanup: bool,
}

impl AcpStreamDiagnostics {
    pub(in crate::api::acp) fn merge_from(&mut self, other: Self) {
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

    pub(in crate::api::acp) fn observe_runtime_event(
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

    pub(in crate::api::acp) fn observe_parsed_event(&mut self, value: &serde_json::Value) -> Vec<String> {
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

    pub(in crate::api::acp) fn observe_mapped_event(&mut self, event: &AcpGatewayEvent) {
        self.mapped_events += 1;
        Self::increment(&mut self.adapter_event_types, acp_event_adapter_type(event));
        self.saw_visible_output |= acp_event_has_visible_output(event);
        self.saw_error |= matches!(event, AcpGatewayEvent::Error { .. });
        self.saw_turn_complete |= matches!(event, AcpGatewayEvent::TurnComplete { .. } | AcpGatewayEvent::TurnResult { .. });
    }

    pub(in crate::api::acp) fn observe_unmapped_event(&mut self, value: &serde_json::Value) {
        self.unmapped_events += 1;
        if self.unmapped_event_samples.len() < 5 {
            self.unmapped_event_samples
                .push(super::logging::summarize_letta_event_for_log(value).to_string());
        }
    }

    pub(in crate::api::acp) fn empty_turn_error_event(&mut self, context: &AcpStreamContext) -> Option<AcpGatewayEvent> {
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

    pub(in crate::api::acp) fn mark_runtime_cleanup_emitted(&mut self) {
        self.emitted_runtime_cleanup = true;
    }

    pub(in crate::api::acp) fn diagnostic_json_with_turn_controller(&self, context: &AcpStreamContext, turn_controller: Option<&AcpTurnController>) -> serde_json::Value {
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

    pub(in crate::api::acp) fn log_summary(&self, context: &AcpStreamContext) {
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

