//! Routes Bear cognition writes to per-Bear SQLite when `AGENT_RUNTIME=native`.

use den_core::{config::Config, DenError};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use den_memory::{
    self as store, complete_reflection_run_outcome, create_memory_observation,
    create_memory_proposal, create_reflection_run_outcome, create_verified_hat_proposal,
    get_memory_proposal, list_memory_proposals, mark_observation_review_queued,
    resolve_memory_proposal, MemoryStoreManager, SqliteMemoryProposal, VerifiedHatProposalSource,
};
use den_service::bears::BearProfile;
use den_service::memory_proposals::{
    CreateMemoryProposal, MemoryProposalRow, ProposalResolutionParams,
};

use crate::bear_observations::{self, BearObservationRow, CreateBearObservation};

pub async fn create_proposal(
    _pool: &PgPool,
    _config: &Config,
    stores: &MemoryStoreManager,
    params: CreateMemoryProposal<'_>,
) -> Result<MemoryProposalRow, DenError> {
    let store = stores.store_for_bear(params.bear_id).await?;
    let payload = json!({
        "source_profile": params.source_profile.as_str(),
        "source_agent_id": params.source_agent_id,
        "source_paths": params.source_paths,
        "source_refs": params.source_refs,
        "target_ref": params.target_ref,
        "title": params.title,
        "summary": params.summary,
        "rationale": params.rationale,
        "proposed_content": params.proposed_content,
        "proposed_patch": params.proposed_patch,
        "refs": params.refs,
        "suggested_action": params.suggested_action,
        "sensitivity": params.sensitivity,
        "requires_human": params.requires_human,
    });
    let sqlite = create_memory_proposal(
        &store,
        params.suggested_action,
        params.sensitivity,
        params.requires_human,
        &payload,
    )
    .await?;
    sqlite_proposal_to_row(params.bear_id, &sqlite, params.source_profile)
}

/// Store Den-verified canonical source/hat columns independently of model-provided
/// proposal text. There is intentionally no publication in this intake path.
pub async fn create_verified_proposal(
    stores: &MemoryStoreManager,
    params: CreateMemoryProposal<'_>,
    verified: VerifiedHatProposalSource,
) -> Result<MemoryProposalRow, DenError> {
    if params.source_profile != BearProfile::Pair
        || params.suggested_action != "propose_hat"
        || !params.source_paths.is_empty()
        || params.target_ref.is_some()
        || params.proposed_patch.is_some()
    {
        return Err(DenError::ValidationError(
            "verified hat intake requires a Pair source and no path or patch".into(),
        ));
    }
    let store = stores.store_for_bear(params.bear_id).await?;
    let payload = json!({
        "source_profile": params.source_profile.as_str(),
        "source_agent_id": params.source_agent_id,
        "source_paths": params.source_paths,
        "source_refs": params.source_refs,
        "target_ref": params.target_ref,
        "title": params.title,
        "summary": params.summary,
        "rationale": params.rationale,
        "proposed_content": params.proposed_content,
        "proposed_patch": params.proposed_patch,
        "refs": params.refs,
        "suggested_action": "propose_hat",
        "sensitivity": params.sensitivity,
        "requires_human": params.requires_human,
    });
    let sqlite = create_verified_hat_proposal(
        &store,
        params.sensitivity,
        params.requires_human,
        &payload,
        verified,
    )
    .await?;
    sqlite_proposal_to_row(params.bear_id, &sqlite, params.source_profile)
}

pub async fn create_observation(
    _pool: &PgPool,
    _config: &Config,
    stores: &MemoryStoreManager,
    params: CreateBearObservation<'_>,
) -> Result<BearObservationRow, DenError> {
    let store = stores.store_for_bear(params.bear_id).await?;
    let logical_path = bear_observations::observation_logical_path(params.observation_id);
    let sqlite = create_memory_observation(
        &store,
        params.observation_id,
        params.summary,
        params.salience,
        &logical_path,
        &params.source,
    )
    .await?;
    Ok(sqlite_observation_to_row(params.bear_id, &sqlite))
}

pub async fn get_observation(
    _pool: &PgPool,
    _config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    observation_id: &str,
) -> Result<Option<BearObservationRow>, DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    let sqlite = store::get_memory_observation(&store, observation_id).await?;
    Ok(sqlite.map(|row| sqlite_observation_to_row(bear_id, &row)))
}

pub async fn mark_observation_review_queued_for_bear(
    _config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    observation_id: &str,
    proposal_id: Uuid,
) -> Result<(), DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    mark_observation_review_queued(&store, observation_id, &proposal_id.to_string()).await
}

pub async fn list_proposals(
    _pool: &PgPool,
    _config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<MemoryProposalRow>, DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    let rows = list_memory_proposals(&store, status, limit).await?;
    rows.into_iter()
        .map(|row| sqlite_proposal_to_row(bear_id, &row, BearProfile::Curate))
        .collect()
}

pub async fn get_proposal(
    _pool: &PgPool,
    _config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    proposal_id: Uuid,
) -> Result<Option<MemoryProposalRow>, DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    get_memory_proposal(&store, &proposal_id.to_string())
        .await?
        .map(|row| sqlite_proposal_to_row(bear_id, &row, BearProfile::Curate))
        .transpose()
}

