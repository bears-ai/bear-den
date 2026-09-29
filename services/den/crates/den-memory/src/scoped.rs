//! Canonical, scoped SQLite reads for a bound conversation or Work run.
//!
//! These are separate from legacy profile-local tools: a caller cannot derive
//! authority from a logical path, profile name, or untrusted model argument.

use std::collections::BTreeSet;

use den_core::{ids::HatId, DenError};
use sqlx::{QueryBuilder, Sqlite};

use crate::{
    access::{record_visible, AccessContext},
    logical_path::MemorySource,
    records::{BearMemoryStore, MemoryRecordRow, MemoryRecordSqlRow},
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryReadGrant {
    source: MemorySource,
    hat_id: Option<HatId>,
}

impl MemoryReadGrant {
    /// Only construct this after resolving the current Bear's canonical source
    /// and its hat from Den-owned state, never from model-supplied arguments.
    pub fn new(source: MemorySource, hat_id: Option<HatId>) -> Self {
        Self { source, hat_id }
    }

    pub fn source(self) -> MemorySource {
        self.source
    }

    pub fn hat_id(self) -> Option<HatId> {
        self.hat_id
    }
}

// sqlx-dynamic: the optional, verified hat branch changes the SQLite predicate;
// one typed builder owns that branch for direct read, browse, and keyword search.
fn push_scope(builder: &mut QueryBuilder<'_, Sqlite>, grant: MemoryReadGrant) {
    builder
        .push(
            " AND (scope_type = 'shared' OR (scope_type = 'source_local' AND scope_source_kind = ",
        )
        .push_bind(grant.source.kind())
        .push(" AND scope_source_id = ")
        .push_bind(grant.source.id().to_string())
        .push(')');
    if let Some(hat_id) = grant.hat_id {
        builder
            .push(" OR (scope_type = 'hat' AND scope_hat_id = ")
            .push_bind(hat_id.to_string())
            .push(')');
    }
    builder.push(')');
}

/// Read active versions at a path only when each record is visible to the grant.
/// A path shared by differently scoped records cannot widen this result.
pub async fn read_path(
    store: &BearMemoryStore,
    grant: MemoryReadGrant,
    access: &AccessContext,
    path: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    // sqlx-dynamic: optional hat visibility is composed by push_scope; all values are bound.
    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
                logical_path, work_surface_ref, metadata_json, created_at, salience,
                supersedes_memory_id, invalid_at FROM memory_records WHERE bear_id = ",
    );
    builder
        .push_bind(store.bear_id().to_string())
        .push(" AND logical_path = ")
        .push_bind(path);
    push_scope(&mut builder, grant);
    builder
        .push(
            " AND visibility = 'normal' AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer
              WHERE newer.bear_id = memory_records.bear_id
                AND newer.supersedes_memory_id = memory_records.memory_id
          )
          ORDER BY sequence_no DESC LIMIT ",
        )
        .push_bind(limit);
    let rows = builder
        .build_query_as::<MemoryRecordSqlRow>()
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("scoped memory read failed: {e}")))?;
    let mut visible = Vec::new();
    for row in rows {
        let row = row.into_row();
        if record_visible(store, &row.memory_id, access).await? {
            visible.push(row);
        }
    }
    Ok(visible)
}

