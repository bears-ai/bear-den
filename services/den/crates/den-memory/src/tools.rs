use den_core::{BearProfile, DenError};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    append_memory_record, list_records_for_logical_path, records::normalize_lifecycle_status,
    BearMemoryStore, LogicalMemoryPath, MemoryRecordRow, MemoryScopeType, MemorySource,
    MemoryStoreManager,
};

pub async fn sqlite_write_at_path(
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    logical_path: &str,
    author_profile: &str,
    title: &str,
    body: &str,
    metadata: Value,
) -> Result<Value, DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    let logical = LogicalMemoryPath::from_logical_path(logical_path);
    let content = if body.starts_with('#') {
        body.to_string()
    } else {
        format!("# {title}\n\n{body}")
    };
    let mut metadata_obj = metadata.as_object().cloned().unwrap_or_default();
    metadata_obj.insert("title".to_string(), json!(title));
    metadata_obj.insert(
        "claim_fingerprint".to_string(),
        json!(crate::promotions::memory_claim_fingerprint(&content)),
    );
    metadata_obj.insert("storage".to_string(), json!("sqlite"));
    metadata_obj.insert("runtime".to_string(), json!("native"));
    let row = append_memory_record(
        &store,
        &logical,
        &logical.kind,
        author_profile,
        None,
        &content,
        &Value::Object(metadata_obj),
    )
    .await?;
    Ok(json!({
        "bear_id": bear_id,
        "profile": author_profile,
        "kind": row.kind,
        "entry_id": row.memory_id,
        "path": row.logical_path,
        "sequence_no": row.sequence_no,
        "storage": "sqlite",
        "lifecycle_status": row.lifecycle_status,
        "freshness_trend": row.freshness_trend,
    }))
}

pub struct SqliteMemoryEntryWrite<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub tags: &'a [String],
    pub refs: Option<Value>,
    pub lifecycle: Option<Value>,
    pub source: Option<Value>,
    pub author: Option<String>,
}

pub async fn sqlite_write_profile_entry(
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    profile: &str,
    kind: &str,
    title: &str,
    body: &str,
    tags: &[String],
    refs: Option<Value>,
    lifecycle: Option<Value>,
    source: Option<Value>,
    author: Option<String>,
) -> Result<Value, DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    write_semantic_entry(
        &store,
        LogicalMemoryPath::profile_local(profile, kind),
        profile,
        SqliteMemoryEntryWrite {
            kind,
            title,
            body,
            tags,
            refs,
            lifecycle,
            source,
            author,
        },
    )
    .await
}

/// Source scope must come from a Den-verified conversation, Work run, or intake unit.
/// Model arguments may propose the content, never select its scope.
pub async fn sqlite_write_source_entry(
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    memory_source: MemorySource,
    author_profile: &str,
    entry: SqliteMemoryEntryWrite<'_>,
) -> Result<Value, DenError> {
    let store = stores.store_for_bear(bear_id).await?;
    let logical = LogicalMemoryPath::source_local(memory_source, entry.kind);
    write_semantic_entry(&store, logical, author_profile, entry).await
}

async fn write_semantic_entry(
    store: &BearMemoryStore,
    logical: LogicalMemoryPath,
    profile: &str,
    entry: SqliteMemoryEntryWrite<'_>,
) -> Result<Value, DenError> {
    let SqliteMemoryEntryWrite {
        kind,
        title,
        body,
        tags,
        refs,
        lifecycle,
        source,
        author,
    } = entry;
    let content = format!("# {title}\n\n{body}");
    let mut lifecycle = lifecycle.unwrap_or_else(|| json!({}));
    if let Some(status) = lifecycle.get("status").and_then(Value::as_str) {
        let normalized = normalize_lifecycle_status(status)?;
        if let Some(obj) = lifecycle.as_object_mut() {
            obj.insert("status".to_string(), json!(normalized));
        }
    }
    let metadata = json!({
        "title": title,
        "tags": tags,
        "refs": refs,
        "lifecycle": lifecycle,
        "source": source,
        "author": author,
        "claim_fingerprint": crate::promotions::memory_claim_fingerprint(&content),
        "storage": "sqlite",
        "runtime": "native",
    });
    let row =
        append_memory_record(store, &logical, kind, profile, None, &content, &metadata).await?;
    Ok(json!({
        "bear_id": store.bear_id(),
        "profile": profile,
        "kind": row.kind,
        "entry_id": row.memory_id,
        "path": row.logical_path,
        "sequence_no": row.sequence_no,
        "storage": "sqlite",
        "lifecycle_status": row.lifecycle_status,
        "freshness_trend": row.freshness_trend,
    }))
}

