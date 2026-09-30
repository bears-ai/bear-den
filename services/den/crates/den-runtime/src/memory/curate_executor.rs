use den_core::{config::Config, ids::BearId, DenError};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use den_memory::MemoryStoreManager;
use den_service::memory_proposals::{MemoryProposalRow, ProposalResolutionParams};

use crate::memory::{get_proposal, promote_core_content_at_path, resolve_proposal};
use den_service::bears::{hats, BearProfile};

pub const MEMORY_CURATE_RUNNER_AGENT_ID: &str = "memory_curate_runner";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurateProposalOutcome {
    pub proposal_id: Uuid,
    pub status: String,
    pub suggested_action: String,
    pub triage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurateBriefingItem {
    pub proposal_id: Uuid,
    pub title: String,
    pub summary: String,
    pub suggested_action: String,
    pub source_profile: String,
    pub status: String,
    pub triage: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCurateRunOutput {
    pub resolved_proposal_ids: Vec<String>,
    pub outcomes: Vec<CurateProposalOutcome>,
    pub resolution_status: String,
    pub status_counts: serde_json::Value,
    pub briefing: Vec<CurateBriefingItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CurateTriage {
    RetainProfileLocal {
        review_notes: &'static str,
        decision_summary: &'static str,
    },
    Reject {
        review_notes: &'static str,
        decision_summary: &'static str,
    },
    Defer {
        review_notes: &'static str,
        decision_summary: &'static str,
    },
    EscalateHuman {
        review_notes: &'static str,
        decision_summary: &'static str,
    },
    PromoteToCore {
        review_notes: &'static str,
        decision_summary: &'static str,
    },
}

impl CurateTriage {
    fn triage_label(&self) -> &'static str {
        match self {
            Self::RetainProfileLocal { .. } => "retain_profile_local",
            Self::Reject { .. } => "reject",
            Self::Defer { .. } => "defer",
            Self::EscalateHuman { .. } => "escalate_human",
            Self::PromoteToCore { .. } => "promote_to_core",
        }
    }

    fn resolution_status(&self) -> &'static str {
        match self {
            Self::RetainProfileLocal { .. } => "retained_local",
            Self::Reject { .. } => "rejected",
            Self::Defer { .. } => "deferred",
            Self::EscalateHuman { .. } => "needs_human_review",
            Self::PromoteToCore { .. } => "approved",
        }
    }

    fn review_notes(&self) -> &'static str {
        match self {
            Self::RetainProfileLocal { review_notes, .. }
            | Self::Reject { review_notes, .. }
            | Self::Defer { review_notes, .. }
            | Self::EscalateHuman { review_notes, .. }
            | Self::PromoteToCore { review_notes, .. } => review_notes,
        }
    }

    fn decision_summary(&self) -> &'static str {
        match self {
            Self::RetainProfileLocal {
                decision_summary, ..
            }
            | Self::Reject {
                decision_summary, ..
            }
            | Self::Defer {
                decision_summary, ..
            }
            | Self::EscalateHuman {
                decision_summary, ..
            }
            | Self::PromoteToCore {
                decision_summary, ..
            } => decision_summary,
        }
    }
}

