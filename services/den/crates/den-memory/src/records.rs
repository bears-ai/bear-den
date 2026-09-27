use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::DenError;

use super::{
    clock::now_rfc3339,
    logical_path::{entity_anchor_path, LogicalMemoryPath, MemoryScopeType},
};

#[derive(Debug, Clone, Serialize)]
pub struct MemoryRecordRow {
    pub memory_id: String,
    pub sequence_no: i64,
    pub scope_type: MemoryScopeType,
    pub scope_profile: Option<String>,
    pub kind: String,
    pub content_text: String,
    pub logical_path: Option<String>,
    pub work_surface_ref: Option<String>,
    pub metadata_json: Value,
    pub created_at: String,
    pub salience: String,
    pub supersedes_memory_id: Option<String>,
    pub invalid_at: Option<String>,
    pub lifecycle_status: String,
    pub freshness_trend: String,
}

pub struct BearMemoryStore {
    bear_id: Uuid,
    pool: SqlitePool,
}

impl BearMemoryStore {
    pub fn new(bear_id: Uuid, pool: SqlitePool) -> Self {
        Self { bear_id, pool }
    }

    pub fn bear_id(&self) -> Uuid {
        self.bear_id
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn next_sequence(&self) -> Result<i64, DenError> {
        sqlx::query("UPDATE bear_sequence SET next_sequence = next_sequence + 1 WHERE id = 1")
            .execute(&self.pool)
            .await
            .map_err(|e| DenError::System(format!("bear sequence bump failed: {e}")))?;
        let row = sqlx::query_scalar::<_, i64>(
            "SELECT next_sequence - 1 FROM bear_sequence WHERE id = 1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DenError::System(format!("bear sequence alloc failed: {e}")))?;
        Ok(row)
    }

    pub async fn append_record(
        &self,
        logical: &LogicalMemoryPath,
        kind: &str,
        author_profile: &str,
        author_agent_id: Option<&str>,
        content_text: &str,
        metadata_json: &Value,
        visibility: &str,
    ) -> Result<MemoryRecordRow, DenError> {
        self.append_record_with_options(
            logical,
            kind,
            author_profile,
            author_agent_id,
            content_text,
            metadata_json,
            visibility,
            "normal",
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn append_record_with_options(
        &self,
        logical: &LogicalMemoryPath,
        kind: &str,
        author_profile: &str,
        author_agent_id: Option<&str>,
        content_text: &str,
        metadata_json: &Value,
        visibility: &str,
        salience: &str,
        supersedes_memory_id: Option<&str>,
    ) -> Result<MemoryRecordRow, DenError> {
        let salience = normalize_salience(salience)?;
        if let Some(superseded) = supersedes_memory_id {
            ensure_record_exists(self, superseded).await?;
        }

        let memory_id = Uuid::new_v4().to_string();
        let sequence_no = self.next_sequence().await?;
        let created_at = now_rfc3339()?;
        let logical_path = logical.to_logical_path();
        let source_kind = logical.source.map(|source| source.kind());
        let source_id = logical.source.map(|source| source.id().to_string());
        let hat_id = logical.hat_id.map(|id| id.to_string());
        sqlx::query(
            r"
            INSERT INTO memory_records (
                memory_id, bear_id, sequence_no, scope_type, scope_profile,
                scope_source_kind, scope_source_id, scope_hat_id, kind,
                author_profile, author_agent_id, created_at, content_text, metadata_json,
                visibility, logical_path, work_surface_ref, valid_from, salience, supersedes_memory_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&memory_id)
        .bind(self.bear_id.to_string())
        .bind(sequence_no)
        .bind(logical.scope_type.as_str())
        .bind(&logical.scope_profile)
        .bind(source_kind)
        .bind(source_id)
        .bind(hat_id)
        .bind(kind)
        .bind(author_profile)
        .bind(author_agent_id)
        .bind(&created_at)
        .bind(content_text)
        .bind(metadata_json.to_string())
        .bind(visibility)
        .bind(&logical_path)
        .bind(&logical.work_surface_ref)
        .bind(&created_at)
        .bind(salience)
        .bind(supersedes_memory_id)
        .execute(&self.pool)
        .await
        .map_err(|e| DenError::System(format!("append memory_record failed: {e}")))?;

        if let Some(superseded) = supersedes_memory_id {
            sqlx::query(
                "UPDATE memory_records SET invalid_at = COALESCE(invalid_at, ?) WHERE bear_id = ? AND memory_id = ?",
            )
            .bind(&created_at)
            .bind(self.bear_id.to_string())
            .bind(superseded)
            .execute(&self.pool)
            .await
            .map_err(|e| DenError::System(format!("invalidate superseded memory_record failed: {e}")))?;
        }

        let lifecycle_status = lifecycle_status(metadata_json, supersedes_memory_id, None);
        let freshness_trend = freshness_trend(&lifecycle_status, None);
        Ok(MemoryRecordRow {
            memory_id,
            sequence_no,
            scope_type: logical.scope_type,
            scope_profile: logical.scope_profile.clone(),
            kind: kind.to_string(),
            content_text: content_text.to_string(),
            logical_path: Some(logical_path),
            work_surface_ref: logical.work_surface_ref.clone(),
            metadata_json: metadata_json.clone(),
            created_at,
            salience: salience.to_string(),
            supersedes_memory_id: supersedes_memory_id.map(str::to_string),
            invalid_at: None,
            lifecycle_status,
            freshness_trend,
        })
    }
}

fn normalize_salience(value: &str) -> Result<&'static str, DenError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Ok("low"),
        "" | "normal" => Ok("normal"),
        "high" => Ok("high"),
        "critical" => Ok("critical"),
        other => Err(DenError::ValidationError(format!(
            "salience must be low, normal, high, or critical; got {other}"
        ))),
    }
}

pub async fn append_memory_record(
    store: &BearMemoryStore,
    logical: &LogicalMemoryPath,
    kind: &str,
    author_profile: &str,
    author_agent_id: Option<&str>,
    content_text: &str,
    metadata_json: &Value,
) -> Result<MemoryRecordRow, DenError> {
    store
        .append_record(
            logical,
            kind,
            author_profile,
            author_agent_id,
            content_text,
            metadata_json,
            "normal",
        )
        .await
}

async fn ensure_record_exists(store: &BearMemoryStore, memory_id: &str) -> Result<(), DenError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_records WHERE bear_id = ? AND memory_id = ?",
    )
    .bind(store.bear_id.to_string())
    .bind(memory_id)
    .fetch_one(store.pool())
    .await
    .map_err(|e| DenError::System(format!("superseded memory_record lookup failed: {e}")))?;
    if count == 0 {
        return Err(DenError::ValidationError(format!(
            "supersedes_memory_id does not exist for this bear: {memory_id}"
        )));
    }
    Ok(())
}

pub fn normalize_lifecycle_status(value: &str) -> Result<&'static str, DenError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "active" => Ok("active"),
        "superseded" => Ok("superseded"),
        "stale" => Ok("stale"),
        "archived" => Ok("archived"),
        "archive-candidate" | "archive_candidate" => Ok("archive-candidate"),
        other => Err(DenError::ValidationError(format!(
            "lifecycle.status must be active, superseded, stale, archived, or archive-candidate; got {other}"
        ))),
    }
}

