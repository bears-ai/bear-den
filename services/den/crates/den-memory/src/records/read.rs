use den_core::DenError;
use serde::Serialize;

use super::{
    freshness_trend, lifecycle_status, BearMemoryStore, MemoryRecordRow, MemoryRecordSqlRow,
};
use crate::access::{record_visible, AccessContext};

/// Unscoped callers cannot read records with access-rule history or personal classification.
pub async fn fetch_record_by_id(
    store: &BearMemoryStore,
    memory_id: &str,
) -> Result<Option<MemoryRecordRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    let row = sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND memory_id = ?
        "#,
        bear_id,
        memory_id,
    )
    .fetch_optional(store.pool())
    .await?;
    Ok(row.map(MemoryRecordSqlRow::into_row))
}

pub async fn head_record_for_logical_path(
    store: &BearMemoryStore,
    logical_path: &str,
) -> Result<Option<MemoryRecordRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    let row = sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND logical_path = ? AND visibility = 'normal'
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer
              WHERE newer.bear_id = memory_records.bear_id
                AND newer.supersedes_memory_id = memory_records.memory_id
          )
        ORDER BY sequence_no DESC LIMIT 1
        "#,
        bear_id,
        logical_path,
    )
    .fetch_optional(store.pool())
    .await?;
    Ok(row.map(MemoryRecordSqlRow::into_row))
}

/// Trusted projection must supply its actual access context, not fall back to generic reads.
/// The canonical head is selected before authorization; a denied head never exposes an older one.
pub async fn head_record_for_logical_path_with_access(
    store: &BearMemoryStore,
    logical_path: &str,
    access: &AccessContext,
) -> Result<Option<MemoryRecordRow>, DenError> {
    if access.is_empty() {
        return head_record_for_logical_path(store, logical_path).await;
    }
    let Some(row) = canonical_head(store, logical_path).await? else {
        return Ok(None);
    };
    if !record_visible(store, &row.memory_id, access).await? {
        return Ok(None);
    }
    Ok(Some(row.into_row()))
}

async fn canonical_head(
    store: &BearMemoryStore,
    logical_path: &str,
) -> Result<Option<MemoryRecordSqlRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    Ok(sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ? AND logical_path = ? AND visibility = 'normal'
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer
              WHERE newer.bear_id = memory_records.bear_id
                AND newer.supersedes_memory_id = memory_records.memory_id
          )
        ORDER BY sequence_no DESC LIMIT 1
        "#,
        bear_id,
        logical_path,
    )
    .fetch_optional(store.pool())
    .await?)
}

pub async fn has_work_surface_canonical_anchor(
    store: &BearMemoryStore,
    slug: &str,
) -> Result<bool, DenError> {
    for path in work_surface_paths(slug) {
        if head_record_for_logical_path(store, &path).await?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn has_work_surface_canonical_anchor_with_access(
    store: &BearMemoryStore,
    slug: &str,
    access: &AccessContext,
) -> Result<bool, DenError> {
    for path in work_surface_paths(slug) {
        if head_record_for_logical_path_with_access(store, &path, access)
            .await?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn work_surface_paths(slug: &str) -> [String; 2] {
    [
        format!("core/work_surfaces/{slug}/index.md"),
        format!("core/work_surfaces/{slug}/overview.md"),
    ]
}

pub async fn list_profile_local_head_records(
    store: &BearMemoryStore,
    profile: &str,
    work_surface_ref: Option<&str>,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    let rows = sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND scope_type = 'profile_local' AND scope_profile = ?
          AND visibility = 'normal' AND work_surface_ref IS ?
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer
              WHERE newer.bear_id = memory_records.bear_id
                AND newer.supersedes_memory_id = memory_records.memory_id
          )
        ORDER BY sequence_no DESC LIMIT ?
        "#,
        bear_id,
        profile,
        work_surface_ref,
        limit,
    )
    .fetch_all(store.pool())
    .await?;
    Ok(rows.into_iter().map(MemoryRecordSqlRow::into_row).collect())
}

pub async fn list_records_for_logical_path(
    store: &BearMemoryStore,
    logical_path: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    let rows = sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND logical_path = ? AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer
              WHERE newer.bear_id = memory_records.bear_id
                AND newer.supersedes_memory_id = memory_records.memory_id
          )
        ORDER BY sequence_no DESC LIMIT ?
        "#,
        bear_id,
        logical_path,
        limit,
    )
    .fetch_all(store.pool())
    .await?;
    Ok(rows.into_iter().map(MemoryRecordSqlRow::into_row).collect())
}

pub async fn list_record_history_for_logical_path(
    store: &BearMemoryStore,
    logical_path: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    let rows = sqlx::query_as!(
        MemoryRecordSqlRow,
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND logical_path = ? ORDER BY sequence_no DESC LIMIT ?
        "#,
        bear_id,
        logical_path,
        limit,
    )
    .fetch_all(store.pool())
    .await?;
    Ok(rows.into_iter().map(MemoryRecordSqlRow::into_row).collect())
}

/// Minimal record fields for rendering an unprotected, role-scoped recall hit.
#[derive(Debug, Clone, Serialize)]
pub struct RecallRecordMin {
    pub memory_id: String,
    pub logical_path: Option<String>,
    pub kind: String,
    pub content_text: String,
    pub salience: String,
    pub lifecycle_status: String,
    pub freshness_trend: String,
}

/// Fetch current unprotected shared/own-profile records; input IDs confer no authority.
pub async fn fetch_records_min(
    store: &BearMemoryStore,
    memory_ids: &[String],
    role: &str,
) -> Result<Vec<RecallRecordMin>, DenError> {
    if memory_ids.is_empty() {
        return Ok(Vec::new());
    }
    let bear_id = store.bear_id().to_string();
    let memory_ids = serde_json::to_string(memory_ids)?;
    let rows = sqlx::query!(
        r#"
        SELECT memory_id AS "memory_id!", logical_path, kind AS "kind!",
               content_text AS "content_text!", salience AS "salience!",
               metadata_json AS "metadata_json!", supersedes_memory_id, invalid_at
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND visibility = 'normal' AND invalid_at IS NULL
          AND (scope_type = 'shared' OR scope_profile = ?)
          AND memory_id IN (SELECT value FROM json_each(?))
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer
              WHERE newer.bear_id = memory_records.bear_id
                AND newer.supersedes_memory_id = memory_records.memory_id
          )
        "#,
        bear_id,
        role,
        memory_ids,
    )
    .fetch_all(store.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let metadata =
                serde_json::from_str(&row.metadata_json).unwrap_or_else(|_| serde_json::json!({}));
            let lifecycle_status = lifecycle_status(
                &metadata,
                row.supersedes_memory_id.as_deref(),
                row.invalid_at.as_deref(),
            );
            let freshness_trend = freshness_trend(&lifecycle_status, row.invalid_at.as_deref());
            RecallRecordMin {
                memory_id: row.memory_id,
                logical_path: row.logical_path,
                kind: row.kind,
                content_text: row.content_text,
                salience: row.salience,
                lifecycle_status,
                freshness_trend,
            }
        })
        .collect())
}
