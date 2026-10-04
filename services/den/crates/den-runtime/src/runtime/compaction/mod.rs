use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::compaction::grouping::semantic_group_from_runtime_message;
use crate::runtime::text::push_unique;
use crate::runtime_conversations::{
    RuntimeCompactionArtifactKind, RuntimeCompactionArtifactRef, RuntimeCompactionBoundary,
    RuntimeCompactionTriggerKind, RuntimeIterativeSummary, RuntimeSemanticGroup,
    RuntimeSemanticGroupKind,
};

pub mod artifact_store;
pub mod grouping;
pub mod lifecycle;
pub mod overflow;
pub mod policy;
pub mod render;
pub mod service;
pub mod summarize;

pub use grouping::{semantic_groups_from_conversation_messages, TranscriptGroupingRow};
pub use lifecycle::{
    enqueue_compaction_after_turn, load_compaction_context, on_turn_assemble_compaction,
    prepare_turn_compaction, render_compaction_prompt_context, run_compaction_job,
    TurnCompactionState, TurnCompactionTrigger,
};
pub use overflow::{den_error_indicates_context_overflow, is_context_length_overflow_message};
pub use policy::{
    compaction_policy_for_source, CompactionMode, CompactionSource, CompactionTiming,
};
pub use render::render_compacted_context_block;
pub use summarize::summarize_compacted_groups;

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
    /// Soft transcript char budget for token-pressure evaluation (Den uses chars, not tokens).
    pub max_transcript_chars: usize,
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
    messages
        .iter()
        .map(semantic_group_from_runtime_message)
        .collect()
}

pub fn choose_compaction_decision(
    groups: &[RuntimeSemanticGroup],
    trigger: RuntimeCompactionTriggerKind,
    policy: &RuntimeCompactionPolicy,
) -> Option<RuntimeCompactionDecision> {
    let force = matches!(
        trigger,
        RuntimeCompactionTriggerKind::ModelSafetyMargin | RuntimeCompactionTriggerKind::Manual
    );
    let conversation_review = matches!(trigger, RuntimeCompactionTriggerKind::ConversationReview);
    if groups.is_empty() {
        return None;
    }
    // ponytail: ConversationReview reuses the iterative-summary pass for memory discovery even
    // below prompt-pressure thresholds. Ceiling: it still uses compaction artifact storage and the
    // protected recent floor; upgrade path is a first-class conversation-review artifact/watermark.
    if !force && !conversation_review && groups.len() <= policy.max_groups_before_compaction {
        return None;
    }

    let eligible_end = groups
        .len()
        .saturating_sub(policy.protected_recent_group_count);
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
            "{}:{}:{}",
            group.kind.as_str(),
            group.start_message_id.as_deref().unwrap_or("start"),
            group.end_message_id.as_deref().unwrap_or("end")
        );
        push_unique(bucket, label);
        if group.protected {
            push_unique(
                &mut summary.important_constraints,
                format!("protected:{}", group.kind.as_str()),
            );
        }
    }

    summary
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
mod eval_tests;
#[cfg(test)]
mod tests;
