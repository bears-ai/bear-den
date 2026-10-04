use den_core::{config::Config, DenError};
use den_memory::{
    harvest_source_hash_marked, harvest_source_marked, record_harvest_mark, MemoryStoreManager,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use den_service::bears::RuntimeContextLabel;

use crate::{
    agent_loop::load_transcript_grouping_rows,
    memory::extraction::{
        create_proposals_from_extraction, run_memory_extraction, MemoryExtractionArtifact,
        MemoryExtractionBundle, MemoryExtractionCandidate, MemoryExtractionCompactionContext,
        MemoryExtractionDiscard, MemoryExtractionMessage, MemoryExtractionResult, MemoryExtractor,
    },
    runtime_conversations::RuntimeIterativeSummary,
};

#[derive(Debug, Clone, sqlx::FromRow)]
struct CompactionArtifactHarvestRow {
    id: Uuid,
    conversation_id: Uuid,
    external_conversation_id: Option<String>,
    artifact_kind: String,
    policy_version: String,
    trigger: String,
    source_message_start_seq: i64,
    source_message_end_seq: i64,
    source_group_start: Option<i32>,
    source_group_end: Option<i32>,
    artifact_json: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveHarvestOutput {
    pub scanned_artifacts: usize,
    pub candidate_count: usize,
    pub discarded_count: usize,
    pub no_candidate_count: usize,
    pub created_proposal_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HarvestExtraction {
    candidates: Vec<HarvestCandidate>,
    discarded: Vec<HarvestDiscard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HarvestCandidate {
    kind: &'static str,
    text: String,
    confidence_basis: &'static str,
    sensitivity: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HarvestDiscard {
    kind: &'static str,
    reason: &'static str,
}

struct ArchiveSummaryExtractor<'a> {
    summary: &'a RuntimeIterativeSummary,
}

impl MemoryExtractor for ArchiveSummaryExtractor<'_> {
    fn extract(&self, bundle: &MemoryExtractionBundle) -> Result<MemoryExtractionResult, DenError> {
        let user_message_ids = bundle
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        let source_artifact_ids = bundle
            .artifacts
            .iter()
            .map(|artifact| artifact.id.clone())
            .collect::<Vec<_>>();
        let extraction = extract_harvest_candidate(self.summary);
        Ok(MemoryExtractionResult {
            candidates: extraction
                .candidates
                .into_iter()
                .map(|candidate| MemoryExtractionCandidate {
                    kind: candidate.kind.to_string(),
                    content: candidate.text,
                    rationale: "Extracted by archive harvest from source transcript evidence scoped by a compaction artifact.".to_string(),
                    source_message_ids: user_message_ids.clone(),
                    source_artifact_ids: source_artifact_ids.clone(),
                    confidence: match candidate.confidence_basis {
                        "high" => 0.85,
                        "medium" => 0.65,
                        _ => 0.5,
                    },
                    sensitivity: candidate.sensitivity.to_string(),
                    suggested_action: "human_review".to_string(),
                })
                .collect(),
            discarded: extraction
                .discarded
                .into_iter()
                .map(|discard| MemoryExtractionDiscard {
                    source_message_ids: Vec::new(),
                    source_artifact_ids: source_artifact_ids.clone(),
                    reason: format!("{}:{}", discard.kind, discard.reason),
                })
                .collect(),
        })
    }
}

fn db_err(context: &'static str) -> impl FnOnce(sqlx::Error) -> DenError {
    move |err| match DenError::from(err) {
        DenError::Database(message) => DenError::Database(format!("{context}: {message}")),
        DenError::DatabaseUnavailable(message) => {
            DenError::DatabaseUnavailable(format!("{context}: {message}"))
        }
        other => other,
    }
}

fn json_parse_err(context: &'static str) -> impl FnOnce(serde_json::Error) -> DenError {
    move |err| DenError::Parsing(format!("{context}: {err}"))
}

pub async fn harvest_compaction_artifacts_once(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    limit: i64,
    run_id: Option<&str>,
) -> Result<ArchiveHarvestOutput, DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    let artifacts = list_unmined_compaction_artifacts(pool, bear_id, limit).await?;
    let mut output = ArchiveHarvestOutput {
        scanned_artifacts: artifacts.len(),
        candidate_count: 0,
        discarded_count: 0,
        no_candidate_count: 0,
        created_proposal_ids: Vec::new(),
    };

    for artifact in artifacts {
        let CompactionArtifactHarvestRow {
            id,
            conversation_id,
            external_conversation_id,
            artifact_kind,
            policy_version,
            trigger,
            source_message_start_seq,
            source_message_end_seq,
            source_group_start,
            source_group_end,
            artifact_json,
        } = artifact;

        let source_ref = id.to_string();
        let source_hash = source_hash(&artifact_json);
        // ponytail: exact content hash only; upgrade path is source equivalence metadata when
        // artifacts can be reserialized with semantically identical but byte-different JSON.
        if harvest_source_marked(&store, "compaction_artifact", &source_ref).await?
            || harvest_source_hash_marked(&store, "compaction_artifact", &source_hash).await?
        {
            continue;
        }
        let summary = decode_summary(artifact_json.clone())?;
        let messages = if let Some(conversation_key) = &external_conversation_id {
            load_transcript_grouping_rows(pool, bear_id, conversation_key).await?
        } else {
            Vec::new()
        };
        let bundle = archive_harvest_bundle(
            bear_id,
            &source_ref,
            conversation_id,
            external_conversation_id.as_deref(),
            ArtifactScope {
                artifact_kind,
                policy_version,
                trigger,
                source_message_start_seq,
                source_message_end_seq,
                source_group_start,
                source_group_end,
            },
            artifact_json,
            messages,
            &summary,
        );
        let extraction_output =
            run_memory_extraction(&bundle, &ArchiveSummaryExtractor { summary: &summary })?;
        output.candidate_count += extraction_output.proposal_drafts.len();
        output.discarded_count += extraction_output.discarded.len();
        if extraction_output.proposal_drafts.is_empty() {
            output.no_candidate_count += 1;
            record_harvest_mark(
                &store,
                "compaction_artifact",
                &source_ref,
                Some(&source_hash),
                run_id,
                &[],
            )
            .await?;
            continue;
        }
        let created_ids = create_proposals_from_extraction(
            pool,
            config,
            stores,
            RuntimeContextLabel::Curation,
            Some("archive_harvest".to_string()),
            &bundle,
            &extraction_output,
        )
        .await?;
        output.created_proposal_ids.extend(created_ids.clone());
        record_harvest_mark(
            &store,
            "compaction_artifact",
            &source_ref,
            Some(&source_hash),
            run_id,
            &created_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
        )
        .await?;
    }

    Ok(output)
}

async fn list_unmined_compaction_artifacts(
    pool: &PgPool,
    bear_id: Uuid,
    limit: i64,
) -> Result<Vec<CompactionArtifactHarvestRow>, DenError> {
    sqlx::query_as!(
        CompactionArtifactHarvestRow,
        r#"
        SELECT a.id as "id: _",
               a.conversation_id as "conversation_id: _",
               c.external_conversation_id as "external_conversation_id: _",
               a.artifact_kind as "artifact_kind: _",
               a.policy_version as "policy_version: _",
               a.trigger as "trigger: _",
               a.source_message_start_seq as "source_message_start_seq: _",
               a.source_message_end_seq as "source_message_end_seq: _",
               a.source_group_start as "source_group_start: _",
               a.source_group_end as "source_group_end: _",
               a.artifact_json as "artifact_json: _"
        FROM conversation_compaction_artifacts a
        JOIN conversations c ON c.id = a.conversation_id
        WHERE c.bear_id = $1
          AND a.superseded_by IS NULL
          AND NOT EXISTS (
              SELECT 1
              FROM bear_memory_proposals p
              WHERE p.bear_id = $1
                AND (
                    (p.source_refs->>'source' = 'archive_harvest'
                     AND p.source_refs->>'artifact_id' = a.id::text)
                    OR
                    (p.source_refs->>'source' = 'memory_extraction'
                     AND p.source_refs->>'source_kind' = 'archive_harvest'
                     AND p.source_refs->>'source_ref' = a.id::text)
                )
          )
        ORDER BY a.created_at ASC
        LIMIT $2
        "#,
        bear_id,
        limit
    )
    .fetch_all(pool)
    .await
    .map_err(db_err("list/decode compaction artifacts for harvest"))
}

fn decode_summary(value: serde_json::Value) -> Result<RuntimeIterativeSummary, DenError> {
    serde_json::from_value(value).map_err(json_parse_err("decode compaction summary for harvest"))
}

fn source_hash(value: &serde_json::Value) -> String {
    let encoded = serde_json::to_vec(value).unwrap_or_else(|_| value.to_string().into_bytes());
    let digest = Sha256::digest(encoded);
    format!("sha256:{digest:x}")
}

#[derive(Debug, Clone)]
struct ArtifactScope {
    artifact_kind: String,
    policy_version: String,
    trigger: String,
    source_message_start_seq: i64,
    source_message_end_seq: i64,
    source_group_start: Option<i32>,
    source_group_end: Option<i32>,
}

fn archive_harvest_bundle(
    bear_id: Uuid,
    source_ref: &str,
    conversation_uuid: Uuid,
    conversation_id: Option<&str>,
    scope: ArtifactScope,
    artifact_json: serde_json::Value,
    rows: Vec<crate::runtime::compaction::TranscriptGroupingRow>,
    summary: &RuntimeIterativeSummary,
) -> MemoryExtractionBundle {
    let messages = rows
        .into_iter()
        .filter(|row| {
            let seq = row.sequence_no.unwrap_or_default();
            seq >= scope.source_message_start_seq
                && seq <= scope.source_message_end_seq
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
        source_kind: "archive_harvest".to_string(),
        source_ref: source_ref.to_string(),
        bear_id,
        conversation_id: conversation_id.map(ToOwned::to_owned),
        session_id: None,
        compaction: Some(MemoryExtractionCompactionContext {
            artifact_id: Some(source_ref.to_string()),
            policy_version: Some(scope.policy_version.clone()),
            source_message_start_seq: Some(scope.source_message_start_seq),
            source_message_end_seq: Some(scope.source_message_end_seq),
            hints: compaction_hints(summary),
        }),
        messages,
        artifacts: vec![MemoryExtractionArtifact {
            id: source_ref.to_string(),
            kind: scope.artifact_kind,
            content: serde_json::json!({
                "conversation_uuid": conversation_uuid,
                "trigger": scope.trigger,
                "source_group_start": scope.source_group_start,
                "source_group_end": scope.source_group_end,
                "artifact": artifact_json,
            })
            .to_string(),
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

fn extract_harvest_candidate(summary: &RuntimeIterativeSummary) -> HarvestExtraction {
    let mut candidates = Vec::new();
    let mut discarded = Vec::new();

    for value in &summary.decisions_made {
        push_candidate(&mut candidates, "decision", value, "high");
    }
    for value in &summary.important_constraints {
        push_candidate(&mut candidates, "constraint", value, "high");
    }
    for value in &summary.active_user_goals {
        let text = value.trim();
        if text.is_empty() {
            continue;
        }
        if looks_durable_goal(text) {
            push_candidate(&mut candidates, "preference", text, "medium");
        } else {
            discarded.push(HarvestDiscard {
                kind: "goal",
                reason: "transient_goal",
            });
        }
    }
    discarded.extend(
        summary
            .artifact_refs
            .iter()
            .filter(|value| !value.trim().is_empty())
            .map(|_| HarvestDiscard {
                kind: "artifact",
                reason: "reference_without_semantic_claim",
            }),
    );
    discarded.extend(
        summary
            .workflow_state_refs
            .iter()
            .filter(|value| !value.trim().is_empty())
            .map(|_| HarvestDiscard {
                kind: "workflow_state",
                reason: "transient_workflow_state",
            }),
    );
    discarded.extend(
        summary
            .unresolved_followups
            .iter()
            .filter(|value| !value.trim().is_empty())
            .map(|_| HarvestDiscard {
                kind: "followup",
                reason: "transient_followup",
            }),
    );

    // ponytail: archive harvest still uses summary fields as extractor hints for this slice.
    // Ceiling: claims may need source-turn-aware/model-assisted synthesis; upgrade path is a
    // production extractor that reads bundle.messages instead of these compaction buckets.
    HarvestExtraction {
        candidates,
        discarded,
    }
}

fn push_candidate(
    candidates: &mut Vec<HarvestCandidate>,
    kind: &'static str,
    value: &str,
    confidence_basis: &'static str,
) {
    let text = value.trim();
    if text.is_empty() {
        return;
    }
    candidates.push(HarvestCandidate {
        kind,
        text: text.to_string(),
        confidence_basis,
        sensitivity: sensitivity_for_text(text),
    });
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

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::compaction::TranscriptGroupingRow;

    fn summary_with(
        constraints: &[&str],
        decisions: &[&str],
        artifacts: &[&str],
        followups: &[&str],
    ) -> RuntimeIterativeSummary {
        RuntimeIterativeSummary {
            active_user_goals: Vec::new(),
            important_constraints: constraints
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            decisions_made: decisions.iter().map(|value| (*value).to_string()).collect(),
            artifact_refs: artifacts.iter().map(|value| (*value).to_string()).collect(),
            workflow_state_refs: Vec::new(),
            unresolved_followups: followups.iter().map(|value| (*value).to_string()).collect(),
        }
    }

    #[test]
    fn extraction_maps_summary_hints_to_memory_contract() {
        let summary = RuntimeIterativeSummary {
            active_user_goals: vec![
                "ship compaction".to_string(),
                "remember the long-term curation policy".to_string(),
            ],
            important_constraints: vec!["do not cross approval floors".to_string()],
            decisions_made: Vec::new(),
            artifact_refs: vec![
                "docs/roadmap/DEN_CONTEXT_COMPACTION_IMPLEMENTATION_PLAN.md".to_string()
            ],
            workflow_state_refs: Vec::new(),
            unresolved_followups: vec!["wire archive harvest".to_string()],
        };

        let extraction = extract_harvest_candidate(&summary);

        assert_eq!(extraction.candidates.len(), 2);
        assert!(extraction.candidates.iter().any(|candidate| {
            candidate.kind == "preference"
                && candidate.text == "remember the long-term curation policy"
        }));
        assert!(extraction.candidates.iter().any(|candidate| {
            candidate.kind == "constraint" && candidate.text == "do not cross approval floors"
        }));
        assert!(extraction.discarded.iter().any(|discard| {
            discard.kind == "artifact" && discard.reason == "reference_without_semantic_claim"
        }));
        assert!(extraction
            .discarded
            .iter()
            .any(|discard| discard.kind == "followup" && discard.reason == "transient_followup"));
        assert!(extraction
            .discarded
            .iter()
            .any(|discard| discard.kind == "goal" && discard.reason == "transient_goal"));
    }

    #[test]
    fn extraction_drops_transient_followups_only() {
        let summary = summary_with(&[], &[], &[], &["remember to rerun tests"]);

        let extraction = extract_harvest_candidate(&summary);

        assert!(extraction.candidates.is_empty());
        assert_eq!(extraction.discarded.len(), 1);
    }

    #[test]
    fn extraction_keeps_durable_decisions_with_high_confidence() {
        let summary = summary_with(
            &["Do not auto-promote raw transcripts."],
            &["Use SQLite as canonical memory."],
            &[],
            &[],
        );
        let extraction = extract_harvest_candidate(&summary);

        assert_eq!(extraction.candidates.len(), 2);
        assert!(extraction
            .candidates
            .iter()
            .all(|candidate| candidate.confidence_basis == "high"));
        assert!(extraction
            .candidates
            .iter()
            .all(|candidate| candidate.sensitivity == "normal"));
    }

    #[test]
    fn extraction_drops_artifact_references_without_semantic_claims() {
        let summary = summary_with(
            &[],
            &[],
            &["docs/roadmap/MEMORY_AUTOMATION_ROADMAP.md"],
            &[],
        );

        let extraction = extract_harvest_candidate(&summary);

        assert!(extraction.candidates.is_empty());
        assert_eq!(extraction.discarded[0].kind, "artifact");
    }

    #[test]
    fn extraction_drops_goals_and_workflow_state_without_durable_signals() {
        let mut summary = summary_with(&[], &[], &[], &["revisit later"]);
        summary.active_user_goals = vec!["finish the current task".to_string()];
        summary.workflow_state_refs = vec!["job-123".to_string()];

        let extraction = extract_harvest_candidate(&summary);

        assert!(extraction.candidates.is_empty());
        assert_eq!(extraction.discarded.len(), 3);
    }

    #[test]
    fn extraction_flags_secret_external_and_person_risk() {
        let summary = summary_with(
            &["Hans prefers not to share the API key from https://example.invalid."],
            &[],
            &[],
            &[],
        );
        let extraction = extract_harvest_candidate(&summary);

        let candidate = &extraction.candidates[0];
        assert_eq!(candidate.sensitivity, "secret_risk");
    }

    #[test]
    fn archive_bundle_filters_to_source_span_and_preserves_hints() {
        let bear_id = Uuid::nil();
        let artifact_id = Uuid::new_v4();
        let conversation_uuid = Uuid::new_v4();
        let summary = summary_with(
            &["Prefer source-backed memory proposals."],
            &["Use extraction contract for archive harvest."],
            &[],
            &[],
        );
        let rows = vec![
            TranscriptGroupingRow::new("user", "outside", serde_json::Value::Null)
                .with_message_id("m0")
                .with_sequence_no(1),
            TranscriptGroupingRow::new(
                "user",
                "For this project, prefer SQLite-first storage.",
                serde_json::Value::Null,
            )
            .with_message_id("m1")
            .with_sequence_no(2),
            TranscriptGroupingRow::new("assistant", "Acknowledged.", serde_json::Value::Null)
                .with_message_id("m2")
                .with_sequence_no(3),
        ];

        let bundle = archive_harvest_bundle(
            bear_id,
            &artifact_id.to_string(),
            conversation_uuid,
            Some("conv-key"),
            ArtifactScope {
                artifact_kind: "iterative_summary".to_string(),
                policy_version: "test-policy".to_string(),
                trigger: "test".to_string(),
                source_message_start_seq: 2,
                source_message_end_seq: 3,
                source_group_start: Some(1),
                source_group_end: Some(2),
            },
            serde_json::json!({"summary": true}),
            rows,
            &summary,
        );

        assert_eq!(bundle.source_kind, "archive_harvest");
        assert_eq!(bundle.messages.len(), 2);
        assert_eq!(bundle.messages[0].id, "m1");
        assert_eq!(bundle.messages[1].id, "m2");
        assert_eq!(
            bundle.compaction.as_ref().unwrap().hints,
            vec![
                "possible_decision".to_string(),
                "possible_constraint".to_string()
            ]
        );
    }

    #[test]
    fn archive_extractor_uses_shared_memory_contract() {
        let artifact_id = Uuid::new_v4().to_string();
        let summary = summary_with(
            &["Prefer source-backed memory proposals."],
            &[],
            &["docs/roadmap/MEMORY_AUTOMATION_ROADMAP.md"],
            &[],
        );
        let bundle = MemoryExtractionBundle {
            source_kind: "archive_harvest".to_string(),
            source_ref: artifact_id.clone(),
            bear_id: Uuid::nil(),
            conversation_id: Some("conv-key".to_string()),
            session_id: None,
            compaction: None,
            messages: vec![MemoryExtractionMessage {
                id: "m1".to_string(),
                seq: Some(1),
                role: "user".to_string(),
                content: "Prefer source-backed memory proposals.".to_string(),
                created_at: None,
            }],
            artifacts: vec![MemoryExtractionArtifact {
                id: artifact_id,
                kind: "iterative_summary".to_string(),
                content: "{}".to_string(),
            }],
        };

        let output = run_memory_extraction(&bundle, &ArchiveSummaryExtractor { summary: &summary })
            .expect("extraction output");

        assert_eq!(output.proposal_drafts.len(), 1);
        assert_eq!(
            output.proposal_drafts[0].proposed_content,
            "Prefer source-backed memory proposals."
        );
        assert_eq!(output.discarded.len(), 1);
        assert_eq!(
            output.discarded[0].reason,
            "artifact:reference_without_semantic_claim"
        );
    }
}
