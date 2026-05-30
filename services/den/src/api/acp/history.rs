use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    core::{
        acp_letta_events::AcpGatewayEvent,
        acp_sessions,
        letta::sanitize_visible_transcript_text,
        runtime_compaction::{
            choose_compaction_decision, semantic_groups_from_runtime_messages,
            RuntimeCompactionDecision, RuntimeCompactionPolicy,
        },
        runtime_compaction_observability::{
            build_compaction_applied_event, build_compaction_skipped_event,
            RuntimeCompactionEvent,
        },
        runtime_conversations::{
            RuntimeCompactionTriggerKind, RuntimeIterativeSummary, RuntimeSemanticGroup,
        },
    },
    errors::CustomError,
};

use super::{
    format_acp_session_timestamp, AcpCompactionStatusResponse, AcpConversationHistoryMessage,
};

pub(crate) fn normalize_acp_conversation_id(raw: Option<&str>) -> Result<String, CustomError> {
    let s = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    if s == "default" {
        return Ok("default".to_string());
    }
    let ok = (s.starts_with("conv-") || s.starts_with("new-"))
        && s.len() > 8
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(s.to_string())
    } else {
        Err(CustomError::ValidationError(format!(
            "invalid conversation_id (expected 'default', a runtime conv- id, or a pending new- id): {s}"
        )))
    }
}

fn runtime_messages_top_array(v: &serde_json::Value) -> &[serde_json::Value] {
    if let Some(a) = v.as_array() {
        return a.as_slice();
    }
    if let Some(a) = v.get("messages").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = v.get("items").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    &[]
}

fn runtime_inner_for_acp_history(msg: &serde_json::Value) -> &serde_json::Value {
    match msg.get("contents") {
        Some(c) if c.get("message_type").is_some() => c,
        _ => msg,
    }
}

