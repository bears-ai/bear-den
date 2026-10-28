//! Owner management holds the reviewed identity through the actual storage operation.

use std::fmt;

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_memory::{
    personal_candidates::{self, PersonalCaptureRevision, VerifiedOwnerSource},
    personal_facts, resolve_authenticated_user_entity, AuthenticatedUserEntity, MemoryStoreManager,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    authority, publication, review, CandidateReviewDecision, CandidateReviewId,
    CandidateReviewReceipt, PersonalCandidate, PersonalCandidateId, PersonalFactHead,
    PersonalFactId, PreferredName, PublicationReceipt,
};

/// Retained server-owned authority snapshot. Never deserialize this from a form.
#[derive(Clone)]
pub struct PersonalMemoryOwner {
    binding: AuthenticatedUserEntity,
    capture_revision: PersonalCaptureRevision,
}

impl fmt::Debug for PersonalMemoryOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PersonalMemoryOwner([redacted])")
    }
}

/// Deliberately projected for the owner only; candidates are the latest 100, not a full export.
#[derive(Debug)]
pub struct PersonalMemoryInspection {
    pub owner: PersonalMemoryOwner,
    pub capture_enabled: bool,
    pub current: Option<PersonalFactHead>,
    pub candidates: Vec<PersonalCandidate>,
}

#[derive(Debug)]
pub enum OwnerMemoryAction {
    SetCapture(bool),
    RequestPreferredName {
        request_id: Uuid,
        name: PreferredName,
    },
    Publish {
        candidate_id: PersonalCandidateId,
        expected_head: Option<PersonalFactId>,
    },
    Review {
        candidate_id: PersonalCandidateId,
        decision: CandidateReviewDecision,
        expected_review: Option<CandidateReviewId>,
    },
    Retract {
        expected_head: PersonalFactId,
    },
}

/// A completed operation's receipt is historical, not a claim of current fact activity.
#[derive(Debug)]
pub enum OwnerMemoryReceipt {
    CaptureChanged,
    CandidateRequested(PersonalCandidateId),
    Published(PublicationReceipt),
    Reviewed(CandidateReviewReceipt),
    Retracted(bool),
}

/// Load only the authenticated member's own state. Bear Admin has no ownership override.
pub async fn inspect_personal_memory(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
) -> Result<PersonalMemoryInspection, DenError> {
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let locked = authority::lock_member(&pool, bear_id, user_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, user_id).await?;
        let capture = personal_candidates::capture_setting(&store, &binding).await?;
        let current = personal_facts::current_preferred_name(&store, &binding).await?;
        let candidates = personal_candidates::list_candidates(&store, &binding, 100).await?;
        drop(locked);
        Ok(PersonalMemoryInspection {
            owner: PersonalMemoryOwner {
                binding,
                capture_revision: capture.revision,
            },
            capture_enabled: capture.enabled,
            current,
            candidates,
        })
    })
    .await
    .map_err(|_| {
        DenError::System("Personal-memory inspection is unavailable; retry the read.".into())
    })?
}

/// Apply a human action to the SAME reviewed person/generation under current membership locks.
/// A fresh principal resolution here would let an old form act on a reconnected identity.
/// Source/head/review CAS and consent checks remain owned by the existing storage operations.
pub async fn apply_owner_memory_action(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
    owner: &PersonalMemoryOwner,
    action: OwnerMemoryAction,
) -> Result<OwnerMemoryReceipt, DenError> {
    if owner.binding.bear_id() != bear_id || owner.binding.user_id() != user_id {
        return Err(DenError::Authorization(
            "Personal-memory owner review is stale or belongs to another account.".into(),
        ));
    }
    let pool = pool.clone();
    let stores = stores.clone();
    let owner = owner.clone();
    tokio::spawn(async move {
        let mut locked = authority::lock_member(&pool, bear_id, user_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = &owner.binding;
        let receipt = match action {
            OwnerMemoryAction::SetCapture(enabled) => {
                personal_candidates::set_capture_enabled_if_current(
                    &store,
                    binding,
                    enabled,
                    owner.capture_revision,
                )
                .await?;
                OwnerMemoryReceipt::CaptureChanged
            }
            OwnerMemoryAction::RequestPreferredName { request_id, name } => {
                let source = VerifiedOwnerSource::for_authorized_owner_request(binding, request_id)
                    .with_capture_revision(owner.capture_revision);
                let candidate =
                    personal_candidates::stage_owner_preferred_name(&store, &source, name).await?;
                OwnerMemoryReceipt::CandidateRequested(candidate.id())
            }
            OwnerMemoryAction::Publish {
                candidate_id,
                expected_head,
            } => OwnerMemoryReceipt::Published(
                publication::publish_for_binding(
                    &mut locked,
                    &store,
                    binding,
                    candidate_id,
                    expected_head,
                )
                .await?,
            ),
            OwnerMemoryAction::Review {
                candidate_id,
                decision,
                expected_review,
            } => OwnerMemoryReceipt::Reviewed(
                review::review_for_binding(
                    &mut locked,
                    &store,
                    binding,
                    candidate_id,
                    decision,
                    expected_review,
                )
                .await?,
            ),
            OwnerMemoryAction::Retract { expected_head } => OwnerMemoryReceipt::Retracted(
                personal_facts::retract_preferred_name(&store, binding, expected_head).await?,
            ),
        };
        drop(locked);
        Ok(receipt)
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory action outcome is unavailable; inspect About you before retrying."
                .into(),
        )
    })?
}