fn decide_curate_triage(proposal: &MemoryProposalRow, trigger: Option<&str>) -> CurateTriage {
    if proposal.requires_human {
        return CurateTriage::EscalateHuman {
            review_notes: "Proposal was flagged requires_human=true.",
            decision_summary:
                "Escalated to human review because the proposal requires human follow-up.",
        };
    }

    if sensitivity_requires_human(&proposal.sensitivity) {
        return CurateTriage::EscalateHuman {
            review_notes: "Proposal sensitivity requires human review.",
            decision_summary:
                "Escalated to human review because sensitivity policy blocks autonomous resolution.",
        };
    }

    match proposal.suggested_action.as_str() {
        "human_review" => CurateTriage::EscalateHuman {
            review_notes: "Proposal requested explicit human review.",
            decision_summary: "Escalated to human review per suggested_action=human_review.",
        },
        "retain_profile_local" => CurateTriage::RetainProfileLocal {
            review_notes:
                "Role-local memory remains the durable source; no shared-memory write needed.",
            decision_summary: "Autonomous curate retained the proposal as role-local memory.",
        },
        "delete_after_review" => CurateTriage::Reject {
            review_notes: "Proposal requested deletion after review.",
            decision_summary: "Autonomous curate rejected the proposal per delete_after_review.",
        },
        "promote_to_core" | "summarize_into_core" => {
            if can_auto_promote_to_core(proposal) {
                CurateTriage::PromoteToCore {
                    review_notes: "Low-risk core promotion candidate with bounded summary content.",
                    decision_summary:
                        "Autonomous curate applied a distilled summary to core memory.",
                }
            } else {
                CurateTriage::Defer {
                    review_notes: "Core promotion requires curate-agent review or additional proposal content; no silent core write was applied.",
                    decision_summary: "Deferred core promotion until curate can review with richer context.",
                }
            }
        }
        "cabinet_update" | "skill_review" | "archive_index" | "task_context" => {
            CurateTriage::Defer {
                review_notes:
                    "Specialized promotion path is not handled by the autonomous substrate yet.",
                decision_summary:
                    "Deferred until curate can route the proposal through the appropriate workflow.",
            }
        }
        "unspecified" => {
            if trigger == Some("pair_reflection") && proposal.sensitivity == "normal" {
                CurateTriage::RetainProfileLocal {
                    review_notes: "Pair reflection summary is durable in pair-local memory; shared promotion is not automatic.",
                    decision_summary: "Autonomous curate retained the pair reflection summary as role-local memory.",
                }
            } else if trigger == Some("watch_observation") {
                CurateTriage::Defer {
                    review_notes:
                        "Watch observation recorded; curate review decides promotion or dismissal.",
                    decision_summary: "Deferred watch observation for curate review.",
                }
            } else {
                CurateTriage::Defer {
                    review_notes: "Ambiguous proposal needs curate-agent review.",
                    decision_summary:
                        "Deferred unspecified proposal until curate can decide the final outcome.",
                }
            }
        }
        _ => CurateTriage::Defer {
            review_notes: "Unknown suggested_action; deferring to curate review.",
            decision_summary: "Deferred proposal with unrecognized suggested_action.",
        },
    }
}

fn sensitivity_requires_human(sensitivity: &str) -> bool {
    matches!(
        sensitivity,
        "person" | "secret_risk" | "external_untrusted" | "unknown"
    )
}

fn can_auto_promote_to_core(proposal: &MemoryProposalRow) -> bool {
    if proposal.requires_human || sensitivity_requires_human(&proposal.sensitivity) {
        return false;
    }
    if proposal
        .proposed_patch
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return false;
    }
    if proposal
        .refs
        .get("archive_harvest")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }
    let has_body = proposal
        .proposed_content
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || proposal.summary.trim().len() >= 16;
    has_body && !proposal.source_paths.is_empty()
}

fn core_target_path(proposal: &MemoryProposalRow) -> String {
    proposal
        .target_ref
        .as_deref()
        .map(str::trim)
        .filter(|path| path.starts_with("core/"))
        .map(str::to_string)
        .unwrap_or_else(|| "core/knowledge.md".to_string())
}

fn promotion_body(proposal: &MemoryProposalRow) -> String {
    let distilled = proposal
        .proposed_content
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| proposal.summary.trim());
    format!(
        "{distilled}\n\n---\nSource proposal: `{}`\nSource role: `{}`\nSource paths: {}\n",
        proposal.id,
        proposal.source_profile,
        proposal.source_paths.join(", ")
    )
}

