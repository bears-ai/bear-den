//! Canonical hat → Bear-core review. Both the newly authored core record and
//! its provenance are committed together; a hat path or supplied ID is never
//! authorization to read or publish another source.

use den_core::{
    ids::{HatId, UserId},
    DenError,
};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{clock::now_rfc3339, logical_path::LogicalMemoryPath, BearMemoryStore};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, FromRow)]
pub struct ReviewCandidate {
    pub memory_id: String,
    pub kind: String,
    pub content_text: String,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct CoreHead {
    pub memory_id: String,
    pub content_text: String,
    pub visibility: String,
    pub invalid_at: Option<String>,
    pub lifecycle_status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreReviewOutcome {
    pub memory_id: Uuid,
    pub promotion_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ReviewedCoreEntry {
    pub hat_id: HatId,
    pub source_memory_id: Uuid,
    pub kind: String,
    pub reviewed_content: String,
    pub expected_head: Option<Uuid>,
    pub review_notes: String,
    pub reviewer: UserId,
}

pub async fn candidates(
    store: &BearMemoryStore,
    hat_id: HatId,
    limit: i64,
) -> Result<Vec<ReviewCandidate>, DenError> {
    let bear_id = store.bear_id().to_string();
    let hat_id = hat_id.to_string();
    let limit = limit.clamp(1, 100);
    sqlx::query_as!(
        ReviewCandidate,
        r#"
        SELECT m.memory_id AS "memory_id!", m.kind AS "kind!", m.content_text AS "content_text!"
        FROM generic_memory_records m
        WHERE m.bear_id = ? AND m.scope_type = 'hat' AND m.scope_hat_id = ?
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
        hat_id,
        limit,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|error| DenError::System(format!("list reviewable hat entries: {error}")))
}

pub async fn candidate(
    store: &BearMemoryStore,
    hat_id: HatId,
    memory_id: Uuid,
) -> Result<ReviewCandidate, DenError> {
    load_candidate(store.pool(), store.bear_id(), hat_id, memory_id).await
}

async fn load_candidate<'e, E>(
    executor: E,
    bear_id: Uuid,
    hat_id: HatId,
    memory_id: Uuid,
) -> Result<ReviewCandidate, DenError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let bear_id = bear_id.to_string();
    let hat_id = hat_id.to_string();
    let memory_id = memory_id.to_string();
    sqlx::query_as!(
        ReviewCandidate,
        r#"
        SELECT m.memory_id AS "memory_id!", m.kind AS "kind!", m.content_text AS "content_text!"
        FROM generic_memory_records m
        WHERE m.bear_id = ? AND m.scope_type = 'hat' AND m.scope_hat_id = ?
          AND m.memory_id = ? AND m.visibility = 'normal' AND m.invalid_at IS NULL
          AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                AND newer.supersedes_memory_id = m.memory_id
          )
        "#,
        bear_id,
        hat_id,
        memory_id,
    )
    .fetch_optional(executor)
    .await
    .map_err(|error| DenError::System(format!("read reviewable hat entry: {error}")))?
    .ok_or_else(|| DenError::NotFound("reviewable hat entry not found".into()))
}

// Select the physical head before gating its body: a denied head must not make
// an older generic version look current, or make the path appear writable.
async fn load_core_head(
    connection: &mut sqlx::SqliteConnection,
    bear_id: Uuid,
    path: &str,
) -> Result<Option<CoreHead>, DenError> {
    let bear_id = bear_id.to_string();
    let head_id = sqlx::query_scalar!(
        r#"SELECT memory_id AS "memory_id!" FROM memory_records
           WHERE bear_id = ? AND scope_type = 'shared' AND logical_path = ?
           ORDER BY sequence_no DESC LIMIT 1"#,
        bear_id,
        path,
    )
    .fetch_optional(&mut *connection)
    .await?;
    let Some(head_id) = head_id else {
        return Ok(None);
    };
    let head = sqlx::query_as!(
        CoreHead,
        r#"
        SELECT memory_id AS "memory_id!", content_text AS "content_text!",
               visibility AS "visibility!", invalid_at,
               COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active') AS "lifecycle_status!: String"
        FROM generic_memory_records WHERE bear_id = ? AND memory_id = ?
        "#,
        bear_id,
        head_id,
    )
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| DenError::Authorization("core target is not generic-readable".into()))?;
    Ok(Some(head))
}

pub async fn current_core_head(
    store: &BearMemoryStore,
    kind: &str,
) -> Result<Option<CoreHead>, DenError> {
    let path = LogicalMemoryPath::shared_core(kind).to_logical_path();
    let mut connection = store.pool().acquire().await?;
    load_core_head(&mut connection, store.bear_id(), &path).await
}

