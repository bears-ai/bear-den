//! Direct human-settings intake; request IDs deduplicate protected requests, never generic notes.

use den_core::DenError;
use uuid::Uuid;

use super::{
    authority::{denied, write_transaction},
    read::get_inside,
    settings::setting_inside,
    CandidateSource, PersonalCandidate, PersonalCandidateId, PreferredName, VerifiedOwnerSource,
};
use crate::{clock::now_rfc3339, BearMemoryStore};

/// The trusted service must hold current membership locks through this operation.
/// Capture consent and any supplied capture revision are checked even for retries, before
/// deduplication. A request ID is immutable across actors, values, identity generations and
/// terminal states; a retry never revives old content.
pub async fn stage_owner_preferred_name(
    store: &BearMemoryStore,
    source: &VerifiedOwnerSource,
    name: PreferredName,
) -> Result<PersonalCandidate, DenError> {
    let binding = &source.binding;
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let person_entity_id = binding.entity_id();
    let capture = setting_inside(
        &mut tx,
        &bear_id,
        person_entity_id,
        binding.authority_generation,
    )
    .await?;
    if source
        .capture_revision
        .is_some_and(|expected| expected != capture.revision)
    {
        return Err(denied());
    }
    if !capture.enabled {
        return Err(DenError::Authorization(
            "Personal-memory candidate capture is disabled.".into(),
        ));
    }
    let request_id = source.request_id.to_string();
    // Bear-scoped, not actor-scoped: reusing another person's ID cannot mint new authority.
    // This lookup returns no body; the owner filter is applied before decoding any value.
    let previous = sqlx::query_scalar!(
        "SELECT candidate_id FROM personal_memory_candidates WHERE bear_id = ? AND source_request_id = ?",
        bear_id,
        request_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(previous) = previous {
        let id = PersonalCandidateId(Uuid::parse_str(&previous).map_err(|_| denied())?);
        let candidate = get_inside(&mut tx, &bear_id, person_entity_id, id)
            .await?
            .ok_or_else(denied)?;
        if candidate.source()
            != (CandidateSource::OwnerRequest {
                request_id: source.request_id,
            })
            || candidate.source_bear_id() != binding.bear_id()
            || candidate.source_user_id() != binding.user_id()
            || candidate.source_binding_generation() != binding.authority_generation
            || candidate.preferred_name() != &name
        {
            return Err(denied());
        }
        tx.commit().await?;
        return Ok(candidate);
    }
    let candidate_id = PersonalCandidateId(Uuid::new_v4());
    let id = candidate_id.to_string();
    let sequence_no = sqlx::query_scalar!(
        r#"UPDATE bear_sequence SET next_sequence = next_sequence + 1 WHERE id = 1
           RETURNING next_sequence - 1 AS "sequence_no!: i64""#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let created_at = now_rfc3339()?;
    let actor = binding.user_id().get();
    let value = name.as_str();
    sqlx::query!(
        r"
        INSERT INTO personal_memory_candidates (
            candidate_id, bear_id, person_entity_id, sequence_no, fact_type, value_text,
            evidence_kind, retention, source_kind, source_request_id,
            source_bear_id, source_user_id, source_binding_generation, state, created_at
        ) VALUES (?, ?, ?, ?, 'preferred_name', ?, 'explicit_user_request', 'durable',
                  'owner_request', ?, ?, ?, ?, 'pending', ?)
        ",
        id,
        bear_id,
        person_entity_id,
        sequence_no,
        value,
        request_id,
        bear_id,
        actor,
        binding.authority_generation,
        created_at,
    )
    .execute(&mut *tx)
    .await?;
    let candidate = get_inside(&mut tx, &bear_id, person_entity_id, candidate_id)
        .await?
        .ok_or_else(denied)?;
    tx.commit().await?;
    Ok(candidate)
}