pub async fn execute_memory_curate_proposals(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    trigger: Option<&str>,
    proposal_ids: &[Uuid],
) -> Result<MemoryCurateRunOutput, DenError> {
    let mut outcomes = Vec::new();
    for proposal_id in proposal_ids {
        let Some(proposal) = get_proposal(pool, config, stores, bear_id, *proposal_id).await?
        else {
            continue;
        };
        if proposal.status != "pending" {
            continue;
        }
        let outcome =
            resolve_curate_proposal(pool, config, stores, bear_id, &proposal, trigger).await?;
        outcomes.push(outcome);
    }

    let resolved_proposal_ids = outcomes
        .iter()
        .map(|outcome| outcome.proposal_id.to_string())
        .collect::<Vec<_>>();
    let mut status_counts = std::collections::HashMap::<String, u64>::new();
    for outcome in &outcomes {
        *status_counts.entry(outcome.status.clone()).or_default() += 1;
    }
    let resolution_status = aggregate_resolution_status(&outcomes);
    let briefing = build_curate_briefing(pool, config, stores, bear_id, &outcomes).await?;

    Ok(MemoryCurateRunOutput {
        resolved_proposal_ids,
        outcomes,
        resolution_status,
        status_counts: serde_json::to_value(status_counts).map_err(|err| {
            DenError::System(format!("serialize curate status counts failed: {err}"))
        })?,
        briefing,
    })
}

async fn build_curate_briefing(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    outcomes: &[CurateProposalOutcome],
) -> Result<Vec<CurateBriefingItem>, DenError> {
    let mut briefing = Vec::new();
    for outcome in outcomes {
        if !matches!(outcome.status.as_str(), "deferred" | "needs_human_review") {
            continue;
        }
        let Some(proposal) =
            get_proposal(pool, config, stores, bear_id, outcome.proposal_id).await?
        else {
            continue;
        };
        briefing.push(CurateBriefingItem {
            proposal_id: proposal.id,
            title: proposal.title,
            summary: proposal.summary,
            suggested_action: proposal.suggested_action,
            source_profile: proposal.source_profile,
            status: outcome.status.clone(),
            triage: outcome.triage.clone(),
        });
    }
    Ok(briefing)
}

