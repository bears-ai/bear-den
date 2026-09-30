//! An explicit, auditable off → on Work decision for populated Bear hats.
//! Hold the canonical Postgres hat lock and SQLite writer lock across the
//! snapshot check and transition; curation uses the same lock order.

use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};
use den_memory::{
    hat_review::{self, HatReviewRecord},
    MemoryStoreManager,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::identity::identity_fingerprint;
use super::manage::get_hat;
use crate::bears::db::{membership_role_for_user, role_is_bear_admin};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize)]
pub struct WorkMemoryReviewSnapshot {
    pub total_records: i64,
    pub records: Vec<HatReviewRecord>,
    pub complete: bool,
    pub sha256: Option<String>,
    pub identity_sha256: String,
    pub page: u32,
    pub page_count: u32,
}

fn fingerprint(records: &[HatReviewRecord]) -> Result<String, DenError> {
    let serialized = serde_json::to_vec(records)
        .map_err(|err| DenError::System(format!("serialize hat memory review: {err}")))?;
    Ok(format!("{:x}", Sha256::digest(serialized)))
}

fn review_snapshot(
    snapshot: hat_review::HatReviewSnapshot,
    page: u32,
    identity_sha256: String,
) -> Result<WorkMemoryReviewSnapshot, DenError> {
    let complete = snapshot.complete();
    let sha256 = if complete {
        Some(fingerprint(&snapshot.records)?)
    } else {
        None
    };
    let shown = snapshot.records.len();
    let page_count = shown.div_ceil(hat_review::REVIEW_PAGE_SIZE).max(1) as u32;
    if page == 0 || page > page_count {
        return Err(DenError::ValidationError("invalid hat review page".into()));
    }
    let records = snapshot
        .records
        .into_iter()
        .skip((page as usize - 1) * hat_review::REVIEW_PAGE_SIZE)
        .take(hat_review::REVIEW_PAGE_SIZE)
        .collect();
    Ok(WorkMemoryReviewSnapshot {
        total_records: snapshot.total_records,
        records,
        complete,
        sha256,
        identity_sha256,
        page,
        page_count,
    })
}