fn runtime_message_text(inner: &serde_json::Value) -> Option<String> {
    let content = inner.get("content")?;
    if let Some(s) = content.as_str() {
        let s = s.trim();
        return if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        };
    }
    if let Some(obj) = content.as_object() {
        if let Some(t) = obj.get("text").and_then(|x| x.as_str()) {
            let t = t.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    let parts = content.as_array()?;
    let mut out = String::new();
    for part in parts {
        if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
            out.push_str(t);
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn runtime_message_id_string(msg: &serde_json::Value) -> Option<String> {
    match msg.get("id")? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn runtime_message_created_at(msg: &serde_json::Value) -> Option<String> {
    msg.get("date")
        .or_else(|| msg.get("created_at"))
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

pub(crate) fn runtime_messages_for_compaction(
    body: &serde_json::Value,
) -> Vec<serde_json::Value> {
    runtime_messages_top_array(body)
        .iter()
        .map(runtime_inner_for_acp_history)
        .cloned()
        .collect()
}

pub(crate) fn runtime_semantic_groups_for_compaction(
    body: &serde_json::Value,
) -> Vec<RuntimeSemanticGroup> {
    let messages = runtime_messages_for_compaction(body);
    semantic_groups_from_runtime_messages(&messages)
}

pub(crate) fn runtime_iterative_summary_for_compaction(
    body: &serde_json::Value,
) -> RuntimeIterativeSummary {
    let groups = runtime_semantic_groups_for_compaction(body);
    build_iterative_summary_from_groups(&groups)
}

pub(crate) fn default_runtime_compaction_policy() -> RuntimeCompactionPolicy {
    RuntimeCompactionPolicy {
        policy_version: "acp-history-v1".to_string(),
        protected_recent_group_count: 3,
        max_groups_before_compaction: 6,
    }
}

pub(crate) fn runtime_compaction_decision_for_history(
    body: &serde_json::Value,
    trigger: RuntimeCompactionTriggerKind,
) -> Option<RuntimeCompactionDecision> {
    let groups = runtime_semantic_groups_for_compaction(body);
    let policy = default_runtime_compaction_policy();
    choose_compaction_decision(&groups, trigger, &policy)
}

pub(crate) fn runtime_compaction_event_for_history(
    conversation_id: &str,
    body: &serde_json::Value,
    trigger: RuntimeCompactionTriggerKind,
) -> RuntimeCompactionEvent {
    let policy = default_runtime_compaction_policy();
    match runtime_compaction_decision_for_history(body, trigger.clone()) {
        Some(decision) => {
            let artifact = crate::core::runtime_compaction::artifact_ref_from_decision(
                format!("{conversation_id}:{}-{}", decision.selected_group_start, decision.selected_group_end),
                &decision,
                &policy,
            );
            build_compaction_applied_event(conversation_id.to_string(), &decision, &policy, artifact)
        }
        None => build_compaction_skipped_event(
            conversation_id.to_string(),
            trigger,
            &policy,
            "no eligible history groups outside protected floors",
        ),
    }
}

fn build_iterative_summary_from_groups(groups: &[RuntimeSemanticGroup]) -> RuntimeIterativeSummary {
    let mut summary = RuntimeIterativeSummary::default();
    for group in groups {
        let label = format!(
            "{:?}:{}:{}",
            group.kind,
            group.start_message_id.as_deref().unwrap_or("start"),
            group.end_message_id.as_deref().unwrap_or("end")
        );
        match group.kind {
            crate::core::runtime_conversations::RuntimeSemanticGroupKind::UserTurn => {
                push_unique_summary_value(&mut summary.active_user_goals, label);
            }
            crate::core::runtime_conversations::RuntimeSemanticGroupKind::AssistantReply => {
                push_unique_summary_value(&mut summary.unresolved_followups, label);
            }
            crate::core::runtime_conversations::RuntimeSemanticGroupKind::ToolInteraction
            | crate::core::runtime_conversations::RuntimeSemanticGroupKind::ArtifactUpdate => {
                push_unique_summary_value(&mut summary.artifact_refs, label);
            }
            crate::core::runtime_conversations::RuntimeSemanticGroupKind::ApprovalInteraction => {
                push_unique_summary_value(&mut summary.decisions_made, label);
            }
            crate::core::runtime_conversations::RuntimeSemanticGroupKind::WorkflowUpdate => {
                push_unique_summary_value(&mut summary.workflow_state_refs, label);
            }
            crate::core::runtime_conversations::RuntimeSemanticGroupKind::PriorCompactionArtifact
            | crate::core::runtime_conversations::RuntimeSemanticGroupKind::SystemEvent => {
                push_unique_summary_value(&mut summary.important_constraints, label);
            }
        }
        if group.protected {
            push_unique_summary_value(
                &mut summary.important_constraints,
                format!("protected:{:?}", group.kind),
            );
        }
    }
    summary
}

fn push_unique_summary_value(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn runtime_user_message_role_is_human(inner: &serde_json::Value, msg: &serde_json::Value) -> bool {
    for v in [inner, msg] {
        let Some(role) = v.get("role").and_then(|x| x.as_str()) else {
            continue;
        };
        let r = role.trim();
        if r.eq_ignore_ascii_case("system") || r.eq_ignore_ascii_case("developer") {
            return false;
        }
    }
    true
}

pub(crate) fn map_compaction_status_for_history(
    conversation_id: &str,
    body: &serde_json::Value,
) -> AcpCompactionStatusResponse {
    let event = runtime_compaction_event_for_history(
        conversation_id,
        body,
        RuntimeCompactionTriggerKind::SemanticGroupCount,
    );
    let status = match event.status {
        crate::core::runtime_compaction_observability::RuntimeCompactionEventStatus::Applied => {
            "applied"
        }
        crate::core::runtime_compaction_observability::RuntimeCompactionEventStatus::Skipped => {
            "skipped"
        }
        crate::core::runtime_compaction_observability::RuntimeCompactionEventStatus::Failed => {
            "failed"
        }
    }
    .to_string();
    AcpCompactionStatusResponse {
        status,
        policy_version: event.policy_version,
        source_group_start: event.source_group_start,
        source_group_end: event.source_group_end,
        diagnostic: event.diagnostic,
        artifact: event.artifact.and_then(|artifact| serde_json::to_value(artifact).ok()),
    }
}

pub(super) fn map_acp_history_page(
    body: &serde_json::Value,
    page_limit: u32,
) -> (Vec<AcpConversationHistoryMessage>, bool, Option<String>) {
    let raw = runtime_messages_top_array(body);
    let has_more = raw.len() >= page_limit as usize;
    let next_before = raw.iter().filter_map(runtime_message_id_string).next_back();
    let mut rows = Vec::new();
    for msg in raw.iter().rev() {
        let inner = runtime_inner_for_acp_history(msg);
        let message_type = inner
            .get("message_type")
            .and_then(|x| x.as_str())
            .or_else(|| msg.get("message_type").and_then(|x| x.as_str()))
            .unwrap_or("");
        let role = match message_type {
            "user_message" => "user",
            "assistant_message" => "assistant",
            _ => continue,
        };
        if message_type == "user_message" && !runtime_user_message_role_is_human(inner, msg) {
            continue;
        }
        let Some(text) = runtime_message_text(inner).or_else(|| runtime_message_text(msg)) else {
            continue;
        };
        let text = sanitize_visible_transcript_text(&text);
        if text.trim().is_empty() {
            continue;
        }
        rows.push(AcpConversationHistoryMessage {
            id: runtime_message_id_string(msg),
            role: role.to_string(),
            text,
            created_at: runtime_message_created_at(msg),
        });
    }
    (rows, has_more, next_before)
}

pub(super) async fn pending_session_title_update_event(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    bear_slug: &str,
    acp_session_id: &str,
) -> Result<Option<AcpGatewayEvent>, CustomError> {
    let Some(session) =
        acp_sessions::find_for_user_bear_session(pool, user_id, bear_slug, acp_session_id).await?
    else {
        return Ok(None);
    };
    if let Some(event) = session_title_update_event_from_row(&session) {
        acp_sessions::mark_title_synced(pool, user_id, bear_id, acp_session_id).await?;
        Ok(Some(event))
    } else {
        Ok(None)
    }
}

pub(super) fn acp_auto_title_instruction(session: &acp_sessions::AcpSessionRow) -> Option<String> {
    let has_title = session
        .conversation_title
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if has_title {
        return None;
    }
    let has_conversation_binding = session
        .resolved_conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        || session.conversation_id.trim().starts_with("conv-")
        || session.conversation_id.trim().starts_with("new-");
    if !has_conversation_binding {
        return None;
    }
    Some(
        "This conversation is currently untitled. Once the main subject is clear enough to summarize in a short, specific title, proactively call `set_conversation_title` in that turn without waiting for the user to ask. Prefer doing this before or alongside your normal response when the topic first becomes clear. Do not title vague openings such as greetings when the subject is not yet clear, and do not automatically rename again after a title has been set unless the human asks for a rename or the existing title is clearly wrong.".to_string(),
    )
}

fn session_title_update_event_from_row(
    session: &acp_sessions::AcpSessionRow,
) -> Option<AcpGatewayEvent> {
    let title = session
        .conversation_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)?;
    let needs_sync = match (
        session.conversation_title_updated_at,
        session.conversation_title_synced_at,
    ) {
        (Some(updated), Some(synced)) => synced < updated,
        (Some(_), None) => true,
        _ => false,
    };
    needs_sync.then_some(AcpGatewayEvent::SessionInfoUpdate {
        title: Some(title),
        updated_at: session
            .conversation_title_updated_at
            .map(format_acp_session_timestamp),
        meta: None,
    })
}
