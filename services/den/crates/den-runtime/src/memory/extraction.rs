use std::collections::HashSet;

use den_core::{config::Config, DenError};
use den_memory::MemoryStoreManager;
use den_service::{bears::RuntimeContextLabel, memory_proposals::CreateMemoryProposal};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::create_proposal;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExtractionBundle {
    pub source_kind: String,
    pub source_ref: String,
    pub bear_id: Uuid,
    pub conversation_id: Option<String>,
    pub session_id: Option<String>,
    pub compaction: Option<MemoryExtractionCompactionContext>,
    pub messages: Vec<MemoryExtractionMessage>,
    pub artifacts: Vec<MemoryExtractionArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExtractionCompactionContext {
    pub artifact_id: Option<String>,
    pub policy_version: Option<String>,
    pub source_message_start_seq: Option<i64>,
    pub source_message_end_seq: Option<i64>,
    pub hints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExtractionMessage {
    pub id: String,
    pub seq: Option<i64>,
    pub role: String,
    pub content: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExtractionArtifact {
    pub id: String,
    pub kind: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryExtractionResult {
    pub candidates: Vec<MemoryExtractionCandidate>,
    pub discarded: Vec<MemoryExtractionDiscard>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryExtractionCandidate {
    pub kind: String,
    pub content: String,
    pub rationale: String,
    pub source_message_ids: Vec<String>,
    pub source_artifact_ids: Vec<String>,
    pub confidence: f32,
    pub sensitivity: String,
    pub suggested_action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExtractionDiscard {
    pub source_message_ids: Vec<String>,
    pub source_artifact_ids: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryExtractionPipelineOutput {
    pub proposal_drafts: Vec<MemoryProposalDraft>,
    pub discarded: Vec<MemoryExtractionDiscard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryProposalDraft {
    pub title: String,
    pub proposed_content: String,
    pub summary: String,
    pub rationale: String,
    pub suggested_action: String,
    pub sensitivity: String,
    pub requires_human: bool,
    pub source_refs: serde_json::Value,
    pub refs: serde_json::Value,
}

pub trait MemoryExtractor {
    fn extract(&self, bundle: &MemoryExtractionBundle) -> Result<MemoryExtractionResult, DenError>;
}

pub fn run_memory_extraction(
    bundle: &MemoryExtractionBundle,
    extractor: &impl MemoryExtractor,
) -> Result<MemoryExtractionPipelineOutput, DenError> {
    validate_bundle(bundle)?;
    let raw = extractor.extract(bundle)?;
    Ok(validate_extraction_result(bundle, raw))
}

pub async fn create_proposals_from_extraction(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    source_profile: RuntimeContextLabel,
    source_agent_id: Option<String>,
    bundle: &MemoryExtractionBundle,
    output: &MemoryExtractionPipelineOutput,
) -> Result<Vec<Uuid>, DenError> {
    let mut proposal_ids = Vec::new();
    for draft in &output.proposal_drafts {
        let proposal = create_proposal(
            pool,
            config,
            stores,
            CreateMemoryProposal {
                bear_id: bundle.bear_id,
                source_profile,
                source_agent_id: source_agent_id.clone(),
                source_paths: Vec::new(),
                source_refs: draft.source_refs.clone(),
                suggested_action: &draft.suggested_action,
                target_ref: None,
                title: &draft.title,
                summary: &draft.summary,
                rationale: &draft.rationale,
                proposed_content: Some(&draft.proposed_content),
                proposed_patch: None,
                refs: draft.refs.clone(),
                sensitivity: &draft.sensitivity,
                requires_human: draft.requires_human,
                project_to_conversation: false,
            },
        )
        .await?;
        proposal_ids.push(proposal.id);
    }
    Ok(proposal_ids)
}

fn validate_bundle(bundle: &MemoryExtractionBundle) -> Result<(), DenError> {
    if bundle.source_kind.trim().is_empty() {
        return Err(DenError::ValidationError(
            "memory extraction source_kind is required".to_string(),
        ));
    }
    if bundle.source_ref.trim().is_empty() {
        return Err(DenError::ValidationError(
            "memory extraction source_ref is required".to_string(),
        ));
    }
    Ok(())
}

fn validate_extraction_result(
    bundle: &MemoryExtractionBundle,
    raw: MemoryExtractionResult,
) -> MemoryExtractionPipelineOutput {
    let message_ids: HashSet<&str> = bundle
        .messages
        .iter()
        .map(|message| message.id.as_str())
        .collect();
    let artifact_ids: HashSet<&str> = bundle
        .artifacts
        .iter()
        .map(|artifact| artifact.id.as_str())
        .collect();
    let user_message_ids: HashSet<&str> = bundle
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .map(|message| message.id.as_str())
        .collect();

    let mut proposal_drafts = Vec::new();
    let mut discarded = raw.discarded;

    for candidate in raw.candidates {
        if let Some(reason) =
            invalid_candidate_reason(&candidate, &message_ids, &artifact_ids, &user_message_ids)
        {
            discarded.push(MemoryExtractionDiscard {
                source_message_ids: candidate.source_message_ids,
                source_artifact_ids: candidate.source_artifact_ids,
                reason,
            });
            continue;
        }
        proposal_drafts.push(proposal_draft_for_candidate(bundle, candidate));
    }

    for draft in &mut proposal_drafts {
        draft.refs["discarded"] = serde_json::json!(discarded);
    }

    MemoryExtractionPipelineOutput {
        proposal_drafts,
        discarded,
    }
}

fn invalid_candidate_reason(
    candidate: &MemoryExtractionCandidate,
    message_ids: &HashSet<&str>,
    artifact_ids: &HashSet<&str>,
    user_message_ids: &HashSet<&str>,
) -> Option<String> {
    if candidate.content.trim().is_empty() {
        return Some("invalid_candidate:empty_content".to_string());
    }
    if !matches!(
        candidate.kind.as_str(),
        "preference" | "decision" | "fact" | "constraint" | "lesson"
    ) {
        return Some("invalid_candidate:unknown_kind".to_string());
    }
    if !matches!(
        candidate.sensitivity.as_str(),
        "normal" | "person" | "secret_risk" | "external_untrusted"
    ) {
        return Some("invalid_candidate:unknown_sensitivity".to_string());
    }
    if !matches!(
        candidate.suggested_action.as_str(),
        "retain_profile_local" | "human_review" | "discard"
    ) {
        return Some("invalid_candidate:unknown_suggested_action".to_string());
    }
    if candidate.source_message_ids.is_empty() && candidate.source_artifact_ids.is_empty() {
        return Some("invalid_candidate:missing_evidence".to_string());
    }
    if candidate
        .source_message_ids
        .iter()
        .any(|id| !message_ids.contains(id.as_str()))
    {
        return Some("invalid_candidate:unknown_message_ref".to_string());
    }
    if candidate
        .source_artifact_ids
        .iter()
        .any(|id| !artifact_ids.contains(id.as_str()))
    {
        return Some("invalid_candidate:unknown_artifact_ref".to_string());
    }
    if matches!(
        candidate.kind.as_str(),
        "preference" | "decision" | "constraint"
    ) && !candidate
        .source_message_ids
        .iter()
        .any(|id| user_message_ids.contains(id.as_str()))
    {
        return Some("invalid_candidate:missing_user_evidence".to_string());
    }
    None
}

fn proposal_draft_for_candidate(
    bundle: &MemoryExtractionBundle,
    candidate: MemoryExtractionCandidate,
) -> MemoryProposalDraft {
    let requires_human =
        candidate.sensitivity != "normal" || candidate.suggested_action == "human_review";
    let title = format!(
        "Memory extraction {}: {}",
        candidate.kind,
        truncate_chars(candidate.content.trim(), 80)
    );
    let source_refs = serde_json::json!({
        "source": "memory_extraction",
        "source_kind": bundle.source_kind,
        "source_ref": bundle.source_ref,
        "conversation_id": bundle.conversation_id,
        "session_id": bundle.session_id,
        "compaction": bundle.compaction,
        "candidate_kind": candidate.kind,
        "source_message_ids": candidate.source_message_ids,
        "source_artifact_ids": candidate.source_artifact_ids,
    });
    let refs = serde_json::json!({
        "memory_extraction": true,
        "discarded": [],
        "quality": {
            "confidence": candidate.confidence,
            "detector": "memory-extraction-contract-v0",
            "adr": "ADR-0041"
        }
    });
    MemoryProposalDraft {
        title,
        proposed_content: candidate.content.trim().to_string(),
        summary: "Review memory extraction candidate for durable memory.".to_string(),
        rationale: candidate.rationale.trim().to_string(),
        suggested_action: candidate.suggested_action,
        sensitivity: candidate.sensitivity,
        requires_human,
        source_refs,
        refs,
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeExtractor;

    impl MemoryExtractor for FakeExtractor {
        fn extract(
            &self,
            _bundle: &MemoryExtractionBundle,
        ) -> Result<MemoryExtractionResult, DenError> {
            Ok(MemoryExtractionResult {
                candidates: vec![MemoryExtractionCandidate {
                    kind: "preference".to_string(),
                    content: "The user prefers SQLite-first storage for this project unless a specific exception applies."
                        .to_string(),
                    rationale: "This guides future storage decisions without rereading the session."
                        .to_string(),
                    source_message_ids: vec!["m1".to_string()],
                    source_artifact_ids: Vec::new(),
                    confidence: 0.92,
                    sensitivity: "normal".to_string(),
                    suggested_action: "retain_profile_local".to_string(),
                }],
                discarded: vec![
                    MemoryExtractionDiscard {
                        source_message_ids: vec!["m2".to_string()],
                        source_artifact_ids: Vec::new(),
                        reason: "assistant_only".to_string(),
                    },
                    MemoryExtractionDiscard {
                        source_message_ids: vec!["m3".to_string()],
                        source_artifact_ids: Vec::new(),
                        reason: "transient_followup".to_string(),
                    },
                ],
            })
        }
    }

    fn golden_bundle() -> MemoryExtractionBundle {
        MemoryExtractionBundle {
            source_kind: "pair_session".to_string(),
            source_ref: "session-golden".to_string(),
            bear_id: Uuid::nil(),
            conversation_id: Some("conv-golden".to_string()),
            session_id: Some("session-golden".to_string()),
            compaction: Some(MemoryExtractionCompactionContext {
                artifact_id: Some("artifact-golden".to_string()),
                policy_version: Some("test".to_string()),
                source_message_start_seq: Some(1),
                source_message_end_seq: Some(4),
                hints: vec!["possible_preference".to_string()],
            }),
            messages: vec![
                MemoryExtractionMessage {
                    id: "m1".to_string(),
                    seq: Some(1),
                    role: "user".to_string(),
                    content: "For this project, prefer SQLite-first storage unless there is a specific reason not to."
                        .to_string(),
                    created_at: None,
                },
                MemoryExtractionMessage {
                    id: "m2".to_string(),
                    seq: Some(2),
                    role: "assistant".to_string(),
                    content: "Makes sense, I’ll use SQLite-first.".to_string(),
                    created_at: None,
                },
                MemoryExtractionMessage {
                    id: "m3".to_string(),
                    seq: Some(3),
                    role: "user".to_string(),
                    content: "Also remind me tomorrow to check the auth logs.".to_string(),
                    created_at: None,
                },
                MemoryExtractionMessage {
                    id: "m4".to_string(),
                    seq: Some(4),
                    role: "assistant".to_string(),
                    content: "I can’t set reminders here, but we can add it to the task list."
                        .to_string(),
                    created_at: None,
                },
            ],
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn golden_fixture_maps_fake_extractor_candidate_to_one_proposal() {
        let output = run_memory_extraction(&golden_bundle(), &FakeExtractor).expect("extraction");

        assert_eq!(output.proposal_drafts.len(), 1);
        let proposal = &output.proposal_drafts[0];
        assert_eq!(
            proposal.proposed_content,
            "The user prefers SQLite-first storage for this project unless a specific exception applies."
        );
        assert!(!proposal.proposed_content.contains("For this project,"));
        assert_eq!(proposal.suggested_action, "retain_profile_local");
        assert!(!proposal.requires_human);
        assert_eq!(
            proposal.source_refs["source_message_ids"],
            serde_json::json!(["m1"])
        );
        assert_eq!(
            proposal.source_refs["compaction"]["hints"],
            serde_json::json!(["possible_preference"])
        );

        let discard_reasons: Vec<&str> = output
            .discarded
            .iter()
            .map(|discard| discard.reason.as_str())
            .collect();
        assert_eq!(
            discard_reasons,
            vec!["assistant_only", "transient_followup"]
        );
    }

    #[test]
    fn invalid_candidate_becomes_discard_instead_of_proposal() {
        struct BadExtractor;
        impl MemoryExtractor for BadExtractor {
            fn extract(
                &self,
                _bundle: &MemoryExtractionBundle,
            ) -> Result<MemoryExtractionResult, DenError> {
                Ok(MemoryExtractionResult {
                    candidates: vec![MemoryExtractionCandidate {
                        kind: "preference".to_string(),
                        content: "Assistant says SQLite-first is good.".to_string(),
                        rationale: "assistant-only claim".to_string(),
                        source_message_ids: vec!["m2".to_string()],
                        source_artifact_ids: Vec::new(),
                        confidence: 0.7,
                        sensitivity: "normal".to_string(),
                        suggested_action: "retain_profile_local".to_string(),
                    }],
                    discarded: Vec::new(),
                })
            }
        }

        let output = run_memory_extraction(&golden_bundle(), &BadExtractor).expect("extraction");

        assert!(output.proposal_drafts.is_empty());
        assert_eq!(output.discarded.len(), 1);
        assert_eq!(
            output.discarded[0].reason,
            "invalid_candidate:missing_user_evidence"
        );
    }
}