pub fn lifecycle_status(
    metadata_json: &Value,
    supersedes_memory_id: Option<&str>,
    invalid_at: Option<&str>,
) -> String {
    if invalid_at.is_some() {
        return "superseded".to_string();
    }
    if supersedes_memory_id.is_some() {
        return metadata_json
            .pointer("/lifecycle/status")
            .and_then(Value::as_str)
            .and_then(|value| normalize_lifecycle_status(value).ok())
            .unwrap_or("active")
            .to_string();
    }
    metadata_json
        .pointer("/lifecycle/status")
        .and_then(Value::as_str)
        .and_then(|value| normalize_lifecycle_status(value).ok())
        .unwrap_or("active")
        .to_string()
}

pub fn freshness_trend(lifecycle_status: &str, invalid_at: Option<&str>) -> String {
    if invalid_at.is_some() {
        return "stale".to_string();
    }
    match lifecycle_status {
        "superseded" | "stale" | "archived" => "stale".to_string(),
        "archive-candidate" => "weakening".to_string(),
        _ => "stable".to_string(),
    }
}

pub async fn mark_memory_record_lifecycle(
    store: &BearMemoryStore,
    memory_id: &str,
    status: &str,
    reason: Option<&str>,
) -> Result<MemoryRecordRow, DenError> {
    let status = normalize_lifecycle_status(status)?;
    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT metadata_json, invalid_at FROM memory_records WHERE bear_id = ? AND memory_id = ?",
    )
    .bind(store.bear_id.to_string())
    .bind(memory_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("fetch memory lifecycle metadata failed: {e}")))?
    .ok_or_else(|| DenError::NotFound("memory record not found".to_string()))?;

    let mut metadata: Value =
        serde_json::from_str(&row.0).unwrap_or_else(|_| Value::Object(Default::default()));
    if !metadata.is_object() {
        metadata = Value::Object(Default::default());
    }
    let now = now_rfc3339()?;
    let lifecycle = metadata
        .as_object_mut()
        .expect("metadata object initialized")
        .entry("lifecycle".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !lifecycle.is_object() {
        *lifecycle = Value::Object(Default::default());
    }
    let lifecycle_obj = lifecycle
        .as_object_mut()
        .expect("lifecycle object initialized");
    lifecycle_obj.insert("status".to_string(), Value::String(status.to_string()));
    lifecycle_obj.insert("marked_at".to_string(), Value::String(now.clone()));
    if let Some(reason) = reason.map(str::trim).filter(|value| !value.is_empty()) {
        lifecycle_obj.insert("reason".to_string(), Value::String(reason.to_string()));
    }

    let invalid_at = if status == "superseded" && row.1.is_none() {
        Some(now.as_str())
    } else {
        None
    };
    if let Some(invalid_at) = invalid_at {
        sqlx::query(
            "UPDATE memory_records SET metadata_json = ?, invalid_at = ? WHERE bear_id = ? AND memory_id = ?",
        )
        .bind(metadata.to_string())
        .bind(invalid_at)
        .bind(store.bear_id.to_string())
        .bind(memory_id)
        .execute(store.pool())
        .await
        .map_err(|e| DenError::System(format!("mark memory lifecycle failed: {e}")))?;
    } else {
        sqlx::query(
            "UPDATE memory_records SET metadata_json = ? WHERE bear_id = ? AND memory_id = ?",
        )
        .bind(metadata.to_string())
        .bind(store.bear_id.to_string())
        .bind(memory_id)
        .execute(store.pool())
        .await
        .map_err(|e| DenError::System(format!("mark memory lifecycle failed: {e}")))?;
    }

    fetch_record_by_id(store, memory_id)
        .await?
        .ok_or_else(|| DenError::NotFound("memory record not found".to_string()))
}

