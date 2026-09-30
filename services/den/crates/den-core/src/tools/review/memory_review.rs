//! Memory review/curation tools — orchestration layer.
//!
//! `list_proposals` / `read_proposal` / `resolve_proposal` / `request_review`.
//! Runtime-agnostic: role gating, argument validation, and
//! projection-scope computation over the [`MemoryReviewStore`] seam; the `den`
//! impl owns the capability calls and `conversation_events` projections.

use crate::{BearProfile, DenError};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::tools::{
    context::DenToolInvocationContext,
    memory::source_client_session_id,
    support::{clean_optional, validate_bounded_text, validate_optional_object},
};

use super::store::{
    MarkMemoryLifecycleRequest, MemoryLifecycleStatus, MemoryProposalResolution,
    MemoryProposalStatus, MemoryReviewStore, MemorySensitivity, MemorySuggestedAction,
    ProposalProjection, RequestReviewRequest, ResolveProposalRequest,
};

#[derive(Debug, Deserialize)]
pub struct MemoryListProposalsArguments {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryReadProposalArguments {
    pub proposal_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct MemoryResolveProposalArguments {
    pub proposal_id: Uuid,
    pub status: String,
    #[serde(default)]
    pub review_notes: Option<String>,
    #[serde(default)]
    pub decision_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryMarkLifecycleArguments {
    pub memory_id: String,
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryRequestReviewArguments {
    #[serde(default)]
    pub source_paths: Vec<String>,
    #[serde(default)]
    pub source_memory_id: Option<Uuid>,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub suggested_action: Option<String>,
    #[serde(default)]
    pub target_ref: Option<String>,
    #[serde(default)]
    pub refs: Option<Value>,
    #[serde(default)]
    pub sensitivity: Option<String>,
    #[serde(default)]
    pub requires_human: bool,
    #[serde(default)]
    pub proposed_content: Option<String>,
    #[serde(default)]
    pub proposed_patch: Option<String>,
}

fn normalize_suggested_action(value: Option<&str>) -> Result<MemorySuggestedAction, DenError> {
    let value = value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unspecified");
    MemorySuggestedAction::parse(value).ok_or_else(|| {
        DenError::ValidationError(format!(
            "suggested_action must be one of the supported memory review actions; got {value}"
        ))
    })
}

fn normalize_memory_sensitivity(value: Option<&str>) -> Result<MemorySensitivity, DenError> {
    let value = value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("normal");
    MemorySensitivity::parse(value).ok_or_else(|| {
        DenError::ValidationError(format!(
            "sensitivity must be normal, person, secret_risk, external_untrusted, or unknown; got {value}"
        ))
    })
}

fn validate_optional_review_text(
    field: &str,
    value: Option<&str>,
    max_len: usize,
) -> Result<Option<String>, DenError> {
    value
        .map(|value| validate_bounded_text(field, value, 0, max_len))
        .transpose()
}

fn normalize_proposal_status_filter(
    value: Option<&str>,
) -> Result<Option<MemoryProposalStatus>, DenError> {
    let Some(value) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    MemoryProposalStatus::parse(value).map(Some).ok_or_else(|| {
        DenError::ValidationError(format!(
            "status must be pending, rejected, retained_local, deferred, superseded, or needs_human_review; got {value}"
        ))
    })
}

fn bounded_proposal_limit(value: Option<i64>) -> i64 {
    value.unwrap_or(50).clamp(1, 200)
}

/// The `conversation_events` projection scope id for this invocation.
fn projection_scope_id(
    context: &DenToolInvocationContext,
    bear_id: Uuid,
    role: BearProfile,
) -> String {
    source_client_session_id(context)
        .or_else(|| clean_optional(&context.session_id))
        .unwrap_or_else(|| format!("bear:{}:role:{}", bear_id, role.as_str()))
}

fn projection(
    context: &DenToolInvocationContext,
    bear_id: Uuid,
    role: BearProfile,
) -> ProposalProjection {
    ProposalProjection {
        user_id: context.user_id,
        conversation_id: clean_optional(&context.conversation_id),
        scope_id: projection_scope_id(context, bear_id, role),
    }
}

pub async fn mark_memory_lifecycle(
    store: &impl MemoryReviewStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, DenError> {
    if role != BearProfile::Curate {
        return Err(DenError::Authorization(
            "den.memory.mark_lifecycle is available only to curate".to_string(),
        ));
    }
    let args: MemoryMarkLifecycleArguments = serde_json::from_value(arguments)?;
    let memory_id = validate_bounded_text("memory_id", &args.memory_id, 1, 200)?;
    let status = MemoryLifecycleStatus::parse(args.status.trim()).ok_or_else(|| {
        DenError::ValidationError(
            "status must be active, stale, superseded, archived, or archive-candidate".to_string(),
        )
    })?;
    let reason = args
        .reason
        .as_deref()
        .map(|value| validate_bounded_text("reason", value, 0, 1_000))
        .transpose()?;
    let record = store
        .mark_memory_lifecycle(MarkMemoryLifecycleRequest {
            bear_id: context.bear_id,
            reviewer_profile: role,
            binding_id: context.binding_id.clone(),
            memory_id,
            status,
            reason,
        })
        .await?;
    Ok(json!({ "bear_id": context.bear_id, "record": record }))
}

pub async fn list_memory_proposals(
    store: &impl MemoryReviewStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, DenError> {
    if role != BearProfile::Curate {
        return Err(DenError::Authorization(
            "den.memory.list_proposals is available only to curate".to_string(),
        ));
    }
    let args: MemoryListProposalsArguments = serde_json::from_value(arguments)?;
    let status = normalize_proposal_status_filter(args.status.as_deref())?;
    let proposals = store
        .list_proposals(context.bear_id, status, bounded_proposal_limit(args.limit))
        .await?;
    Ok(json!({ "bear_id": context.bear_id, "proposals": proposals }))
}

pub async fn read_memory_proposal(
    store: &impl MemoryReviewStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, DenError> {
    if role != BearProfile::Curate {
        return Err(DenError::Authorization(
            "den.memory.read_proposal is available only to curate".to_string(),
        ));
    }
    let args: MemoryReadProposalArguments = serde_json::from_value(arguments)?;
    let proposal = store
        .get_proposal(context.bear_id, args.proposal_id)
        .await?
        .ok_or_else(|| DenError::NotFound("memory proposal not found".to_string()))?;
    Ok(json!({ "bear_id": context.bear_id, "proposal": proposal }))
}

pub async fn resolve_memory_proposal(
    store: &impl MemoryReviewStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, DenError> {
    if role != BearProfile::Curate {
        return Err(DenError::Authorization(
            "den.memory.resolve_proposal is available only to curate".to_string(),
        ));
    }
    let args: MemoryResolveProposalArguments = serde_json::from_value(arguments)?;
    let status = MemoryProposalResolution::parse(args.status.trim()).ok_or_else(|| {
        DenError::ValidationError(
            "status must be rejected, retained_local, deferred, superseded, or needs_human_review"
                .to_string(),
        )
    })?;
    let proposal = store
        .resolve_proposal(ResolveProposalRequest {
            bear_id: context.bear_id,
            reviewer_profile: role,
            binding_id: context.binding_id.clone(),
            proposal_id: args.proposal_id,
            status,
            review_notes: args.review_notes,
            decision_summary: args.decision_summary,
            projection: projection(context, context.bear_id, role),
        })
        .await?;
    Ok(json!({ "bear_id": context.bear_id, "proposal": proposal }))
}

// `.md` is the canonical, case-sensitive role-memory extension; keep the exact
// original comparison rather than clippy's case-insensitive suggestion.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub async fn request_memory_review(
    store: &impl MemoryReviewStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, DenError> {
    if !matches!(role, BearProfile::Pair) {
        return Err(DenError::Authorization(
            "den.memory.request_review is currently available only to pair".to_string(),
        ));
    }
    let args: MemoryRequestReviewArguments = serde_json::from_value(arguments)?;
    let source_paths = args
        .source_paths
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    if source_paths.is_empty() == args.source_memory_id.is_none() {
        return Err(DenError::ValidationError(
            "provide either one canonical source_memory_id or source_paths, not both".into(),
        ));
    }
    if source_paths.len() > 20 {
        return Err(DenError::ValidationError(
            "source_paths must include at most 20 paths".to_string(),
        ));
    }
    for path in &source_paths {
        if !path.starts_with(role.as_str()) || !path.ends_with(".md") {
            return Err(DenError::ValidationError(format!(
                "source path must be a role-local Markdown path under {}/: {path}",
                role.as_str()
            )));
        }
    }
    let title = validate_bounded_text("title", &args.title, 1, 200)?;
    let summary = validate_bounded_text("summary", &args.summary, 1, 4_000)?;
    let rationale = validate_bounded_text("rationale", &args.rationale, 0, 4_000)?;
    let proposed_content = validate_optional_review_text(
        "proposed_content",
        args.proposed_content.as_deref(),
        20_000,
    )?;
    let proposed_patch =
        validate_optional_review_text("proposed_patch", args.proposed_patch.as_deref(), 20_000)?;
    validate_optional_object("refs", &args.refs)?;
    let suggested_action = normalize_suggested_action(args.suggested_action.as_deref())?;
    if (suggested_action == MemorySuggestedAction::ProposeHat) != args.source_memory_id.is_some() {
        return Err(DenError::ValidationError(
            "propose_hat requires a canonical source_memory_id; path proposals cannot select a hat"
                .into(),
        ));
    }
    if args.source_memory_id.is_some()
        && (args
            .target_ref
            .as_deref()
            .is_some_and(|path| !path.trim().is_empty())
            || proposed_patch.is_some())
    {
        return Err(DenError::ValidationError(
            "verified hat proposals cannot name a target path or supply a patch".into(),
        ));
    }
    let sensitivity = normalize_memory_sensitivity(args.sensitivity.as_deref())?;
    let source_refs = json!({
        "conversation_id": clean_optional(&context.conversation_id),
        "session_id": source_client_session_id(context).or_else(|| clean_optional(&context.session_id)),
        "request_id": context.request_id,
        "runtime_target": context.runtime_target,
    });
    let proposal = store
        .request_review(RequestReviewRequest {
            bear_id: context.bear_id,
            source_profile: role,
            binding_id: clean_optional(&context.binding_id),
            source_paths,
            source_memory_id: args.source_memory_id,
            source_refs,
            suggested_action,
            target_ref: args
                .target_ref
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            title,
            summary,
            rationale,
            proposed_content,
            proposed_patch,
            refs: args.refs.unwrap_or_else(|| json!({})),
            sensitivity,
            requires_human: args.requires_human,
            projection: projection(context, context.bear_id, role),
        })
        .await?;
    Ok(json!({
        "bear_id": context.bear_id,
        "proposal": proposal,
        "note": "Review requested. Reflection/curate decides the final outcome; this did not write core, Cabinet, skills, tasks, observations, or run results."
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_review_defaults_to_safe_action_and_sensitivity() {
        assert_eq!(
            normalize_suggested_action(None).unwrap().as_str(),
            "unspecified"
        );
        assert_eq!(
            normalize_suggested_action(Some("  ")).unwrap().as_str(),
            "unspecified"
        );
        assert_eq!(
            normalize_memory_sensitivity(None).unwrap().as_str(),
            "normal"
        );
        assert_eq!(
            normalize_memory_sensitivity(Some("  ")).unwrap().as_str(),
            "normal"
        );
    }

    #[test]
    fn memory_review_rejects_unknown_action_and_sensitivity() {
        assert!(normalize_suggested_action(Some("archive-index")).is_err());
        assert!(normalize_memory_sensitivity(Some("private")).is_err());
    }

    #[test]
    fn memory_review_bounds_optional_large_text() {
        assert_eq!(
            validate_optional_review_text("proposed_content", Some("ok"), 2).unwrap(),
            Some("ok".to_string())
        );
        assert!(validate_optional_review_text("proposed_content", Some("too long"), 3).is_err());
    }

    #[test]
    fn memory_review_actions_and_sensitivities_preserve_storage_strings() {
        assert_eq!(
            MemorySuggestedAction::ArchiveIndex.as_str(),
            "archive_index"
        );
        assert_eq!(
            MemorySuggestedAction::parse("task_context"),
            Some(MemorySuggestedAction::TaskContext)
        );
        assert_eq!(MemorySuggestedAction::parse("archive-index"), None);
        assert_eq!(
            MemorySensitivity::ExternalUntrusted.as_str(),
            "external_untrusted"
        );
        assert_eq!(
            MemorySensitivity::parse("secret_risk"),
            Some(MemorySensitivity::SecretRisk)
        );
        assert_eq!(MemorySensitivity::parse("private"), None);
    }

    #[test]
    fn memory_review_statuses_preserve_storage_strings_and_reject_unknown_values() {
        assert_eq!(
            MemoryProposalResolution::NeedsHumanReview.as_str(),
            "needs_human_review"
        );
        assert_eq!(
            MemoryProposalResolution::parse("deferred"),
            Some(MemoryProposalResolution::Deferred)
        );
        assert_eq!(MemoryProposalResolution::parse("pending"), None);
        assert_eq!(
            MemoryLifecycleStatus::ArchiveCandidate.as_str(),
            "archive-candidate"
        );
        assert_eq!(
            MemoryLifecycleStatus::parse("archived"),
            Some(MemoryLifecycleStatus::Archived)
        );
        assert_eq!(MemoryLifecycleStatus::parse("deleted"), None);
    }

    #[test]
    fn list_proposals_validates_status_and_bounds_limit() {
        assert_eq!(normalize_proposal_status_filter(None).unwrap(), None);
        assert_eq!(
            normalize_proposal_status_filter(Some(" pending ")).unwrap(),
            Some(MemoryProposalStatus::Pending)
        );
        assert_eq!(
            MemoryProposalStatus::NeedsHumanReview.as_str(),
            "needs_human_review"
        );
        assert!(normalize_proposal_status_filter(Some("done")).is_err());
        assert_eq!(bounded_proposal_limit(None), 50);
        assert_eq!(bounded_proposal_limit(Some(-10)), 1);
        assert_eq!(bounded_proposal_limit(Some(500)), 200);
    }
}