/// Newest current notes from this verified source only, for its human owner.
/// Unlike general keyword search, unrelated curated records cannot consume the
/// page limit before source-local results are selected.
pub async fn recent_source(
    store: &BearMemoryStore,
    grant: MemoryReadGrant,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    const PAGE_SIZE: i64 = 64;
    let mut visible = Vec::new();
    let mut cursor: Option<(i64, String)> = None;
    let access = AccessContext::empty();
    let limit = limit.clamp(1, 50) as usize;
    loop {
        // sqlx-dynamic: SQLite is per-Bear; the keyset cursor is optional and
        // all Bear/source identifiers and values are bound, not interpolated.
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
                    logical_path, work_surface_ref, metadata_json, created_at, salience,
                    supersedes_memory_id, invalid_at FROM memory_records WHERE bear_id = ",
        );
        builder
            .push_bind(store.bear_id().to_string())
            .push(" AND scope_type = 'source_local' AND scope_source_kind = ")
            .push_bind(grant.source.kind())
            .push(" AND scope_source_id = ")
            .push_bind(grant.source.id().to_string())
            .push(
                " AND visibility = 'normal' AND invalid_at IS NULL
                  AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active')
                      NOT IN ('archived', 'archive-candidate')
                  AND NOT EXISTS (
                      SELECT 1 FROM memory_records newer
                      WHERE newer.bear_id = memory_records.bear_id
                        AND newer.supersedes_memory_id = memory_records.memory_id
                  )",
            );
        if let Some((sequence_no, memory_id)) = &cursor {
            builder
                .push(" AND (sequence_no < ")
                .push_bind(*sequence_no)
                .push(" OR (sequence_no = ")
                .push_bind(*sequence_no)
                .push(" AND memory_id < ")
                .push_bind(memory_id.clone())
                .push("))");
        }
        builder
            .push(" ORDER BY sequence_no DESC, memory_id DESC LIMIT ")
            .push_bind(PAGE_SIZE);
        let rows = builder
            .build_query_as::<MemoryRecordSqlRow>()
            .fetch_all(store.pool())
            .await
            .map_err(|err| DenError::System(format!("source notes read failed: {err}")))?;
        let fetched = rows.len();
        for row in rows {
            let row = row.into_row();
            cursor = Some((row.sequence_no, row.memory_id.clone()));
            if record_visible(store, &row.memory_id, &access).await? {
                visible.push(row);
                if visible.len() == limit {
                    return Ok(visible);
                }
            }
        }
        if fetched < PAGE_SIZE as usize {
            return Ok(visible);
        }
    }
}

pub async fn browse(
    store: &BearMemoryStore,
    grant: MemoryReadGrant,
    access: &AccessContext,
) -> Result<Vec<String>, DenError> {
    // sqlx-dynamic: optional hat visibility is composed by push_scope; all values are bound.
    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT memory_id, logical_path FROM memory_records
         WHERE bear_id = ",
    );
    builder
        .push_bind(store.bear_id().to_string())
        .push(" AND logical_path IS NOT NULL");
    push_scope(&mut builder, grant);
    builder.push(
        " AND visibility = 'normal' AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer
              WHERE newer.bear_id = memory_records.bear_id
                AND newer.supersedes_memory_id = memory_records.memory_id
          )
          ORDER BY logical_path ASC, sequence_no DESC",
    );
    let rows = builder
        .build_query_as::<(String, String)>()
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("scoped memory browse failed: {e}")))?;
    let mut paths = BTreeSet::new();
    for (memory_id, path) in rows {
        if record_visible(store, &memory_id, access).await? {
            paths.insert(path);
        }
    }
    Ok(paths.into_iter().collect())
}

pub async fn search(
    store: &BearMemoryStore,
    grant: MemoryReadGrant,
    access: &AccessContext,
    query: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let pattern = format!("%{}%", crate::admin_inspect::escape_like(query));
    // sqlx-dynamic: optional hat visibility is composed by push_scope; all values are bound.
    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
                logical_path, work_surface_ref, metadata_json, created_at, salience,
                supersedes_memory_id, invalid_at FROM memory_records
         WHERE bear_id = ",
    );
    builder.push_bind(store.bear_id().to_string());
    push_scope(&mut builder, grant);
    builder
        .push(
            " AND visibility = 'normal' AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active') != 'archived'
          AND content_text LIKE ",
        )
        .push_bind(pattern)
        .push(
            " ESCAPE '\\'
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer
              WHERE newer.bear_id = memory_records.bear_id
                AND newer.supersedes_memory_id = memory_records.memory_id
          )
          ORDER BY sequence_no DESC LIMIT ",
        )
        .push_bind(limit);
    let rows = builder
        .build_query_as::<MemoryRecordSqlRow>()
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("scoped memory search failed: {e}")))?;
    let mut visible = Vec::new();
    for row in rows {
        let row = row.into_row();
        if record_visible(store, &row.memory_id, access).await? {
            visible.push(row);
        }
    }
    Ok(visible)
}