pub async fn fetch_record_by_id(
    store: &BearMemoryStore,
    memory_id: &str,
) -> Result<Option<MemoryRecordRow>, DenError> {
    let row = sqlx::query_as::<_, MemoryRecordSqlRow>(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ? AND memory_id = ?
        ",
    )
    .bind(store.bear_id.to_string())
    .bind(memory_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("fetch memory_record by id failed: {e}")))?;
    Ok(row.map(MemoryRecordSqlRow::into_row))
}

pub async fn memory_sequence_high_water(store: &BearMemoryStore) -> Result<i64, DenError> {
    let row = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(sequence_no) FROM memory_records WHERE bear_id = ?",
    )
    .bind(store.bear_id.to_string())
    .fetch_one(store.pool())
    .await
    .map_err(|e| DenError::System(format!("memory sequence high water failed: {e}")))?;
    Ok(row.unwrap_or(0))
}

pub async fn head_record_for_logical_path(
    store: &BearMemoryStore,
    logical_path: &str,
) -> Result<Option<MemoryRecordRow>, DenError> {
    let row = sqlx::query_as::<_, MemoryRecordSqlRow>(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ? AND logical_path = ? AND visibility = 'normal'
          AND NOT EXISTS (
            SELECT 1 FROM memory_records newer
            WHERE newer.bear_id = memory_records.bear_id
              AND newer.supersedes_memory_id = memory_records.memory_id
          )
        ORDER BY sequence_no DESC
        LIMIT 1
        ",
    )
    .bind(store.bear_id.to_string())
    .bind(logical_path)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("head memory_record lookup failed: {e}")))?;
    Ok(row.map(MemoryRecordSqlRow::into_row))
}

