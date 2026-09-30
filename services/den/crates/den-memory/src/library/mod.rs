//! Curated library reads across shared memory and explicitly granted Bear-owned hats.
//! Callers must resolve hat ownership before constructing a grant; paths and record IDs
//! are locators, not authority.

use std::collections::{BTreeMap, BTreeSet};

use den_core::{ids::HatId, DenError};
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use crate::{
    access::{record_visible, AccessContext},
    admin_inspect::{escape_like, MemoryRecordDetail, PathSummary},
    logical_path::MemoryScopeType,
    records::{BearMemoryStore, MemoryRecordRow, MemoryRecordSqlRow},
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct CuratedMemoryGrant {
    hat_ids: Vec<HatId>,
}

impl CuratedMemoryGrant {
    /// `hat_ids` must already have been resolved as belonging to the current Bear
    /// by the caller, not taken from a model-supplied path or ID.
    pub fn new(hat_ids: Vec<HatId>) -> Self {
        Self { hat_ids }
    }

    pub fn hat_ids(&self) -> &[HatId] {
        &self.hat_ids
    }
}

// sqlx-dynamic: the number of Bear-owned hat IDs varies; every ID is bound before
// any limit or grouping, and empty grants still admit shared records.
fn push_eligible(builder: &mut QueryBuilder<'_, Sqlite>, grant: &CuratedMemoryGrant) {
    builder.push(" AND (scope_type = 'shared'");
    if !grant.hat_ids.is_empty() {
        builder.push(" OR (scope_type = 'hat' AND scope_hat_id IN (");
        let mut ids = builder.separated(", ");
        for id in &grant.hat_ids {
            ids.push_bind(id.to_string());
        }
        ids.push_unseparated("))");
    }
    builder.push(
        ") AND visibility = 'normal' AND invalid_at IS NULL
        AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active')
            NOT IN ('archived', 'archive-candidate')",
    );
}

fn push_current(builder: &mut QueryBuilder<'_, Sqlite>) {
    builder.push(
        " AND NOT EXISTS (
            SELECT 1 FROM memory_records newer
            WHERE newer.bear_id = memory_records.bear_id
              AND newer.supersedes_memory_id = memory_records.memory_id
        )",
    );
}

const ROW_COLUMNS: &str = "SELECT memory_id, sequence_no, scope_type, scope_profile, kind,
    content_text, logical_path, work_surface_ref, metadata_json, created_at, salience,
    supersedes_memory_id, invalid_at FROM memory_records WHERE bear_id = ";

enum CanonicalScope {
    Shared,
    Hat(HatId),
}

enum ReadKind<'a> {
    Recent,
    Search(&'a str), // Already escaped and wrapped for LIKE.
    History {
        scope: CanonicalScope,
        path: &'a str,
    },
}

// sqlx-dynamic: scope, keyword and keyset predicates are composed with bound
// values. A fixed page size bounds each fetch; the access gate decides when the
// result limit is met, even if multiple pages contain only restricted records.
async fn paged_visible_rows(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
    kind: ReadKind<'_>,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    const PAGE_SIZE: i64 = 64;
    let mut visible = Vec::new();
    let mut cursor: Option<(i64, String)> = None;
    let access = AccessContext::empty();
    loop {
        let mut builder = QueryBuilder::<Sqlite>::new(ROW_COLUMNS);
        builder.push_bind(store.bear_id().to_string());
        push_eligible(&mut builder, grant);
        match &kind {
            ReadKind::Recent => push_current(&mut builder),
            ReadKind::Search(pattern) => {
                push_current(&mut builder);
                builder
                    .push(" AND content_text LIKE ")
                    .push_bind(*pattern)
                    .push(" ESCAPE '\\'");
            }
            ReadKind::History { scope, path } => {
                builder.push(" AND logical_path = ").push_bind(*path);
                match scope {
                    CanonicalScope::Shared => {
                        builder.push(" AND scope_type = 'shared'");
                    }
                    CanonicalScope::Hat(id) => {
                        builder
                            .push(" AND scope_type = 'hat' AND scope_hat_id = ")
                            .push_bind(id.to_string());
                    }
                }
            }
        }
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
            .map_err(|e| DenError::System(format!("library read failed: {e}")))?;
        let fetched = rows.len();
        for row in rows {
            let row = row.into_row();
            cursor = Some((row.sequence_no, row.memory_id.clone()));
            if record_visible(store, &row.memory_id, &access).await? {
                visible.push(row);
                if visible.len() as i64 == limit {
                    return Ok(visible);
                }
            }
        }
        if fetched < PAGE_SIZE as usize {
            return Ok(visible);
        }
    }
}

