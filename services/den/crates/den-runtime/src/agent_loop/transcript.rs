use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use den_service::conversation::events::{
    canonical_persistence_context, persist_canonical_conversation_record_with_id,
    spawn_persist_assistant_output, spawn_persist_model_tool_request,
    spawn_persist_model_tool_result, CanonicalToolRequestRecord, CanonicalToolResultRecord,
    ConversationEventProvenance,
};

use crate::llm::{ChatMessage, ChatToolCall};

use super::{
    pending_tools::pending_tool_calls,
    tool_outcome::{is_incomplete_tool_result, tool_result_persistence_status},
    tool_policy::provider_tool_requires_approval,
};

fn parse_tool_arguments(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| Value::String(arguments.to_string()))
}

fn native_policy_reason(approval_required: bool) -> Option<String> {
    approval_required.then(|| "native runtime policy".to_string())
}

pub async fn persist_native_assistant_output_with_id(
    pool: PgPool,
    bear_id: Uuid,
    user_id: Option<i32>,
    conversation_id: String,
    client_session_id: String,
    request_id: Option<String>,
    assistant_text: String,
) -> Result<Option<Uuid>, den_core::DenError> {
    if assistant_text.trim().is_empty() {
        return Ok(None);
    }
    let provenance = ConversationEventProvenance::client_session(client_session_id.clone());
    let context = canonical_persistence_context(
        pool,
        bear_id,
        user_id,
        conversation_id,
        Some(client_session_id.clone()),
        request_id.clone(),
        client_session_id,
        false,
    );
    persist_canonical_conversation_record_with_id(
        &context,
        &den_service::conversation::events::CanonicalConversationRecord::assistant_output(
            assistant_text,
            &provenance,
            None,
            request_id,
        ),
    )
    .await
}

pub fn spawn_persist_native_agent_step(
    pool: PgPool,
    bear_id: Uuid,
    user_id: Option<i32>,
    conversation_id: String,
    client_session_id: String,
    request_id: Option<String>,
    assistant_text: String,
    tool_calls: &[ChatToolCall],
    preauthorized_call_id: Option<&str>,
) {
    if assistant_text.trim().is_empty() && tool_calls.is_empty() {
        return;
    }
    let provenance = ConversationEventProvenance::client_session(client_session_id.clone());
    let context = canonical_persistence_context(
        pool,
        bear_id,
        user_id,
        conversation_id,
        Some(client_session_id.clone()),
        request_id.clone(),
        client_session_id,
        false,
    );
    if !assistant_text.trim().is_empty() {
        spawn_persist_assistant_output(
            context.clone(),
            assistant_text,
            &provenance,
            None,
            request_id.clone(),
        );
    }
    for call in tool_calls {
        let args = parse_tool_arguments(&call.function.arguments);
        let approval_required = preauthorized_call_id != Some(call.id.as_str())
            && provider_tool_requires_approval(&call.function.name);
        spawn_persist_model_tool_request(
            context.clone(),
            CanonicalToolRequestRecord::new(
                call.function.name.clone(),
                call.id.clone(),
                request_id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
                None,
                args,
                approval_required,
                native_policy_reason(approval_required),
                "native_runtime".to_string(),
            ),
            &provenance,
        );
    }
}

/// Persist a completed browser web chat turn after deferred step writes.
pub fn spawn_persist_web_chat_turn(
    pool: PgPool,
    bear_id: Uuid,
    user_id: i32,
    conversation_id: String,
    session_id: String,
    request_id: String,
    messages: &[ChatMessage],
    from_index: usize,
) {
    if from_index >= messages.len() {
        return;
    }
    let provenance = ConversationEventProvenance::client_session(session_id.clone());
    let context = canonical_persistence_context(
        pool,
        bear_id,
        Some(user_id),
        conversation_id,
        Some(session_id.clone()),
        Some(request_id.clone()),
        session_id,
        false,
    );
    for message in messages.iter().skip(from_index) {
        match message.role.as_str() {
            "assistant" => {
                // Browser-visible assistant text is persisted by the SSE proxy from streamed
                // `assistant_delta` events (including interrupted turns). This path only records
                // tool-call metadata for the agent loop.
                if let Some(calls) = &message.tool_calls {
                    for call in calls {
                        let args = parse_tool_arguments(&call.function.arguments);
                        let approval_required =
                            provider_tool_requires_approval(&call.function.name);
                        spawn_persist_model_tool_request(
                            context.clone(),
                            CanonicalToolRequestRecord::new(
                                call.function.name.clone(),
                                call.id.clone(),
                                request_id.clone(),
                                None,
                                args,
                                approval_required,
                                native_policy_reason(approval_required),
                                "native_web_chat".to_string(),
                            ),
                            &provenance,
                        );
                    }
                }
            }
            "tool" => {
                let Some(tool_call_id) = message.tool_call_id.clone() else {
                    continue;
                };
                let status = tool_result_persistence_status(message.content.as_deref());
                let persisted_content = if is_incomplete_tool_result(message.content.as_deref()) {
                    None
                } else {
                    message.content.clone()
                };
                spawn_persist_model_tool_result(
                    context.clone(),
                    CanonicalToolResultRecord::new(
                        message.name.clone(),
                        tool_call_id,
                        None,
                        status,
                        persisted_content,
                        Value::Null,
                        serde_json::json!({
                            "component": "den.web_chat",
                            "phase": "server_side_tool_result",
                        }),
                        Some(request_id.clone()),
                    ),
                    &provenance,
                );
            }
            _ => {}
        }
    }
}

