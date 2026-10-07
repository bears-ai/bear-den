use serde_json::{json, Value};

use bearwire_protocol::wire::{
    bearwire_event_to_json_rpc_notification, BearWireEvent, ExecutionTargetWire,
    JsonRpcNotification, ToolCallFinishStatusWire, ToolCallFinishWire, ToolCallRefWire,
    ToolCallRequestedWire, ToolCallWaitingWire, ToolCallWire, ToolPermissionWire,
};
use den_core::{
    client_tools::{
        client_tool_display_for_provider, client_tool_policy_json_for_provider, ClientToolName,
    },
    tools::descriptor::den_tool_display_json_for_provider,
};
use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};

use super::runtime_error_category_code;

pub fn tool_call_wire(
    tool_call_id: &str,
    tool_name: &str,
    title: Option<&str>,
    kind: &str,
    arguments: &Value,
) -> ToolCallWire {
    let display = den_tool_display_json_for_provider(tool_name, arguments)
        .unwrap_or_else(|| client_tool_display_for_provider(tool_name, arguments));
    ToolCallWire {
        id: tool_call_id.to_string(),
        name: tool_name.to_string(),
        title: title.map(str::to_string),
        kind: kind.to_string(),
        arguments: arguments.clone(),
        display,
    }
}

fn tool_call_execution_target(tool_name: &str) -> ExecutionTargetWire {
    match crate::turn_waits::resolve_tool_execution_owner(tool_name) {
        Ok(crate::turn_waits::ToolExecutionOwner::Den) => ExecutionTargetWire::Den,
        Ok(crate::turn_waits::ToolExecutionOwner::Armature) | Err(_) => {
            ExecutionTargetWire::ArmatureLocal
        }
    }
}

fn tool_call_policy(tool_name: &str) -> Option<Value> {
    match tool_call_execution_target(tool_name) {
        ExecutionTargetWire::Den => Some(json!({ "execution_target": "den" })),
        ExecutionTargetWire::ArmatureLocal => ClientToolName::from_provider_alias(tool_name)
            .map(|_| client_tool_policy_json_for_provider(tool_name)),
    }
}

