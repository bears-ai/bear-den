//! Bear-admin-only reauthoring of legacy profile-local records. No code path
//! upgrades these records into a fabricated conversation or Work-run source.

use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};
use den_memory::{
    legacy_review::{
        self, LegacyCandidate, LegacyReauthoring, LegacyReviewPage, ReauthoredHatEntry,
    },
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
            "Bear admin access required for unattributed legacy notes".into(),
        ));
    }
    get_hat(pool, bear_id, hat_id).await?;
    Ok(())
}

pub async fn inventory_page(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    hat_id: HatId,
    before: Option<i64>,
) -> Result<LegacyReviewPage, DenError> {
    require_admin(pool, bear_id, reviewer, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    legacy_review::inventory_page(&store, before, 50).await
}

pub async fn candidate(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    hat_id: HatId,
    memory_id: &str,
) -> Result<LegacyCandidate, DenError> {
    require_admin(pool, bear_id, reviewer, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    legacy_review::candidate(&store, memory_id).await
}

#[derive(Debug, Clone)]
pub struct LegacyReviewDecision {
    pub source_memory_id: String,
    pub target_hat: HatId,
    pub kind: String,
    pub reviewed_content: String,
    pub expected_head: Option<Uuid>,
    pub review_notes: String,
    pub acknowledge_unverified_source_and_members: bool,
    pub work_audience_reviewed: bool,
}

pub async fn reauthor(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    reviewer: UserId,
    decision: LegacyReviewDecision,
) -> Result<ReauthoredHatEntry, DenError> {
    if !decision.acknowledge_unverified_source_and_members {
        return Err(DenError::Authorization(
            "acknowledge that the legacy source has no verified owner and the new text is safe for all hat members".into(),
        ));
    }
    let notes = decision.review_notes.trim();
    if !(12..=4_000).contains(&notes.len()) {
        return Err(DenError::ValidationError(
            "review rationale must be 12–4000 bytes".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let work_enabled = sqlx::query_scalar!(
        r#"SELECT h.work_enabled FROM bear_hats h
           JOIN user_bear membership ON membership.bear_id = h.bear_id
           WHERE h.bear_id = $1 AND h.id = $2 AND membership.user_id = $3
             AND lower(btrim(coalesce(membership.role, ''))) = $4
           FOR UPDATE OF h"#,
        bear_id.as_uuid(),
        decision.target_hat.as_uuid(),
        reviewer.get(),
        crate::bears::db::BEAR_ROLE_ADMIN,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DenError::Authorization("Bear admin or selected hat unavailable".into()))?;
    if work_enabled && !decision.work_audience_reviewed {
        return Err(DenError::Authorization(
            "review new hat text for autonomous Work before publishing to a Work-enabled hat"
                .into(),
        ));
    }
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    let result = legacy_review::reauthor_into_hat(
        &store,
        LegacyReauthoring {
            source_memory_id: decision.source_memory_id,
            target_hat: decision.target_hat,
            kind: decision.kind,
            reviewed_content: decision.reviewed_content,
            expected_head: decision.expected_head,
            review_notes: notes.to_string(),
            reviewer,
            work_audience_reviewed: work_enabled && decision.work_audience_reviewed,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