/// Newest current, eligible records. Access-bearing records are omitted.
pub async fn recent(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    paged_visible_rows(store, grant, ReadKind::Recent, limit.clamp(1, 50)).await
}

/// Literal, case-insensitive keyword search over current eligible records.
pub async fn search(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
    query: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let pattern = format!("%{}%", escape_like(query));
    paged_visible_rows(
        store,
        grant,
        ReadKind::Search(&pattern),
        limit.clamp(1, 100),
    )
    .await
}

#[derive(sqlx::FromRow)]
struct CurrentEntryCandidate {
    memory_id: String,
    scope_type: String,
    scope_hat_id: Option<String>,
    logical_path: String,
}

/// Count visible current entries by canonical scope, hat ID, and logical path.
/// Unlike `browse`, this fetches only current eligible path-bearing heads and
/// never reads historical versions or their access rules.
pub async fn count_current_entries(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
) -> Result<usize, DenError> {
    // sqlx-dynamic: the grant's variable hat IDs are bound by push_eligible.
    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT memory_id, scope_type, scope_hat_id, logical_path
         FROM memory_records WHERE bear_id = ",
    );
    builder.push_bind(store.bear_id().to_string());
    push_eligible(&mut builder, grant);
    push_current(&mut builder);
    builder.push(" AND logical_path IS NOT NULL");
    let rows = builder
        .build_query_as::<CurrentEntryCandidate>()
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("library current entry count failed: {e}")))?;

    let access = AccessContext::empty();
    let mut entries = BTreeSet::new();
    for row in rows {
        if record_visible(store, &row.memory_id, &access).await? {
            entries.insert((row.scope_type, row.scope_hat_id, row.logical_path));
        }
    }
    Ok(entries.len())
}

#[derive(sqlx::FromRow)]
struct BrowseCandidate {
    memory_id: String,
    logical_path: String,
    scope_type: String,
    scope_hat_id: Option<String>,
    scope_profile: Option<String>,
    kind: String,
    created_at: String,
    is_current: bool,
}

/// One summary per path and canonical scope (including the specific hat ID).
/// Counts include only individually eligible, access-visible versions; a path
/// without a visible current head is omitted.
pub async fn browse(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
) -> Result<Vec<PathSummary>, DenError> {
    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT memory_id, logical_path, scope_type, scope_hat_id, scope_profile, kind,
            created_at, NOT EXISTS (
                SELECT 1 FROM memory_records newer
                WHERE newer.bear_id = memory_records.bear_id
                  AND newer.supersedes_memory_id = memory_records.memory_id
            ) AS is_current
         FROM memory_records WHERE bear_id = ",
    );
    builder.push_bind(store.bear_id().to_string());
    push_eligible(&mut builder, grant);
    builder.push(" AND logical_path IS NOT NULL ORDER BY logical_path, scope_type, scope_hat_id, sequence_no DESC");
    let rows = builder
        .build_query_as::<BrowseCandidate>()
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("library browse failed: {e}")))?;

    // Group only after the access gate, so restricted versions never inflate a count.
    let access = AccessContext::empty();
    let mut paths: BTreeMap<(String, String, Option<String>), (PathSummary, bool)> =
        BTreeMap::new();
    for row in rows {
        if !record_visible(store, &row.memory_id, &access).await? {
            continue;
        }
        let key = (
            row.logical_path.clone(),
            row.scope_type.clone(),
            row.scope_hat_id,
        );
        let (summary, has_head) = paths.entry(key).or_insert_with(|| {
            (
                PathSummary {
                    logical_path: row.logical_path,
                    scope_type: row.scope_type,
                    scope_profile: row.scope_profile.clone(),
                    kind: row.kind.clone(),
                    head_memory_id: row.memory_id.clone(),
                    head_created_at: row.created_at.clone(),
                    version_count: 0,
                },
                false,
            )
        });
        summary.version_count += 1;
        if row.is_current && !*has_head {
            summary.scope_profile = row.scope_profile;
            summary.kind = row.kind;
            summary.head_memory_id = row.memory_id;
            summary.head_created_at = row.created_at;
            *has_head = true;
        }
    }
    Ok(paths
        .into_values()
        .filter_map(|(summary, has_head)| has_head.then_some(summary))
        .collect())
}

