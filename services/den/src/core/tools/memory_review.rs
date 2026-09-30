//! `den`-side wiring for the memory review/curation tools and observations.
//!
//! Orchestration (gating, validation, projection-scope computation) lives in
//! `den-tools`; this module provides the concrete [`MemoryReviewStore`] —
//! composing proposal/observation persistence, the native/legacy core-update
//! paths, the memory-curate enqueue, and `conversation_events` projections —
//! wired into the dispatcher via `DenToolContext`.

use serde_json::{json, Value};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::ids::BearId;
use den_core::tools::review::{
    ApplyCoreUpdateRequest, MarkMemoryLifecycleRequest, MemoryProposalStatus, MemoryReviewStore,
    MemorySensitivity, ObservationRecord, ObservationWriteRequest, ProposalProjection,
    RequestReviewRequest, ResolveProposalRequest,
};

use crate::{config::Config, errors::DenError};
use den_memory::{mark_memory_record_lifecycle, MemoryStoreManager};
use den_runtime::{
    bear_observations::{self, BearObservationRow},
    memory::{
        create_observation, create_proposal, get_observation, get_proposal as db_get_proposal,
        list_proposals as db_list_proposals, mark_observation_review_queued_for_bear,
        promote_core_content_at_path, resolve_proposal as db_resolve_proposal,
    },
    reflection_conductor::{self, ProposalEnqueueParams},
};
use den_service::{
    bears::{hats, BearProfile},
    conversation::events::{
        memory_proposal_resolved_projection, memory_review_requested_projection,
        project_to_conversation, ProjectionProvenance, ProjectionSource,
    },
    memory_proposals::{CreateMemoryProposal, MemoryProposalRow, ProposalResolutionParams},
};

fn observation_record(row: &BearObservationRow) -> ObservationRecord {
    ObservationRecord {
        bear_id: row.bear_id,
        observation_id: row.observation_id.clone(),
        summary: row.summary.clone(),
        salience: row.salience.clone(),
        payload_ref: row.payload_ref.clone(),
        logical_path: row.logical_path.clone(),
        status: row.status.clone(),
        proposal_id: row.proposal_id,
    }
}

fn observation_requires_human(salience: &str) -> bool {
    matches!(salience, "high" | "critical")
}

fn sensitivity_requires_human(sensitivity: MemorySensitivity) -> bool {
    sensitivity != MemorySensitivity::Normal
}

/// Concrete [`MemoryReviewStore`] over the runtime pool/config/stores.
pub(crate) struct DenMemoryReviewStore<'a> {
    pool: &'a PgPool,
    config: &'a Config,
    stores: &'a MemoryStoreManager,
}

impl<'a> DenMemoryReviewStore<'a> {
    pub(crate) fn new(
        pool: &'a PgPool,
        config: &'a Config,
        stores: &'a MemoryStoreManager,
    ) -> Self {
        Self {
            pool,
            config,
            stores,
        }
    }

    fn provenance(&self, projection: &ProposalProjection) -> ProjectionProvenance {
        ProjectionProvenance {
            source: ProjectionSource::DenTools,
            scope_id: projection.scope_id.clone(),
        }
    }

    fn project_resolved(&self, projection: &ProposalProjection, resolved: &MemoryProposalRow) {
        project_to_conversation(
            self.pool,
            resolved.bear_id,
            Some(projection.user_id),
            projection.conversation_id.as_deref(),
            memory_proposal_resolved_projection(
                self.provenance(projection),
                resolved.id,
                &resolved.source_profile,
                &resolved.suggested_action,
                &resolved.title,
                &resolved.status,
                resolved.reviewer_profile.clone(),
                resolved.result_path.clone(),
                resolved.result_commit.clone(),
            ),
        );
    }

