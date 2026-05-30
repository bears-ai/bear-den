use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::{
    api::acp::AcpCompactionStatusResponse,
    core::runtime_compaction_observability::RuntimeCompactionEvent,
    errors::CustomError,
};

pub async fn record_runtime_compaction_event(
    pool: &PgPool,
    event: &RuntimeCompactionEvent,
) -> Result<(), CustomError> {
    let event_hash = runtime_compaction_event_hash(event)?;
    let boundary = serde_json::to_value(&event.boundary).map_err(|err| {
        CustomError::System(format!("serialize compaction boundary: {err}"))
    })?;
    let artifact = serde_json::to_value(&event.artifact).map_err(|err| {
        CustomError::System(format!("serialize compaction artifact: {err}"))
    })?;
    sqlx::query(
        r#"
        INSERT INTO runtime_compaction_events (
            conversation_id,
            trigger,
            policy_version,
            status,
            event_hash,
            boundary,
            source_group_start,
            source_group_end,
            artifact,
            diagnostic
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (conversation_id, event_hash) DO NOTHING
        "#,
    )
    .bind(&event.conversation_id)
    .bind(format!("{:?}", event.trigger))
    .bind(&event.policy_version)
    .bind(format!("{:?}", event.status))
    .bind(&event_hash)
    .bind(boundary)
    .bind(event.source_group_start.map(|v| v as i32))
    .bind(event.source_group_end.map(|v| v as i32))
    .bind(artifact)
    .bind(&event.diagnostic)
    .execute(pool)
    .await
    .map_err(|err| CustomError::Database(format!("insert runtime_compaction_events: {err}")))?;
    Ok(())
}

pub(crate) async fn list_runtime_compaction_events(
    pool: &PgPool,
    conversation_id: &str,
    limit: i64,
) -> Result<Vec<AcpCompactionStatusResponse>, CustomError> {
    let rows = sqlx::query(
        r#"
        SELECT
            status,
            policy_version,
            source_group_start,
            source_group_end,
            diagnostic,
            artifact
        FROM runtime_compaction_events
        WHERE conversation_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(conversation_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|err| CustomError::Database(format!("select runtime_compaction_events: {err}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let artifact = row
            .try_get::<Option<serde_json::Value>, _>("artifact")
            .map_err(|err| CustomError::Database(format!("decode compaction artifact: {err}")))?;
        items.push(AcpCompactionStatusResponse {
            status: row
                .try_get::<String, _>("status")
                .map_err(|err| CustomError::Database(format!("decode compaction status: {err}")))?,
            policy_version: row.try_get::<String, _>("policy_version").map_err(|err| {
                CustomError::Database(format!("decode compaction policy_version: {err}"))
            })?,
            source_group_start: row
                .try_get::<Option<i32>, _>("source_group_start")
                .map_err(|err| {
                    CustomError::Database(format!("decode compaction source_group_start: {err}"))
                })?
                .map(|v| v as usize),
            source_group_end: row
                .try_get::<Option<i32>, _>("source_group_end")
                .map_err(|err| {
                    CustomError::Database(format!("decode compaction source_group_end: {err}"))
                })?
                .map(|v| v as usize),
            diagnostic: row
                .try_get::<Option<String>, _>("diagnostic")
                .map_err(|err| {
                    CustomError::Database(format!("decode compaction diagnostic: {err}"))
                })?,
            artifact,
        });
    }
    Ok(items)
}

fn runtime_compaction_event_hash(event: &RuntimeCompactionEvent) -> Result<String, CustomError> {
    let payload = serde_json::json!({
        "conversation_id": event.conversation_id,
        "trigger": format!("{:?}", event.trigger),
        "policy_version": event.policy_version,
        "status": format!("{:?}", event.status),
        "boundary": event.boundary,
        "source_group_start": event.source_group_start,
        "source_group_end": event.source_group_end,
        "artifact": event.artifact,
        "diagnostic": event.diagnostic,
    });
    let bytes = serde_json::to_vec(&payload)
        .map_err(|err| CustomError::System(format!("serialize compaction event hash payload: {err}")))?;
    let digest = Sha256::digest(bytes);
    Ok(format!("{:x}", digest))
}
