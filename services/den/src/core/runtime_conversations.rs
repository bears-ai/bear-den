use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::CustomError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConversationRow {
    pub id: String,
    pub title: String,
    pub last_message_at: Option<String>,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConversationSnapshot {
    pub active: Vec<RuntimeConversationRow>,
    pub all: Vec<RuntimeConversationRow>,
    pub archived_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimePendingApproval {
    pub tool_call_id: String,
    pub approval_request_id: Option<String>,
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeConversationListRequest {
    pub binding_id: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeConversationMessagesRequest {
    pub conversation_id: String,
    pub binding_id: Option<String>,
    pub limit: usize,
    pub before: Option<String>,
    pub ascending: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeApprovalRequest {
    pub conversation_id: String,
    pub binding_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RuntimeApprovalActionMode {
    InspectOnly,
    Deny,
}

#[derive(Debug, Clone)]
pub struct RuntimeApprovalActionRequest {
    pub conversation_id: String,
    pub binding_id: Option<String>,
    pub reason: String,
    pub mode: RuntimeApprovalActionMode,
}

#[allow(async_fn_in_trait)]
pub trait RuntimeConversationBackend {
    async fn list_conversations(
        &self,
        request: RuntimeConversationListRequest,
    ) -> Result<RuntimeConversationSnapshot, CustomError>;

    async fn list_messages(
        &self,
        request: RuntimeConversationMessagesRequest,
    ) -> Result<Value, CustomError>;

    async fn pending_approvals(
        &self,
        request: RuntimeApprovalRequest,
    ) -> Result<Vec<RuntimePendingApproval>, CustomError>;

    async fn apply_approval_action(
        &self,
        request: RuntimeApprovalActionRequest,
    ) -> Result<Vec<RuntimePendingApproval>, CustomError>;
}
