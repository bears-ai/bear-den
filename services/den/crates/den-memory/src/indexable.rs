//! Generic-readable canonical heads for derived recall indexing. Indexing policy
//! and conversion to provider requests remain the caller's responsibility.

use den_core::DenError;

use crate::BearMemoryStore;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IndexableHeadRow {
    pub memory_id: String,
    pub sequence_no: i64,
    pub scope_type: String,
    pub scope_profile: Option<String>,
    pub scope_hat_id: Option<String>,
    pub kind: String,
    pub visibility: String,
    pub salience: String,
    pub metadata_json: String,
    pub supersedes_memory_id: Option<String>,
    pub invalid_at: Option<String>,
    pub logical_path: Option<String>,
    pub work_surface_ref: Option<String>,
    pub content_text: String,
}

/// Current shared/hat head candidates with a logical path. The generic view
/// excludes access-rule history and permanent personal classification, even if
/// a classified record has an unexpected path and its access rules are removed.
/// Physical head/supersession checks prevent older generic versions resurfacing.
pub async fn list_generic_indexable_heads(
    store: &BearMemoryStore,
) -> Result<Vec<IndexableHeadRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    sqlx::query_as!(
        IndexableHeadRow,
        r#"
        SELECT m.memory_id AS "memory_id!", m.sequence_no AS "sequence_no!",
               m.scope_type AS "scope_type!", m.scope_profile, m.scope_hat_id,
               m.kind AS "kind!", m.visibility AS "visibility!", m.salience AS "salience!",
               m.metadata_json AS "metadata_json!", m.supersedes_memory_id, m.invalid_at,
               m.logical_path, m.work_surface_ref, m.content_text AS "content_text!"
        FROM generic_memory_records m
        WHERE m.bear_id = ?
          AND m.scope_type IN ('shared', 'hat')
          AND m.scope_profile IS NULL
          AND m.scope_source_kind IS NULL
          AND m.scope_source_id IS NULL
          AND m.visibility = 'normal'
          AND m.invalid_at IS NULL
          AND m.logical_path IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM memory_records n
              WHERE n.bear_id = m.bear_id AND n.supersedes_memory_id = m.memory_id
          )
          AND m.sequence_no = (
              SELECT MAX(h.sequence_no) FROM memory_records h
              WHERE h.bear_id = m.bear_id
                AND h.logical_path = m.logical_path
                AND h.scope_type = m.scope_type
                AND h.scope_hat_id IS m.scope_hat_id
                AND h.visibility = 'normal'
          )
        ORDER BY m.logical_path
        "#,
        bear_id,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list indexable heads: {e}")))
}