async fn require_admin(pool: &PgPool, bear_id: BearId, reviewer: UserId) -> Result<(), DenError> {
    let role = membership_role_for_user(pool, reviewer.get(), bear_id.as_uuid()).await?;
    if !role.is_some_and(|role| role_is_bear_admin(role.as_deref())) {
        return Err(DenError::Authorization(
            "Bear admin access required to review hat memory".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkReviewHistoryEntry {
    pub id: Uuid,
    pub reviewed_by: String,
    pub record_count: i64,
    pub identity_sha256: Option<String>,
    pub rationale: String,
    pub reviewed_at: String,
}

pub async fn list_receipts(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    reviewer: UserId,
) -> Result<Vec<WorkReviewHistoryEntry>, DenError> {
    require_admin(pool, bear_id, reviewer).await?;
    get_hat(pool, bear_id, hat_id).await?;
    let rows = sqlx::query!(
        "SELECT r.id, r.record_count, r.identity_sha256, r.rationale, r.reviewed_at, u.username AS \"reviewed_by!\"
         FROM bear_hat_work_reviews r JOIN users u ON u.id = r.reviewed_by_user_id
         WHERE r.bear_id = $1 AND r.hat_id = $2 ORDER BY r.reviewed_at DESC LIMIT 20",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| WorkReviewHistoryEntry {
            id: row.id,
            reviewed_by: row.reviewed_by,
            record_count: row.record_count,
            identity_sha256: row.identity_sha256,
            rationale: row.rationale,
            reviewed_at: row.reviewed_at.to_string(),
        })
        .collect())
}

pub async fn snapshot_for_admin(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    hat_id: HatId,
    reviewer: UserId,
) -> Result<WorkMemoryReviewSnapshot, DenError> {
    require_admin(pool, bear_id, reviewer).await?;
    let hat = get_hat(pool, bear_id, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    review_snapshot(
        hat_review::snapshot_for_hat(&store, hat_id).await?,
        1,
        identity_fingerprint(&hat.name, &hat.purpose, &hat.identity_prompt),
    )
}

pub async fn snapshot_page_for_admin(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    hat_id: HatId,
    reviewer: UserId,
    page: u32,
) -> Result<WorkMemoryReviewSnapshot, DenError> {
    require_admin(pool, bear_id, reviewer).await?;
    let hat = get_hat(pool, bear_id, hat_id).await?;
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    review_snapshot(
        hat_review::snapshot_for_hat(&store, hat_id).await?,
        page,
        identity_fingerprint(&hat.name, &hat.purpose, &hat.identity_prompt),
    )
}

#[derive(Debug, Clone)]
pub struct WorkReviewDecision {
    pub expected_sha256: String,
    pub expected_record_count: i64,
    pub expected_identity_sha256: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkReviewReceipt {
    pub id: Uuid,
    pub record_count: i64,
}

pub async fn review_and_enable(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    hat_id: HatId,
    reviewer: UserId,
    decision: WorkReviewDecision,
) -> Result<WorkReviewReceipt, DenError> {
    let rationale = decision.rationale.trim();
    if !(12..=4_000).contains(&rationale.len()) {
        return Err(DenError::ValidationError(
            "Work memory review rationale must be 12–4000 characters".into(),
        ));
    }
    if decision.expected_sha256.len() != 64
        || !decision
            .expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || decision.expected_identity_sha256.len() != 64
        || !decision
            .expected_identity_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DenError::ValidationError(
            "invalid hat memory review fingerprint".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let hat_state = sqlx::query!(
        r#"SELECT h.work_enabled, h.name, h.purpose, h.identity_prompt FROM bear_hats h
           JOIN user_bear membership ON membership.bear_id = h.bear_id
           WHERE h.bear_id = $1 AND h.id = $2 AND membership.user_id = $3
             AND lower(btrim(coalesce(membership.role, ''))) = $4
           FOR UPDATE OF h"#,
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        reviewer.get(),
        crate::bears::db::BEAR_ROLE_ADMIN,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DenError::Authorization("Bear admin or hat grant unavailable".into()))?;
    if hat_state.work_enabled {
        return Err(DenError::ValidationError(
            "Work is already enabled for this hat".into(),
        ));
    }
    if identity_fingerprint(
        &hat_state.name,
        &hat_state.purpose,
        &hat_state.identity_prompt,
    ) != decision.expected_identity_sha256
    {
        return Err(DenError::ValidationError(
            "hat identity changed since review; refresh and inspect it again".into(),
        ));
    }
    let surfaces = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!: i64\" FROM bear_hat_work_surfaces WHERE bear_id = $1 AND hat_id = $2",
        bear_id.as_uuid(), hat_id.as_uuid(),
    ).fetch_one(&mut *tx).await?;
    if surfaces == 0 {
        return Err(DenError::ValidationError(
            "select at least one Bear-assigned work surface first".into(),
        ));
    }
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    let mut sqlite = store
        .pool()
        .acquire()
        .await
        .map_err(|err| DenError::System(format!("lock Bear memory for Work review: {err}")))?;
    // sqlx-dynamic: SQLite is per-Bear and built at runtime. BEGIN IMMEDIATE
    // fences canonical memory writers while this decision commits in Postgres.
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *sqlite)
        .await
        .map_err(|err| DenError::System(format!("begin hat review fence: {err}")))?;
    let outcome: Result<WorkReviewReceipt, DenError> = async {
        let snapshot = review_snapshot(hat_review::snapshot_for_hat_on(&mut sqlite, bear_id.as_uuid(), hat_id).await?, 1, identity_fingerprint(&hat_state.name, &hat_state.purpose, &hat_state.identity_prompt))?;
        if !snapshot.complete || snapshot.total_records == 0 {
            return Err(DenError::Authorization("this hat is empty or exceeds the bounded historical review limit".into()));
        }
        if snapshot.total_records != decision.expected_record_count
            || snapshot.sha256.as_deref() != Some(decision.expected_sha256.as_str())
        {
            return Err(DenError::ValidationError("hat memory changed since review; refresh and inspect it again".into()));
        }
        let receipt_id = sqlx::query_scalar!(
            "INSERT INTO bear_hat_work_reviews (bear_id, hat_id, reviewed_by_user_id, memory_sha256, record_count, rationale, identity_sha256)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
            bear_id.as_uuid(), hat_id.as_uuid(), reviewer.get(), &decision.expected_sha256,
            snapshot.total_records, rationale, &decision.expected_identity_sha256,
        ).fetch_one(&mut *tx).await?;
        sqlx::query!(
            "UPDATE bear_hats SET work_enabled = true, updated_at = NOW() WHERE bear_id = $1 AND id = $2",
            bear_id.as_uuid(), hat_id.as_uuid(),
        ).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(WorkReviewReceipt { id: receipt_id, record_count: snapshot.total_records })
    }.await;
    let finish = if outcome.is_ok() {
        "COMMIT"
    } else {
        "ROLLBACK"
    };
    sqlx::query(finish)
        .execute(&mut *sqlite)
        .await
        .map_err(|err| DenError::System(format!("finish hat review fence: {err}")))?;
    outcome
}