pub async fn has_work_surface_canonical_anchor(
    store: &BearMemoryStore,
    slug: &str,
) -> Result<bool, DenError> {
    for path in [
        format!("core/work_surfaces/{slug}/index.md"),
        format!("core/work_surfaces/{slug}/overview.md"),
    ] {
        if head_record_for_logical_path(store, &path).await?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn list_profile_local_head_records(
    store: &BearMemoryStore,
    profile: &str,
    work_surface_ref: Option<&str>,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let rows = if let Some(surface) = work_surface_ref {
        sqlx::query_as::<_, MemoryRecordSqlRow>(
            r"
            SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
                   logical_path, work_surface_ref, metadata_json, created_at, salience,
                   supersedes_memory_id, invalid_at
            FROM memory_records
            WHERE bear_id = ? AND scope_type = 'profile_local' AND scope_profile = ?
              AND visibility = 'normal' AND work_surface_ref = ?
              AND NOT EXISTS (
                SELECT 1 FROM memory_records newer
                WHERE newer.bear_id = memory_records.bear_id
                  AND newer.supersedes_memory_id = memory_records.memory_id
              )
            ORDER BY sequence_no DESC
            LIMIT ?
            ",
        )
        .bind(store.bear_id.to_string())
        .bind(profile)
        .bind(surface)
        .bind(limit)
        .fetch_all(store.pool())
        .await
    } else {
        sqlx::query_as::<_, MemoryRecordSqlRow>(
            r"
            SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
                   logical_path, work_surface_ref, metadata_json, created_at, salience,
                   supersedes_memory_id, invalid_at
            FROM memory_records
            WHERE bear_id = ? AND scope_type = 'profile_local' AND scope_profile = ?
              AND visibility = 'normal' AND work_surface_ref IS NULL
              AND NOT EXISTS (
                SELECT 1 FROM memory_records newer
                WHERE newer.bear_id = memory_records.bear_id
                  AND newer.supersedes_memory_id = memory_records.memory_id
              )
            ORDER BY sequence_no DESC
            LIMIT ?
            ",
        )
        .bind(store.bear_id.to_string())
        .bind(profile)
        .bind(limit)
        .fetch_all(store.pool())
        .await
    }
    .map_err(|e| DenError::System(format!("list profile_local memory_records failed: {e}")))?;
    Ok(rows.into_iter().map(MemoryRecordSqlRow::into_row).collect())
}

pub async fn list_records_for_logical_path(
    store: &BearMemoryStore,
    logical_path: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let rows = sqlx::query_as::<_, MemoryRecordSqlRow>(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ? AND logical_path = ?
          AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active') NOT IN ('archived', 'archive-candidate')
        ORDER BY sequence_no DESC
        LIMIT ?
        ",
    )
    .bind(store.bear_id.to_string())
    .bind(logical_path)
    .bind(limit)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list memory_records failed: {e}")))?;
    Ok(rows.into_iter().map(MemoryRecordSqlRow::into_row).collect())
}

pub async fn list_record_history_for_logical_path(
    store: &BearMemoryStore,
    logical_path: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let rows = sqlx::query_as::<_, MemoryRecordSqlRow>(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ? AND logical_path = ?
        ORDER BY sequence_no DESC
        LIMIT ?
        ",
    )
    .bind(store.bear_id.to_string())
    .bind(logical_path)
    .bind(limit)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list memory_record history failed: {e}")))?;
    Ok(rows.into_iter().map(MemoryRecordSqlRow::into_row).collect())
}

/// Explicit entity anchor records eligible for key-memory projection (ADR-0042 Phase 5 v1).
///
/// Policy is intentionally conservative: an entity must be resolved/confirmed, descriptor
/// anchor-eligible, satisfy the subject-linked salience threshold, and have an explicit canonical
/// anchor record at a generated path. No fallback content is synthesized from arbitrary relations.
pub async fn list_entity_anchor_head_records(
    store: &BearMemoryStore,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let candidates = sqlx::query_as::<_, EntityAnchorCandidate>(
        r"
        SELECT e.entity_id, e.type, e.display_name, e.resolution, e.canonical_ref,
               SUM(CASE WHEN m.salience IN ('high', 'critical') THEN 1 ELSE 0 END) AS high_count,
               SUM(CASE WHEN m.salience = 'normal' THEN 1 ELSE 0 END) AS normal_count,
               MAX(m.sequence_no) AS latest_sequence
        FROM entities e
        INNER JOIN memory_relations r
            ON r.bear_id = e.bear_id
           AND r.entity_id = e.entity_id
           AND r.state = 'active'
           AND r.relation = 'den.memory.relation.subject'
        INNER JOIN memory_records m
            ON m.bear_id = e.bear_id
           AND m.memory_id = r.src_memory_id
           AND m.visibility = 'normal'
        WHERE e.bear_id = ?
          AND e.resolution IN ('resolved', 'confirmed')
          AND e.superseded_by_entity_id IS NULL
        GROUP BY e.entity_id, e.type, e.display_name, e.resolution, e.canonical_ref
        ORDER BY latest_sequence DESC
        LIMIT ?
        ",
    )
    .bind(store.bear_id.to_string())
    .bind(limit.saturating_mul(4).clamp(1, 200))
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list entity anchor candidates failed: {e}")))?;

    let mut out = Vec::new();
    for candidate in candidates {
        if out.len() >= limit.max(0) as usize {
            break;
        }
        let eligible = candidate.high_count.unwrap_or(0) > 0
            || (candidate.resolution == "confirmed" && candidate.normal_count.unwrap_or(0) >= 2);
        if !eligible {
            continue;
        }
        let Some(anchor_ref) = candidate
            .display_name
            .as_deref()
            .or(candidate.canonical_ref.as_deref())
            .or(Some(candidate.entity_id.as_str()))
        else {
            continue;
        };
        for kind in ["profile", "overview", "index"] {
            let Some(path) = entity_anchor_path(&candidate.entity_type, anchor_ref, kind) else {
                continue;
            };
            if let Some(record) = head_record_for_logical_path(store, &path).await? {
                out.push(record);
                break;
            }
        }
    }
    Ok(out)
}

#[derive(sqlx::FromRow)]
struct EntityAnchorCandidate {
    entity_id: String,
    #[sqlx(rename = "type")]
    entity_type: String,
    display_name: Option<String>,
    resolution: String,
    canonical_ref: Option<String>,
    high_count: Option<i64>,
    normal_count: Option<i64>,
}

/// Minimal record fields for rendering a recall hit (bounded-graph leg). Cheaper than a full
/// [`MemoryRecordRow`] and carries no metadata/sequence.
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

/// Fetch minimal fields for a set of memory ids, **role-scoped** to memory visible to `role`
/// (shared ∨ own profile-local) at normal visibility. Order is unspecified and role-hidden ids are
/// dropped; the caller re-associates by id. Used by the bounded-graph recall leg.
pub async fn fetch_records_min(
    store: &BearMemoryStore,
    memory_ids: &[String],
    role: &str,
) -> Result<Vec<RecallRecordMin>, DenError> {
    if memory_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; memory_ids.len()].join(",");
    let sql = format!(
        "SELECT memory_id, logical_path, kind, content_text, salience, metadata_json, \
                supersedes_memory_id, invalid_at FROM memory_records \
         WHERE bear_id = ? AND visibility = 'normal' AND invalid_at IS NULL \
          AND (scope_type = 'shared' OR scope_profile = ?) \
          AND memory_id IN ({placeholders})"
    );
    let mut query = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ),
    >(&sql)
    .bind(store.bear_id().to_string())
    .bind(role);
    for id in memory_ids {
        query = query.bind(id);
    }
    let rows = query
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("fetch records min failed: {e}")))?;
    Ok(rows
        .into_iter()
        .map(
            |(
                memory_id,
                logical_path,
                kind,
                content_text,
                salience,
                metadata_json,
                supersedes_memory_id,
                invalid_at,
            )| {
                let metadata_json: Value = serde_json::from_str(&metadata_json)
                    .unwrap_or_else(|_| Value::Object(Default::default()));
                let lifecycle_status = lifecycle_status(
                    &metadata_json,
                    supersedes_memory_id.as_deref(),
                    invalid_at.as_deref(),
                );
                let freshness_trend = freshness_trend(&lifecycle_status, invalid_at.as_deref());
                RecallRecordMin {
                    memory_id,
                    logical_path,
                    kind,
                    content_text,
                    salience,
                    lifecycle_status,
                    freshness_trend,
                }
            },
        )
        .collect())
}