    async fn enqueue_observation_review(
        &self,
        request: &ObservationWriteRequest,
        observation: &BearObservationRow,
        salience: &str,
    ) -> Result<MemoryProposalRow, DenError> {
        let requires_human = observation_requires_human(salience);
        let conversation_id = request.conversation_id.clone();
        let proposal = create_proposal(
            self.pool,
            self.config,
            self.stores,
            CreateMemoryProposal {
                bear_id: request.bear_id,
                source_profile: BearProfile::Watch,
                source_agent_id: Some(request.binding_id.clone()),
                source_paths: vec![observation.logical_path.clone()],
                source_refs: json!({
                    "observation_id": observation.observation_id,
                    "observation_row_id": observation.id,
                    "conversation_id": conversation_id,
                    "session_id": request.session_id,
                    "request_id": request.request_id,
                }),
                suggested_action: if requires_human {
                    "human_review"
                } else {
                    "unspecified"
                },
                target_ref: None,
                title: &format!("Review watch observation: {}", observation.observation_id),
                summary: observation.summary.as_str(),
                rationale: "Watch recorded an inbound observation that may warrant curate review.",
                proposed_content: None,
                proposed_patch: None,
                refs: json!({
                    "observation_id": observation.observation_id,
                    "salience": salience,
                    "payload_ref": observation.payload_ref,
                    "logical_path": observation.logical_path,
                }),
                sensitivity: "normal",
                requires_human,
                project_to_conversation: conversation_id.is_some(),
            },
        )
        .await?;

        let reflection_date = OffsetDateTime::now_utc().date();
        let conversation_key = format!("memory_curate:{reflection_date}");
        reflection_conductor::enqueue_memory_curate_for_proposals(
            self.pool,
            ProposalEnqueueParams {
                bear_id: request.bear_id,
                binding_id: Some(request.binding_id.as_str()),
                conversation_id: conversation_id.as_deref(),
                conversation_key: Some(&conversation_key),
                conversation_date: Some(reflection_date),
                trigger: "watch_observation",
                proposal_ids: vec![proposal.id],
            },
        )
        .await?;

        Ok(proposal)
    }
}

