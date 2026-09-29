//! Canonical SQLite snapshot for reviewing every hat record before widening
//! the hat's audience to autonomous Work. Historical/invalid records count too.

use den_core::{ids::HatId, DenError};
use serde::Serialize;
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::BearMemoryStore;

// Bound the canonical snapshot and its fingerprint while still allowing several
// individually readable review pages. Hats beyond this ceiling remain blocked.
pub const MAX_REVIEW_RECORDS: i64 = 500;
pub const REVIEW_PAGE_SIZE: usize = 100;

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
}

impl HatReviewSnapshot {
    pub fn complete(&self) -> bool {
        self.total_records <= MAX_REVIEW_RECORDS && self.records.len() as i64 == self.total_records
    }
}

pub async fn snapshot_for_hat(
    store: &BearMemoryStore,
    hat_id: HatId,
) -> Result<HatReviewSnapshot, DenError> {
    let mut conn =
        store.pool().acquire().await.map_err(|err| {
            DenError::System(format!("acquire Bear memory review snapshot: {err}"))
        })?;
    snapshot_for_hat_on(&mut conn, store.bear_id(), hat_id).await
}

// sqlx-dynamic: SQLite is created per Bear at runtime, not a compile-time SQLx
// database. These queries use static SQL and bind canonical Bear/hat IDs.
pub async fn snapshot_for_hat_on(
    conn: &mut SqliteConnection,
    bear_id: Uuid,
    hat_id: HatId,
) -> Result<HatReviewSnapshot, DenError> {
    let total_records: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'hat' AND scope_hat_id = ?"
    ).bind(bear_id.to_string()).bind(hat_id.to_string())
        .fetch_one(&mut *conn).await
        .map_err(|err| DenError::System(format!("count historical hat records: {err}")))?;
    let records = sqlx::query_as::<_, HatReviewRecord>(
        "SELECT memory_id, sequence_no, kind, content_text, metadata_json, visibility,
                invalid_at, created_at FROM memory_records
         WHERE bear_id = ? AND scope_type = 'hat' AND scope_hat_id = ?
         ORDER BY sequence_no DESC LIMIT ?",
    )
    .bind(bear_id.to_string())
    .bind(hat_id.to_string())
    .bind(MAX_REVIEW_RECORDS)
    .fetch_all(&mut *conn)
    .await
    .map_err(|err| DenError::System(format!("read historical hat records: {err}")))?;
    Ok(HatReviewSnapshot {
        total_records,
        records,
    })
}
