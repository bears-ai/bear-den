//! Explicit human-reviewed promotion from one canonical source-local record to
//! a Bear hat. The source remains private; only reviewed, newly authored content
//! crosses into the shared hat scope. No logical path or model text grants access.

use den_core::{
    ids::{HatId, UserId},
    DenError,
};

use uuid::Uuid;

use crate::{
    clock::now_rfc3339,
    logical_path::{LogicalMemoryPath, MemorySource},
    BearMemoryStore,
};

mod curate;
mod read;
use read::{load_candidate, load_hat_head};
#[cfg(test)]
mod tests;
pub use curate::promote_curated_proposal_to_hat;

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

struct ReviewCandidateSqlRow {
    memory_id: String,
    scope_source_id: String,
    scope_source_kind: String,
    kind: String,
    content_text: String,
    sequence_no: i64,
}

fn decode_candidate(row: ReviewCandidateSqlRow) -> Result<ReviewCandidate, DenError> {
    let source_id = Uuid::parse_str(&row.scope_source_id)
        .map_err(|err| DenError::System(format!("invalid canonical source ID: {err}")))?;
    Ok(ReviewCandidate {
        memory_id: Uuid::parse_str(&row.memory_id)
            .map_err(|err| DenError::System(format!("invalid canonical memory ID: {err}")))?,
        source: source_kind(&row.scope_source_kind, source_id)?,
        kind: row.kind,
        content_text: row.content_text,
        sequence_no: row.sequence_no,
    })
}

pub async fn review_candidates(
    store: &BearMemoryStore,
    limit: i64,
) -> Result<Vec<ReviewCandidate>, DenError> {
    let bear_id = store.bear_id().to_string();
    let limit = limit.clamp(1, 100);
    let rows = sqlx::query_as!(
        ReviewCandidateSqlRow,
        r#"
        SELECT m.memory_id AS "memory_id!", m.scope_source_kind AS "scope_source_kind!",
               m.scope_source_id AS "scope_source_id!", m.kind AS "kind!",
               m.content_text AS "content_text!", m.sequence_no AS "sequence_no!"
        FROM generic_memory_records m
        WHERE m.bear_id = ? AND m.scope_type = 'source_local'
          AND m.visibility = 'normal' AND m.invalid_at IS NULL
          AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                AND newer.supersedes_memory_id = m.memory_id
          )
        ORDER BY m.sequence_no DESC LIMIT ?
        "#,
        bear_id,
        limit,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|err| DenError::System(format!("list source-local review candidates: {err}")))?;
    rows.into_iter().map(decode_candidate).collect()
}

pub async fn review_candidate(
    store: &BearMemoryStore,
    memory_id: Uuid,
) -> Result<ReviewCandidate, DenError> {
    load_candidate(store.pool(), store.bear_id(), memory_id).await
}

pub async fn current_hat_head(
    store: &BearMemoryStore,
    hat_id: HatId,
    kind: &str,
) -> Result<Option<HatHead>, DenError> {
    let path = LogicalMemoryPath::hat(hat_id, kind).to_logical_path();
    let mut connection = store.pool().acquire().await?;
    load_hat_head(&mut connection, store.bear_id(), hat_id, &path).await
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
    let source = load_candidate(&mut *tx, store.bear_id(), source_memory_id).await?;
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
    let current_head = load_hat_head(&mut tx, store.bear_id(), hat_id, &logical_path)
        .await?
        .map(|head| head.memory_id.to_string());
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
