//! Explicit owner decisions; source provenance is selected from protected candidates only.

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_memory::{
    personal_candidates::{self, CandidateState, VerifiedPersonalSource},
    personal_facts, resolve_authenticated_user_entity, MemoryStoreManager,
};
use sqlx::PgPool;

use super::{authority, PersonalCandidateId, PersonalFactHead, PersonalFactId, PublicationReceipt};

/// Publish only after explicit approval by the candidate's authenticated owner.
///
/// `user_id` must come from the trusted server principal, never model/client-selected
/// identity. The candidate ID is only a locator; its protected source supplies provenance.
/// New decisions require current membership, binding, and an owned active conversation.
/// Memory rechecks capture, candidate generation, exact source eligibility and head CAS
/// while atomically publishing the fact and invalidating any previous version.
///
/// A published candidate replays its immutable receipt without requiring its original
/// source to remain online or capture to remain enabled. The receipt does not mean the
/// fact is still active. The owning task holds authority locks through completion even
/// if the caller cancels; after a lost result, retry the same candidate to recover it.
/// This is not an automatic Curate, HTTP, or model-caller entry point.
pub async fn publish_preferred_name(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
    candidate_id: PersonalCandidateId,
    expected_head: Option<PersonalFactId>,
) -> Result<PublicationReceipt, DenError> {
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let mut locked = authority::lock_member(&pool, bear_id, user_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, user_id).await?;
        let candidate = personal_candidates::get_candidate(&store, &binding, candidate_id)
            .await?
            .ok_or_else(|| {
                DenError::Authorization(
                    "Personal-memory approval requires an owned candidate.".into(),
                )
            })?;
        if candidate.state() != CandidateState::Published {
            locked
                .lock_owned_conversation(candidate.source_conversation_id())
                .await?;
        }
        let source = VerifiedPersonalSource::for_authorized_conversation(
            &binding,
            candidate.source_conversation_id(),
            candidate.source_memory_id(),
        );
        let published =
            personal_facts::publish_preferred_name(&store, &source, candidate_id, expected_head)
                .await;
        drop(locked);
        published
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory approval outcome is unavailable; retry the same owned candidate to recover its receipt."
                .into(),
        )
    })?
}

/// Read the authenticated member's current head, including retracted/suppressed states.
///
/// Only current membership and binding are required, not capture consent or access to
/// the original source conversation. Protected values are not a generic recall surface.
pub async fn current_preferred_name(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
) -> Result<Option<PersonalFactHead>, DenError> {
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let locked = authority::lock_member(&pool, bear_id, user_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, user_id).await?;
        let current = personal_facts::current_preferred_name(&store, &binding).await;
        drop(locked);
        current
    })
    .await
    .map_err(|_| {
        DenError::System("Personal-memory current head is unavailable; retry the read.".into())
    })?
}

/// Retract the authenticated owner's exact current head, even while capture is disabled.
///
/// No original-source access is required. A stale head cannot retract a newer version.
/// This retains protected history, not transcript erasure. Once started, the owning task
/// continues on caller cancellation; inspect the current head before retrying a lost result.
pub async fn retract_preferred_name(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
    expected: PersonalFactId,
) -> Result<bool, DenError> {
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let locked = authority::lock_member(&pool, bear_id, user_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, user_id).await?;
        let retracted = personal_facts::retract_preferred_name(&store, &binding, expected).await;
        drop(locked);
        retracted
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory retraction outcome is unavailable; inspect the current head before retrying."
                .into(),
        )
    })?
}