fn spawn_persist_incomplete_tool_results(
    context: den_service::conversation::events::ConversationPersistenceContext,
    provenance: &ConversationEventProvenance,
    request_id: Option<String>,
    tool_calls: &[ChatToolCall],
    reason: &str,
    phase: &str,
) {
    for call in tool_calls {
        spawn_persist_model_tool_result(
            context.clone(),
            CanonicalToolResultRecord::new(
                Some(call.function.name.clone()),
                call.id.clone(),
                None,
                den_core::tools::result_compaction::ToolResultStatus::Incomplete,
                None,
                Value::Null,
                serde_json::json!({
                    "component": "den.agent_loop",
                    "phase": phase,
                    "reason": reason,
                    "tool_name": call.function.name,
                }),
                request_id.clone(),
            ),
            provenance,
        );
    }
}

/// Persist a web chat turn that ended before all tool calls completed.
pub fn spawn_persist_web_chat_interrupted_turn(
    pool: PgPool,
    bear_id: Uuid,
    user_id: i32,
    conversation_id: String,
    session_id: String,
    request_id: String,
    messages: &[ChatMessage],
    from_index: usize,
    reason: &str,
) {
    if from_index >= messages.len() {
        return;
    }
    let provenance = ConversationEventProvenance::client_session(session_id.clone());
    let context = canonical_persistence_context(
        pool.clone(),
        bear_id,
        Some(user_id),
        conversation_id,
        Some(session_id.clone()),
        Some(request_id.clone()),
        session_id,
        false,
    );
    spawn_persist_web_chat_turn(
        pool,
        bear_id,
        user_id,
        context.external_conversation_id.clone(),
        provenance.scope_id.clone(),
        request_id.clone(),
        messages,
        from_index,
    );
    let pending = pending_tool_calls(&messages[from_index..]);
    spawn_persist_incomplete_tool_results(
        context,
        &provenance,
        Some(request_id),
        &pending,
        reason,
        "web_chat_interrupted_turn",
    );
}

pub fn spawn_persist_abandoned_native_tool_results(
    pool: PgPool,
    bear_id: Uuid,
    user_id: Option<i32>,
    conversation_id: String,
    client_session_id: String,
    request_id: Option<String>,
    tool_calls: &[ChatToolCall],
    reason: &str,
) {
    if tool_calls.is_empty() {
        return;
    }
    let provenance = ConversationEventProvenance::client_session(client_session_id.clone());
    let context = canonical_persistence_context(
        pool,
        bear_id,
        user_id,
        conversation_id,
        Some(client_session_id.clone()),
        request_id.clone(),
        client_session_id,
        false,
    );
    for call in tool_calls {
        spawn_persist_model_tool_result(
            context.clone(),
            CanonicalToolResultRecord::new(
                Some(call.function.name.clone()),
                call.id.clone(),
                None,
                den_core::tools::result_compaction::ToolResultStatus::Error,
                Some(format!(
                    "error: native turn ended before tool execution ({reason})"
                )),
                Value::Null,
                serde_json::json!({
                    "component": "den.agent_loop",
                    "phase": "native_turn_terminal_settlement",
                    "reason": reason,
                    "tool_name": call.function.name,
                }),
                request_id.clone(),
            ),
            &provenance,
        );
    }
}

pub fn spawn_persist_incomplete_acp_tool_results(
    pool: PgPool,
    bear_id: Uuid,
    user_id: Option<i32>,
    conversation_id: String,
    client_session_id: String,
    request_id: Option<String>,
    tool_calls: &[ChatToolCall],
    reason: &str,
) {
    if tool_calls.is_empty() {
        return;
    }
    let provenance = ConversationEventProvenance::client_session(client_session_id.clone());
    let context = canonical_persistence_context(
        pool,
        bear_id,
        user_id,
        conversation_id,
        Some(client_session_id.clone()),
        request_id.clone(),
        client_session_id,
        false,
    );
    spawn_persist_incomplete_tool_results(
        context,
        &provenance,
        request_id,
        tool_calls,
        reason,
        "client_interrupted_turn",
    );
}
