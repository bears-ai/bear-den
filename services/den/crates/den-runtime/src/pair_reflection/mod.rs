use den_core::{config::Config, DenError};
use den_memory::MemoryStoreManager;
use den_service::bears::RuntimeContextLabel;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    agent_loop::load_transcript_grouping_rows,
    memory::extraction::{
        create_proposals_from_extraction, run_memory_extraction, MemoryExtractionArtifact,
        MemoryExtractionBundle, MemoryExtractionCandidate, MemoryExtractionCompactionContext,
        MemoryExtractionDiscard, MemoryExtractionMessage, MemoryExtractionResult, MemoryExtractor,
    },
    reflection::conductor::{enqueue_memory_curate_for_proposals, ProposalEnqueueParams},
    runtime::compaction::artifact_store::{
        load_latest_iterative_summary, CompactionArtifactRecord,
    },
    runtime_conversations::RuntimeIterativeSummary,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairReflectionProposalOutput {
    pub created_proposal_ids: Vec<Uuid>,
    pub candidate_count: usize,
    pub discarded_count: usize,
    pub discarded_reasons: Vec<String>,
    pub dropped_followup_count: usize,
    pub skipped_reason: Option<&'static str>,
    pub source_message_start_seq: Option<i64>,
    pub source_message_end_seq: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PairReflectionExtraction {
    candidates: Vec<PairReflectionCandidate>,
    discarded: Vec<PairReflectionDiscard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PairReflectionCandidate {
    kind: &'static str,
    text: String,
    title: String,
    suggested_action: &'static str,
    target_ref: Option<&'static str>,
    sensitivity: &'static str,
    requires_human: bool,
    confidence: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PairReflectionDiscard {
    kind: &'static str,
    reason: &'static str,
    text: String,
}

struct PairSummaryExtractor<'a> {
    summary: &'a RuntimeIterativeSummary,
}

impl MemoryExtractor for PairSummaryExtractor<'_> {
    fn extract(&self, bundle: &MemoryExtractionBundle) -> Result<MemoryExtractionResult, DenError> {
        let source_message_ids = bundle
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        let extraction = extract_candidates_from_summary(self.summary);
        Ok(MemoryExtractionResult {
            candidates: extraction
                .candidates
                .into_iter()
                .map(|candidate| {
                    memory_candidate_from_pair_candidate(candidate, source_message_ids.clone())
                })
                .collect(),
            discarded: extraction
                .discarded
                .into_iter()
                .map(memory_discard_from_pair_discard)
                .collect(),
        })
    }
}

pub async fn create_pair_reflection_proposals_from_latest_summary(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    conversation_id: &str,
    session_id: &str,
) -> Result<PairReflectionProposalOutput, DenError> {
    let Some(artifact) = load_latest_iterative_summary(pool, bear_id, conversation_id).await?
    else {
        return Ok(PairReflectionProposalOutput {
            skipped_reason: Some("no_compaction_artifact"),
            ..PairReflectionProposalOutput::default()
        });
    };
    create_pair_reflection_proposals_for_artifact(
        pool,
        config,
        stores,
        bear_id,
        conversation_id,
        session_id,
        &artifact,
    )
    .await
}

async fn create_pair_reflection_proposals_for_artifact(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    conversation_id: &str,
    session_id: &str,
    artifact: &CompactionArtifactRecord,
) -> Result<PairReflectionProposalOutput, DenError> {
    let rows = load_transcript_grouping_rows(pool, bear_id, conversation_id).await?;
    let bundle = pair_reflection_bundle(bear_id, conversation_id, session_id, artifact, rows);
    let extraction_output = run_memory_extraction(
        &bundle,
        &PairSummaryExtractor {
            summary: &artifact.summary,
        },
    )?;
    let discarded_reasons = extraction_output
        .discarded
        .iter()
        .map(|discard| discard.reason.clone())
        .collect::<Vec<_>>();
    let candidate_count = extraction_output.proposal_drafts.len();
    let discarded_count = extraction_output.discarded.len();
    let created_proposal_ids = create_proposals_from_extraction(
        pool,
        config,
        stores,
        RuntimeContextLabel::ArmatureConversation,
        Some("pair_reflection".to_string()),
        &bundle,
        &extraction_output,
    )
    .await?;
    let output = PairReflectionProposalOutput {
        created_proposal_ids,
        candidate_count,
        discarded_count,
        discarded_reasons,
        dropped_followup_count: artifact.summary.unresolved_followups.len(),
        skipped_reason: None,
        source_message_start_seq: Some(artifact.source_message_start_seq),
        source_message_end_seq: Some(artifact.source_message_end_seq),
    };

    if !output.created_proposal_ids.is_empty() {
        let _ = enqueue_memory_curate_for_proposals(
            pool,
            ProposalEnqueueParams {
                bear_id,
                binding_id: Some("pair_reflection"),
                conversation_id: Some(conversation_id),
                conversation_key: Some(conversation_id),
                conversation_date: None,
                trigger: "pair_reflection",
                proposal_ids: output.created_proposal_ids.clone(),
            },
        )
        .await?;
    }

    Ok(output)
}

fn pair_reflection_bundle(
    bear_id: Uuid,
    conversation_id: &str,
    session_id: &str,
    artifact: &CompactionArtifactRecord,
    rows: Vec<crate::runtime::compaction::TranscriptGroupingRow>,
) -> MemoryExtractionBundle {
    let messages = rows
        .into_iter()
        .filter(|row| {
            let seq = row.sequence_no.unwrap_or_default();
            seq >= artifact.source_message_start_seq
                && seq <= artifact.source_message_end_seq
                && matches!(row.message_type.as_str(), "user" | "assistant")
                && !row.content_text.trim().is_empty()
        })
        .map(|row| MemoryExtractionMessage {
            id: row
                .message_id
                .unwrap_or_else(|| format!("seq-{}", row.sequence_no.unwrap_or_default())),
            seq: row.sequence_no,
            role: row.message_type,
            content: row.content_text,
            created_at: None,
        })
        .collect();

    MemoryExtractionBundle {
        source_kind: "pair_reflection".to_string(),
        source_ref: artifact.artifact_id.to_string(),
        bear_id,
        conversation_id: Some(conversation_id.to_string()),
        session_id: Some(session_id.to_string()),
        compaction: Some(MemoryExtractionCompactionContext {
            artifact_id: Some(artifact.artifact_id.to_string()),
            policy_version: Some(artifact.policy_version.clone()),
            source_message_start_seq: Some(artifact.source_message_start_seq),
            source_message_end_seq: Some(artifact.source_message_end_seq),
            hints: compaction_hints(&artifact.summary),
        }),
        messages,
        artifacts: vec![MemoryExtractionArtifact {
            id: artifact.artifact_id.to_string(),
            kind: "iterative_summary".to_string(),
            content: serde_json::to_string(&artifact.summary).unwrap_or_default(),
        }],
    }
}

fn compaction_hints(summary: &RuntimeIterativeSummary) -> Vec<String> {
    let mut hints = Vec::new();
    if !summary.decisions_made.is_empty() {
        hints.push("possible_decision".to_string());
    }
    if !summary.important_constraints.is_empty() {
        hints.push("possible_constraint".to_string());
    }
    if !summary.active_user_goals.is_empty() {
        hints.push("possible_user_goal".to_string());
    }
    hints
}

fn memory_candidate_from_pair_candidate(
    candidate: PairReflectionCandidate,
    source_message_ids: Vec<String>,
) -> MemoryExtractionCandidate {
    MemoryExtractionCandidate {
        kind: pair_candidate_kind(candidate.kind).to_string(),
        content: candidate.text,
        rationale: "Extracted from pair reflection over source transcript evidence.".to_string(),
        source_message_ids,
        source_artifact_ids: Vec::new(),
        confidence: match candidate.confidence {
            "high" => 0.85,
            "medium" => 0.65,
            _ => 0.5,
        },
        sensitivity: candidate.sensitivity.to_string(),
        suggested_action: candidate.suggested_action.to_string(),
    }
}

fn memory_discard_from_pair_discard(discard: PairReflectionDiscard) -> MemoryExtractionDiscard {
    MemoryExtractionDiscard {
        source_message_ids: Vec::new(),
        source_artifact_ids: Vec::new(),
        reason: format!("{}:{}", discard.kind, discard.reason),
    }
}

fn pair_candidate_kind(kind: &str) -> &str {
    match kind {
        "goal" => "preference",
        other => other,
    }
}

fn extract_candidates_from_summary(summary: &RuntimeIterativeSummary) -> PairReflectionExtraction {
    let mut candidates = Vec::new();
    let mut discarded = Vec::new();
    for value in &summary.decisions_made {
        push_candidate(
            &mut candidates,
            "decision",
            value,
            "Pair reflection decision",
        );
    }
    for value in &summary.important_constraints {
        push_candidate(
            &mut candidates,
            "constraint",
            value,
            "Pair reflection constraint",
        );
    }
    for value in &summary.active_user_goals {
        push_goal_candidate(&mut candidates, &mut discarded, value);
    }
    for value in &summary.workflow_state_refs {
        push_discard(
            &mut discarded,
            "workflow_state",
            "transient_workflow_state",
            value,
        );
    }
    for value in &summary.artifact_refs {
        push_discard(
            &mut discarded,
            "artifact",
            "reference_without_semantic_claim",
            value,
        );
    }
    for value in &summary.unresolved_followups {
        push_discard(&mut discarded, "followup", "transient_followup", value);
    }
    // ponytail: extraction v2 only trusts summary buckets that already encode a durable
    // semantic claim. Ceiling: artifact refs and unresolved follow-ups can contain real
    // memories; upgrade path is source-turn-aware/model-assisted extraction instead of
    // bucket promotion.
    PairReflectionExtraction {
        candidates,
        discarded,
    }
}

fn push_candidate(
    candidates: &mut Vec<PairReflectionCandidate>,
    kind: &'static str,
    value: &str,
    title_prefix: &'static str,
) {
    let text = value.trim();
    if text.is_empty() {
        return;
    }
    let sensitivity = sensitivity_for_text(text);
    candidates.push(PairReflectionCandidate {
        kind,
        text: text.to_string(),
        title: format!("{title_prefix}: {}", truncate_chars(text, 80)),
        suggested_action: "retain_profile_local",
        target_ref: None,
        sensitivity,
        requires_human: sensitivity != "normal",
        confidence: "high",
    });
}

fn push_goal_candidate(
    candidates: &mut Vec<PairReflectionCandidate>,
    discarded: &mut Vec<PairReflectionDiscard>,
    value: &str,
) {
    let text = value.trim();
    if text.is_empty() {
        return;
    }
    if !looks_durable_goal(text) {
        push_discard(discarded, "goal", "transient_goal", text);
        return;
    }
    let sensitivity = sensitivity_for_text(text);
    candidates.push(PairReflectionCandidate {
        kind: "goal",
        text: text.to_string(),
        title: format!("Pair reflection goal: {}", truncate_chars(text, 80)),
        suggested_action: "retain_profile_local",
        target_ref: None,
        sensitivity,
        requires_human: sensitivity != "normal",
        confidence: "medium",
    });
}

fn push_discard(
    discarded: &mut Vec<PairReflectionDiscard>,
    kind: &'static str,
    reason: &'static str,
    value: &str,
) {
    let text = value.trim();
    if text.is_empty() {
        return;
    }
    discarded.push(PairReflectionDiscard {
        kind,
        reason,
        text: text.to_string(),
    });
}

fn looks_durable_goal(text: &str) -> bool {
    let haystack = text.to_ascii_lowercase();
    contains_any(
        &haystack,
        &[
            "remember",
            "durable",
            "long-term",
            "long term",
            "ongoing",
            "always",
            "prefer",
            "preference",
            "policy",
        ],
    )
}

fn sensitivity_for_text(text: &str) -> &'static str {
    let haystack = text.to_ascii_lowercase();
    if contains_any(
        &haystack,
        &[
            "api key",
            "password",
            "secret",
            "credential",
            "private key",
            "bearer ",
            "access token",
        ],
    ) {
        "secret_risk"
    } else if haystack.contains("http://")
        || haystack.contains("https://")
        || haystack.contains("external")
        || haystack.contains("untrusted")
    {
        "external_untrusted"
    } else if contains_any(
        &haystack,
        &["prefers", "preference", "human ", "user ", "personally"],
    ) {
        "person"
    } else {
        "normal"
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = text.chars().take(max.saturating_sub(1)).collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> RuntimeIterativeSummary {
        RuntimeIterativeSummary {
            active_user_goals: vec!["finish the task".to_string()],
            important_constraints: vec!["Use proposal review before shared promotion.".to_string()],
            decisions_made: vec!["Keep pair reflection local by default.".to_string()],
            artifact_refs: vec!["docs/roadmap/MEMORY_CURATION_PLAN.md".to_string()],
            workflow_state_refs: vec!["job-123".to_string()],
            unresolved_followups: vec!["rerun tests".to_string()],
        }
    }

    #[test]
    fn extraction_keeps_only_durable_semantic_candidates() {
        let extraction = extract_candidates_from_summary(&summary());
        let candidates = extraction.candidates;

        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|c| c.kind == "decision"));
        assert!(candidates.iter().any(|c| c.kind == "constraint"));
        assert_eq!(extraction.discarded.len(), 4);
        assert!(extraction
            .discarded
            .iter()
            .any(|d| d.kind == "followup" && d.reason == "transient_followup"));
        assert!(extraction
            .discarded
            .iter()
            .any(|d| d.kind == "artifact" && d.reason == "reference_without_semantic_claim"));
    }

    #[test]
    fn extraction_discards_assistant_reply_like_followups() {
        let summary = RuntimeIterativeSummary {
            unresolved_followups: vec![
                "Yeah, that sounds plausible — and the symptom chain makes sense".to_string(),
            ],
            ..RuntimeIterativeSummary::default()
        };
        let extraction = extract_candidates_from_summary(&summary);

        assert!(extraction.candidates.is_empty());
        assert_eq!(extraction.discarded[0].reason, "transient_followup");
    }

    #[test]
    fn extraction_keeps_explicit_durable_goals() {
        let summary = RuntimeIterativeSummary {
            active_user_goals: vec![
                "Remember the user's preference for ADR-first memory extraction.".to_string(),
            ],
            ..RuntimeIterativeSummary::default()
        };
        let extraction = extract_candidates_from_summary(&summary);
        let candidates = extraction.candidates;

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].kind, "goal");
        assert_eq!(candidates[0].confidence, "medium");
    }

    #[test]
    fn candidates_route_pair_decisions_to_profile_local_review() {
        let candidates = extract_candidates_from_summary(&summary()).candidates;
        let decision = candidates.iter().find(|c| c.kind == "decision").unwrap();

        assert_eq!(decision.suggested_action, "retain_profile_local");
        assert_eq!(decision.sensitivity, "normal");
        assert!(!decision.requires_human);
    }

    #[test]
    fn candidates_force_human_review_for_risky_person_content() {
        let summary = RuntimeIterativeSummary {
            decisions_made: vec!["The user prefers not to share the API key.".to_string()],
            ..RuntimeIterativeSummary::default()
        };
        let extraction = extract_candidates_from_summary(&summary);
        let candidates = extraction.candidates;

        assert_eq!(candidates[0].sensitivity, "secret_risk");
        assert!(candidates[0].requires_human);
    }

    #[test]
    #[ignore = "manual smoke eval for the production pair-reflection extractor contract"]
    fn production_pair_extractor_finds_obvious_durable_preference() {
        let summary = RuntimeIterativeSummary {
            active_user_goals: vec![
                "Remember the user's preference for SQLite-first storage unless there is a specific exception."
                    .to_string(),
            ],
            unresolved_followups: vec!["remind me tomorrow to check the auth logs".to_string()],
            ..RuntimeIterativeSummary::default()
        };
        let bundle = MemoryExtractionBundle {
            source_kind: "manual_smoke_eval".to_string(),
            source_ref: "pair-reflection-golden".to_string(),
            bear_id: Uuid::nil(),
            conversation_id: Some("conv-golden".to_string()),
            session_id: Some("session-golden".to_string()),
            compaction: Some(MemoryExtractionCompactionContext {
                artifact_id: Some("artifact-golden".to_string()),
                policy_version: Some("manual-smoke".to_string()),
                source_message_start_seq: Some(1),
                source_message_end_seq: Some(3),
                hints: vec!["possible_user_goal".to_string()],
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
                    content: "Makes sense; I'll use SQLite-first.".to_string(),
                    created_at: None,
                },
                MemoryExtractionMessage {
                    id: "m3".to_string(),
                    seq: Some(3),
                    role: "user".to_string(),
                    content: "Also remind me tomorrow to check the auth logs.".to_string(),
                    created_at: None,
                },
            ],
            artifacts: Vec::new(),
        };

        let output = run_memory_extraction(&bundle, &PairSummaryExtractor { summary: &summary })
            .expect("production extractor smoke eval should run");

        assert_eq!(output.proposal_drafts.len(), 1);
        assert!(output.proposal_drafts[0]
            .proposed_content
            .contains("SQLite-first storage"));
        assert_eq!(
            output.proposal_drafts[0].source_refs["candidate_kind"],
            "preference"
        );
        assert!(output
            .discarded
            .iter()
            .any(|discard| discard.reason == "followup:transient_followup"));
    }
}
