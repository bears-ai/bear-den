//! Canonical SQLite snapshot for reviewing every hat record before widening
//! its audience to autonomous Work, including historical and invalid records.
//! Only the requested page is retained; the fingerprint covers the full history.

use den_core::{ids::HatId, DenError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::BearMemoryStore;

#[cfg(test)]
#[path = "hat_review/tests.rs"]
mod tests;

pub const REVIEW_PAGE_SIZE: usize = 100;
const SCAN_BATCH_SIZE: i64 = 128;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct HatReviewRecord {
    pub memory_id: String,
    pub sequence_no: i64,
    pub kind: String,
    pub content_text: String,
    pub metadata_json: String,
    pub visibility: String,
    pub invalid_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HatReviewSnapshot {
    pub total_records: i64,
    pub records: Vec<HatReviewRecord>,
    pub sha256: String,
}

pub async fn hat_history_count(store: &BearMemoryStore, hat_id: HatId) -> Result<i64, DenError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'hat' AND scope_hat_id = ?",
    )
    .bind(store.bear_id().to_string())
    .bind(hat_id.to_string())
    .fetch_one(store.pool())
    .await
    .map_err(|err| DenError::System(format!("count historical hat records: {err}")))
}

pub async fn snapshot_for_hat(
    store: &BearMemoryStore,
    hat_id: HatId,
    page: u32,
) -> Result<HatReviewSnapshot, DenError> {
    let mut conn =
        store.pool().acquire().await.map_err(|err| {
            DenError::System(format!("acquire Bear memory review snapshot: {err}"))
        })?;
    // A read transaction holds a consistent SQLite snapshot across all batches.
    // The Work-enable caller already holds BEGIN IMMEDIATE and uses `_on` directly.
    sqlx::query("BEGIN")
        .execute(&mut *conn)
        .await
        .map_err(|err| DenError::System(format!("begin hat review snapshot: {err}")))?;
    let result = snapshot_for_hat_on(&mut conn, store.bear_id(), hat_id, page).await;
    sqlx::query(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })
        .execute(&mut *conn)
        .await
        .map_err(|err| DenError::System(format!("finish hat review snapshot: {err}")))?;
    result
}

pub async fn snapshot_for_hat_on(
    conn: &mut SqliteConnection,
    bear_id: Uuid,
    hat_id: HatId,
    page: u32,
) -> Result<HatReviewSnapshot, DenError> {
    let bear_id = bear_id.to_string();
    let hat_id = hat_id.to_string();
    let counts = sqlx::query!(
        r#"
        SELECT COUNT(*) AS "total_records!: i64",
               COALESCE(SUM(CASE WHEN NOT EXISTS (
                   SELECT 1 FROM generic_memory_records generic
                   WHERE generic.bear_id = memory_records.bear_id
                     AND generic.memory_id = memory_records.memory_id
               ) THEN 1 ELSE 0 END), 0) AS "protected_records!: i64"
        FROM memory_records
        WHERE bear_id = ? AND scope_type = 'hat' AND scope_hat_id = ?
        "#,
        bear_id,
        hat_id,
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(|err| DenError::System(format!("count historical hat records: {err}")))?;
    if counts.protected_records != 0 {
        return Err(DenError::Authorization(
            "Hat contains audience-protected records; resolve their scope before enabling Work."
                .into(),
        ));
    }
    let total_records = counts.total_records;
    let total = usize::try_from(total_records)
        .map_err(|_| DenError::System("invalid historical hat record count".into()))?;
    let page_count = total.div_ceil(REVIEW_PAGE_SIZE).max(1);
    if page == 0
        || usize::try_from(page)
            .ok()
            .is_none_or(|page| page > page_count)
    {
        return Err(DenError::ValidationError("invalid hat review page".into()));
    }
    let start = (page as usize - 1)
        .checked_mul(REVIEW_PAGE_SIZE)
        .ok_or_else(|| DenError::ValidationError("hat review page overflow".into()))?;
    let end = start.saturating_add(REVIEW_PAGE_SIZE);
    let mut records = Vec::with_capacity(REVIEW_PAGE_SIZE.min(total));
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"[");
    let mut scanned = 0_usize;
    let mut cursor: Option<(i64, String)> = None;
    loop {
        let cursor_sequence = cursor.as_ref().map(|(sequence, _)| *sequence);
        let cursor_id = cursor.as_ref().map(|(_, id)| id.as_str());
        let rows = sqlx::query_as!(
            HatReviewRecord,
            r#"
            SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!", kind AS "kind!",
                   content_text AS "content_text!", metadata_json AS "metadata_json!",
                   visibility AS "visibility!", invalid_at, created_at AS "created_at!"
            FROM generic_memory_records AS memory_records
            WHERE bear_id = ? AND scope_type = 'hat' AND scope_hat_id = ?
              AND (? IS NULL OR sequence_no < ? OR (sequence_no = ? AND memory_id < ?))
            ORDER BY sequence_no DESC, memory_id DESC LIMIT ?
            "#,
            bear_id,
            hat_id,
            cursor_sequence,
            cursor_sequence,
            cursor_sequence,
            cursor_id,
            SCAN_BATCH_SIZE,
        )
        .fetch_all(&mut *conn)
        .await
        .map_err(|err| DenError::System(format!("scan historical hat records: {err}")))?;
        if rows.is_empty() {
            break;
        }
        cursor = rows
            .last()
            .map(|row| (row.sequence_no, row.memory_id.clone()));
        for record in rows {
            if scanned != 0 {
                fingerprint.update(b",");
            }
            fingerprint.update(
                serde_json::to_vec(&record)
                    .map_err(|err| DenError::System(format!("fingerprint hat record: {err}")))?,
            );
            if (start..end).contains(&scanned) {
                records.push(record);
            }
            scanned += 1;
        }
    }
    fingerprint.update(b"]");
    if scanned != total {
        return Err(DenError::ValidationError(
            "hat history changed during review; restart at page 1".into(),
        ));
    }
    Ok(HatReviewSnapshot {
        total_records,
        records,
        sha256: format!("{:x}", fingerprint.finalize()),
    })
}