async fn resolve_curate_proposal(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    proposal: &MemoryProposalRow,
    trigger: Option<&str>,
) -> Result<CurateProposalOutcome, DenError> {
    let mut triage = decide_curate_triage(proposal, trigger);
    if matches!(triage, CurateTriage::PromoteToCore { .. })
        && !hats::list_hats(pool, BearId::new(bear_id))
            .await?
            .is_empty()
    {
        triage = CurateTriage::EscalateHuman {
            review_notes: "Configured hats require a Bear-admin review of canonical hat knowledge before Bear-wide core publication.",
            decision_summary: "Escalated core promotion: a legacy proposal cannot prove the hat and Work audience for this Bear.",
        };
    }
    let triage_label = triage.triage_label().to_string();

    if matches!(triage, CurateTriage::PromoteToCore { .. }) {
        match apply_core_promotion(pool, config, stores, bear_id, proposal, &triage).await {
            Ok((result_path, _result_commit)) => {
                return Ok(CurateProposalOutcome {
                    proposal_id: proposal.id,
                    status: "approved".to_string(),
                    suggested_action: proposal.suggested_action.clone(),
                    triage: triage_label,
                    result_path: Some(result_path),
                    error: None,
                });
            }
            Err(error) => {
                let deferred = resolve_proposal(
                    pool,
                    config,
                    stores,
                    ProposalResolutionParams {
                        bear_id,
                        proposal_id: proposal.id,
                        reviewer_profile: BearProfile::Curate,
                        reviewer_agent_id: Some(MEMORY_CURATE_RUNNER_AGENT_ID),
                        status: "deferred",
                        review_notes: Some(&format!(
                            "Autonomous core promotion failed; deferred for curate review: {error}"
                        )),
                        decision_summary: Some(
                            "Deferred after autonomous core promotion attempt failed.",
                        ),
                        result_path: None,
                        result_commit: None,
                        project_to_conversation: true,
                    },
                )
                .await?;
                return Ok(CurateProposalOutcome {
                    proposal_id: deferred.id,
                    status: deferred.status,
                    suggested_action: deferred.suggested_action,
                    triage: "defer_after_error".to_string(),
                    result_path: None,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    let resolved = resolve_proposal(
        pool,
        config,
        stores,
        ProposalResolutionParams {
            bear_id,
            proposal_id: proposal.id,
            reviewer_profile: BearProfile::Curate,
            reviewer_agent_id: Some(MEMORY_CURATE_RUNNER_AGENT_ID),
            status: triage.resolution_status(),
            review_notes: Some(triage.review_notes()),
            decision_summary: Some(triage.decision_summary()),
            result_path: None,
            result_commit: None,
            project_to_conversation: true,
        },
    )
    .await?;

    Ok(CurateProposalOutcome {
        proposal_id: resolved.id,
        status: resolved.status,
        suggested_action: resolved.suggested_action,
        triage: triage_label,
        result_path: resolved.result_path,
        error: None,
    })
}

async fn apply_core_promotion(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    proposal: &MemoryProposalRow,
    triage: &CurateTriage,
) -> Result<(String, Option<String>), DenError> {
    let target_path = core_target_path(proposal);
    let kind = target_path
        .split('/')
        .next_back()
        .unwrap_or("note")
        .trim_end_matches(".md");
    let (memory_id, _promotion_id) = promote_core_content_at_path(
        stores,
        bear_id,
        &proposal.id.to_string(),
        &target_path,
        kind,
        &promotion_body(proposal),
        BearProfile::Curate.as_str(),
    )
    .await?;

    // Derived recall is updated asynchronously (ADR-0038 Phase 1b): enqueue a reconcile so the
    // recall_index worker re-indexes this Bear's canonical heads. Best-effort.
    crate::reflection::conductor::enqueue_recall_index_if_enabled(
        pool,
        config,
        bear_id,
        "memory_curate_core_promotion",
    )
    .await;

    let (result_path, result_commit) = (target_path, Some(memory_id));
    resolve_proposal(
        pool,
        config,
        stores,
        ProposalResolutionParams {
            bear_id,
            proposal_id: proposal.id,
            reviewer_profile: BearProfile::Curate,
            reviewer_agent_id: Some(MEMORY_CURATE_RUNNER_AGENT_ID),
            status: "approved",
            review_notes: Some(triage.review_notes()),
            decision_summary: Some(triage.decision_summary()),
            result_path: Some(result_path.as_str()),
            result_commit: result_commit.as_deref(),
            project_to_conversation: true,
        },
    )
    .await?;
    Ok((result_path, result_commit))
}

fn aggregate_resolution_status(outcomes: &[CurateProposalOutcome]) -> String {
    if outcomes.is_empty() {
        return "no_pending_proposals".to_string();
    }
    let mut distinct = outcomes
        .iter()
        .map(|outcome| outcome.status.as_str())
        .collect::<Vec<_>>();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() == 1 {
        distinct[0].to_string()
    } else {
        "mixed".to_string()
    }
}

#[cfg(test)]
#[path = "curate_executor/hat_tests.rs"]
mod hat_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn sample_proposal(
        suggested_action: &str,
        sensitivity: &str,
        requires_human: bool,
    ) -> MemoryProposalRow {
        MemoryProposalRow {
            id: Uuid::new_v4(),
            bear_id: Uuid::new_v4(),
            source_profile: "pair".to_string(),
            source_agent_id: Some("pair-agent".to_string()),
            source_paths: vec!["pair/summaries/example.md".to_string()],
            source_refs: serde_json::json!({}),
            proposal_type: "memory_review".to_string(),
            suggested_action: suggested_action.to_string(),
            target_ref: None,
            title: "Test proposal".to_string(),
            summary: "Useful durable lesson from the pair session.".to_string(),
            rationale: "rationale".to_string(),
            proposed_content: None,
            proposed_patch: None,
            refs: serde_json::json!({}),
            sensitivity: sensitivity.to_string(),
            requires_human,
            status: "pending".to_string(),
            reviewer_profile: None,
            reviewer_agent_id: None,
            review_notes: None,
            decision_summary: None,
            result_path: None,
            result_commit: None,
            created_at: OffsetDateTime::now_utc(),
            reviewed_at: None,
        }
    }

    #[test]
    fn pair_reflection_unspecified_normal_is_retained_local() {
        let proposal = sample_proposal("unspecified", "normal", false);
        let triage = decide_curate_triage(&proposal, Some("pair_reflection"));
        assert_eq!(triage.resolution_status(), "retained_local");
    }

    #[test]
    fn unspecified_without_pair_reflection_is_deferred() {
        let proposal = sample_proposal("unspecified", "normal", false);
        let triage = decide_curate_triage(&proposal, Some("manual"));
        assert_eq!(triage.resolution_status(), "deferred");
    }

    #[test]
    fn requires_human_escalates() {
        let proposal = sample_proposal("retain_profile_local", "normal", true);
        let triage = decide_curate_triage(&proposal, Some("pair_reflection"));
        assert_eq!(triage.resolution_status(), "needs_human_review");
    }

    #[test]
    fn risky_sensitivity_escalates_to_human_review() {
        for sensitivity in ["person", "secret_risk", "external_untrusted", "unknown"] {
            let proposal = sample_proposal("promote_to_core", sensitivity, false);
            let triage = decide_curate_triage(&proposal, None);
            assert_eq!(
                triage.resolution_status(),
                "needs_human_review",
                "sensitivity={sensitivity}"
            );
        }
    }

    #[test]
    fn promote_to_core_with_summary_is_promotable() {
        let proposal = sample_proposal("promote_to_core", "normal", false);
        let triage = decide_curate_triage(&proposal, None);
        assert_eq!(triage.triage_label(), "promote_to_core");
    }

    #[test]
    fn archive_harvest_candidates_are_not_auto_promoted() {
        let mut proposal = sample_proposal("promote_to_core", "normal", false);
        proposal.refs = serde_json::json!({ "archive_harvest": true });
        let triage = decide_curate_triage(&proposal, None);
        assert_eq!(triage.resolution_status(), "deferred");
    }

    #[test]
    fn proposed_patches_are_not_auto_promoted() {
        let mut proposal = sample_proposal("promote_to_core", "normal", false);
        proposal.proposed_patch = Some("@@ questionable patch @@".to_string());
        let triage = decide_curate_triage(&proposal, None);
        assert_eq!(triage.resolution_status(), "deferred");
    }

    #[test]
    fn cabinet_update_is_deferred() {
        let proposal = sample_proposal("cabinet_update", "normal", false);
        let triage = decide_curate_triage(&proposal, None);
        assert_eq!(triage.resolution_status(), "deferred");
    }

    #[test]
    fn watch_observation_unspecified_is_deferred() {
        let proposal = sample_proposal("unspecified", "normal", false);
        let triage = decide_curate_triage(&proposal, Some("watch_observation"));
        assert_eq!(triage.resolution_status(), "deferred");
    }

    #[test]
    fn aggregate_resolution_status_reports_mixed_outcomes() {
        let status = aggregate_resolution_status(&[
            CurateProposalOutcome {
                proposal_id: Uuid::new_v4(),
                status: "retained_local".to_string(),
                suggested_action: "unspecified".to_string(),
                triage: "retain_profile_local".to_string(),
                result_path: None,
                error: None,
            },
            CurateProposalOutcome {
                proposal_id: Uuid::new_v4(),
                status: "deferred".to_string(),
                suggested_action: "cabinet_update".to_string(),
                triage: "defer".to_string(),
                result_path: None,
                error: None,
            },
        ]);
        assert_eq!(status, "mixed");
    }
}
