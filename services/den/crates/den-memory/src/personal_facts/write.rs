use den_core::DenError;
use uuid::Uuid;

use super::{
    read::{invalid, publication, relation_ids},
    retract::{invalidate, RuleDisposition},
    PersonalFactId, PublicationReceipt,
};
use crate::{
    clock::now_rfc3339,
    personal_candidates::{
        enabled, get_inside, source_is_eligible, write_transaction, CandidateState,
        PersonalCandidateId, VerifiedPersonalSource,
    },
    BearMemoryStore,
};

/// Trusted explicit owner approval only. There is no automatic Curate/model caller.
/// The service keeps current membership and canonical source ownership locked through this
/// transaction. A retry returns its immutable original decision, never reactivates old content.
pub async fn publish_preferred_name(
    store: &BearMemoryStore,
    source: &VerifiedPersonalSource,
    candidate_id: PersonalCandidateId,
    expected_head: Option<PersonalFactId>,
) -> Result<PublicationReceipt, DenError> {
    let binding = &source.binding;
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let person_entity_id = binding.entity_id();
    let candidate_id_text = candidate_id.to_string();
    if let Some(existing) = publication(
        &mut tx,
        &bear_id,
        Some(person_entity_id),
        None,
        Some(&candidate_id_text),
    )
    .await?
    {
        let receipt = existing.receipt()?;
        tx.commit().await?;
        return Ok(receipt);
    }
    let candidate = get_inside(&mut tx, &bear_id, person_entity_id, candidate_id)
        .await?
        .ok_or_else(invalid)?;
    if candidate.state() != CandidateState::Pending
        || candidate.source_bear_id() != binding.bear_id()
        || candidate.source_user_id() != binding.user_id()
        || candidate.source_binding_generation() != binding.authority_generation
        || candidate.source_conversation_id() != source.conversation_id
        || candidate.source_memory_id() != source.memory_id
        || !enabled(
            &mut tx,
            &bear_id,
            person_entity_id,
            binding.authority_generation,
        )
        .await?
        || !source_is_eligible(&mut tx, &bear_id, source.conversation_id, source.memory_id).await?
    {
        return Err(invalid());
    }
    let previous = publication(&mut tx, &bear_id, Some(person_entity_id), None, None).await?;
    let previous_fact_id = previous
        .as_ref()
        .map(|row| row.receipt().map(|receipt| receipt.fact_id))
        .transpose()?;
    if previous_fact_id != expected_head {
        return Err(invalid());
    }

    let sequence = sqlx::query_scalar!(
        r#"UPDATE bear_sequence SET next_sequence = next_sequence + 4 WHERE id = 1
           RETURNING next_sequence - 4 AS "sequence!: i64""#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let fact_id = PersonalFactId(Uuid::new_v4());
    let memory_id = fact_id.to_string();
    let previous_memory_id = previous_fact_id.map(|id| id.to_string());
    let created_at = now_rfc3339()?;
    let value = candidate.preferred_name().as_str();
    // The value is the minimal fact, not a prompt/template or a transcript copy. Its category
    // supplies semantics to a future explicitly authorized projector; no projection is enabled.
    sqlx::query!(
        r"
        INSERT INTO memory_records (
            memory_id, bear_id, sequence_no, scope_type, kind, author_profile,
            created_at, content_text, metadata_json, visibility, valid_from,
            salience, supersedes_memory_id
        ) VALUES (?, ?, ?, 'shared', 'personal_preferred_name', 'curate', ?, ?, '{}', 'normal', ?, 'normal', ?)
        ",
        memory_id,
        bear_id,
        sequence,
        created_at,
        value,
        created_at,
        previous_memory_id,
    ).execute(&mut *tx).await?;
    let (subject, audience) = relation_ids()?;
    let subject_id = Uuid::new_v4().to_string();
    let subject_sequence = sequence + 1;
    sqlx::query!(
        r"
        INSERT INTO memory_relations (
            link_id, bear_id, sequence_no, src_memory_id, entity_id, relation,
            qualifiers_json, author_profile, state, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, '{}', 'curate', 'active', ?)
        ",
        subject_id,
        bear_id,
        subject_sequence,
        memory_id,
        person_entity_id,
        subject,
        created_at,
    )
    .execute(&mut *tx)
    .await?;
    let audience_id = Uuid::new_v4().to_string();
    let audience_sequence = sequence + 2;
    sqlx::query!(
        r"
        INSERT INTO memory_access_rules (
            link_id, bear_id, sequence_no, src_memory_id, entity_id, relation,
            qualifiers_json, author_profile, state, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, '{}', 'curate', 'active', ?)
        ",
        audience_id,
        bear_id,
        audience_sequence,
        memory_id,
        person_entity_id,
        audience,
        created_at,
    )
    .execute(&mut *tx)
    .await?;
    if let Some(previous) = previous {
        invalidate(
            &mut tx,
            &bear_id,
            &previous.memory_id,
            &created_at,
            RuleDisposition::Superseded,
        )
        .await?;
    }
    let decision_sequence = sequence + 3;
    let actor = binding.user_id().get();
    // Late immutable decision insert: any failure rolls back the body, mandatory relations,
    // previous-version changes and sequence allocation together.
    sqlx::query!(
        r"
        INSERT INTO personal_memory_publications (
            memory_id, bear_id, sequence_no, person_entity_id, candidate_id, fact_type,
            previous_memory_id, published_on_bear_id, published_by_user_id, authority_generation, created_at
        ) VALUES (?, ?, ?, ?, ?, 'preferred_name', ?, ?, ?, ?, ?)
        ",
        memory_id,
        bear_id,
        decision_sequence,
        person_entity_id,
        candidate_id_text,
        previous_memory_id,
        bear_id,
        actor,
        binding.authority_generation,
        created_at,
    ).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(PublicationReceipt {
        fact_id,
        candidate_id,
        previous_fact_id,
    })
}
