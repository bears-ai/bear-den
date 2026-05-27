use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{types::AdapterContract, AcpSessionHttp};

#[derive(Debug, Deserialize)]
pub(super) struct AcpSetModeRequest {
    pub(super) mode: String,
    #[serde(default)]
    pub(super) reason: Option<String>,
    #[serde(default)]
    pub(super) adapter_contract: Option<AdapterContract>,
}

#[derive(Debug, Serialize)]
pub(super) struct AcpSetModeResponse {
    pub(super) requested_mode: String,
    pub(super) effective_mode: String,
    pub(super) session_policy: serde_json::Value,
    pub(super) workflow_state: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) plan_mode: Option<serde_json::Value>,
    pub(super) message: String,
}

#[derive(Debug, Deserialize)]
pub struct AcpPromptRequest {
    pub message: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub client_capabilities: serde_json::Value,
    #[serde(default)]
    pub client_context: serde_json::Value,
    /// Adapter-local mode selected before Den has necessarily persisted the ACP
    /// session binding. Den treats this as initial intent for new sessions only;
    /// existing sessions continue to use Den's stored current_mode/plan state.
    #[serde(default)]
    pub requested_mode: Option<String>,
    #[serde(default)]
    pub adapter_contract: Option<AdapterContract>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AcpToolResultResponse {
    pub(super) accepted: bool,
    pub(super) reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) settlement: Option<String>,
    pub(super) turn_id: Option<String>,
    pub(super) tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) diagnostic: Option<serde_json::Value>,
}

#[cfg(test)]
impl AcpToolResultResponse {
    pub(super) fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("AcpToolResultResponse serializes")
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct AcpPermissionDecisionRequest {
    pub(super) decision: String,
    #[serde(default)]
    pub(super) plan_mode_id: Option<Uuid>,
    #[serde(default)]
    pub(super) adapter_contract: Option<AdapterContract>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AcpAdapterEnvironmentRequest {
    pub(super) environment: serde_json::Value,
    #[serde(default)]
    pub(super) conversation_title: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) adapter_contract: Option<AdapterContract>,
}

#[derive(Debug, Serialize)]
pub(super) struct AcpPermissionDecisionResponse {
    pub(super) accepted: bool,
    pub(super) reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) local_tool_request: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct AcpCloseSessionResponse {
    pub(super) ok: bool,
    pub(super) archived: bool,
    pub(super) conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub(super) unwedged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) workflow_state: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AcpErrorResponse {
    pub(super) error: String,
    pub(super) error_code: &'static str,
    pub(super) request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) adapter_contract_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) minimum_adapter_contract_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) current_adapter_contract_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) maximum_adapter_contract_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) suggested_action: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AcpConversationsQuery {
    #[serde(default)]
    pub(super) include_archived: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AcpConversationRow {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) last_message_at: Option<String>,
    pub(super) archived: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct AcpConversationsResponse {
    pub(super) conversations: Vec<AcpConversationRow>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AcpConversationHistoryQuery {
    #[serde(default)]
    pub(super) before: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AcpConversationHistoryMessage {
    pub(super) id: Option<String>,
    pub(super) role: String,
    pub(super) text: String,
    pub(super) created_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct AcpConversationHistoryResponse {
    pub(super) messages: Vec<AcpConversationHistoryMessage>,
    pub(super) has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) next_before: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AcpSessionsListQuery {
    #[serde(default)]
    pub(super) include_closed: bool,
    #[serde(default)]
    pub(super) cwd: Option<String>,
    #[serde(default)]
    pub(super) cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct AcpSessionsListHttpResponse {
    pub(super) sessions: Vec<AcpSessionHttp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) next_cursor: Option<String>,
}
