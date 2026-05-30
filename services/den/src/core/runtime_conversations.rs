use std::cmp::Ordering;

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
    pub limit: usize,
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

pub fn value_has_true_flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(|x| x.as_bool()) == Some(true)
        || v.get(key).and_then(|x| x.as_str()) == Some("true")
}

fn object_marks_archived(v: &Value) -> bool {
    value_has_true_flag(v, "archived")
        || value_has_true_flag(v, "is_archived")
        || value_has_true_flag(v, "deleted")
        || value_has_true_flag(v, "hidden")
        || v.get("archived_at").is_some_and(|x| !x.is_null())
        || v.get("status")
            .and_then(|x| x.as_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("archived"))
}

pub fn runtime_conversation_is_archived(v: &Value) -> bool {
    object_marks_archived(v)
        || v.get("metadata").is_some_and(object_marks_archived)
        || v.get("attributes").is_some_and(object_marks_archived)
        || v.get("tags")
            .and_then(|x| x.as_array())
            .is_some_and(|tags| {
                tags.iter().any(|tag| {
                    tag.as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case("archived"))
                })
            })
}

pub fn cmp_runtime_conversation_row_newest_first(
    a: &RuntimeConversationRow,
    b: &RuntimeConversationRow,
) -> Ordering {
    match (&a.last_message_at, &b.last_message_at) {
        (Some(al), Some(bl)) => bl.cmp(al),
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
        (Some(_), None) => Ordering::Less,
    }
}

pub fn runtime_messages_top_array(value: &Value) -> &[Value] {
    if let Some(a) = value.as_array() {
        return a.as_slice();
    }
    if let Some(a) = value.get("messages").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = value.get("data").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = value.get("items").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    &[]
}

pub fn summarize_runtime_messages(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let messages = runtime_messages_top_array(value);
    if messages.is_empty() {
        return Vec::new();
    }
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

pub fn runtime_conversations_top_array(value: &Value) -> &[Value] {
    if let Some(a) = value.as_array() {
        return a.as_slice();
    }
    if let Some(a) = value.get("conversations").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = value.get("data").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = value.get("items").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    &[]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuntimeSemanticGroupKind {
    UserTurn,
    AssistantReply,
    ToolInteraction,
    ApprovalInteraction,
    WorkflowUpdate,
    ArtifactUpdate,
    PriorCompactionArtifact,
    SystemEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSemanticGroup {
    pub kind: RuntimeSemanticGroupKind,
    pub start_message_id: Option<String>,
    pub end_message_id: Option<String>,
    pub message_count: usize,
    pub protected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuntimeCompactionArtifactKind {
    IterativeSummary,
    CollapsedToolBundle,
    StructuredWorkflowSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCompactionArtifactRef {
    pub artifact_id: String,
    pub kind: RuntimeCompactionArtifactKind,
    pub source_group_start: usize,
    pub source_group_end: usize,
    pub policy_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuntimeCompactionTriggerKind {
    TokenPressure,
    SemanticGroupCount,
    Manual,
    ModelSafetyMargin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCompactionBoundary {
    pub retained_group_count: usize,
    pub compacted_group_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runtime_conversation_is_archived_detects_top_level_and_nested_flags() {
        assert!(runtime_conversation_is_archived(&json!({"archived": true})));
        assert!(runtime_conversation_is_archived(&json!({"metadata": {"status": "archived"}})));
        assert!(runtime_conversation_is_archived(&json!({"attributes": {"hidden": "true"}})));
        assert!(runtime_conversation_is_archived(&json!({"tags": ["active", "archived"]})));
        assert!(!runtime_conversation_is_archived(&json!({"status": "active"})));
    }

    #[test]
    fn summarize_runtime_messages_prefers_recent_nonempty_messages() {
        let summary = summarize_runtime_messages(Some(&json!({
            "messages": [
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "   "},
                {"role": "assistant", "content": "second"}
            ]
        })));
        assert_eq!(summary, vec!["assistant: second", "user: first"]);
    }

    #[test]
    fn semantic_group_and_compaction_types_serialize_stably() {
        let group = RuntimeSemanticGroup {
            kind: RuntimeSemanticGroupKind::ToolInteraction,
            start_message_id: Some("m1".into()),
            end_message_id: Some("m3".into()),
            message_count: 3,
            protected: true,
        };
        let artifact = RuntimeCompactionArtifactRef {
            artifact_id: "artifact-1".into(),
            kind: RuntimeCompactionArtifactKind::IterativeSummary,
            source_group_start: 0,
            source_group_end: 4,
            policy_version: "v1".into(),
        };
        let trigger = RuntimeCompactionTriggerKind::ModelSafetyMargin;
        let boundary = RuntimeCompactionBoundary {
            retained_group_count: 5,
            compacted_group_count: 12,
        };

        assert_eq!(serde_json::to_value(&group).unwrap()["kind"], "ToolInteraction");
        assert_eq!(serde_json::to_value(&artifact).unwrap()["kind"], "IterativeSummary");
        assert_eq!(serde_json::to_value(&trigger).unwrap(), "ModelSafetyMargin");
        assert_eq!(serde_json::to_value(&boundary).unwrap()["retained_group_count"], 5);
    }
}