pub(crate) fn parse_rfc3339(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

/// Effective event time `COALESCE(valid_from, created_at)` parsed to UTC instants, keyed by memory
/// id (Phase 3.5 temporal leg). Unparseable timestamps are skipped. Empty in ⇒ empty out.
pub async fn effective_time_by_ids(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<std::collections::HashMap<String, OffsetDateTime>, DenError> {
    if memory_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = vec!["?"; memory_ids.len()].join(",");
    let sql = format!(
        "SELECT memory_id, COALESCE(valid_from, created_at) FROM memory_records \
         WHERE bear_id = ? AND memory_id IN ({placeholders})"
    );
    let mut query = sqlx::query_as::<_, (String, String)>(&sql).bind(store.bear_id().to_string());
    for id in memory_ids {
        query = query.bind(id);
    }
    let rows = query
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("effective time lookup failed: {e}")))?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, ts)| parse_rfc3339(&ts).map(|t| (id, t)))
        .collect())
}

/// For each of `memory_ids`, the effective times of records that supersede it (records whose
/// `supersedes_memory_id` points at it). Used to exclude records already superseded as of a
/// point-in-time query (Phase 3.5).
pub async fn superseder_times_by_superseded(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<OffsetDateTime>>, DenError> {
    if memory_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = vec!["?"; memory_ids.len()].join(",");
    let sql = format!(
        "SELECT supersedes_memory_id, COALESCE(valid_from, created_at) FROM memory_records \
         WHERE bear_id = ? AND supersedes_memory_id IN ({placeholders})"
    );
    let mut query =
        sqlx::query_as::<_, (Option<String>, String)>(&sql).bind(store.bear_id().to_string());
    for id in memory_ids {
        query = query.bind(id);
    }
    let rows = query
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("superseder time lookup failed: {e}")))?;
    let mut map: std::collections::HashMap<String, Vec<OffsetDateTime>> =
        std::collections::HashMap::new();
    for (superseded, ts) in rows {
        if let (Some(id), Some(t)) = (superseded, parse_rfc3339(&ts)) {
            map.entry(id).or_default().push(t);
        }
    }
    Ok(map)
}