/// A record by ID, including a superseded version only if it is still individually
/// eligible (not invalidated, archived, hidden, or access-bearing).
pub async fn detail(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
    memory_id: &str,
) -> Result<Option<MemoryRecordDetail>, DenError> {
    authorized_detail_row(store, grant, memory_id, false)
        .await?
        .map(decode_detail)
        .transpose()
}

/// A semantic candidate is only usable if it is still the current canonical
/// head. Qdrant's ID, scope, path, text and lifecycle payload are untrusted
/// locators; callers reconstruct all displayed content from this SQLite row.
pub async fn current_detail(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
    memory_id: &str,
) -> Result<Option<MemoryRecordDetail>, DenError> {
    authorized_detail_row(store, grant, memory_id, true)
        .await?
        .map(decode_detail)
        .transpose()
}

async fn authorized_detail_row(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
    memory_id: &str,
    require_current: bool,
) -> Result<Option<SqliteRow>, DenError> {
    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT memory_id, sequence_no, scope_type, scope_hat_id, scope_profile, kind, author_profile,
            author_agent_id, visibility, supersedes_memory_id, logical_path, work_surface_ref,
            content_text, metadata_json, created_at FROM memory_records WHERE bear_id = ",
    );
    builder.push_bind(store.bear_id().to_string());
    push_eligible(&mut builder, grant);
    if require_current {
        push_current(&mut builder);
    }
    builder.push(" AND memory_id = ").push_bind(memory_id);
    let row = builder
        .build()
        .fetch_optional(store.pool())
        .await
        .map_err(|e| DenError::System(format!("library detail failed: {e}")))?;
    match row {
        Some(row) if record_visible(store, memory_id, &AccessContext::empty()).await? => {
            Ok(Some(row))
        }
        _ => Ok(None),
    }
}

fn decode_detail(row: SqliteRow) -> Result<MemoryRecordDetail, DenError> {
    let metadata_raw: String = row
        .try_get("metadata_json")
        .map_err(|e| DenError::System(format!("decode library detail metadata: {e}")))?;
    let metadata_json =
        serde_json::from_str(&metadata_raw).unwrap_or_else(|_| serde_json::json!({}));
    let decode = |e: sqlx::Error| DenError::System(format!("decode library detail: {e}"));
    Ok(MemoryRecordDetail {
        memory_id: row.try_get("memory_id").map_err(decode)?,
        sequence_no: row.try_get("sequence_no").map_err(decode)?,
        scope_type: row.try_get("scope_type").map_err(decode)?,
        scope_profile: row.try_get("scope_profile").map_err(decode)?,
        kind: row.try_get("kind").map_err(decode)?,
        author_profile: row.try_get("author_profile").map_err(decode)?,
        author_agent_id: row.try_get("author_agent_id").map_err(decode)?,
        visibility: row.try_get("visibility").map_err(decode)?,
        supersedes_memory_id: row.try_get("supersedes_memory_id").map_err(decode)?,
        logical_path: row.try_get("logical_path").map_err(decode)?,
        work_surface_ref: row.try_get("work_surface_ref").map_err(decode)?,
        content_text: row.try_get("content_text").map_err(decode)?,
        metadata_json,
        created_at: row.try_get("created_at").map_err(decode)?,
    })
}

/// Newest individually eligible versions of an authorized record's exact
/// canonical scope (including its hat ID) and path. A guessed or restricted
/// anchor returns no history; a path alone cannot confer access.
pub async fn history(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
    memory_id: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let Some(anchor) = authorized_detail_row(store, grant, memory_id, false).await? else {
        return Ok(Vec::new());
    };
    let decode = |e: sqlx::Error| DenError::System(format!("decode library history anchor: {e}"));
    let Some(path): Option<String> = anchor.try_get("logical_path").map_err(decode)? else {
        return Ok(Vec::new());
    };
    let scope_type: String = anchor.try_get("scope_type").map_err(decode)?;
    let scope = match MemoryScopeType::parse(&scope_type) {
        Some(MemoryScopeType::Shared) => CanonicalScope::Shared,
        Some(MemoryScopeType::Hat) => {
            let Some(raw): Option<String> = anchor.try_get("scope_hat_id").map_err(decode)? else {
                return Ok(Vec::new());
            };
            let id = raw
                .parse::<HatId>()
                .map_err(|e| DenError::System(format!("invalid library anchor hat ID: {e}")))?;
            CanonicalScope::Hat(id)
        }
        _ => return Ok(Vec::new()),
    };
    paged_visible_rows(
        store,
        grant,
        ReadKind::History { scope, path: &path },
        limit.clamp(1, 100),
    )
    .await
}
