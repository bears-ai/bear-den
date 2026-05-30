use sqlx::PgPool;

use crate::{
    core::runtime_compaction_observability::RuntimeCompactionEvent,
    errors::CustomError,
};

pub async fn record_runtime_compaction_event(
    pool: &PgPool,
    event: &RuntimeCompactionEvent,
) -> Result<(), CustomError> {
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
            boundary,
            source_group_start,
            source_group_end,
            artifact,
            diagnostic
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(&event.conversation_id)
    .bind(format!("{:?}", event.trigger))
    .bind(&event.policy_version)
    .bind(format!("{:?}", event.status))
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
