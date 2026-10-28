//! Awaited tool-transcript settlement shared by controlled channels and browser turns.
use crate::{
    agent_loop::{
        is_incomplete_tool_result, pending_tool_calls, provider_tool_requires_approval,
        tool_result_persistence_status,
    },
    llm::ChatMessage,
};
use den_core::DenError;
use den_service::conversation::events::{
    canonical_persistence_context, persist_canonical_conversation_record,
    CanonicalConversationRecord, CanonicalToolRequestRecord, CanonicalToolResultRecord,
    ConversationEventProvenance,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) struct ToolTranscript<'a> {
    pub pool: &'a PgPool,
    pub bear: Uuid,
    pub user: i32,
    pub external: &'a str,
    pub session: &'a str,
    pub request: &'a str,
}

impl ToolTranscript<'_> {
    pub async fn persist(&self, messages: &[ChatMessage]) -> Result<(), DenError> {
        let provenance = ConversationEventProvenance::client_session(self.session.to_owned());
        let context = canonical_persistence_context(
            self.pool.clone(),
            self.bear,
            Some(self.user),
            self.external.to_owned(),
            Some(self.session.to_owned()),
            Some(self.request.to_owned()),
            self.session.to_owned(),
            false,
        );
        for message in messages {
            if let Some(calls) = &message.tool_calls {
                for call in calls {
                    // Malformed provider arguments are still evidence of an attempted call.
                    let arguments = serde_json::from_str(&call.function.arguments)
                        .unwrap_or_else(|_| Value::String(call.function.arguments.clone()));
                    let approval_required = provider_tool_requires_approval(&call.function.name);
                    persist_canonical_conversation_record(
                        &context,
                        &CanonicalConversationRecord::model_tool_request(
                            CanonicalToolRequestRecord::new(
                                call.function.name.clone(),
                                call.id.clone(),
                                self.request.to_owned(),
                                None,
                                arguments,
                                approval_required,
                                approval_required.then(|| "native runtime policy".into()),
                                "native_channel",
                            ),
                            &provenance,
                        ),
                    )
                    .await?;
                }
            }
            if let Some(call) = message.tool_call_id.as_ref() {
                let content = message
                    .content
                    .clone()
                    .filter(|_| !is_incomplete_tool_result(message.content.as_deref()));
                persist_canonical_conversation_record(
                    &context,
                    &CanonicalConversationRecord::model_tool_result(
                        CanonicalToolResultRecord::new(
                            message.name.clone(),
                            call.clone(),
                            None,
                            tool_result_persistence_status(message.content.as_deref()),
                            content,
                            Value::Null,
                            json!({"component":"native_channel"}),
                            Some(self.request.to_owned()),
                        ),
                        &provenance,
                    ),
                )
                .await?;
            }
        }
        for call in pending_tool_calls(messages) {
            persist_canonical_conversation_record(
                &context,
                &CanonicalConversationRecord::model_tool_result(
                    CanonicalToolResultRecord::new(
                        Some(call.function.name),
                        call.id,
                        None,
                        den_core::tools::result_compaction::ToolResultStatus::Incomplete,
                        None,
                        Value::Null,
                        json!({"component":"native_channel","outcome":"interrupted"}),
                        Some(self.request.to_owned()),
                    ),
                    &provenance,
                ),
            )
            .await?;
        }
        Ok(())
    }
}