/// Point-in-time recall (Phase 3.5): the record that was head at `logical_path` as of instant
/// `at`, walking the supersession chain. A record is the as-of head when its effective time is
/// `<= at`, no record whose effective time is `<= at` supersedes it, and (among those) it has the
/// greatest effective time. Returns `None` if nothing existed at the path by then.
pub async fn head_record_as_of(
    store: &BearMemoryStore,
    logical_path: &str,
    at: OffsetDateTime,
) -> Result<Option<MemoryRecordRow>, DenError> {
    let rows = sqlx::query_as::<_, AsOfSqlRow>(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at, COALESCE(valid_from, created_at) AS effective_at
        FROM memory_records
        WHERE bear_id = ? AND logical_path = ? AND visibility = 'normal'
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(logical_path)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("as-of head lookup failed: {e}")))?;

    let parsed: Vec<(AsOfSqlRow, OffsetDateTime)> = rows
        .into_iter()
        .filter_map(|r| parse_rfc3339(&r.effective_at).map(|t| (r, t)))
        .collect();

    // Records superseded by something already effective by `at` are not the as-of head.
    let mut superseded_as_of: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (row, effective) in &parsed {
        if *effective <= at {
            if let Some(prior) = &row.supersedes_memory_id {
                superseded_as_of.insert(prior.clone());
            }
        }
    }

    let best = parsed
        .into_iter()
        .filter(|(row, effective)| *effective <= at && !superseded_as_of.contains(&row.memory_id))
        .max_by(|(a, ta), (b, tb)| ta.cmp(tb).then(a.sequence_no.cmp(&b.sequence_no)));
    Ok(best.map(|(row, _)| row.into_row()))
}

#[derive(sqlx::FromRow)]
struct AsOfSqlRow {
    memory_id: String,
    sequence_no: i64,
    scope_type: String,
    scope_profile: Option<String>,
    kind: String,
    content_text: String,
    logical_path: Option<String>,
    work_surface_ref: Option<String>,
    metadata_json: String,
    created_at: String,
    supersedes_memory_id: Option<String>,
    invalid_at: Option<String>,
    effective_at: String,
    salience: String,
}

impl AsOfSqlRow {
    fn into_row(self) -> MemoryRecordRow {
        let metadata_json: Value = serde_json::from_str(&self.metadata_json)
            .unwrap_or_else(|_| Value::Object(Default::default()));
        let lifecycle_status = lifecycle_status(
            &metadata_json,
            self.supersedes_memory_id.as_deref(),
            self.invalid_at.as_deref(),
        );
        let freshness_trend = freshness_trend(&lifecycle_status, self.invalid_at.as_deref());
        MemoryRecordRow {
            memory_id: self.memory_id,
            sequence_no: self.sequence_no,
            scope_type: MemoryScopeType::parse(&self.scope_type)
                .unwrap_or(MemoryScopeType::ProfileLocal),
            scope_profile: self.scope_profile,
            kind: self.kind,
            content_text: self.content_text,
            logical_path: self.logical_path,
            work_surface_ref: self.work_surface_ref,
            metadata_json,
            created_at: self.created_at,
            salience: self.salience,
            supersedes_memory_id: self.supersedes_memory_id,
            invalid_at: self.invalid_at,
            lifecycle_status,
            freshness_trend,
        }
    }
}

#[cfg(test)]
mod as_of_tests {
    use super::*;
    use crate::relations::append_relation;
    use crate::resolver::{resolve, Assertion, Resolution, Signal};
    use crate::test_support::new_test_store;
    use serde_json::json;
    use time::format_description::well_known::Rfc3339;

    fn at(rfc: &str) -> OffsetDateTime {
        OffsetDateTime::parse(rfc, &Rfc3339).unwrap()
    }

