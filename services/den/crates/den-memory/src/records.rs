use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use den_core::DenError;

use super::{
    clock::now_rfc3339,
    logical_path::{LogicalMemoryPath, MemoryScopeType},
};

mod anchors;
mod read;
mod temporal;
#[cfg(test)]
mod tests;

pub use anchors::{list_entity_anchor_head_records, list_entity_anchor_head_records_with_access};
pub use read::{
    fetch_record_by_id, fetch_records_min, has_work_surface_canonical_anchor,
    has_work_surface_canonical_anchor_with_access, head_record_for_logical_path,
    head_record_for_logical_path_with_access, list_profile_local_head_records,
    list_record_history_for_logical_path, list_records_for_logical_path, RecallRecordMin,
};
pub(crate) use temporal::parse_rfc3339;
pub use temporal::{
    effective_time_by_ids, head_record_as_of, memory_sequence_high_water,
    superseder_times_by_superseded,
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
            crate::personal_facts::ensure_generic_mutation_allowed(self, superseded).await?;
            ensure_record_exists(self, superseded).await?;
        }

        let memory_id = Uuid::new_v4().to_string();
        let sequence_no = self.next_sequence().await?;
        let created_at = now_rfc3339()?;
        let logical_path = logical.to_logical_path();
        let source_kind = logical.source.map(|source| source.kind());
        let source_id = logical.source.map(|source| source.id().to_string());
        let hat_id = logical.hat_id.map(|id| id.to_string());
        let bear_id = self.bear_id.to_string();
        let scope_type = logical.scope_type.as_str();
        let metadata_text = metadata_json.to_string();
        sqlx::query!(
            r"
            INSERT INTO memory_records (
                memory_id, bear_id, sequence_no, scope_type, scope_profile,
                scope_source_kind, scope_source_id, scope_hat_id, kind,
                author_profile, author_agent_id, created_at, content_text, metadata_json,
                visibility, logical_path, work_surface_ref, valid_from, salience, supersedes_memory_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
            memory_id,
            bear_id,
            sequence_no,
            scope_type,
            logical.scope_profile,
            source_kind,
            source_id,
            hat_id,
            kind,
            author_profile,
            author_agent_id,
            created_at,
            content_text,
            metadata_text,
            visibility,
            logical_path,
            logical.work_surface_ref,
            created_at,
            salience,
            supersedes_memory_id,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DenError::System(format!("append memory_record failed: {e}")))?;

        if let Some(superseded) = supersedes_memory_id {
            sqlx::query!(
                "UPDATE memory_records SET invalid_at = COALESCE(invalid_at, ?) WHERE bear_id = ? AND memory_id = ?",
                created_at,
                bear_id,
                superseded,
            )
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
    let bear_id = store.bear_id.to_string();
    let count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM memory_records WHERE bear_id = ? AND memory_id = ?",
        bear_id,
        memory_id,
    )
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
    crate::personal_facts::ensure_generic_mutation_allowed(store, memory_id).await?;
    let bear_id = store.bear_id.to_string();
    let row = sqlx::query!(
        r#"SELECT metadata_json AS "metadata_json!", invalid_at
           FROM generic_memory_records AS memory_records WHERE bear_id = ? AND memory_id = ?"#,
        bear_id,
        memory_id,
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("fetch memory lifecycle metadata failed: {e}")))?
    .ok_or_else(|| DenError::NotFound("memory record not found".to_string()))?;

    let mut metadata: Value = serde_json::from_str(&row.metadata_json)
        .unwrap_or_else(|_| Value::Object(Default::default()));
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

    let invalid_at = if status == "superseded" && row.invalid_at.is_none() {
        Some(now.as_str())
    } else {
        None
    };
    let metadata_text = metadata.to_string();
    if let Some(invalid_at) = invalid_at {
        sqlx::query!(
            "UPDATE memory_records SET metadata_json = ?, invalid_at = ? WHERE bear_id = ? AND memory_id = ?",
            metadata_text,
            invalid_at,
            bear_id,
            memory_id,
        )
        .execute(store.pool())
        .await
        .map_err(|e| DenError::System(format!("mark memory lifecycle failed: {e}")))?;
    } else {
        sqlx::query!(
            "UPDATE memory_records SET metadata_json = ? WHERE bear_id = ? AND memory_id = ?",
            metadata_text,
            bear_id,
            memory_id,
        )
        .execute(store.pool())
        .await
        .map_err(|e| DenError::System(format!("mark memory lifecycle failed: {e}")))?;
    }

    fetch_record_by_id(store, memory_id)
        .await?
        .ok_or_else(|| DenError::NotFound("memory record not found".to_string()))
}

#[derive(sqlx::FromRow)]
pub(crate) struct MemoryRecordSqlRow {
    pub(crate) memory_id: String,
    pub(crate) sequence_no: i64,
    pub(crate) scope_type: String,
    pub(crate) scope_profile: Option<String>,
    pub(crate) kind: String,
    pub(crate) content_text: String,
    pub(crate) logical_path: Option<String>,
    pub(crate) work_surface_ref: Option<String>,
    pub(crate) metadata_json: String,
    pub(crate) created_at: String,
    pub(crate) salience: String,
    pub(crate) supersedes_memory_id: Option<String>,
    pub(crate) invalid_at: Option<String>,
}

impl MemoryRecordSqlRow {
    pub(crate) fn into_row(self) -> MemoryRecordRow {
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
