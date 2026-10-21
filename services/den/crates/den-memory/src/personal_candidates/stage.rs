use den_core::DenError;
use uuid::Uuid;

use super::{
    authority::{denied, write_transaction},
    read::get_inside,
    settings::enabled,
    types::{PersonalCandidate, PersonalCandidateId, PreferredName, VerifiedPersonalSource},
};
use crate::{clock::now_rfc3339, BearMemoryStore};

/// Stage an explicit preferred-name request only. This is not approval, synthesis, or
/// publication, and it never copies text into generic notes, proposals, or observations.
pub async fn stage_preferred_name(
    store: &BearMemoryStore,
    source: &VerifiedPersonalSource,
    name: PreferredName,
) -> Result<PersonalCandidate, DenError> {
    let binding = &source.binding;
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let person_entity_id = binding.entity_id();
    if !enabled(
        &mut tx,
        &bear_id,
        person_entity_id,
        binding.authority_generation,
    )
    .await?
    {
        return Err(DenError::Authorization(
            "Personal-memory candidate capture is disabled.".into(),
        ));
    }
    let conversation_id = source.conversation_id.to_string();
    let source_memory_id = source.memory_id.to_string();
    // Initial admission accepts only the exact current, unprotected source-local note.
    // We read no source body, do not reinterpret its JSON as identity, and never reclassify it.
    let eligible = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM memory_records m WHERE m.bear_id = ? AND m.memory_id = ?
              AND m.scope_type = 'source_local' AND m.scope_source_kind = 'conversation'
              AND m.scope_source_id = ? AND m.visibility = 'normal' AND m.invalid_at IS NULL
              AND json_valid(m.metadata_json)
              AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active') = 'active'
              AND NOT EXISTS (
                  SELECT 1 FROM memory_records newer
                  WHERE newer.bear_id = m.bear_id AND newer.supersedes_memory_id = m.memory_id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM memory_access_rules r WHERE r.bear_id = m.bear_id AND r.src_memory_id = m.memory_id
              )
        ) AS "eligible!: bool"
        "#,
        bear_id,
        source_memory_id,
        conversation_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    if !eligible {
        return Err(denied());
    }
    let value_text = name.as_str();
    let previous = sqlx::query_scalar!(
        r"
        SELECT candidate_id FROM personal_memory_candidates
        WHERE bear_id = ? AND person_entity_id = ? AND source_conversation_id = ?
          AND source_memory_id = ? AND fact_type = 'preferred_name' AND value_text = ? AND state = 'pending'
        ",
        bear_id,
        person_entity_id,
        conversation_id,
        source_memory_id,
        value_text,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let candidate_id = if let Some(id) = previous {
        PersonalCandidateId(Uuid::parse_str(&id).map_err(|_| denied())?)
    } else {
        let candidate_id = PersonalCandidateId(Uuid::new_v4());
        let id = candidate_id.to_string();
        let sequence_no = sqlx::query_scalar!(
            r#"UPDATE bear_sequence SET next_sequence = next_sequence + 1 WHERE id = 1
               RETURNING next_sequence - 1 AS "sequence_no!: i64""#,
        )
        .fetch_one(&mut *tx)
        .await?;
        let created_at = now_rfc3339()?;
        let user_id = binding.user_id().get();
        sqlx::query!(
            r"
            INSERT INTO personal_memory_candidates (
                candidate_id, bear_id, person_entity_id, sequence_no, fact_type, value_text,
                evidence_kind, retention, source_conversation_id, source_memory_id,
                source_bear_id, source_user_id, source_binding_generation, state, created_at
            ) VALUES (?, ?, ?, ?, 'preferred_name', ?, 'explicit_user_request', 'durable', ?, ?, ?, ?, ?, 'pending', ?)
            ",
            id,
            bear_id,
            person_entity_id,
            sequence_no,
            value_text,
            conversation_id,
            source_memory_id,
            bear_id,
            user_id,
            binding.authority_generation,
            created_at,
        )
        .execute(&mut *tx)
        .await?;
        candidate_id
    };
    let candidate = get_inside(&mut tx, &bear_id, person_entity_id, candidate_id)
        .await?
        .ok_or_else(denied)?;
    tx.commit().await?;
    Ok(candidate)
}
