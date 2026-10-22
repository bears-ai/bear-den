//! Read-only admin inspection helpers for per-Bear SQLite memory.

use std::path::PathBuf;

use serde::Serialize;

use uuid::Uuid;

use den_core::{config::Config, ids::HatId, DenError};

use crate::{
    list_memory_proposals, memory_sequence_high_water, records::MemoryRecordSqlRow,
    MemoryRecordRow, MemoryStoreManager,
};

#[derive(Debug, Clone, Serialize)]
pub struct BearMemoryAdminStats {
    pub db_path: String,
    pub db_exists: bool,
    pub db_size_bytes: Option<u64>,
    pub record_count: i64,
    pub shared_count: i64,
    pub profile_local_count: i64,
    pub sequence_high_water: i64,
    pub pending_proposals: i64,
    pub pending_observations: i64,
    pub distinct_paths: i64,
}

/// A labeled count bucket for grouped memory stats (by kind, by role, etc.).
#[derive(Debug, Clone, Serialize)]
pub struct MemoryCountBucket {
    pub label: String,
    pub count: i64,
}

/// An extended memory record for the single-entry admin page — includes the fields the
/// lightweight [`MemoryRecordRow`] omits (`author`, `visibility`, `supersedes_memory_id`).
#[derive(Debug, Clone, Serialize)]
pub struct MemoryRecordDetail {
    pub memory_id: String,
    pub sequence_no: i64,
    pub scope_type: String,
    pub scope_profile: Option<String>,
    pub kind: String,
    pub author_profile: String,
    pub author_agent_id: Option<String>,
    pub visibility: String,
    pub supersedes_memory_id: Option<String>,
    pub logical_path: Option<String>,
    pub work_surface_ref: Option<String>,
    pub content_text: String,
    pub metadata_json: serde_json::Value,
    pub created_at: String,
}

pub fn bear_sqlite_db_path(config: &Config, bear_id: Uuid) -> PathBuf {
    PathBuf::from(&config.bear_sqlite_data_dir).join(format!("{bear_id}.sqlite"))
}

pub async fn bear_memory_admin_stats(
    manager: &MemoryStoreManager,
    config: &Config,
    bear_id: Uuid,
) -> Result<BearMemoryAdminStats, DenError> {
    let db_path = bear_sqlite_db_path(config, bear_id);
    let db_path_display = db_path.display().to_string();
    let db_exists = db_path.exists();
    let db_size_bytes = db_exists
        .then(|| std::fs::metadata(&db_path).ok().map(|m| m.len()))
        .flatten();

    let store = manager.store_for_bear(bear_id).await?;
    let pool = store.pool();

    // Physical integrity count: legacy import uses this to detect a nonempty store.
    let record_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM memory_records WHERE bear_id = ?")
            .bind(bear_id.to_string())
            .fetch_one(pool)
            .await
            .map_err(|e| DenError::System(format!("memory record count failed: {e}")))?;

    let bear_id_text = bear_id.to_string();
    let shared_count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM generic_memory_records WHERE bear_id = ? AND scope_type = 'shared'"#,
        bear_id_text,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| DenError::System(format!("memory shared count failed: {e}")))?;

    let profile_local_count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM generic_memory_records WHERE bear_id = ? AND scope_type = 'profile_local'"#,
        bear_id_text,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| DenError::System(format!("memory profile_local count failed: {e}")))?;

    let distinct_paths = sqlx::query_scalar!(
        r#"SELECT COUNT(DISTINCT logical_path) AS "count!: i64" FROM generic_memory_records WHERE bear_id = ? AND logical_path IS NOT NULL"#,
        bear_id_text,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| DenError::System(format!("memory path count failed: {e}")))?;

    let pending_proposals = list_memory_proposals(&store, Some("pending"), 500)
        .await?
        .len() as i64;

    let pending_observations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_observations WHERE bear_id = ? AND status = 'pending_review'",
    )
    .bind(bear_id.to_string())
    .fetch_one(pool)
    .await
    .map_err(|e| DenError::System(format!("memory observation count failed: {e}")))?;

    let sequence_high_water = memory_sequence_high_water(&store).await?;

    Ok(BearMemoryAdminStats {
        db_path: db_path_display,
        db_exists,
        db_size_bytes,
        record_count,
        shared_count,
        profile_local_count,
        sequence_high_water,
        pending_proposals,
        pending_observations,
        distinct_paths,
    })
}

