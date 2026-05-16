use serde_json::Value;

use crate::{core::letta::LettaClient, errors::CustomError};

/// Shared boundary for sending a `pair` role turn to Letta.
///
/// Invariant: `human_message` is the only content serialized into Letta `messages[].content`.
/// Channel/runtime context must flow through structured fields (`client_tools`, Den tool context,
/// session_info backing state, UI events, or explicit `override_system` replacement semantics), not
/// by concatenating text into the user message.
pub struct PairTurnRequest<'a> {
    pub conversation_id: &'a str,
    pub role_agent_id: &'a str,
    pub human_message: &'a str,
    pub client_tools: Option<Value>,
    pub stream_tokens: bool,
    pub override_system: Option<&'a str>,
    pub boundary: PairTurnBoundaryLog<'a>,
}

pub struct PairTurnBoundaryLog<'a> {
    pub request_id: &'a str,
    pub channel_family: &'a str,
    pub session_id: &'a str,
    pub runtime_context_len: usize,
}

impl PairTurnRequest<'_> {
    pub fn client_tools_count(&self) -> usize {
        self.client_tools
            .as_ref()
            .and_then(|tools| tools.as_array())
            .map(Vec::len)
            .unwrap_or(0)
    }
}

pub async fn post_pair_turn_messages_streaming(
    letta: &LettaClient,
    request: PairTurnRequest<'_>,
) -> Result<reqwest::Response, CustomError> {
    let override_system_present = request
        .override_system
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    tracing::info!(
        request_id = %request.boundary.request_id,
        channel_family = %request.boundary.channel_family,
        session_id = %request.boundary.session_id,
        letta_conversation_id = %request.conversation_id,
        role_agent_id = %request.role_agent_id,
        user_content_len = request.human_message.len(),
        user_content_has_system_reminder = request.human_message.contains("<system-reminder>")
            || request.human_message.contains("<system_reminder>"),
        user_content_has_workflow_scaffolding = request.human_message.contains("ACP workflow state")
            || request.human_message.contains("AUTHORITATIVE WORKFLOW STATE")
            || request.human_message.contains("Den workboard context")
            || request.human_message.contains("Trusted ACP session mode this turn"),
        runtime_context_len = request.boundary.runtime_context_len,
        runtime_context_sent_as_user_content = false,
        client_tools_count = request.client_tools_count(),
        override_system_present,
        "letta_outbound_message_boundary"
    );
    letta
        .post_conversation_messages_streaming(
            request.conversation_id,
            Some(request.role_agent_id),
            request.human_message,
            request.client_tools,
            request.stream_tokens,
            request.override_system,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pair_turn_request_counts_client_tools() {
        let request = PairTurnRequest {
            conversation_id: "conv-test",
            role_agent_id: "agent-test",
            human_message: "hello",
            client_tools: Some(json!([{ "name": "session_info" }, { "name": "memory_read" }])),
            stream_tokens: false,
            override_system: None,
            boundary: PairTurnBoundaryLog {
                request_id: "request-test",
                channel_family: "acp",
                session_id: "session-test",
                runtime_context_len: 42,
            },
        };
        assert_eq!(request.client_tools_count(), 2);
    }
}
