use den_core::DenError;
use sqlx::SqliteConnection;
use uuid::Uuid;

/// Owner requests have no note to validate. Their authority is the protected candidate's
/// actor/host/generation plus the caller's verified current binding and consent, checked by
/// publication/review. Generic note metadata can never select this provenance variant.
pub(crate) async fn candidate_source_is_eligible(
    connection: &mut SqliteConnection,
    bear_id: &str,
    source: super::CandidateSource,
) -> Result<bool, DenError> {
    match source {
        super::CandidateSource::Conversation {
            conversation_id,
            memory_id,
        } => source_is_eligible(connection, bear_id, conversation_id, memory_id).await,
        super::CandidateSource::OwnerRequest { .. } => Ok(true),
    }
}

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
                  SELECT 1 FROM personal_memory_candidates c
                  JOIN personal_memory_erasures e
                    ON e.bear_id = c.bear_id AND e.person_entity_id = c.person_entity_id
                      AND c.sequence_no < e.sequence_no
                  WHERE c.bear_id = m.bear_id AND c.source_kind = 'conversation'
                    AND c.source_memory_id = m.memory_id AND c.source_conversation_id = m.scope_source_id
              )
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