pub async fn list_all_logical_paths(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<String>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let bear_id = bear_id.to_string();
    let rows = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT logical_path AS "logical_path!"
        FROM generic_memory_records
        WHERE bear_id = ? AND logical_path IS NOT NULL
        ORDER BY logical_path ASC
        "#,
        bear_id,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list memory paths failed: {e}")))?;
    Ok(rows)
}

pub async fn get_memory_record_by_id(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    memory_id: &str,
) -> Result<Option<MemoryRecordRow>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let bear_id = bear_id.to_string();
    let row = sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at
        FROM generic_memory_records
        WHERE bear_id = ? AND memory_id = ?
        "#,
        bear_id,
        memory_id,
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("get memory record failed: {e}")))?;

    Ok(row.map(MemoryRecordSqlRow::into_row))
}

/// Record counts grouped by `kind`, highest first (dashboard "what kinds of memory").
pub async fn count_records_by_kind(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<MemoryCountBucket>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let bear_id = bear_id.to_string();
    let rows = sqlx::query_as!(
        MemoryCountBucket,
        r#"
        SELECT kind AS "label!", COUNT(*) AS "count!: i64"
        FROM generic_memory_records
        WHERE bear_id = ?
        GROUP BY kind
        ORDER BY COUNT(*) DESC, kind ASC
        "#,
        bear_id,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("count by kind failed: {e}")))?;
    Ok(rows)
}

/// Record counts grouped by canonical scope, highest first. A missing profile
/// does not imply core: source-local and hat records also have no profile.
pub async fn count_records_by_profile(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<MemoryCountBucket>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let bear_id = bear_id.to_string();
    let rows = sqlx::query_as!(
        MemoryCountBucket,
        r#"
        SELECT CASE scope_type
                   WHEN 'shared' THEN 'core'
                   WHEN 'hat' THEN 'hat'
                   WHEN 'source_local' THEN 'session'
                   ELSE COALESCE(scope_profile, 'legacy profile')
               END AS "label!: String", COUNT(*) AS "count!: i64"
        FROM generic_memory_records
        WHERE bear_id = ?
        GROUP BY 1
        ORDER BY COUNT(*) DESC, 1 ASC
        "#,
        bear_id,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("count by profile failed: {e}")))?;
    Ok(rows)
}

/// Distinct current entries: logical paths with at least one `normal`-visibility record.
/// (Supersession is latest-at-path today, so this approximates the live head count.)
pub async fn head_entry_count(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<i64, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let bear_id = bear_id.to_string();
    sqlx::query_scalar!(
        r#"
        SELECT COUNT(DISTINCT logical_path) AS "count!: i64"
        FROM generic_memory_records
        WHERE bear_id = ? AND logical_path IS NOT NULL AND visibility = 'normal'
        "#,
        bear_id,
    )
    .fetch_one(store.pool())
    .await
    .map_err(|e| DenError::System(format!("head entry count failed: {e}")))
}

/// One row per logical path: the current head record plus how many versions exist there.
/// Powers the browse/library view without an N+1 per-path query.
#[derive(Debug, Clone, Serialize)]
pub struct PathSummary {
    pub logical_path: String,
    pub scope_type: String,
    pub scope_profile: Option<String>,
    pub scope_hat_id: Option<HatId>,
    pub kind: String,
    pub head_memory_id: String,
    pub head_created_at: String,
    pub version_count: i64,
}