pub fn tool_call_finish_wire(
    tool_call_id: &str,
    tool_name: Option<&str>,
    status: &str,
    summary: Option<&str>,
    error_message: Option<&str>,
    content: Option<&str>,
    structured_content: Option<Value>,
    error: Option<Value>,
    compacted: Option<Value>,
) -> ToolCallFinishWire {
    let compacted_summary = compacted
        .as_ref()
        .and_then(|value| value.get("output_summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty());
    let summary = summary
        .map(str::to_string)
        .or_else(|| error_message.map(str::to_string))
        .or_else(|| compacted_summary.map(str::to_string))
        .or_else(|| content.map(|text| text.chars().take(160).collect::<String>()));
    let tool_name = tool_name.map(str::to_string);
    ToolCallFinishWire {
        tool_call: ToolCallRefWire {
            id: tool_call_id.to_string(),
            name: tool_name,
        },
        status: ToolCallFinishStatusWire::from_wire_str(status),
        summary,
        error_message: error_message.map(str::to_string),
        content: content.map(str::to_string),
        structured_content,
        error,
        compacted,
    }
}

pub fn runtime_stream_event_to_bearwire_events(event: RuntimeStreamEvent) -> Vec<BearWireEvent> {
    match event {
        RuntimeStreamEvent::Semantic(event) => runtime_semantic_event_to_bearwire_events(event),
        RuntimeStreamEvent::ProviderActivity
        | RuntimeStreamEvent::UntranslatedProviderEvent { .. } => Vec::new(),
    }
}

pub fn runtime_stream_event_to_bearwire_notifications(
    event: RuntimeStreamEvent,
) -> Vec<JsonRpcNotification<BearWireEvent>> {
    runtime_stream_event_to_bearwire_events(event)
        .into_iter()
        .map(bearwire_event_to_json_rpc_notification)
        .collect()
}

pub fn runtime_semantic_event_to_bearwire_events(
    event: RuntimeSemanticEvent,
) -> Vec<BearWireEvent> {
    match event {
        RuntimeSemanticEvent::AssistantTextDelta { text } => vec![BearWireEvent::ephemeral(
            "message.delta",
            json!({
                "delta": text,
            }),
        )],
        RuntimeSemanticEvent::ReasoningTextDelta { text } => vec![BearWireEvent::ephemeral(
            "message.reasoning.delta",
            json!({
                "delta": text,
                "source": "provider_reasoning",
                "replay_policy": "thought",
            }),
        )],
        RuntimeSemanticEvent::StatusText { text } => vec![BearWireEvent::ephemeral(
            "run.progress",
            json!({
                "kind": "status_text",
                "text": text,
            }),
        )],
        RuntimeSemanticEvent::RunProgress {
            kind,
            text,
            phase,
            detail,
        } => {
            if kind == "session_info_update" {
                let title = detail
                    .as_ref()
                    .and_then(|value| value.get("title"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let updated_at = detail
                    .as_ref()
                    .and_then(|value| value.get("updated_at"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                return vec![BearWireEvent::ephemeral(
                    "session_info_update",
                    json!({
                        "title": title,
                        "updated_at": updated_at,
                    }),
                )];
            }
            let persistent = matches!(
                kind.as_str(),
                "model_request_profile_resolved" | "reasoning_effort_override"
            );
            let data = json!({
                "kind": kind,
                "text": text,
                "phase": phase,
                "detail": detail,
            });
            vec![if persistent {
                BearWireEvent::persistent_typed("run.progress", data)
            } else {
                BearWireEvent::ephemeral("run.progress", data)
            }]
        }
        RuntimeSemanticEvent::RunPaused {
            reason,
            resume_token,
            expires_at,
        } => vec![BearWireEvent::ephemeral(
            "run.paused",
            json!({
                "reason": reason,
                "resume_token": resume_token,
                "expires_at": expires_at,
            }),
        )],
        RuntimeSemanticEvent::BoundedSlice { reason } => vec![BearWireEvent::ephemeral(
            "run.bounded_slice",
            json!({ "reason": reason }),
        )],
        RuntimeSemanticEvent::ToolCallRequested {
            tool_call_id,
            tool_name,
            title,
            kind,
            arguments,
            approval_request_id,
            approval_required,
            approval_reason,
            run_id,
        } => {
            let has_permission_id = approval_request_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|id| !id.is_empty());
            let effective_kind = kind.unwrap_or_else(|| "function".to_string());
            let tool_call = tool_call_wire(
                &tool_call_id,
                &tool_name,
                title.as_deref(),
                &effective_kind,
                &arguments,
            );
            let execution_target = tool_call_execution_target(&tool_name);
            let event = if approval_required && has_permission_id {
                let permission_id = approval_request_id.clone().unwrap_or_default();
                BearWireEvent::tool_call_waiting(ToolCallWaitingWire {
                    expected_responder_action: None,
                    expected_client_method: "client.permission.result".to_string(),
                    obligation_id: None,
                    tool_call,
                    permission: ToolPermissionWire {
                        id: permission_id,
                        reason: approval_reason,
                        title: None,
                        target: None,
                    },
                    approval_required: true,
                    execution_target,
                    policy: tool_call_policy(&tool_name),
                    turn_step_id: None,
                })
            } else {
                BearWireEvent::tool_call_requested(ToolCallRequestedWire {
                    expected_responder_action: None,
                    obligation_id: None,
                    policy: tool_call_policy(&tool_name),
                    tool_call,
                    approval_required: false,
                    execution_target,
                    approval_request_id: approval_request_id.clone(),
                    reason: approval_reason,
                })
            };
            vec![event
                .with_run_id(run_id)
                .with_tool_call(tool_call_id)
                .with_permission_request(approval_request_id)]
        }
        RuntimeSemanticEvent::ToolCallFinished {
            tool_call_id,
            tool_name,
            status,
            summary,
            error_message,
        } => {
            vec![BearWireEvent::tool_call_finished(tool_call_finish_wire(
                &tool_call_id,
                Some(&tool_name),
                status.as_str(),
                summary.as_deref(),
                error_message.as_deref(),
                None,
                None,
                None,
                None,
            ))
            .with_tool_call(tool_call_id)]
        }
        RuntimeSemanticEvent::Error {
            message,
            detail,
            error_type,
            request_id,
            context,
        } => vec![BearWireEvent::ephemeral(
            "run.failed",
            json!({
                "message": message,
                "detail": detail,
                "error_type": error_type,
                "request_id": request_id,
                "context": context,
            }),
        )],
        RuntimeSemanticEvent::ConversationResolved { conversation } => {
            vec![BearWireEvent::ephemeral(
                "session.bound",
                json!({
                    "binding": {
                        "conversation_id": conversation.id,
                    }
                }),
            )
            .with_session(conversation.id)]
        }
        RuntimeSemanticEvent::TurnCompleted { turn } => vec![BearWireEvent::ephemeral(
            "run.completed",
            json!({
                "outcome": "ok",
                "turn": turn,
            }),
        )],
        RuntimeSemanticEvent::TurnFailed {
            turn,
            category,
            message,
        } => vec![BearWireEvent::ephemeral(
            "run.failed",
            json!({
                "reason": runtime_error_category_code(category),
                "message": message,
                "turn": turn,
            }),
        )],
        RuntimeSemanticEvent::TurnCancelled { turn } => vec![BearWireEvent::ephemeral(
            "run.cancelled",
            json!({
                "reason": "runtime_turn_cancelled",
                "turn": turn,
            }),
        )],
    }
}
