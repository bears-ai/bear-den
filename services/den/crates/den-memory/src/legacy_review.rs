//! Admin-only inventory and deliberate reauthoring of unattributed legacy
//! profile-local memory. No path, imported git metadata, or client-session ID
//! is treated as evidence of conversation ownership.

use den_core::{
    ids::{HatId, UserId},
    DenError,
};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{clock::now_rfc3339, BearMemoryStore, LogicalMemoryPath};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct LegacyCandidate {
    pub memory_id: String,
    pub scope_profile: Option<String>,
    pub logical_path: Option<String>,
    pub kind: String,
    pub content_text: String,
    pub sequence_no: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct LegacyInventoryBucket {
    pub scope_profile: Option<String>,
    pub total: i64,
    pub reviewable: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LegacyReviewPage {
    pub inventory: Vec<LegacyInventoryBucket>,
    pub candidates: Vec<LegacyCandidate>,
    pub next_before: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct LegacyReauthoring {
    pub source_memory_id: String,
    pub target_hat: HatId,
    pub kind: String,
    pub reviewed_content: String,
    pub expected_head: Option<Uuid>,
    pub review_notes: String,
    pub reviewer: UserId,
    pub work_audience_reviewed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReauthoredHatEntry {
    pub memory_id: Uuid,
    pub promotion_id: Uuid,
}

pub async fn inventory_page(
    store: &BearMemoryStore,
    before: Option<i64>,
    limit: i64,
) -> Result<LegacyReviewPage, DenError> {
    let bear_id = store.bear_id().to_string();
    let buckets = sqlx::query_as!(
        LegacyInventoryBucket,
        r#"
        SELECT scope_profile, count(*) AS "total!: i64",
               sum(CASE WHEN visibility = 'normal' AND invalid_at IS NULL
                 AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active')
                     NOT IN ('archived', 'archive-candidate')
                 AND NOT EXISTS (
                   SELECT 1 FROM memory_records newer
                   WHERE newer.bear_id = memory_records.bear_id
                     AND newer.supersedes_memory_id = memory_records.memory_id
                 ) THEN 1 ELSE 0 END) AS "reviewable!: i64"
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND scope_type = 'profile_local'
        GROUP BY scope_profile ORDER BY scope_profile
        "#,
        bear_id,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|error| DenError::System(format!("inventory legacy memory: {error}")))?;
    let limit = limit.clamp(1, 100);
    let candidates = sqlx::query_as!(
        LegacyCandidate,
        r#"
        SELECT m.memory_id AS "memory_id!", m.scope_profile, m.logical_path,
               m.kind AS "kind!", m.content_text AS "content_text!", m.sequence_no AS "sequence_no!"
        FROM generic_memory_records m
        WHERE m.bear_id = ? AND m.scope_type = 'profile_local'
          AND m.visibility = 'normal' AND m.invalid_at IS NULL
          AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                AND newer.supersedes_memory_id = m.memory_id
          )
          AND (? IS NULL OR m.sequence_no < ?)
        ORDER BY m.sequence_no DESC LIMIT ?
        "#,
        bear_id,
        before,
        before,
        limit,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|error| DenError::System(format!("page legacy review candidates: {error}")))?;
    let next_before = (candidates.len() as i64 == limit)
        .then(|| candidates.last().map(|candidate| candidate.sequence_no))
        .flatten();
    Ok(LegacyReviewPage {
        inventory: buckets,
        candidates,
        next_before,
    })
}

pub async fn candidate(
    store: &BearMemoryStore,
    memory_id: &str,
) -> Result<LegacyCandidate, DenError> {
    load_candidate(store.pool(), store.bear_id(), memory_id).await
}

async fn load_candidate<'e, E>(
    executor: E,
    bear_id: Uuid,
    memory_id: &str,
) -> Result<LegacyCandidate, DenError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let bear_id = bear_id.to_string();
    sqlx::query_as!(
        LegacyCandidate,
        r#"
        SELECT m.memory_id AS "memory_id!", m.scope_profile, m.logical_path,
               m.kind AS "kind!", m.content_text AS "content_text!", m.sequence_no AS "sequence_no!"
        FROM generic_memory_records m
        WHERE m.bear_id = ? AND m.scope_type = 'profile_local' AND m.memory_id = ?
          AND m.visibility = 'normal' AND m.invalid_at IS NULL
          AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                AND newer.supersedes_memory_id = m.memory_id
          )
        "#,
        bear_id,
        memory_id,
    )
    .fetch_optional(executor)
    .await
    .map_err(|error| DenError::System(format!("inspect legacy review candidate: {error}")))?
    .ok_or_else(|| DenError::NotFound("reviewable legacy record not found".into()))
}

