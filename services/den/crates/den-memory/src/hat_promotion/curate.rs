//! SQLite half of an internal Curate-owned source→hat publication. The service
//! must hold a Postgres lock on the selected hat and source conversation
//! through this transaction; this function alone cannot establish that audience.

use den_core::DenError;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    clock::now_rfc3339,
    logical_path::{LogicalMemoryPath, MemorySource},
    BearMemoryStore, VerifiedHatProposalSource,
};

use super::{decode_candidate, ReviewedPromotion, REVIEWABLE};

pub async fn promote_curated_proposal_to_hat(
    store: &BearMemoryStore,
    proposal_id: Uuid,
    verified: VerifiedHatProposalSource,
    curated_content: &str,
    curator_agent_id: &str,
    curator_reason: &str,
) -> Result<ReviewedPromotion, DenError> {
    let content = curated_content.trim();
    let reason = curator_reason.trim();
    if content.is_empty()
        || content.len() > 16_000
        || curator_agent_id.trim().is_empty()
        || reason.is_empty()
        || reason.len() > 1_000
    {
        return Err(DenError::ValidationError(
            "curated content must be 1–16000 bytes with a curator ID and bounded rationale".into(),
        ));
    }
    let mut tx = store
        .pool()
        .begin()
        .await
        .map_err(|err| DenError::System(format!("begin Curate hat publication: {err}")))?;
    let proposal = sqlx::query(
        "SELECT status, suggested_action, sensitivity, requires_human,
                source_memory_id, target_hat_id, payload_json
         FROM memory_proposals WHERE bear_id = ? AND proposal_id = ?",
    )
    .bind(store.bear_id().to_string())
    .bind(proposal_id.to_string())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| DenError::System(format!("check Curate proposal: {err}")))?
    .ok_or_else(|| DenError::NotFound("verified hat proposal not found".into()))?;
    let status: String = proposal.try_get("status")?;
    let action: String = proposal.try_get("suggested_action")?;
    let sensitivity: String = proposal.try_get("sensitivity")?;
    let requires_human: i64 = proposal.try_get("requires_human")?;
    let source_id: Option<String> = proposal.try_get("source_memory_id")?;
    let hat_id: Option<String> = proposal.try_get("target_hat_id")?;
    if status != "pending"
        || action != "propose_hat"
        || sensitivity != "normal"
        || requires_human != 0
        || source_id.as_deref() != Some(verified.memory_id.to_string().as_str())
        || hat_id.as_deref() != Some(verified.hat_id.to_string().as_str())
    {
        return Err(DenError::Authorization(
            "proposal lacks a current verified source/hat link or requires a different review lane"
                .into(),
        ));
    }
    let source_sql = format!(
        "SELECT m.memory_id, m.scope_source_kind, m.scope_source_id,
                m.kind, m.content_text, m.sequence_no FROM memory_records m
         WHERE {REVIEWABLE} AND m.memory_id = ?"
    );
    let source = sqlx::query(&source_sql)
        .bind(store.bear_id().to_string())
        .bind(verified.memory_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| DenError::System(format!("recheck Curate source note: {err}")))?
        .ok_or_else(|| DenError::NotFound("current source note not found".into()))?;
    let source = decode_candidate(source)?;
    if !matches!(source.source, MemorySource::Conversation(_)) {
        return Err(DenError::Authorization(
            "only verified conversation notes can enter the current Curate hat lane".into(),
        ));
    }
    // This rejects an accidental verbatim copy; it is not a substitute for a
    // curator's sensitivity/prompt-injection assessment of derived content.
    if source.content_text.trim() == content {
        return Err(DenError::ValidationError(
            "Curate must author new shareable content, not copy a private note verbatim".into(),
        ));
    }
    let already_shared: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_promotions p
         JOIN memory_records target ON target.bear_id = p.bear_id AND target.memory_id = p.target_memory_id
         WHERE p.bear_id = ? AND p.source_memory_id = ?
           AND target.scope_type = 'hat' AND target.scope_hat_id = ?",
    )
    .bind(store.bear_id().to_string())
    .bind(verified.memory_id.to_string())
    .bind(verified.hat_id.to_string())
    .fetch_one(&mut *tx).await
    .map_err(|err| DenError::System(format!("check earlier source→hat publication: {err}")))?;
    if already_shared != 0 {
        return Err(DenError::ValidationError(
            "source note was already promoted to this hat".into(),
        ));
    }
    let sequence: i64 = sqlx::query_scalar(
        "UPDATE bear_sequence SET next_sequence = next_sequence + 2 WHERE id = 1 RETURNING next_sequence - 2"
    ).fetch_one(&mut *tx).await
        .map_err(|err| DenError::System(format!("allocate Curate publication sequence: {err}")))?;
    let target_id = Uuid::new_v4();
    let promotion_id = Uuid::new_v4();
    let path = LogicalMemoryPath::hat(verified.hat_id, &proposal_id.to_string()).to_logical_path();
    let created_at = now_rfc3339()?;
    let metadata = serde_json::json!({
        "promoted_from": verified.memory_id,
        "source_kind": source.source.kind(),
        "source_id": source.source.id(),
        "source_sequence_no": source.sequence_no,
        "proposal_id": proposal_id,
        "curated_by_agent_id": curator_agent_id,
    });
    sqlx::query(
        "INSERT INTO memory_records (memory_id, bear_id, sequence_no, scope_type,
            scope_hat_id, kind, author_profile, created_at, content_text,
            metadata_json, visibility, logical_path, valid_from, salience)
         VALUES (?, ?, ?, 'hat', ?, 'note', 'curate', ?, ?, ?, 'normal', ?, ?, 'normal')",
    )
    .bind(target_id.to_string())
    .bind(store.bear_id().to_string())
    .bind(sequence)
    .bind(verified.hat_id.to_string())
    .bind(&created_at)
    .bind(content)
    .bind(metadata.to_string())
    .bind(&path)
    .bind(&created_at)
    .execute(&mut *tx)
    .await
    .map_err(|err| DenError::System(format!("write Curate hat entry: {err}")))?;
    sqlx::query(
        "INSERT INTO memory_promotions (promotion_id, bear_id, sequence_no,
            source_memory_id, target_memory_id, review_agent_id, action, created_at, notes)
         VALUES (?, ?, ?, ?, ?, ?, 'curate_promote_to_hat', ?, ?)",
    )
    .bind(promotion_id.to_string())
    .bind(store.bear_id().to_string())
    .bind(sequence + 1)
    .bind(verified.memory_id.to_string())
    .bind(target_id.to_string())
    .bind(curator_agent_id)
    .bind(&created_at)
    .bind(reason)
    .execute(&mut *tx)
    .await
    .map_err(|err| DenError::System(format!("write Curate source→hat provenance: {err}")))?;
    let payload: String = proposal.try_get("payload_json")?;
    let mut payload: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|err| DenError::Parsing(format!("invalid Curate proposal payload: {err}")))?;
    if !payload.is_object() {
        return Err(DenError::Parsing(
            "Curate proposal payload must be an object".into(),
        ));
    }
    payload["result_path"] = serde_json::json!(path);
    payload["result_commit"] = serde_json::json!(target_id);
    payload["reviewer_agent_id"] = serde_json::json!(curator_agent_id);
    payload["decision_summary"] = serde_json::json!(reason);
    let updated = sqlx::query(
        "UPDATE memory_proposals SET status = 'approved', payload_json = ?, reviewed_at = ?
         WHERE bear_id = ? AND proposal_id = ? AND status = 'pending'",
    )
    .bind(payload.to_string())
    .bind(&created_at)
    .bind(store.bear_id().to_string())
    .bind(proposal_id.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|err| DenError::System(format!("resolve Curate hat proposal: {err}")))?;
    if updated.rows_affected() != 1 {
        return Err(DenError::ValidationError(
            "proposal changed during Curate publication".into(),
        ));
    }
    tx.commit()
        .await
        .map_err(|err| DenError::System(format!("commit Curate hat publication: {err}")))?;
    Ok(ReviewedPromotion {
        memory_id: target_id,
        promotion_id,
    })
}