    async fn insert_version(
        store: &BearMemoryStore,
        memory_id: &str,
        seq: i64,
        valid_from: &str,
        supersedes: Option<&str>,
    ) {
        sqlx::query(
            r"
            INSERT INTO memory_records (
                memory_id, bear_id, sequence_no, scope_type, scope_profile, kind,
                author_profile, author_agent_id, created_at, content_text, metadata_json,
                visibility, logical_path, work_surface_ref, valid_from, supersedes_memory_id, salience
            ) VALUES (?, ?, ?, 'shared', NULL, 'note', 'curate', NULL, ?, 'policy text', '{}',
                      'normal', 'core/policy.md', NULL, ?, ?, 'normal')
            ",
        )
        .bind(memory_id)
        .bind(store.bear_id().to_string())
        .bind(seq)
        .bind(valid_from)
        .bind(valid_from)
        .bind(supersedes)
        .execute(store.pool())
        .await
        .expect("insert version");
    }

    #[tokio::test]
    async fn head_record_as_of_walks_supersession_chain() {
        let store = new_test_store().await;
        insert_version(&store, "v1", 1, "2026-01-01T00:00:00Z", None).await;
        insert_version(&store, "v2", 2, "2026-03-01T00:00:00Z", Some("v1")).await;

        // Before anything existed.
        assert!(
            head_record_as_of(&store, "core/policy.md", at("2025-12-01T00:00:00Z"))
                .await
                .unwrap()
                .is_none()
        );

        // v1 is current until v2 becomes effective.
        let feb = head_record_as_of(&store, "core/policy.md", at("2026-02-01T00:00:00Z"))
            .await
            .unwrap()
            .expect("v1 head in feb");
        assert_eq!(feb.memory_id, "v1");

        // Once v2 is effective it supersedes v1.
        let apr = head_record_as_of(&store, "core/policy.md", at("2026-04-01T00:00:00Z"))
            .await
            .unwrap()
            .expect("v2 head in apr");
        assert_eq!(apr.memory_id, "v2");
    }

