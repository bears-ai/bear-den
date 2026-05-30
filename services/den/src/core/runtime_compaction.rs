use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::runtime_conversations::{
    RuntimeCompactionArtifactKind, RuntimeCompactionArtifactRef, RuntimeCompactionBoundary,
    RuntimeCompactionTriggerKind, RuntimeIterativeSummary, RuntimeSemanticGroup,
    RuntimeSemanticGroupKind,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuntimeCompactionStrategy {
    CollapseToolBundles,
    UpdateIterativeSummary,
    EnforceRecencyWindow,
    TruncateBackstop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCompactionPolicy {
    pub policy_version: String,
    pub protected_recent_group_count: usize,
    pub max_groups_before_compaction: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCompactionDecision {
    pub trigger: RuntimeCompactionTriggerKind,
    pub strategy: RuntimeCompactionStrategy,
    pub boundary: RuntimeCompactionBoundary,
    pub selected_group_start: usize,
    pub selected_group_end: usize,
}

pub fn semantic_groups_from_runtime_messages(messages: &[Value]) -> Vec<RuntimeSemanticGroup> {
    let mut groups = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .or_else(|| message.get("message_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("message");
        let content = message
            .get("content")
            .and_then(|v| v.as_str())
            .or_else(|| message.get("text").and_then(|v| v.as_str()))
            .unwrap_or_default();
        let tool_call_id = message
            .get("tool_call_id")
            .or_else(|| message.get("id"))
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);

        let kind = if role.eq_ignore_ascii_case("tool")
            || message.get("tool_name").is_some()
            || message.get("tool_call_id").is_some()
        {
            RuntimeSemanticGroupKind::ToolInteraction
        } else if message.get("approval_request_id").is_some()
            || message.get("approvals").is_some()
            || content.contains("approval")
        {
            RuntimeSemanticGroupKind::ApprovalInteraction
        } else if content.contains("workflow_state") || content.contains("plan_mode") {
            RuntimeSemanticGroupKind::WorkflowUpdate
        } else if content.contains("artifact") || content.contains("file://") {
            RuntimeSemanticGroupKind::ArtifactUpdate
        } else if role.eq_ignore_ascii_case("user") {
            RuntimeSemanticGroupKind::UserTurn
        } else if role.eq_ignore_ascii_case("assistant") {
            RuntimeSemanticGroupKind::AssistantReply
        } else if role.eq_ignore_ascii_case("system") {
            RuntimeSemanticGroupKind::SystemEvent
        } else {
            RuntimeSemanticGroupKind::AssistantReply
        };

        let protected = matches!(
            kind,
            RuntimeSemanticGroupKind::ToolInteraction
                | RuntimeSemanticGroupKind::ApprovalInteraction
                | RuntimeSemanticGroupKind::WorkflowUpdate
        );

        groups.push(RuntimeSemanticGroup {
            kind,
            start_message_id: tool_call_id.clone(),
            end_message_id: tool_call_id,
            message_count: 1,
            protected,
        });
    }
    groups
}

pub fn choose_compaction_decision(
    groups: &[RuntimeSemanticGroup],
    trigger: RuntimeCompactionTriggerKind,
    policy: &RuntimeCompactionPolicy,
) -> Option<RuntimeCompactionDecision> {
    if groups.len() <= policy.max_groups_before_compaction {
        return None;
    }

    let eligible_end = groups.len().saturating_sub(policy.protected_recent_group_count);
    if eligible_end == 0 {
        return None;
    }

    let mut selected_start = None;
    let mut selected_end = None;

    for (idx, group) in groups[..eligible_end].iter().enumerate() {
        if group.protected {
            if selected_start.is_some() {
                break;
            }
            continue;
        }
        selected_start.get_or_insert(idx);
        selected_end = Some(idx);
    }

    let (selected_group_start, selected_group_end) = match (selected_start, selected_end) {
        (Some(start), Some(end)) if start <= end => (start, end),
        _ => return None,
    };

    let selected_slice = &groups[selected_group_start..=selected_group_end];
    let strategy = if selected_slice
        .iter()
        .all(|g| g.kind == RuntimeSemanticGroupKind::ToolInteraction)
    {
        RuntimeCompactionStrategy::CollapseToolBundles
    } else {
        RuntimeCompactionStrategy::UpdateIterativeSummary
    };

    Some(RuntimeCompactionDecision {
        trigger,
        strategy,
        boundary: RuntimeCompactionBoundary {
            retained_group_count: groups.len() - (selected_group_end - selected_group_start + 1),
            compacted_group_count: selected_group_end - selected_group_start + 1,
        },
        selected_group_start,
        selected_group_end,
    })
}

pub fn artifact_ref_from_decision(
    artifact_id: impl Into<String>,
    decision: &RuntimeCompactionDecision,
    policy: &RuntimeCompactionPolicy,
) -> RuntimeCompactionArtifactRef {
    RuntimeCompactionArtifactRef {
        artifact_id: artifact_id.into(),
        kind: match decision.strategy {
            RuntimeCompactionStrategy::CollapseToolBundles => {
                RuntimeCompactionArtifactKind::CollapsedToolBundle
            }
            RuntimeCompactionStrategy::UpdateIterativeSummary
            | RuntimeCompactionStrategy::EnforceRecencyWindow
            | RuntimeCompactionStrategy::TruncateBackstop => {
                RuntimeCompactionArtifactKind::IterativeSummary
            }
        },
        source_group_start: decision.selected_group_start,
        source_group_end: decision.selected_group_end,
        policy_version: policy.policy_version.clone(),
    }
}

pub fn merge_iterative_summary(
    prior: Option<&RuntimeIterativeSummary>,
    groups: &[RuntimeSemanticGroup],
) -> RuntimeIterativeSummary {
    let mut summary = prior.cloned().unwrap_or_default();

    for group in groups {
        let bucket = match group.kind {
            RuntimeSemanticGroupKind::UserTurn => &mut summary.active_user_goals,
            RuntimeSemanticGroupKind::AssistantReply => &mut summary.unresolved_followups,
            RuntimeSemanticGroupKind::ToolInteraction => &mut summary.artifact_refs,
            RuntimeSemanticGroupKind::ApprovalInteraction => &mut summary.decisions_made,
            RuntimeSemanticGroupKind::WorkflowUpdate => &mut summary.workflow_state_refs,
            RuntimeSemanticGroupKind::ArtifactUpdate => &mut summary.artifact_refs,
            RuntimeSemanticGroupKind::PriorCompactionArtifact => &mut summary.important_constraints,
            RuntimeSemanticGroupKind::SystemEvent => &mut summary.important_constraints,
        };

        let label = format!(
            "{:?}:{}:{}",
            group.kind,
            group.start_message_id.as_deref().unwrap_or("start"),
            group.end_message_id.as_deref().unwrap_or("end")
        );
        push_unique(bucket, label);
        if group.protected {
            push_unique(
                &mut summary.important_constraints,
                format!("protected:{:?}", group.kind),
            );
        }
    }

    summary
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeContextEnvelopeInput {
    pub active_instructions: Vec<String>,
    pub workflow_state: Vec<String>,
    pub recent_groups: Vec<RuntimeSemanticGroup>,
    pub compacted_summary: Option<RuntimeIterativeSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeContextEnvelope {
    pub instructions: Vec<String>,
    pub workflow_state: Vec<String>,
    pub recent_groups: Vec<RuntimeSemanticGroup>,
    pub compacted_context: Option<RuntimeIterativeSummary>,
}

pub fn build_runtime_context_envelope(
    input: RuntimeContextEnvelopeInput,
) -> RuntimeContextEnvelope {
    RuntimeContextEnvelope {
        instructions: input.active_instructions,
        workflow_state: input.workflow_state,
        recent_groups: input.recent_groups,
        compacted_context: input.compacted_summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn semantic_grouping_classifies_user_assistant_tool_and_approval_messages() {
        let groups = semantic_groups_from_runtime_messages(&[
            json!({"role": "user", "content": "please inspect this file"}),
            json!({"role": "assistant", "content": "I will inspect it"}),
            json!({"role": "tool", "tool_call_id": "call-1", "tool_name": "fs_read_text_file"}),
            json!({"role": "assistant", "approval_request_id": "approval-1", "content": "approval required"}),
            json!({"role": "assistant", "content": "workflow_state: submitted"}),
            json!({"role": "assistant", "content": "artifact saved to file:///tmp/output.md"}),
        ]);

        assert_eq!(groups[0].kind, RuntimeSemanticGroupKind::UserTurn);
        assert_eq!(groups[1].kind, RuntimeSemanticGroupKind::AssistantReply);
        assert_eq!(groups[2].kind, RuntimeSemanticGroupKind::ToolInteraction);
        assert!(groups[2].protected);
        assert_eq!(groups[3].kind, RuntimeSemanticGroupKind::ApprovalInteraction);
        assert!(groups[3].protected);
        assert_eq!(groups[4].kind, RuntimeSemanticGroupKind::WorkflowUpdate);
        assert_eq!(groups[5].kind, RuntimeSemanticGroupKind::ArtifactUpdate);
    }

    #[test]
    fn compaction_policy_skips_protected_groups_and_recent_tail() {
        let groups = vec![
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::UserTurn,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: false,
            },
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::AssistantReply,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: false,
            },
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::ApprovalInteraction,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: true,
            },
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::AssistantReply,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: false,
            },
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::UserTurn,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: false,
            },
        ];
        let policy = RuntimeCompactionPolicy {
            policy_version: "v1".into(),
            protected_recent_group_count: 2,
            max_groups_before_compaction: 3,
        };

        let decision = choose_compaction_decision(
            &groups,
            RuntimeCompactionTriggerKind::SemanticGroupCount,
            &policy,
        )
        .expect("decision");

        assert_eq!(decision.selected_group_start, 0);
        assert_eq!(decision.selected_group_end, 1);
        assert_eq!(decision.strategy, RuntimeCompactionStrategy::UpdateIterativeSummary);
        assert_eq!(decision.boundary.compacted_group_count, 2);
    }

    #[test]
    fn compaction_decision_returns_none_when_only_protected_or_recent_groups_remain() {
        let groups = vec![
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::ToolInteraction,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: true,
            },
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::ApprovalInteraction,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: true,
            },
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::UserTurn,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: false,
            },
        ];
        let policy = RuntimeCompactionPolicy {
            policy_version: "v1".into(),
            protected_recent_group_count: 1,
            max_groups_before_compaction: 2,
        };

        assert!(choose_compaction_decision(
            &groups,
            RuntimeCompactionTriggerKind::TokenPressure,
            &policy,
        )
        .is_none());
    }

    #[test]
    fn artifact_ref_carries_policy_version_and_source_range() {
        let decision = RuntimeCompactionDecision {
            trigger: RuntimeCompactionTriggerKind::Manual,
            strategy: RuntimeCompactionStrategy::CollapseToolBundles,
            boundary: RuntimeCompactionBoundary {
                retained_group_count: 4,
                compacted_group_count: 2,
            },
            selected_group_start: 1,
            selected_group_end: 2,
        };
        let policy = RuntimeCompactionPolicy {
            policy_version: "policy-7".into(),
            protected_recent_group_count: 2,
            max_groups_before_compaction: 5,
        };

        let artifact = artifact_ref_from_decision("artifact-1", &decision, &policy);
        assert_eq!(artifact.kind, RuntimeCompactionArtifactKind::CollapsedToolBundle);
        assert_eq!(artifact.source_group_start, 1);
        assert_eq!(artifact.source_group_end, 2);
        assert_eq!(artifact.policy_version, "policy-7");
    }

    #[test]
    fn iterative_summary_merge_accumulates_unique_entries_and_protected_markers() {
        let prior = RuntimeIterativeSummary {
            active_user_goals: vec!["UserTurn:start:end".into()],
            important_constraints: vec![],
            decisions_made: vec![],
            artifact_refs: vec![],
            workflow_state_refs: vec![],
            unresolved_followups: vec![],
        };
        let groups = vec![
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::UserTurn,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: false,
            },
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::WorkflowUpdate,
                start_message_id: Some("w1".into()),
                end_message_id: Some("w2".into()),
                message_count: 2,
                protected: true,
            },
            RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::ArtifactUpdate,
                start_message_id: Some("a1".into()),
                end_message_id: Some("a1".into()),
                message_count: 1,
                protected: false,
            },
        ];

        let merged = merge_iterative_summary(Some(&prior), &groups);
        assert_eq!(merged.active_user_goals.len(), 1);
        assert!(merged.workflow_state_refs.iter().any(|v| v.contains("WorkflowUpdate:w1:w2")));
        assert!(merged.artifact_refs.iter().any(|v| v.contains("ArtifactUpdate:a1:a1")));
        assert!(merged
            .important_constraints
            .iter()
            .any(|v| v == "protected:WorkflowUpdate"));
    }

    #[test]
    fn prompt_assembly_keeps_compacted_context_separate_from_recent_groups() {
        let envelope = build_runtime_context_envelope(RuntimeContextEnvelopeInput {
            active_instructions: vec!["system".into(), "developer".into()],
            workflow_state: vec!["plan:active".into()],
            recent_groups: vec![RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::AssistantReply,
                start_message_id: Some("m1".into()),
                end_message_id: Some("m1".into()),
                message_count: 1,
                protected: false,
            }],
            compacted_summary: Some(RuntimeIterativeSummary {
                active_user_goals: vec!["ship compaction".into()],
                important_constraints: vec!["do not compact approvals".into()],
                decisions_made: vec![],
                artifact_refs: vec![],
                workflow_state_refs: vec!["plan:active".into()],
                unresolved_followups: vec![],
            }),
        });

        assert_eq!(envelope.instructions, vec!["system", "developer"]);
        assert_eq!(envelope.workflow_state, vec!["plan:active"]);
        assert_eq!(envelope.recent_groups.len(), 1);
        assert!(envelope.compacted_context.is_some());
        assert_eq!(
            envelope.compacted_context.unwrap().important_constraints,
            vec!["do not compact approvals"]
        );
    }
}