pub async fn promote(
    store: &BearMemoryStore,
    review: ReviewedCoreEntry,
) -> Result<CoreReviewOutcome, DenError> {
    let kind = review.kind.trim();
    if kind.is_empty()
        || kind.len() > 64
        || !kind
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-'))
    {
        return Err(DenError::ValidationError(
            "core entry kind must be a lowercase identifier (up to 64 characters)".into(),
        ));
    }
    let text = review.reviewed_content.trim();
    if text.is_empty() || text.len() > 16_000 {
        return Err(DenError::ValidationError(
            "reviewed core content must be 1–16000 bytes".into(),
        ));
    }
    let notes = review.review_notes.trim();
    if notes.len() < 12 || notes.len() > 4_000 {
        return Err(DenError::ValidationError(
            "core review rationale must be 12–4000 bytes".into(),
        ));
    }
    let path = LogicalMemoryPath::shared_core(kind).to_logical_path();
    let mut tx = store
        .pool()
        .begin()
        .await
        .map_err(|error| DenError::System(format!("begin reviewed core promotion: {error}")))?;
    let source = load_candidate(
        &mut *tx,
        store.bear_id(),
        review.hat_id,
        review.source_memory_id,
    )
    .await?;
    let promoted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_promotions p JOIN memory_records target
           ON target.bear_id = p.bear_id AND target.memory_id = p.target_memory_id
         WHERE p.bear_id = ? AND p.source_memory_id = ? AND target.scope_type = 'shared'",
    )
    .bind(store.bear_id().to_string())
    .bind(review.source_memory_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| DenError::System(format!("check prior core promotion: {error}")))?;
    if promoted > 0 {
        return Err(DenError::ValidationError(
            "this hat entry was already promoted to Bear core".into(),
        ));
    }
    let head = load_core_head(&mut tx, store.bear_id(), &path).await?;
    if head.as_ref().is_some_and(|head| {
        head.visibility != "normal"
            || head.invalid_at.is_some()
            || matches!(
                head.lifecycle_status.as_str(),
                "archived" | "archive-candidate"
            )
    }) {
        return Err(DenError::Authorization(
            "core target is not an active, visible head".into(),
        ));
    }
    if head.as_ref().map(|head| head.memory_id.as_str())
        != review.expected_head.map(|id| id.to_string()).as_deref()
    {
        return Err(DenError::ValidationError(
            "core entry changed since review; refresh before replacing it".into(),
        ));
    }
    let sequence: i64 = sqlx::query_scalar(
        "UPDATE bear_sequence SET next_sequence = next_sequence + 2 WHERE id = 1 RETURNING next_sequence - 2",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| DenError::System(format!("allocate reviewed core sequence: {error}")))?;
    let created_at = now_rfc3339()?;
    let target_id = Uuid::new_v4();
    let promotion_id = Uuid::new_v4();
    let metadata = serde_json::json!({
        "promoted_from": review.source_memory_id,
        "source_hat_id": review.hat_id,
        "source_kind": source.kind,
        "reviewed_by_user_id": review.reviewer.get(),
        "reviewed_for_bear_and_work": true,
        "review_notes": notes,
        "promotion_policy": "human_reviewed_hat_to_core",
    });
    sqlx::query(
        "INSERT INTO memory_records (memory_id, bear_id, sequence_no, scope_type, kind,
            author_profile, created_at, content_text, metadata_json, visibility,
            logical_path, valid_from, salience, supersedes_memory_id)
         VALUES (?, ?, ?, 'shared', ?, 'curate', ?, ?, ?, 'normal', ?, ?, 'normal', ?)",
    )
    .bind(target_id.to_string())
    .bind(store.bear_id().to_string())
    .bind(sequence)
    .bind(kind)
    .bind(&created_at)
    .bind(text)
    .bind(metadata.to_string())
    .bind(&path)
    .bind(&created_at)
    .bind(head.as_ref().map(|head| head.memory_id.as_str()))
    .execute(&mut *tx)
    .await
    .map_err(|error| DenError::System(format!("write reviewed core record: {error}")))?;
    if let Some(head) = &head {
        sqlx::query("UPDATE memory_records SET invalid_at = ? WHERE bear_id = ? AND memory_id = ?")
            .bind(&created_at)
            .bind(store.bear_id().to_string())
            .bind(&head.memory_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| DenError::System(format!("supersede reviewed core head: {error}")))?;
    }
    sqlx::query(
        "INSERT INTO memory_promotions (promotion_id, bear_id, sequence_no, source_memory_id,
            target_memory_id, review_agent_id, action, created_at, notes)
         VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?)",
    )
    .bind(promotion_id.to_string())
    .bind(store.bear_id().to_string())
    .bind(sequence + 1)
    .bind(review.source_memory_id.to_string())
    .bind(target_id.to_string())
    .bind(if head.is_some() {
        "supersede_core"
    } else {
        "promote_to_core"
    })
    .bind(&created_at)
    .bind(notes)
    .execute(&mut *tx)
    .await
    .map_err(|error| DenError::System(format!("write core promotion provenance: {error}")))?;
    tx.commit()
        .await
        .map_err(|error| DenError::System(format!("commit reviewed core promotion: {error}")))?;
    Ok(CoreReviewOutcome {
        memory_id: target_id,
        promotion_id,
    })
}
