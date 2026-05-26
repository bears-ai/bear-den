use bytes::Bytes;

use crate::{
    api::{
        acp::{
            persist_stream_event_side_effects, AcpResolvedToolResult, AcpStreamContext,
            PersistedToolRequestEffect,
        },
        acp_stream_support::{
            parse_sse_event_body_to_json, summarize_letta_event_for_log, AcpStreamDiagnostics,
        },
    },
    core::{
        acp_letta_events::{
            acp_event_to_adapter_sse, map_native_letta_stream_event_to_acp_event_with_accumulator,
            AcpGatewayEvent,
        },
        runtime_provider::RuntimeStreamEvent,
    },
};

pub(super) type AcpFrameResult = Result<
    (
        Vec<AcpGatewayEvent>,
        Option<PersistedToolRequestEffect>,
        Option<(String, String, AcpResolvedToolResult)>,
    ),
    std::io::Error,
>;

pub(super) async fn map_runtime_stream_event_to_acp_adapter_events_with_persistence(
    runtime_event: RuntimeStreamEvent,
    context: AcpStreamContext,
    diagnostics: &mut AcpStreamDiagnostics,
) -> AcpFrameResult {
    let value = match runtime_event {
        RuntimeStreamEvent::JsonValue { value } => value,
        RuntimeStreamEvent::ToolCallRequested {
            tool_call_id,
            tool_name,
            title,
            kind,
            arguments,
            approval_request_id,
            approval_required,
            approval_reason,
        } => serde_json::json!({
            "message_type": if approval_required { "approval_request_message" } else { "tool_call_message" },
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "tool_title": title,
            "tool_kind": kind,
            "args": arguments,
            "approval_request_id": approval_request_id,
            "approval_reason": approval_reason,
        }),
        RuntimeStreamEvent::RunPaused { reason, .. } => {
            let stop_reason = if reason == "awaiting_approval" {
                "requires_approval".to_string()
            } else {
                reason
            };
            serde_json::json!({
                "message_type": "stop_reason",
                "stop_reason": stop_reason,
            })
        }
        RuntimeStreamEvent::TurnCompleted { .. } => {
            serde_json::json!({
                "message_type": "stop_reason",
                "stop_reason": "end_turn",
            })
        }
        other => {
            return Err(std::io::Error::other(format!(
                "runtime event not supported by ACP persistence mapper: {other:?}"
            )));
        }
    };
    let observed_run_ids = diagnostics.observe_parsed_event(&value);
    if let Some(mut event) = map_native_letta_stream_event_to_acp_event_with_accumulator(
        &value,
        &mut diagnostics.tool_call_accumulator,
    ) {
        let mut tool_request_effect = persist_stream_event_side_effects(&context, &mut event)
            .await
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        let mut adapter_result_rx = None;
        let events = if let Some(effect) = tool_request_effect.as_mut() {
            if let Some(rx) = effect.den_server_result_rx.take() {
                let tool_call_id = effect.tool_call_id.clone();
                let tool_name = effect.tool_name.clone();
                let result: crate::core::acp_tool_turns::AcpToolResultRequest = rx
                    .await
                    .map_err(|err| std::io::Error::other(err.to_string()))?;
                adapter_result_rx = Some((
                    tool_call_id,
                    tool_name,
                    AcpResolvedToolResult::Ready(Box::new(result)),
                ));
                Vec::new()
            } else {
                vec![event]
            }
        } else {
            vec![event]
        };
        for run_id in observed_run_ids {
            if !diagnostics.run_ids.iter().any(|known| known == &run_id) {
                diagnostics.run_ids.push(run_id);
            }
        }
        Ok((events, tool_request_effect, adapter_result_rx))
    } else {
        diagnostics.observe_unmapped_event(&value);
        Ok((Vec::new(), None, None))
    }
}

pub(super) fn map_letta_stream_frame_to_acp_adapter_events(frame: &[u8]) -> Vec<Bytes> {
    let Some(value) = parse_sse_event_body_to_json(frame).ok().flatten() else {
        return Vec::new();
    };
    let mut accumulator = Default::default();
    map_native_letta_stream_event_to_acp_event_with_accumulator(&value, &mut accumulator)
        .map(|event| vec![acp_event_to_adapter_sse(event)])
        .unwrap_or_default()
}

pub(super) fn summarize_event_for_log(value: &serde_json::Value) -> serde_json::Value {
    summarize_letta_event_for_log(value)
}