    #[tokio::test]
    async fn append_record_options_store_salience_and_supersession() {
        let store = new_test_store().await;
        let path = LogicalMemoryPath::shared_core("policy");
        let first = store
            .append_record_with_options(
                &path,
                "note",
                "curate",
                None,
                "first",
                &serde_json::json!({}),
                "normal",
                "high",
                None,
            )
            .await
            .unwrap();
        let second = store
            .append_record_with_options(
                &path,
                "note",
                "curate",
                None,
                "second",
                &serde_json::json!({}),
                "normal",
                "critical",
                Some(&first.memory_id),
            )
            .await
            .unwrap();

        assert_eq!(first.salience, "high");
        assert_eq!(second.salience, "critical");
        let head = head_record_for_logical_path(&store, "core/policy.md")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.memory_id, second.memory_id);
        let invalidated: Option<String> = sqlx::query_scalar(
            "SELECT invalid_at FROM memory_records WHERE bear_id = ? AND memory_id = ?",
        )
        .bind(store.bear_id().to_string())
        .bind(&first.memory_id)
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert!(invalidated.is_some());
    }

    #[tokio::test]
    async fn lifecycle_markers_track_stale_archive_candidate_and_superseded() {
        let store = new_test_store().await;
        let path = LogicalMemoryPath::profile_local("pair", "note");
        let record = append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "Durable role-local note.",
            &json!({}),
        )
        .await
        .unwrap();

        let stale =
            mark_memory_record_lifecycle(&store, &record.memory_id, "stale", Some("old fact"))
                .await
                .unwrap();
        assert_eq!(stale.lifecycle_status, "stale");
        assert_eq!(stale.freshness_trend, "stale");
        assert!(stale.invalid_at.is_none());

        let archive_candidate = mark_memory_record_lifecycle(
            &store,
            &record.memory_id,
            "archive-candidate",
            Some("low value outside hot recall"),
        )
        .await
        .unwrap();
        assert_eq!(archive_candidate.lifecycle_status, "archive-candidate");
        assert_eq!(archive_candidate.freshness_trend, "weakening");

        let superseded = mark_memory_record_lifecycle(
            &store,
            &record.memory_id,
            "superseded",
            Some("replaced by curated core entry"),
        )
        .await
        .unwrap();
        assert_eq!(superseded.lifecycle_status, "superseded");
        assert_eq!(superseded.freshness_trend, "stale");
        assert!(superseded.invalid_at.is_some());
    }

    #[tokio::test]
    async fn active_path_reads_hide_superseded_archived_and_archive_candidates() {
        let store = new_test_store().await;
        let path = LogicalMemoryPath::profile_local("pair", "note");
        let superseded = append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "superseded note",
            &json!({}),
        )
        .await
        .unwrap();
        mark_memory_record_lifecycle(&store, &superseded.memory_id, "superseded", None)
            .await
            .unwrap();
        append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "archived note",
            &json!({ "lifecycle": { "status": "archived" } }),
        )
        .await
        .unwrap();
        append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "archive candidate note",
            &json!({ "lifecycle": { "status": "archive-candidate" } }),
        )
        .await
        .unwrap();
        let active = append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "active note",
            &json!({}),
        )
        .await
        .unwrap();

        let rows = list_records_for_logical_path(&store, "pair/note.md", 10)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].memory_id, active.memory_id);
        assert_eq!(rows[0].lifecycle_status, "active");
        assert_eq!(rows[0].freshness_trend, "stable");

        let history = list_record_history_for_logical_path(&store, "pair/note.md", 10)
            .await
            .unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].memory_id, active.memory_id);
        assert!(history
            .iter()
            .any(|row| row.lifecycle_status == "superseded"));
        assert!(history.iter().any(|row| row.lifecycle_status == "archived"));
        assert!(history
            .iter()
            .any(|row| row.lifecycle_status == "archive-candidate"));
    }

    #[tokio::test]
    async fn entity_anchor_heads_require_resolved_salient_subject_and_explicit_anchor() {
        let store = new_test_store().await;
        let entity = match resolve(
            &store,
            "person",
            Some("Ryan Dahl"),
            &[Signal::new("email", "ryan@example.com")],
            Assertion::Inferred,
        )
        .await
        .unwrap()
        {
            Resolution::Resolved(entity) => entity,
            other => panic!("expected resolved entity, got {other:?}"),
        };

        let source_path = LogicalMemoryPath::shared_core("notes/ryan-source");
        let source = store
            .append_record_with_options(
                &source_path,
                "note",
                "pair",
                None,
                "Ryan prefers concise architecture notes.",
                &json!({}),
                "normal",
                "high",
                None,
            )
            .await
            .unwrap();
        append_relation(
            &store,
            &source.memory_id,
            &entity.entity_id,
            "subject",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();

        assert!(list_entity_anchor_head_records(&store, 10)
            .await
            .unwrap()
            .is_empty());

        let anchor_path = LogicalMemoryPath::from_logical_path("core/people/ryan-dahl/profile.md");
        let anchor = append_memory_record(
            &store,
            &anchor_path,
            "profile",
            "curate",
            None,
            "Ryan Dahl: concise architecture notes.",
            &json!({}),
        )
        .await
        .unwrap();

        let anchors = list_entity_anchor_head_records(&store, 10).await.unwrap();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].memory_id, anchor.memory_id);
    }
}

#[derive(sqlx::FromRow)]
struct MemoryRecordSqlRow {
    memory_id: String,
    sequence_no: i64,
    scope_type: String,
    scope_profile: Option<String>,
    kind: String,
    content_text: String,
    logical_path: Option<String>,
    work_surface_ref: Option<String>,
    metadata_json: String,
    created_at: String,
    salience: String,
    supersedes_memory_id: Option<String>,
    invalid_at: Option<String>,
}

impl MemoryRecordSqlRow {
    fn into_row(self) -> MemoryRecordRow {
        let metadata_json: Value = serde_json::from_str(&self.metadata_json)
            .unwrap_or_else(|_| Value::Object(Default::default()));
        let lifecycle_status = lifecycle_status(
            &metadata_json,
            self.supersedes_memory_id.as_deref(),
            self.invalid_at.as_deref(),
        );
        let freshness_trend = freshness_trend(&lifecycle_status, self.invalid_at.as_deref());
        MemoryRecordRow {
            memory_id: self.memory_id,
            sequence_no: self.sequence_no,
            scope_type: MemoryScopeType::parse(&self.scope_type)
                .unwrap_or(MemoryScopeType::ProfileLocal),
            scope_profile: self.scope_profile,
            kind: self.kind,
            content_text: self.content_text,
            logical_path: self.logical_path,
            work_surface_ref: self.work_surface_ref,
            metadata_json,
            created_at: self.created_at,
            salience: self.salience,
            supersedes_memory_id: self.supersedes_memory_id,
            invalid_at: self.invalid_at,
            lifecycle_status,
            freshness_trend,
        }
    }
}