pub async fn sqlite_memory_browse(store: &BearMemoryStore, role: &str) -> Result<Value, DenError> {
    let rows = sqlx::query_scalar::<_, String>(
        r"
        SELECT DISTINCT logical_path
        FROM memory_records
        WHERE bear_id = ? AND scope_profile = ? AND logical_path IS NOT NULL
          AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active') NOT IN ('archived', 'archive-candidate')
        ORDER BY logical_path ASC
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(role)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite memory browse failed: {e}")))?;
    let children: Vec<Value> = rows
        .into_iter()
        .map(|path| {
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            json!({
                "name": name,
                "path": path,
                "type": "file",
            })
        })
        .collect();
    Ok(json!({
        "ok": true,
        "configured": true,
        "storage": "sqlite",
        "role": role,
        "children": children,
    }))
}

/// Model-facing legacy read: enforce the same profile-local/core boundary as
/// keyword and vector search. A path is a locator, never an access grant.
pub async fn sqlite_memory_read_for_profile(
    store: &BearMemoryStore,
    profile: BearProfile,
    logical_path: &str,
) -> Result<Value, DenError> {
    let rows = list_records_for_logical_path(store, logical_path, 20)
        .await?
        .into_iter()
        .filter(|row| {
            row.scope_type == MemoryScopeType::Shared
                || (row.scope_type == MemoryScopeType::ProfileLocal
                    && (profile == BearProfile::Curate
                        || row.scope_profile.as_deref() == Some(profile.as_str())))
        })
        .collect();
    Ok(render_memory_read(logical_path, rows))
}

/// Privileged/admin read; ordinary model tools must use a scope-enforcing API.
pub async fn sqlite_memory_read(
    store: &BearMemoryStore,
    logical_path: &str,
) -> Result<Value, DenError> {
    let rows = list_records_for_logical_path(store, logical_path, 20).await?;
    Ok(render_memory_read(logical_path, rows))
}

pub fn render_memory_read(logical_path: &str, rows: Vec<MemoryRecordRow>) -> Value {
    if rows.is_empty() {
        return json!({
            "ok": false,
            "configured": true,
            "storage": "sqlite",
            "path": logical_path,
            "message": "no records at path",
        });
    }
    let body: String = rows
        .iter()
        .rev()
        .map(|r| r.content_text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    let records: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "memory_id": row.memory_id,
                "sequence_no": row.sequence_no,
                "kind": row.kind,
                "salience": row.salience,
                "supersedes_memory_id": row.supersedes_memory_id,
                "invalid_at": row.invalid_at,
                "lifecycle_status": row.lifecycle_status,
                "freshness_trend": row.freshness_trend,
                "created_at": row.created_at,
            })
        })
        .collect();
    json!({
        "ok": true,
        "configured": true,
        "storage": "sqlite",
        "path": logical_path,
        "content": body,
        "record_count": rows.len(),
        "latest_sequence_no": rows.first().map(|r| r.sequence_no),
        "latest_lifecycle_status": rows.first().map(|r| r.lifecycle_status.as_str()),
        "latest_freshness_trend": rows.first().map(|r| r.freshness_trend.as_str()),
        "records": records,
    })
}

