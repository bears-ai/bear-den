//! `den`-side wiring for the memory review/curation tools and observations.
//!
//! Orchestration (gating, validation, projection-scope computation) lives in
//! `den-tools`; this module provides the concrete [`MemoryReviewStore`] —
//! composing proposal/observation persistence, the memory-curate enqueue,
//! and `conversation_events` projections —
//! wired into the dispatcher via `DenToolContext`.

use serde_json::{json, Value};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::ids::BearId;
use den_core::tools::review::{
    MarkMemoryLifecycleRequest, MemoryProposalStatus, MemoryReviewStore, ObservationRecord,
    ObservationWriteRequest, ProposalProjection, RequestReviewRequest, ResolveProposalRequest,
};

use crate::{config::Config, errors::DenError};
use den_memory::{
    hat_promotion, mark_memory_record_lifecycle, MemorySource, MemoryStoreManager,
    VerifiedHatProposalSource,
};
use den_runtime::{
    bear_observations::{self, BearObservationRow},
    memory::{
        create_observation, create_proposal, create_verified_proposal, get_observation,
        get_proposal as db_get_proposal, list_proposals as db_list_proposals,
        mark_observation_review_queued_for_bear, resolve_proposal as db_resolve_proposal,
    },
    reflection_conductor::{self, ProposalEnqueueParams},
};
use den_service::{
    bears::{
        hats,
        hats::memory_binding::{self, ResolvedMemoryBinding},
        RuntimeContextLabel,
    },
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
                source_profile: RuntimeContextLabel::Observation,
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
        let verified = if let Some(memory_id) = request.source_memory_id {
            let conversation_id =
                request
                    .projection
                    .conversation_id
                    .as_deref()
                    .ok_or_else(|| {
                        DenError::Authorization(
                            "canonical conversation required for hat review".into(),
                        )
                    })?;
            let binding = memory_binding::for_external_conversation(
                self.pool,
                BearId::new(request.bear_id),
                conversation_id,
            )
            .await?;
            let ResolvedMemoryBinding::Bound(grant) = binding;
            let MemorySource::Conversation(source_id) = grant.source() else {
                return Err(DenError::Authorization(
                    "hat review requires a verified conversation source".into(),
                ));
            };
            let hat_id = grant
                .hat_id()
                .ok_or_else(|| DenError::Authorization("bound conversation has no hat".into()))?;
            let hat =
                hats::manage::get_hat(self.pool, BearId::new(request.bear_id), hat_id).await?;
            if !hat.auto_curate_enabled {
                return Err(DenError::Authorization(
                    "automatic memory sharing is not enabled for this hat".into(),
                ));
            }
            let owned = sqlx::query_scalar!(
                "SELECT EXISTS (SELECT 1 FROM conversations WHERE bear_id = $1 AND id = $2
                 AND hat_id = $3 AND status = 'active' AND created_by_user_id = $4) AS \"owned!\"",
                request.bear_id,
                source_id,
                hat_id.as_uuid(),
                request.projection.user_id,
            )
            .fetch_one(self.pool)
            .await?;
            if !owned {
                return Err(DenError::Authorization(
                    "source conversation does not belong to the current human".into(),
                ));
            }
            let store = self.stores.store_for_bear(request.bear_id).await?;
            let candidate = hat_promotion::review_candidate(&store, memory_id).await?;
            if candidate.source != grant.source() {
                return Err(DenError::Authorization(
                    "source note belongs to a different conversation".into(),
                ));
            }
            Some(VerifiedHatProposalSource { memory_id, hat_id })
        } else {
            if !hats::list_hats(self.pool, BearId::new(request.bear_id))
                .await?
                .is_empty()
            {
                return Err(DenError::Authorization(
                    "configured Bears require a canonical source_memory_id for hat proposals"
                        .into(),
                ));
            }
            None
        };
        let params = CreateMemoryProposal {
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
        };
        let proposal = if let Some(verified) = verified {
            create_verified_proposal(self.stores, params, verified).await?
        } else {
            create_proposal(self.pool, self.config, self.stores, params).await?
        };
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
        if verified.is_some() {
            let date = OffsetDateTime::now_utc().date();
            let key = format!("memory_curate:{date}");
            if let Err(error) = reflection_conductor::enqueue_memory_curate_for_proposals(
                self.pool,
                ProposalEnqueueParams {
                    bear_id: request.bear_id,
                    binding_id: None,
                    conversation_id: None,
                    conversation_key: Some(&key),
                    conversation_date: Some(date),
                    trigger: "verified_hat_intake",
                    proposal_ids: vec![proposal.id],
                },
            )
            .await
            {
                tracing::warn!(bear_id = %request.bear_id, proposal_id = %proposal.id,
                    error_kind = ?std::mem::discriminant(&error),
                    "verified hat proposal persists, but Curate enqueue failed");
            }
        }
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
}
