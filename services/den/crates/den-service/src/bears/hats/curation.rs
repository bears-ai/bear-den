//! Human-review authority for sharing a source-local note with an entire Bear hat.
//! The Postgres hat row is locked through the SQLite write so enabling Work
//! cannot race a review that assumed Work was off.

use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};
use den_memory::{
    hat_promotion::{self, ReviewCandidate, ReviewedPromotion},
    MemorySource, MemoryStoreManager,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::manage::get_hat;

#[cfg(test)]
mod tests;
use crate::bears::db::{membership_role_for_user, role_is_bear_admin};

#[derive(Debug, Clone)]
pub struct ReviewedHatEntry {
    pub source_memory_id: Uuid,
    pub hat_id: HatId,
    pub kind: String,
    pub reviewed_content: String,
    pub expected_head: Option<Uuid>,
    pub review_notes: String,
    pub work_audience_reviewed: bool,
}

async fn require_bear_admin(
    pool: &PgPool,
    bear_id: BearId,
    reviewer: UserId,
) -> Result<(), DenError> {
    let role = membership_role_for_user(pool, reviewer.get(), bear_id.as_uuid()).await?;
    if !role.is_some_and(|role| role_is_bear_admin(role.as_deref())) {
        return Err(DenError::Authorization(
            "Bear admin review is required for private source notes".into(),
        ));
    }
    Ok(())
}

pub async fn candidates(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    hat_id: HatId,
    limit: i64,
) -> Result<Vec<ReviewCandidate>, DenError> {
    require_bear_admin(pool, bear_id, reviewer).await?;
    get_hat(pool, bear_id, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    hat_promotion::review_candidates(&store, limit).await
}

pub async fn candidate(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    hat_id: HatId,
    source_memory_id: Uuid,
) -> Result<ReviewCandidate, DenError> {
    require_bear_admin(pool, bear_id, reviewer).await?;
    get_hat(pool, bear_id, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    hat_promotion::review_candidate(&store, source_memory_id).await
}

pub async fn promote(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    review: ReviewedHatEntry,
) -> Result<ReviewedPromotion, DenError> {
    let notes = review.review_notes.trim();
    if notes.len() < 12 || notes.len() > 4_000 {
        return Err(DenError::ValidationError(
            "review rationale must be 12–4000 characters".into(),
        ));
    }
    if review.reviewed_content.len() > 16_000 {
        return Err(DenError::ValidationError(
            "reviewed hat entry is too long".into(),
        ));
    }
    // The same Postgres hat lock is used by enable_work_if_empty. If a review
    // writes first, enabling Work will see a nonempty hat and stop; if Work
    // enables first, this review must explicitly acknowledge that audience.
    let mut tx = pool.begin().await?;
    let work_enabled = sqlx::query_scalar!(
        r#"SELECT h.work_enabled
           FROM bear_hats h JOIN user_bear membership ON membership.bear_id = h.bear_id
           WHERE h.bear_id = $1 AND h.id = $2 AND membership.user_id = $3
             AND lower(btrim(coalesce(membership.role, ''))) = $4
           FOR UPDATE OF h"#,
        bear_id.as_uuid(),
        review.hat_id.as_uuid(),
        reviewer.get(),
        crate::bears::db::BEAR_ROLE_ADMIN,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DenError::Authorization("Bear admin or hat grant unavailable".into()))?;
    if work_enabled && !review.work_audience_reviewed {
        return Err(DenError::Authorization(
            "review this entry for autonomous Work before publishing it to a Work-enabled hat"
                .into(),
        ));
    }
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    let source = hat_promotion::review_candidate(&store, review.source_memory_id).await?;
    let source_exists = match source.source {
        MemorySource::Conversation(id) => sqlx::query_scalar!(
            "SELECT EXISTS (SELECT 1 FROM conversations WHERE bear_id = $1 AND id = $2) AS \"exists!\"",
            bear_id.as_uuid(), id,
        ).fetch_one(&mut *tx).await?,
        MemorySource::WorkRun(id) => sqlx::query_scalar!(
            "SELECT EXISTS (SELECT 1 FROM bear_work_runs WHERE bear_id = $1 AND id = $2) AS \"exists!\"",
            bear_id.as_uuid(), id,
        ).fetch_one(&mut *tx).await?,
        // Intake does not yet have a canonical Den owner/lookup to verify.
        MemorySource::Intake(_) => false,
    };
    if !source_exists {
        return Err(DenError::Authorization(
            "source note has no verifiable conversation or Work run in this Bear".into(),
        ));
    }
    let outcome = hat_promotion::promote_reviewed_to_hat(
        &store,
        review.source_memory_id,
        review.hat_id,
        &review.kind,
        &review.reviewed_content,
        reviewer,
        work_enabled && review.work_audience_reviewed,
        review.expected_head,
        notes,
    )
    .await?;
    tx.commit().await?;
    Ok(outcome)
}
