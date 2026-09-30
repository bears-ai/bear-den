//! Deliberate sharing from reviewed hat knowledge to every Bear member and
//! future authorized Work run. A model cannot supply this human review.

use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};
use den_memory::{
    reviewed_core::{self, CoreHead, CoreReviewOutcome, ReviewCandidate, ReviewedCoreEntry},
    MemoryStoreManager,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::manage::get_hat;
use crate::bears::db::{membership_role_for_user, role_is_bear_admin};

#[cfg(test)]
mod tests;

async fn require_admin(
    pool: &PgPool,
    bear_id: BearId,
    reviewer: UserId,
    hat_id: HatId,
) -> Result<(), DenError> {
    let role = membership_role_for_user(pool, reviewer.get(), bear_id.as_uuid()).await?;
    if !role.is_some_and(|role| role_is_bear_admin(role.as_deref())) {
        return Err(DenError::Authorization(
            "Bear admin review is required to share hat knowledge with the Bear".into(),
        ));
    }
    get_hat(pool, bear_id, hat_id).await?;
    Ok(())
}

pub async fn candidates(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    hat_id: HatId,
) -> Result<Vec<ReviewCandidate>, DenError> {
    require_admin(pool, bear_id, reviewer, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    reviewed_core::candidates(&store, hat_id, 50).await
}

pub async fn candidate(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    hat_id: HatId,
    source_memory_id: Uuid,
) -> Result<ReviewCandidate, DenError> {
    require_admin(pool, bear_id, reviewer, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    reviewed_core::candidate(&store, hat_id, source_memory_id).await
}

pub async fn core_head(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    hat_id: HatId,
    kind: &str,
) -> Result<Option<CoreHead>, DenError> {
    require_admin(pool, bear_id, reviewer, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    reviewed_core::current_core_head(&store, kind).await
}

#[derive(Debug, Clone)]
pub struct CoreReviewDecision {
    pub source_memory_id: Uuid,
    pub hat_id: HatId,
    pub kind: String,
    pub reviewed_content: String,
    pub expected_head: Option<Uuid>,
    pub review_notes: String,
    pub acknowledge_bear_and_work_audience: bool,
}

pub async fn promote(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    decision: CoreReviewDecision,
) -> Result<CoreReviewOutcome, DenError> {
    if !decision.acknowledge_bear_and_work_audience {
        return Err(DenError::Authorization("review this entry for all Bear members and future autonomous Work before sharing it in core".into()));
    }
    // Curation/Work configuration uses the same Postgres-hat → SQLite lock
    // order. The final canonical source and core head are rechecked by SQLite
    // inside its transaction, not trusted from a form or a prior GET.
    let mut tx = pool.begin().await?;
    let exists = sqlx::query_scalar!(
        r#"SELECT h.id FROM bear_hats h JOIN user_bear membership ON membership.bear_id = h.bear_id
           WHERE h.bear_id = $1 AND h.id = $2 AND membership.user_id = $3
             AND lower(btrim(coalesce(membership.role, ''))) = $4
           FOR UPDATE OF h"#,
        bear_id.as_uuid(),
        decision.hat_id.as_uuid(),
        reviewer.get(),
        crate::bears::db::BEAR_ROLE_ADMIN,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        return Err(DenError::Authorization(
            "Bear admin or source hat unavailable".into(),
        ));
    }
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    let result = reviewed_core::promote(
        &store,
        ReviewedCoreEntry {
            hat_id: decision.hat_id,
            source_memory_id: decision.source_memory_id,
            kind: decision.kind,
            reviewed_content: decision.reviewed_content,
            expected_head: decision.expected_head,
            review_notes: decision.review_notes,
            reviewer,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
