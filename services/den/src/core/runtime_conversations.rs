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

pub fn summarize_runtime_messages(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let messages = value
        .get("messages")
        .or_else(|| value.get("data"))
        .or_else(|| value.get("items"))
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array());
    let Some(messages) = messages else {
        return Vec::new();
    };
    messages
        .iter()
        .rev()
        .filter_map(|message| {
            let role = message
                .get("role")
                .or_else(|| message.get("message_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("message");
            let content = message
                .get("content")
                .and_then(|v| v.as_str())
                .or_else(|| message.get("text").and_then(|v| v.as_str()))
                .unwrap_or("")
                .trim();
            if content.is_empty() {
                None
            } else {
                Some(format!("{role}: {}", truncate_runtime_message(content, 300)))
            }
        })
        .take(20)
        .collect()
}

pub fn truncate_runtime_message(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
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