pub async fn list_path_summaries(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<PathSummary>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let bear_id = bear_id.to_string();
    let rows = sqlx::query!(
        r#"
        SELECT m.logical_path AS "logical_path!", m.scope_type AS "scope_type!",
               m.scope_profile, m.scope_hat_id, m.kind AS "kind!",
               m.memory_id AS "head_memory_id!", m.created_at AS "head_created_at!",
               agg.version_count AS "version_count!: i64"
        FROM generic_memory_records m
        JOIN (
            SELECT logical_path, MAX(sequence_no) AS max_seq, COUNT(*) AS version_count
            FROM generic_memory_records
            WHERE bear_id = ? AND logical_path IS NOT NULL
            GROUP BY logical_path
        ) agg ON agg.logical_path = m.logical_path AND agg.max_seq = m.sequence_no
        WHERE m.bear_id = ? AND m.logical_path IS NOT NULL
        ORDER BY m.logical_path ASC
        "#,
        bear_id,
        bear_id,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list path summaries failed: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|row| PathSummary {
            logical_path: row.logical_path,
            scope_type: row.scope_type,
            scope_profile: row.scope_profile,
            scope_hat_id: row
                .scope_hat_id
                .and_then(|id| Uuid::parse_str(&id).ok())
                .map(HatId::new),
            kind: row.kind,
            head_memory_id: row.head_memory_id,
            head_created_at: row.head_created_at,
            version_count: row.version_count,
        })
        .collect())
}

/// Extended single record (all columns) for the entry admin page.
pub async fn get_memory_record_detail(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    memory_id: &str,
) -> Result<Option<MemoryRecordDetail>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let bear_id = bear_id.to_string();
    let row = sqlx::query!(
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               author_profile AS "author_profile!", author_agent_id,
               visibility AS "visibility!", supersedes_memory_id, logical_path,
               work_surface_ref, content_text AS "content_text!",
               metadata_json AS "metadata_json!", created_at AS "created_at!"
        FROM generic_memory_records
        WHERE bear_id = ? AND memory_id = ?
        "#,
        bear_id,
        memory_id,
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("get memory record detail failed: {e}")))?;

    let Some(row) = row else {
        return Ok(None);
    };
    let metadata_json =
        serde_json::from_str(&row.metadata_json).unwrap_or_else(|_| serde_json::json!({}));
    Ok(Some(MemoryRecordDetail {
        memory_id: row.memory_id,
        sequence_no: row.sequence_no,
        scope_type: row.scope_type,
        scope_profile: row.scope_profile,
        kind: row.kind,
        author_profile: row.author_profile,
        author_agent_id: row.author_agent_id,
        visibility: row.visibility,
        supersedes_memory_id: row.supersedes_memory_id,
        logical_path: row.logical_path,
        work_surface_ref: row.work_surface_ref,
        content_text: row.content_text,
        metadata_json,
        created_at: row.created_at,
    }))
}

pub async fn list_recent_memory_records(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let bear_id = bear_id.to_string();
    let limit = limit.clamp(1, 50);
    let rows = sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at
        FROM generic_memory_records
        WHERE bear_id = ?
        ORDER BY sequence_no DESC
        LIMIT ?
        "#,
        bear_id,
        limit,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list recent memory records failed: {e}")))?;

    Ok(rows.into_iter().map(MemoryRecordSqlRow::into_row).collect())
}

/// Keyword search across a Bear's generic memory (every role/scope), newest first.
///
/// Case-insensitive `content_text LIKE` — the always-available fallback when semantic
/// recall (Qdrant) is unconfigured. `query` is matched literally (LIKE metacharacters escaped).
pub async fn search_memory_records(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let pattern = format!("%{}%", escape_like(query));
    let bear_id = bear_id.to_string();
    let limit = limit.clamp(1, 100);
    let rows = sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at
        FROM generic_memory_records
        WHERE bear_id = ? AND content_text LIKE ? ESCAPE '\'
        ORDER BY sequence_no DESC
        LIMIT ?
        "#,
        bear_id,
        pattern,
        limit,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("search memory records failed: {e}")))?;

    Ok(rows.into_iter().map(MemoryRecordSqlRow::into_row).collect())
}

/// Escape SQLite `LIKE` metacharacters (`\`, `%`, `_`) so a user query matches literally.
pub(crate) fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests;