/// Keyword (`LIKE`) memory search — the fallback path for `memory_search` when the derived
/// recall index is unavailable. Scoped to memory **visible to `role`**: shared (core) records
/// OR this role's own profile-local records (matching the vector path's `role_scope_filter`, so
/// both strategies honor the same role-local boundary). Provenance mirrors the vector path
/// (`memory_id`, `path`, `snippet`); `score` is `null` since keyword matching is unranked.
pub async fn sqlite_memory_search(
    store: &BearMemoryStore,
    role: &str,
    query: &str,
    limit: i64,
) -> Result<Value, DenError> {
    let pattern = format!("%{}%", crate::admin_inspect::escape_like(query));
    let rows = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            String,
            i64,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ),
    >(
        r"
        SELECT memory_id, logical_path, content_text, sequence_no, kind, salience,
               metadata_json, supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ?
          AND visibility = 'normal'
          AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active') != 'archived'
          AND (scope_type = 'shared' OR scope_profile = ?)
          AND content_text LIKE ? ESCAPE '\'
          AND NOT EXISTS (
            SELECT 1 FROM memory_records newer
            WHERE newer.bear_id = memory_records.bear_id
              AND newer.supersedes_memory_id = memory_records.memory_id
          )
        ORDER BY sequence_no DESC
        LIMIT ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(role)
    .bind(pattern)
    .bind(limit)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite memory search failed: {e}")))?;
    let hits: Vec<Value> = rows
        .into_iter()
        .map(
            |(
                memory_id,
                path,
                content,
                sequence_no,
                kind,
                salience,
                metadata_json,
                supersedes_memory_id,
                invalid_at,
            )| {
                let metadata_json: Value =
                    serde_json::from_str(&metadata_json).unwrap_or_else(|_| json!({}));
                let lifecycle_status = crate::records::lifecycle_status(
                    &metadata_json,
                    supersedes_memory_id.as_deref(),
                    invalid_at.as_deref(),
                );
                let freshness_trend =
                    crate::records::freshness_trend(&lifecycle_status, invalid_at.as_deref());
                json!({
                    "memory_id": memory_id,
                    "path": path,
                    "kind": kind,
                    "salience": salience,
                    "lifecycle_status": lifecycle_status,
                    "freshness_trend": freshness_trend,
                    "supersedes_memory_id": supersedes_memory_id,
                    "invalid_at": invalid_at,
                    "score": Value::Null,
                    "snippet": content.chars().take(240).collect::<String>(),
                    "sequence_no": sequence_no,
                })
            },
        )
        .collect();
    Ok(json!({
        "ok": true,
        "configured": true,
        "storage": "sqlite",
        "strategy": "keyword",
        "query": query,
        "hits": hits,
    }))
}

pub async fn sqlite_collect_role_logical_paths(
    store: &BearMemoryStore,
    role: &str,
) -> Result<Vec<String>, DenError> {
    sqlx::query_scalar::<_, String>(
        r"
        SELECT DISTINCT logical_path
        FROM memory_records
        WHERE bear_id = ? AND scope_profile = ? AND logical_path IS NOT NULL
          AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active') NOT IN ('archived', 'archive-candidate')
        ORDER BY logical_path ASC
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(role)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite collect paths failed: {e}")))
}

pub async fn sqlite_list_plan_artifacts(
    store: &BearMemoryStore,
    role: &str,
    limit: i64,
) -> Result<Value, DenError> {
    let rows = sqlx::query_as::<_, (String, String, String, i64)>(
        r"
        SELECT memory_id, logical_path, content_text, sequence_no
        FROM memory_records
        WHERE bear_id = ? AND scope_profile = ? AND logical_path LIKE ?
        ORDER BY sequence_no DESC
        LIMIT ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(role)
    .bind(format!("{role}/plans/%"))
    .bind(limit)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite list plan artifacts failed: {e}")))?;
    let results: Vec<Value> = rows
        .into_iter()
        .map(|(memory_id, path, content, sequence_no)| {
            json!({
                "memory_id": memory_id,
                "path": path,
                "snippet": content.chars().take(240).collect::<String>(),
                "sequence_no": sequence_no,
                "storage": "sqlite",
            })
        })
        .collect();
    Ok(json!(results))
}

pub async fn sqlite_memory_status(store: &BearMemoryStore, role: &str) -> Result<Value, DenError> {
    let file_count: i64 = sqlx::query_scalar(
        r"
        SELECT COUNT(DISTINCT logical_path)
        FROM memory_records
        WHERE bear_id = ? AND scope_profile = ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(role)
    .fetch_one(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite memory status failed: {e}")))?;
    Ok(json!({
        "configured": true,
        "available": true,
        "storage": "sqlite",
        "role": role,
        "file_count": file_count,
    }))
}

#[cfg(test)]
mod tests;