pub async fn reauthor_into_hat(
    store: &BearMemoryStore,
    review: LegacyReauthoring,
) -> Result<ReauthoredHatEntry, DenError> {
    let kind = review.kind.trim();
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
    let text = review.reviewed_content.trim();
    if text.is_empty() || text.len() > 16_000 {
        return Err(DenError::ValidationError(
            "reviewed hat content must be 1–16000 bytes".into(),
        ));
    }
    let notes = review.review_notes.trim();
    if !(12..=4_000).contains(&notes.len()) {
        return Err(DenError::ValidationError(
            "legacy review rationale must be 12–4000 bytes".into(),
        ));
    }
    if review.source_memory_id.len() > 1_024 || review.source_memory_id.is_empty() {
        return Err(DenError::ValidationError("invalid legacy record ID".into()));
    }
    let path = LogicalMemoryPath::hat(review.target_hat, kind).to_logical_path();
    let mut tx = store
        .pool()
        .begin()
        .await
        .map_err(|error| DenError::System(format!("begin legacy reauthoring: {error}")))?;
    let source = load_candidate(&mut *tx, store.bear_id(), &review.source_memory_id).await?;
    let already_promoted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_promotions p JOIN memory_records target
          ON target.bear_id = p.bear_id AND target.memory_id = p.target_memory_id
         WHERE p.bear_id = ? AND p.source_memory_id = ?
           AND target.scope_type = 'hat' AND target.scope_hat_id = ?",
    )
    .bind(store.bear_id().to_string())
    .bind(&review.source_memory_id)
    .bind(review.target_hat.to_string())
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| DenError::System(format!("check legacy reauthoring provenance: {error}")))?;
    if already_promoted != 0 {
        return Err(DenError::ValidationError(
            "this legacy record was already reviewed into this hat".into(),
        ));
    }
    let bear_id = store.bear_id().to_string();
    let target_hat = review.target_hat.to_string();
    // Select the physical head first, then gate it inside this transaction. A
    // classified head must neither resurrect an old version nor be invalidated.
    let head = sqlx::query!(
        r#"
        SELECT m.memory_id AS "memory_id!", EXISTS (
            SELECT 1 FROM generic_memory_records generic
            WHERE generic.bear_id = m.bear_id AND generic.memory_id = m.memory_id
              AND generic.visibility = 'normal' AND generic.invalid_at IS NULL
              AND COALESCE(json_extract(generic.metadata_json, '$.lifecycle.status'), 'active')
                  NOT IN ('archived', 'archive-candidate')
              AND NOT EXISTS (
                  SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                    AND newer.supersedes_memory_id = m.memory_id
              )
        ) AS "eligible!: bool"
        FROM memory_records m
        WHERE m.bear_id = ? AND m.scope_type = 'hat' AND m.scope_hat_id = ? AND m.logical_path = ?
        ORDER BY m.sequence_no DESC LIMIT 1
        "#,
        bear_id,
        target_hat,
        path,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| DenError::System(format!("read hat head for legacy review: {error}")))?;
    if head.as_ref().is_some_and(|head| !head.eligible) {
        return Err(DenError::Authorization(
            "hat target is not an active, generic-readable head".into(),
        ));
    }
    let head = head.map(|head| head.memory_id);
    if head.as_deref() != review.expected_head.map(|id| id.to_string()).as_deref() {
        return Err(DenError::ValidationError(
            "hat entry changed since review; refresh before replacing it".into(),
        ));
    }
    let sequence: i64 = sqlx::query_scalar(
        "UPDATE bear_sequence SET next_sequence = next_sequence + 2 WHERE id = 1 RETURNING next_sequence - 2",
    )
    .fetch_one(&mut *tx).await
    .map_err(|error| DenError::System(format!("allocate legacy review sequence: {error}")))?;
    let created_at = now_rfc3339()?;
    let target_id = Uuid::new_v4();
    let promotion_id = Uuid::new_v4();
    let metadata = serde_json::json!({
        "promoted_from": source.memory_id,
        "legacy_scope_profile": source.scope_profile,
        "legacy_logical_path": source.logical_path,
        "legacy_source_owner": "unverified",
        "reviewed_by_user_id": review.reviewer.get(),
        "work_audience_reviewed": review.work_audience_reviewed,
        "review_notes": notes,
        "promotion_policy": "human_reauthored_legacy_to_hat",
    });
    sqlx::query(
        "INSERT INTO memory_records (memory_id, bear_id, sequence_no, scope_type, scope_hat_id,
            kind, author_profile, created_at, content_text, metadata_json, visibility,
            logical_path, valid_from, salience, supersedes_memory_id)
         VALUES (?, ?, ?, 'hat', ?, ?, 'curate', ?, ?, ?, 'normal', ?, ?, 'normal', ?)",
    )
    .bind(target_id.to_string())
    .bind(store.bear_id().to_string())
    .bind(sequence)
    .bind(review.target_hat.to_string())
    .bind(kind)
    .bind(&created_at)
    .bind(text)
    .bind(metadata.to_string())
    .bind(&path)
    .bind(&created_at)
    .bind(head.as_deref())
    .execute(&mut *tx)
    .await
    .map_err(|error| DenError::System(format!("write reviewed legacy hat entry: {error}")))?;
    if let Some(old_id) = &head {
        sqlx::query("UPDATE memory_records SET invalid_at = ? WHERE bear_id = ? AND memory_id = ?")
            .bind(&created_at)
            .bind(store.bear_id().to_string())
            .bind(old_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                DenError::System(format!("supersede hat head after legacy review: {error}"))
            })?;
    }
    sqlx::query(
        "INSERT INTO memory_promotions (promotion_id, bear_id, sequence_no, source_memory_id,
            target_memory_id, review_agent_id, action, created_at, notes)
         VALUES (?, ?, ?, ?, ?, NULL, 'human_reauthor_to_hat', ?, ?)",
    )
    .bind(promotion_id.to_string())
    .bind(store.bear_id().to_string())
    .bind(sequence + 1)
    .bind(&review.source_memory_id)
    .bind(target_id.to_string())
    .bind(&created_at)
    .bind(notes)
    .execute(&mut *tx)
    .await
    .map_err(|error| DenError::System(format!("write legacy review provenance: {error}")))?;
    tx.commit()
        .await
        .map_err(|error| DenError::System(format!("commit legacy reauthoring: {error}")))?;
    Ok(ReauthoredHatEntry {
        memory_id: target_id,
        promotion_id,
    })
}