pub async fn resolve_proposal(
    _pool: &PgPool,
    _config: &Config,
    stores: &MemoryStoreManager,
    params: ProposalResolutionParams<'_>,
) -> Result<MemoryProposalRow, DenError> {
    let store = stores.store_for_bear(params.bear_id).await?;
    let review_payload = json!({
        "reviewer_profile": params.reviewer_profile.as_str(),
        "reviewer_agent_id": params.reviewer_agent_id,
        "review_notes": params.review_notes,
        "decision_summary": params.decision_summary,
        "result_path": params.result_path,
        "result_commit": params.result_commit,
    });
    let sqlite = resolve_memory_proposal(
        &store,
        &params.proposal_id.to_string(),
        params.status,
        &review_payload,
    )
    .await?;
    sqlite_proposal_to_row(params.bear_id, &sqlite, params.reviewer_profile)
}

pub async fn record_reflection_outcome_start(
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    run_id: &str,
    lane: &str,
    trigger: &str,
    input_summary: Option<&str>,
) -> Result<(), DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    if store::reflection_outcomes::reflection_outcome_exists(&store, run_id).await? {
        return Ok(());
    }
    create_reflection_run_outcome(&store, run_id, lane, trigger, input_summary).await
}

pub async fn record_reflection_outcome_complete(
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    run_id: &str,
    status: &str,
    output_summary: Option<&str>,
    proposal_ids: &[String],
) -> Result<(), DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    complete_reflection_run_outcome(&store, run_id, status, output_summary, proposal_ids).await
}

#[derive(Debug, Deserialize)]
struct SqliteProposalPayload {
    source_profile: Option<String>,
    source_agent_id: Option<String>,
    #[serde(default)]
    source_paths: Vec<String>,
    #[serde(default = "empty_json_object")]
    source_refs: Value,
    suggested_action: Option<String>,
    target_ref: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    rationale: Option<String>,
    proposed_content: Option<String>,
    proposed_patch: Option<String>,
    #[serde(default = "empty_json_object")]
    refs: Value,
    sensitivity: Option<String>,
    #[serde(default)]
    requires_human: bool,
}

fn empty_json_object() -> Value {
    json!({})
}

fn sqlite_proposal_to_row(
    bear_id: Uuid,
    sqlite: &SqliteMemoryProposal,
    source_profile: BearProfile,
) -> Result<MemoryProposalRow, DenError> {
    let payload: SqliteProposalPayload = serde_json::from_value(sqlite.payload_json.clone())
        .map_err(|err| DenError::Parsing(format!("invalid memory proposal payload: {err}")))?;
    let id = Uuid::parse_str(&sqlite.proposal_id).map_err(|err| {
        DenError::Parsing(format!(
            "invalid memory proposal id {}: {err}",
            sqlite.proposal_id
        ))
    })?;
    Ok(MemoryProposalRow {
        id,
        bear_id,
        source_profile: payload
            .source_profile
            .unwrap_or_else(|| source_profile.as_str().to_string()),
        source_agent_id: payload.source_agent_id,
        source_paths: payload.source_paths,
        source_refs: payload.source_refs,
        verified_hat_source: sqlite.verified_hat_source,
        proposal_type: "memory_review".to_string(),
        suggested_action: payload
            .suggested_action
            .unwrap_or_else(|| "review".to_string()),
        target_ref: payload.target_ref,
        title: payload.title.unwrap_or_else(|| "proposal".to_string()),
        summary: payload.summary.unwrap_or_default(),
        rationale: payload.rationale.unwrap_or_default(),
        proposed_content: payload.proposed_content,
        proposed_patch: payload.proposed_patch,
        refs: payload.refs,
        sensitivity: payload.sensitivity.unwrap_or_else(|| "normal".to_string()),
        requires_human: payload.requires_human,
        status: sqlite.status.clone(),
        reviewer_profile: None,
        reviewer_agent_id: None,
        review_notes: None,
        decision_summary: None,
        result_path: None,
        result_commit: None,
        created_at: time::OffsetDateTime::now_utc(),
        reviewed_at: None,
    })
}

fn sqlite_observation_to_row(
    bear_id: Uuid,
    sqlite: &store::SqliteMemoryObservation,
) -> BearObservationRow {
    // SQLite observations predate the Postgres `bear_observations` row id/salience columns.
    // The native bridge preserves the stable `observation_id` and source payload, and supplies
    // UI-only row metadata for callers that expect the Postgres-shaped DTO.
    BearObservationRow {
        id: Uuid::new_v4(),
        bear_id,
        observation_id: sqlite.observation_id.clone(),
        summary: sqlite.summary.clone(),
        salience: "normal".to_string(),
        payload_ref: None,
        source: sqlite.source_json.clone(),
        logical_path: sqlite.logical_path.clone(),
        status: sqlite.status.as_str().to_string(),
        proposal_id: sqlite
            .proposal_id
            .as_ref()
            .and_then(|id| Uuid::parse_str(id).ok()),
        created_at: time::OffsetDateTime::now_utc(),
        reviewed_at: None,
    }
}
