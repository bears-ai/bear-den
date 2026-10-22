use den_core::DenError;
use den_service::conversation::dependencies::require_unrestricted_metadata;
use serde_json::to_value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::runtime_conversations::{RuntimeCompactionTriggerKind, RuntimeIterativeSummary};

use super::{RuntimeCompactionDecision, RuntimeCompactionPolicy};

#[derive(Debug, Clone)]
pub struct CompactionArtifactRecord {
    pub artifact_id: Uuid,
    pub summary: RuntimeIterativeSummary,
    pub source_message_start_seq: i64,
    pub source_message_end_seq: i64,
    pub policy_version: String,
}

fn db_err(context: &'static str) -> impl FnOnce(sqlx::Error) -> DenError {
    move |err| match DenError::from(err) {
        DenError::Database(message) => DenError::Database(format!("{context}: {message}")),
        DenError::DatabaseUnavailable(message) => {
            DenError::DatabaseUnavailable(format!("{context}: {message}"))
        }
        other => other,
    }
}

/// Metadata must be admitted before serde discards fields outside the summary schema.
/// Shared by prompt/reflection loads and archive harvest, not a declassification path.
pub(crate) fn decode_unrestricted_iterative_summary(
    value: serde_json::Value,
) -> Result<RuntimeIterativeSummary, DenError> {
    require_unrestricted_metadata(&value)?;
    serde_json::from_value(value)
        .map_err(|err| DenError::Parsing(format!("decode compaction artifact_json: {err}")))
}

pub async fn load_latest_iterative_summary(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
) -> Result<Option<CompactionArtifactRecord>, DenError> {
    let Some(conversation_uuid) =
        resolve_conversation_uuid(pool, bear_id, external_conversation_id).await?
    else {
        return Ok(None);
    };

    let row = sqlx::query!(
        r#"
        SELECT id AS "id!", artifact_json AS "artifact_json!",
               source_message_start_seq AS "source_message_start_seq!",
               source_message_end_seq AS "source_message_end_seq!",
               policy_version AS "policy_version!"
        FROM conversation_compaction_artifacts
        WHERE conversation_id = $1
          AND artifact_kind = 'iterative_summary'
          AND superseded_by IS NULL
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        conversation_uuid
    )
    .fetch_optional(pool)
    .await
    .map_err(db_err("load compaction artifact"))?;

    let Some(row) = row else {
        return Ok(None);
    };

    let summary = decode_unrestricted_iterative_summary(row.artifact_json)?;

    Ok(Some(CompactionArtifactRecord {
        artifact_id: row.id,
        summary,
        source_message_start_seq: row.source_message_start_seq,
        source_message_end_seq: row.source_message_end_seq,
        policy_version: row.policy_version,
    }))
}

pub async fn insert_iterative_summary_artifact(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
    decision: &RuntimeCompactionDecision,
    policy: &RuntimeCompactionPolicy,
    trigger: RuntimeCompactionTriggerKind,
    source_message_start_seq: i64,
    source_message_end_seq: i64,
    summary: &RuntimeIterativeSummary,
) -> Result<Uuid, DenError> {
    let conversation_uuid = resolve_conversation_uuid(pool, bear_id, external_conversation_id)
        .await?
        .ok_or_else(|| {
            DenError::NotFound(format!(
                "conversation not found for compaction artifact: {external_conversation_id}"
            ))
        })?;

    let artifact_json = to_value(summary)
        .map_err(|err| DenError::System(format!("serialize compaction summary: {err}")))?;

    let mut tx = pool
        .begin()
        .await
        .map_err(db_err("begin compaction artifact tx"))?;

    let prior_id = sqlx::query_scalar!(
        r#"
        SELECT id AS "id!"
        FROM conversation_compaction_artifacts
        WHERE conversation_id = $1
          AND artifact_kind = 'iterative_summary'
          AND superseded_by IS NULL
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        conversation_uuid
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err("select prior compaction artifact"))?;

    let inserted_id = sqlx::query_scalar!(
        r#"
        INSERT INTO conversation_compaction_artifacts (
            conversation_id,
            artifact_kind,
            policy_version,
            trigger,
            source_message_start_seq,
            source_message_end_seq,
            source_group_start,
            source_group_end,
            artifact_json
        )
        VALUES ($1, 'iterative_summary', $2, $3, $4, $5, $6, $7, $8)
        RETURNING id AS "id!"
        "#,
        conversation_uuid,
        &policy.policy_version,
        trigger.as_str(),
        source_message_start_seq,
        source_message_end_seq,
        decision.selected_group_start as i32,
        decision.selected_group_end as i32,
        artifact_json
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err("insert compaction artifact"))?;

    if let Some(prior_id) = prior_id {
        sqlx::query!(
            r"
            UPDATE conversation_compaction_artifacts
            SET superseded_by = $1
            WHERE id = $2
            ",
            inserted_id,
            prior_id
        )
        .execute(&mut *tx)
        .await
        .map_err(db_err("supersede prior compaction artifact"))?;
    }

    tx.commit()
        .await
        .map_err(db_err("commit compaction artifact tx"))?;

    Ok(inserted_id)
}

async fn resolve_conversation_uuid(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
) -> Result<Option<Uuid>, DenError> {
    let row = sqlx::query_scalar!(
        r#"
        SELECT id AS "id!"
        FROM conversations
        WHERE bear_id = $1 AND external_conversation_id = $2
        LIMIT 1
        "#,
        bear_id,
        external_conversation_id
    )
    .fetch_optional(pool)
    .await
    .map_err(db_err("resolve conversation uuid"))?;
    Ok(row)
}

#[cfg(test)]
#[path = "artifact_dependency_tests.rs"]
mod dependency_tests;
