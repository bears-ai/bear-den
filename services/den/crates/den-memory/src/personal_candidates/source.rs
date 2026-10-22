use den_core::DenError;
use sqlx::SqliteConnection;
use uuid::Uuid;

/// Source evidence is an exact current note, never a path, body, or identity claim.
pub(crate) async fn source_is_eligible(
    connection: &mut SqliteConnection,
    bear_id: &str,
    conversation: Uuid,
    memory: Uuid,
) -> Result<bool, DenError> {
    let conversation_id = conversation.to_string();
    let source_memory_id = memory.to_string();
    Ok(sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM generic_memory_records m WHERE m.bear_id = ? AND m.memory_id = ?
              AND m.scope_type = 'source_local' AND m.scope_source_kind = 'conversation'
              AND m.scope_source_id = ? AND m.visibility = 'normal' AND m.invalid_at IS NULL
              AND CASE WHEN json_valid(m.metadata_json)
                       THEN COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active') = 'active'
                       ELSE 0 END
              AND NOT EXISTS (
                  SELECT 1 FROM memory_records newer
                  WHERE newer.bear_id = m.bear_id AND newer.supersedes_memory_id = m.memory_id
              )
        ) AS "eligible!: bool"
        "#,
        bear_id,
        source_memory_id,
        conversation_id,
    ).fetch_one(connection).await?)
}
