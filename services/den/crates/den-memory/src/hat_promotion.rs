//! Explicit human-reviewed promotion from one canonical source-local record to
//! a Bear hat. The source remains private; only reviewed, newly authored content
//! crosses into the shared hat scope. No logical path or model text grants access.

use den_core::{
    ids::{HatId, UserId},
    DenError,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    clock::now_rfc3339,
    logical_path::{LogicalMemoryPath, MemorySource},
    BearMemoryStore,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct ReviewCandidate {
    pub memory_id: Uuid,
    pub source: MemorySource,
    pub kind: String,
    pub content_text: String,
    pub sequence_no: i64,
}

#[derive(Debug, Clone)]
pub struct HatHead {
    pub memory_id: Uuid,
    pub content_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewedPromotion {
    pub memory_id: Uuid,
    pub promotion_id: Uuid,
}

fn source_kind(kind: &str, id: Uuid) -> Result<MemorySource, DenError> {
    match kind {
        "conversation" => Ok(MemorySource::Conversation(id)),
        "work_run" => Ok(MemorySource::WorkRun(id)),
        "intake" => Ok(MemorySource::Intake(id)),
        _ => Err(DenError::System(
            "unknown canonical memory source kind".into(),
        )),
    }
}

fn decode_candidate(row: sqlx::sqlite::SqliteRow) -> Result<ReviewCandidate, DenError> {
    let decode =
        |err: sqlx::Error| DenError::System(format!("decode source-local review candidate: {err}"));
    let source_id: String = row.try_get("scope_source_id").map_err(decode)?;
    let raw_source_kind: String = row.try_get("scope_source_kind").map_err(decode)?;
    let source_id = Uuid::parse_str(&source_id)
        .map_err(|err| DenError::System(format!("invalid canonical source ID: {err}")))?;
    let memory_id: String = row.try_get("memory_id").map_err(decode)?;
    Ok(ReviewCandidate {
        memory_id: Uuid::parse_str(&memory_id)
            .map_err(|err| DenError::System(format!("invalid canonical memory ID: {err}")))?,
        source: source_kind(&raw_source_kind, source_id)?,
        kind: row.try_get("kind").map_err(decode)?,
        content_text: row.try_get("content_text").map_err(decode)?,
        sequence_no: row.try_get("sequence_no").map_err(decode)?,
    })
}

// sqlx-dynamic: the canonical Bear store is a per-Bear SQLite database created
// at runtime, not a compile-time SQLx database. All values below are bound.
const REVIEWABLE: &str = "m.bear_id = ? AND m.scope_type = 'source_local'
    AND m.visibility = 'normal' AND m.invalid_at IS NULL
    AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
        NOT IN ('archived', 'archive-candidate')
    AND NOT EXISTS (
        SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
          AND newer.supersedes_memory_id = m.memory_id
    )
    AND NOT EXISTS (
        SELECT 1 FROM memory_access_rules rules WHERE rules.bear_id = m.bear_id
          AND rules.src_memory_id = m.memory_id
    )";

pub async fn review_candidates(
    store: &BearMemoryStore,
    limit: i64,
) -> Result<Vec<ReviewCandidate>, DenError> {
    let sql = format!(
        "SELECT m.memory_id, m.scope_source_kind, m.scope_source_id,
        m.kind, m.content_text, m.sequence_no FROM memory_records m WHERE {REVIEWABLE}
        ORDER BY m.sequence_no DESC LIMIT ?"
    );
    let rows = sqlx::query(&sql)
        .bind(store.bear_id().to_string())
        .bind(limit.clamp(1, 100))
        .fetch_all(store.pool())
        .await
        .map_err(|err| DenError::System(format!("list source-local review candidates: {err}")))?;
    rows.into_iter().map(decode_candidate).collect()
}

pub async fn review_candidate(
    store: &BearMemoryStore,
    memory_id: Uuid,
) -> Result<ReviewCandidate, DenError> {
    let sql = format!(
        "SELECT m.memory_id, m.scope_source_kind, m.scope_source_id,
        m.kind, m.content_text, m.sequence_no FROM memory_records m WHERE {REVIEWABLE}
        AND m.memory_id = ?"
    );
    let row = sqlx::query(&sql)
        .bind(store.bear_id().to_string())
        .bind(memory_id.to_string())
        .fetch_optional(store.pool())
        .await
        .map_err(|err| DenError::System(format!("load source-local review candidate: {err}")))?
        .ok_or_else(|| DenError::NotFound("reviewable source note not found".into()))?;
    decode_candidate(row)
}

pub async fn current_hat_head(
    store: &BearMemoryStore,
    hat_id: HatId,
    kind: &str,
) -> Result<Option<HatHead>, DenError> {
    let path = LogicalMemoryPath::hat(hat_id, kind).to_logical_path();
    let row = sqlx::query(
        "SELECT m.memory_id, m.content_text FROM memory_records m WHERE m.bear_id = ?
           AND m.scope_type = 'hat' AND m.scope_hat_id = ? AND m.logical_path = ?
           AND m.invalid_at IS NULL AND m.visibility = 'normal'
           AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
               NOT IN ('archived', 'archive-candidate')
           AND NOT EXISTS (SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                           AND newer.supersedes_memory_id = m.memory_id)
           ORDER BY m.sequence_no DESC LIMIT 1",
    )
    .bind(store.bear_id().to_string())
    .bind(hat_id.to_string())
    .bind(path)
    .fetch_optional(store.pool())
    .await
    .map_err(|err| DenError::System(format!("read current hat entry for review: {err}")))?;
    row.map(|row| {
        let id: String = row
            .try_get("memory_id")
            .map_err(|err| DenError::System(format!("decode hat head ID: {err}")))?;
        Ok(HatHead {
            memory_id: Uuid::parse_str(&id)
                .map_err(|err| DenError::System(format!("invalid hat head ID: {err}")))?,
            content_text: row
                .try_get("content_text")
                .map_err(|err| DenError::System(format!("decode hat head content: {err}")))?,
        })
    })
    .transpose()
}

/// Promote only the reviewed text, never the raw source record itself. The
/// expected head makes replacement an explicit, optimistic review decision.
#[allow(clippy::too_many_arguments)]
pub async fn promote_reviewed_to_hat(
    store: &BearMemoryStore,
    source_memory_id: Uuid,
    hat_id: HatId,
    kind: &str,
    reviewed_content: &str,
    reviewer: UserId,
    work_audience_reviewed: bool,
    expected_head: Option<Uuid>,
    review_notes: &str,
) -> Result<ReviewedPromotion, DenError> {
    let kind = kind.trim();
    if kind.is_empty()
        || kind.len() > 64
        || !kind
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-'))
    {
        return Err(DenError::ValidationError(
            "hat entry kind must be a lowercase identifier (up to 64 characters)".into(),
        ));
    }
    let reviewed_content = reviewed_content.trim();
    if reviewed_content.is_empty() {
        return Err(DenError::ValidationError(
            "reviewed content must not be empty".into(),
        ));
    }
    let logical_path = LogicalMemoryPath::hat(hat_id, kind).to_logical_path();
    let mut tx = store
        .pool()
        .begin()
        .await
        .map_err(|err| DenError::System(format!("begin reviewed hat promotion: {err}")))?;
    let source_sql = format!(
        "SELECT m.memory_id, m.scope_source_kind, m.scope_source_id,
        m.kind, m.content_text, m.sequence_no FROM memory_records m WHERE {REVIEWABLE}
        AND m.memory_id = ?"
    );
    let source = sqlx::query(&source_sql)
        .bind(store.bear_id().to_string())
        .bind(source_memory_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| DenError::System(format!("check reviewable source record: {err}")))?
        .ok_or_else(|| DenError::NotFound("reviewable source note not found".into()))?;
    let source = decode_candidate(source)?;
    let already_promoted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_promotions p JOIN memory_records target ON target.memory_id = p.target_memory_id
         WHERE p.bear_id = ? AND p.source_memory_id = ? AND target.bear_id = ?
           AND target.scope_type = 'hat' AND target.scope_hat_id = ?"
    ).bind(store.bear_id().to_string()).bind(source_memory_id.to_string())
        .bind(store.bear_id().to_string()).bind(hat_id.to_string())
        .fetch_one(&mut *tx).await
        .map_err(|err| DenError::System(format!("check prior hat promotion: {err}")))?;
    if already_promoted > 0 {
        return Err(DenError::ValidationError(
            "this source note has already been promoted to this hat".into(),
        ));
    }
    let current_head: Option<String> = sqlx::query_scalar(
        "SELECT m.memory_id FROM memory_records m WHERE m.bear_id = ? AND m.scope_type = 'hat'
           AND m.scope_hat_id = ? AND m.logical_path = ? AND m.invalid_at IS NULL
           AND m.visibility = 'normal'
           AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
               NOT IN ('archived', 'archive-candidate')
           AND NOT EXISTS (
               SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                 AND newer.supersedes_memory_id = m.memory_id
           ) ORDER BY m.sequence_no DESC LIMIT 1",
    )
    .bind(store.bear_id().to_string())
    .bind(hat_id.to_string())
    .bind(&logical_path)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| DenError::System(format!("read hat head for review: {err}")))?;
    if current_head.as_deref() != expected_head.map(|id| id.to_string()).as_deref() {
        return Err(DenError::ValidationError(
            "hat entry changed since review; refresh before replacing it".into(),
        ));
    }
    let first_sequence: i64 = sqlx::query_scalar(
        "UPDATE bear_sequence SET next_sequence = next_sequence + 2 WHERE id = 1 RETURNING next_sequence - 2"
    ).fetch_one(&mut *tx).await
        .map_err(|err| DenError::System(format!("allocate promotion sequence: {err}")))?;
    let target_id = Uuid::new_v4();
    let promotion_id = Uuid::new_v4();
    let created_at = now_rfc3339()?;
    let metadata = serde_json::json!({
        "promoted_from": source_memory_id,
        "source_kind": source.source.kind(),
        "source_id": source.source.id(),
        "source_sequence_no": source.sequence_no,
        "reviewed_by_user_id": reviewer.get(),
        "work_audience_reviewed": work_audience_reviewed,
        "review_notes": review_notes.trim(),
        "promotion_policy": "human_reviewed_hat",
    });
    sqlx::query(
        "INSERT INTO memory_records (memory_id, bear_id, sequence_no, scope_type, scope_profile,
            scope_source_kind, scope_source_id, scope_hat_id, kind, author_profile,
            created_at, content_text, metadata_json, visibility, logical_path, valid_from,
            salience, supersedes_memory_id)
         VALUES (?, ?, ?, 'hat', NULL, NULL, NULL, ?, ?, 'curate', ?, ?, ?, 'normal', ?, ?, 'normal', ?)"
    ).bind(target_id.to_string()).bind(store.bear_id().to_string()).bind(first_sequence)
        .bind(hat_id.to_string()).bind(kind).bind(&created_at).bind(reviewed_content)
        .bind(metadata.to_string()).bind(&logical_path).bind(&created_at)
        .bind(current_head.as_deref()).execute(&mut *tx).await
        .map_err(|err| DenError::System(format!("write reviewed hat memory: {err}")))?;
    if let Some(old_id) = current_head.as_deref() {
        sqlx::query("UPDATE memory_records SET invalid_at = ? WHERE bear_id = ? AND memory_id = ?")
            .bind(&created_at)
            .bind(store.bear_id().to_string())
            .bind(old_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| DenError::System(format!("supersede reviewed hat head: {err}")))?;
    }
    sqlx::query(
        "INSERT INTO memory_promotions (promotion_id, bear_id, sequence_no, source_memory_id,
            target_memory_id, review_agent_id, action, created_at, notes)
         VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?)",
    )
    .bind(promotion_id.to_string())
    .bind(store.bear_id().to_string())
    .bind(first_sequence + 1)
    .bind(source_memory_id.to_string())
    .bind(target_id.to_string())
    .bind(if current_head.is_some() {
        "supersede_hat"
    } else {
        "promote_to_hat"
    })
    .bind(&created_at)
    .bind(review_notes.trim())
    .execute(&mut *tx)
    .await
    .map_err(|err| DenError::System(format!("write hat promotion provenance: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| DenError::System(format!("commit reviewed hat promotion: {err}")))?;
    Ok(ReviewedPromotion {
        memory_id: target_id,
        promotion_id,
    })
}
