use serde::{Deserialize, Serialize};

use crate::core::runtime_conversations::{
    RuntimeCompactionArtifactRef, RuntimeCompactionBoundary, RuntimeCompactionTriggerKind,
};

use super::runtime_compaction::{RuntimeCompactionDecision, RuntimeCompactionPolicy};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuntimeCompactionEventStatus {
    Applied,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCompactionEvent {
    pub conversation_id: String,
    pub trigger: RuntimeCompactionTriggerKind,
    pub policy_version: String,
    pub status: RuntimeCompactionEventStatus,
    pub boundary: Option<RuntimeCompactionBoundary>,
    pub source_group_start: Option<usize>,
    pub source_group_end: Option<usize>,
    pub artifact: Option<RuntimeCompactionArtifactRef>,
    pub diagnostic: Option<String>,
}

pub fn build_compaction_applied_event(
    conversation_id: impl Into<String>,
    decision: &RuntimeCompactionDecision,
    policy: &RuntimeCompactionPolicy,
    artifact: RuntimeCompactionArtifactRef,
) -> RuntimeCompactionEvent {
    RuntimeCompactionEvent {
        conversation_id: conversation_id.into(),
        trigger: decision.trigger.clone(),
        policy_version: policy.policy_version.clone(),
        status: RuntimeCompactionEventStatus::Applied,
        boundary: Some(decision.boundary.clone()),
        source_group_start: Some(decision.selected_group_start),
        source_group_end: Some(decision.selected_group_end),
        artifact: Some(artifact),
        diagnostic: None,
    }
}

pub fn build_compaction_skipped_event(
    conversation_id: impl Into<String>,
    trigger: RuntimeCompactionTriggerKind,
    policy: &RuntimeCompactionPolicy,
    diagnostic: impl Into<String>,
) -> RuntimeCompactionEvent {
    RuntimeCompactionEvent {
        conversation_id: conversation_id.into(),
        trigger,
        policy_version: policy.policy_version.clone(),
        status: RuntimeCompactionEventStatus::Skipped,
        boundary: None,
        source_group_start: None,
        source_group_end: None,
        artifact: None,
        diagnostic: Some(diagnostic.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runtime_compaction::{
        artifact_ref_from_decision, RuntimeCompactionDecision, RuntimeCompactionPolicy,
        RuntimeCompactionStrategy,
    };
    use crate::core::runtime_conversations::{
        RuntimeCompactionBoundary, RuntimeCompactionTriggerKind,
    };

    #[test]
    fn applied_event_carries_decision_boundary_and_artifact_provenance() {
        let policy = RuntimeCompactionPolicy {
            policy_version: "policy-v1".into(),
            protected_recent_group_count: 2,
            max_groups_before_compaction: 5,
        };
        let decision = RuntimeCompactionDecision {
            trigger: RuntimeCompactionTriggerKind::TokenPressure,
            strategy: RuntimeCompactionStrategy::UpdateIterativeSummary,
            boundary: RuntimeCompactionBoundary {
                retained_group_count: 4,
                compacted_group_count: 3,
            },
            selected_group_start: 1,
            selected_group_end: 3,
        };
        let artifact = artifact_ref_from_decision("artifact-7", &decision, &policy);

        let event = build_compaction_applied_event("conv-1", &decision, &policy, artifact.clone());
        assert_eq!(event.status, RuntimeCompactionEventStatus::Applied);
        assert_eq!(event.policy_version, "policy-v1");
        assert_eq!(event.source_group_start, Some(1));
        assert_eq!(event.source_group_end, Some(3));
        assert_eq!(event.boundary.unwrap().compacted_group_count, 3);
        assert_eq!(event.artifact.unwrap(), artifact);
    }

    #[test]
    fn skipped_event_records_trigger_and_diagnostic_without_artifact() {
        let policy = RuntimeCompactionPolicy {
            policy_version: "policy-v1".into(),
            protected_recent_group_count: 2,
            max_groups_before_compaction: 5,
        };

        let event = build_compaction_skipped_event(
            "conv-2",
            RuntimeCompactionTriggerKind::Manual,
            &policy,
            "no eligible groups outside protected floors",
        );
        assert_eq!(event.status, RuntimeCompactionEventStatus::Skipped);
        assert_eq!(event.trigger, RuntimeCompactionTriggerKind::Manual);
        assert_eq!(
            event.diagnostic.as_deref(),
            Some("no eligible groups outside protected floors")
        );
        assert!(event.artifact.is_none());
        assert!(event.boundary.is_none());
    }
}