impl MemoryReviewStore for DenMemoryReviewStore<'_> {
    async fn find_observation(
        &self,
        bear_id: Uuid,
        observation_id: &str,
    ) -> Result<Option<ObservationRecord>, DenError> {
        let existing =
            get_observation(self.pool, self.config, self.stores, bear_id, observation_id).await?;
        Ok(existing.as_ref().map(observation_record))
    }

    async fn record_observation(
        &self,
        request: ObservationWriteRequest,
    ) -> Result<ObservationRecord, DenError> {
        let salience = request.salience.clone();
        let observation = create_observation(
            self.pool,
            self.config,
            self.stores,
            bear_observations::CreateBearObservation {
                bear_id: request.bear_id,
                observation_id: &request.observation_id,
                summary: &request.summary,
                salience: &salience,
                payload_ref: request.payload_ref.as_deref(),
                source: request.source.clone(),
            },
        )
        .await?;

        let proposal = self
            .enqueue_observation_review(&request, &observation, &salience)
            .await?;

        mark_observation_review_queued_for_bear(
            self.config,
            self.stores,
            request.bear_id,
            &observation.observation_id,
            proposal.id,
        )
        .await?;
        let mut observation = observation;
        observation.status = "review_queued".to_string();
        observation.proposal_id = Some(proposal.id);
        Ok(observation_record(&observation))
    }

    async fn list_proposals(
        &self,
        bear_id: Uuid,
        status: Option<MemoryProposalStatus>,
        limit: i64,
    ) -> Result<Value, DenError> {
        let proposals = db_list_proposals(
            self.pool,
            self.config,
            self.stores,
            bear_id,
            status.map(MemoryProposalStatus::as_str),
            limit,
        )
        .await?;
        Ok(json!(proposals))
    }

    async fn get_proposal(
        &self,
        bear_id: Uuid,
        proposal_id: Uuid,
    ) -> Result<Option<Value>, DenError> {
        let proposal =
            db_get_proposal(self.pool, self.config, self.stores, bear_id, proposal_id).await?;
        Ok(proposal.map(|proposal| json!(proposal)))
    }

    async fn resolve_proposal(&self, request: ResolveProposalRequest) -> Result<Value, DenError> {
        let resolved = db_resolve_proposal(
            self.pool,
            self.config,
            self.stores,
            ProposalResolutionParams {
                bear_id: request.bear_id,
                proposal_id: request.proposal_id,
                reviewer_profile: request.reviewer_profile,
                reviewer_agent_id: Some(request.binding_id.as_str()),
                status: request.status.as_str(),
                review_notes: request.review_notes.as_deref(),
                decision_summary: request.decision_summary.as_deref(),
                result_path: None,
                result_commit: None,
                project_to_conversation: false,
            },
        )
        .await?;
        self.project_resolved(&request.projection, &resolved);
        Ok(json!(resolved))
    }

    async fn request_review(&self, request: RequestReviewRequest) -> Result<Value, DenError> {
        let proposal = create_proposal(
            self.pool,
            self.config,
            self.stores,
            CreateMemoryProposal {
                bear_id: request.bear_id,
                source_profile: request.source_profile,
                source_agent_id: request.binding_id.clone(),
                source_paths: request.source_paths.clone(),
                source_refs: request.source_refs.clone(),
                suggested_action: request.suggested_action.as_str(),
                target_ref: request.target_ref.as_deref(),
                title: &request.title,
                summary: &request.summary,
                rationale: &request.rationale,
                proposed_content: request.proposed_content.as_deref(),
                proposed_patch: request.proposed_patch.as_deref(),
                refs: request.refs.clone(),
                sensitivity: request.sensitivity.as_str(),
                requires_human: request.requires_human,
                project_to_conversation: false,
            },
        )
        .await?;
        project_to_conversation(
            self.pool,
            proposal.bear_id,
            Some(request.projection.user_id),
            request.projection.conversation_id.as_deref(),
            memory_review_requested_projection(
                self.provenance(&request.projection),
                proposal.id,
                &proposal.source_profile,
                &proposal.suggested_action,
                &proposal.title,
                &proposal.status,
                proposal.source_paths.clone(),
            ),
        );
        Ok(json!(proposal))
    }

    async fn mark_memory_lifecycle(
        &self,
        request: MarkMemoryLifecycleRequest,
    ) -> Result<Value, DenError> {
        let store = self.stores.store_for_bear(request.bear_id).await?;
        let record = mark_memory_record_lifecycle(
            &store,
            &request.memory_id,
            request.status.as_str(),
            request.reason.as_deref(),
        )
        .await?;
        reflection_conductor::enqueue_recall_index_if_enabled(
            self.pool,
            self.config,
            request.bear_id,
            "memory_mark_lifecycle",
        )
        .await;
        Ok(json!({
            "memory_id": record.memory_id,
            "logical_path": record.logical_path,
            "kind": record.kind,
            "salience": record.salience,
            "supersedes_memory_id": record.supersedes_memory_id,
            "invalid_at": record.invalid_at,
            "lifecycle_status": record.lifecycle_status,
            "freshness_trend": record.freshness_trend,
            "reviewer_profile": request.reviewer_profile.as_str(),
            "reviewer_agent_id": request.binding_id,
        }))
    }

    async fn apply_core_update(&self, request: ApplyCoreUpdateRequest) -> Result<Value, DenError> {
        // A legacy proposal can name a profile path but cannot prove ownership
        // of canonical conversation/run notes or review the Core/Work audience.
        // Configured Bears use the explicit, human-reviewed hat → core path.
        if !hats::list_hats(self.pool, BearId::new(request.bear_id))
            .await?
            .is_empty()
        {
            return Err(DenError::Authorization(
                "Bear-wide core updates for configured hats require an explicit Bear-admin review of canonical hat knowledge".into(),
            ));
        }
        let proposal = db_get_proposal(
            self.pool,
            self.config,
            self.stores,
            request.bear_id,
            request.proposal_id,
        )
        .await?
        .ok_or_else(|| DenError::NotFound("memory proposal not found".to_string()))?;

        if proposal.requires_human
            || MemorySensitivity::parse(&proposal.sensitivity)
                .map(sensitivity_requires_human)
                .unwrap_or(true)
        {
            return Err(DenError::ValidationError(
                "proposal requires human review; resolve as needs_human_review instead of applying a core update autonomously".to_string(),
            ));
        }
        if !request.target_path.trim().starts_with("core/") {
            return Err(DenError::ValidationError(
                "target_path must be under core/".to_string(),
            ));
        }
        if request.mode != "append_section" && request.mode != "create_file" {
            return Err(DenError::ValidationError(
                "native SQLite core updates currently support append_section or create_file; replace_text must be proposed for human review".to_string(),
            ));
        }

        let content = request.body.clone().unwrap_or_else(|| {
            format!(
                "Applied from proposal `{}` via native SQLite promotion.",
                proposal.id
            )
        });
        let kind = request
            .target_path
            .split('/')
            .next_back()
            .unwrap_or("note")
            .trim_end_matches(".md");
        let (memory_id, promotion_id) = promote_core_content_at_path(
            self.stores,
            request.bear_id,
            &proposal.id.to_string(),
            request.target_path.as_str(),
            kind,
            &content,
            request.reviewer_profile.as_str(),
        )
        .await?;
        let resolved = db_resolve_proposal(
            self.pool,
            self.config,
            self.stores,
            ProposalResolutionParams {
                bear_id: request.bear_id,
                proposal_id: proposal.id,
                reviewer_profile: request.reviewer_profile,
                reviewer_agent_id: Some(request.binding_id.as_str()),
                status: "approved",
                review_notes: request.review_notes.as_deref(),
                decision_summary: Some("Applied reviewed memory proposal to core (SQLite)."),
                result_path: Some(request.target_path.as_str()),
                result_commit: None,
                project_to_conversation: false,
            },
        )
        .await?;
        self.project_resolved(&request.projection, &resolved);
        Ok(json!({
            "bear_id": request.bear_id,
            "proposal": resolved,
            "core_update": {
                "path": request.target_path,
                "memory_id": memory_id,
                "promotion_id": promotion_id,
            },
        }))
    }
}
