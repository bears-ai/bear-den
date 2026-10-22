//! Owner-bound candidate review; source locators come only from protected storage.

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_memory::{personal_candidates, resolve_authenticated_user_entity, MemoryStoreManager};
use sqlx::PgPool;

use super::{
    authority, CandidateReviewDecision, CandidateReviewId, CandidateReviewReceipt,
    PersonalCandidateId,
};

/// Review an unpublished candidate as its authenticated owner, never as a model tool.
///
/// `user_id` must come from the trusted server principal, not client-selected identity.
/// Every call requires current Bear membership, a current person binding and an owned
/// candidate. Reject/Expire need no access to the historical source. New Defer/Resume
/// decisions additionally require an owned active conversation with an existing Bear-owned
/// hat; memory rechecks capture consent, candidate origin and the exact live SQLite note.
///
/// A matching latest decision replays its original immutable receipt regardless of the
/// supplied review CAS; every new decision requires the exact `expected_review` head.
/// This service also checks current conversation ownership/activity for Defer/Resume
/// replays: only the latest review ID, not its decision, is exposed by the candidate API.
/// A replay is historical, not proof of current consent/source eligibility or resurrection.
/// Expire closes an unpublished candidate; it is neither durable-fact TTL nor erasure.
///
/// The owning task holds Postgres authority locks through the SQLite operation even if
/// the caller cancels. After a lost result, inspect the owned candidate before retrying.
pub async fn review_candidate(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
    candidate_id: PersonalCandidateId,
    decision: CandidateReviewDecision,
    expected_review: Option<CandidateReviewId>,
) -> Result<CandidateReviewReceipt, DenError> {
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let mut locked = authority::lock_member(&pool, bear_id, user_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, user_id).await?;
        let candidate = personal_candidates::get_candidate(&store, &binding, candidate_id)
            .await?
            .ok_or_else(|| {
                DenError::Authorization("Personal-memory review requires an owned candidate.".into())
            })?;
        if matches!(
            decision,
            CandidateReviewDecision::Defer | CandidateReviewDecision::Resume
        ) {
            locked
                .lock_owned_conversation(candidate.source_conversation_id())
                .await?;
        }
        let reviewed = personal_candidates::review_candidate(
            &store,
            &binding,
            candidate_id,
            decision,
            expected_review,
        )
        .await;
        drop(locked);
        reviewed
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory review outcome is unavailable; inspect the owned candidate before retrying."
                .into(),
        )
    })?
}
