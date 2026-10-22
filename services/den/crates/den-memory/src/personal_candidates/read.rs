use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use super::{
    authority::{ensure_current, write_transaction},
    types::{CandidateState, PersonalCandidate, PersonalCandidateId, PreferredName},
};
use crate::{clock::now_rfc3339, AuthenticatedUserEntity, BearMemoryStore};

#[derive(sqlx::FromRow)]
pub(super) struct CandidateRow {
    candidate_id: String,
    person_entity_id: String,
    fact_type: String,
    value_text: String,
    evidence_kind: String,
    retention: String,
    state: String,
    source_conversation_id: String,
    source_memory_id: String,
    source_bear_id: String,
    source_user_id: i32,
    source_binding_generation: i64,
    created_at: String,
    withdrawn_at: Option<String>,
}

fn invalid_candidate() -> DenError {
    DenError::ValidationError("Invalid protected personal-memory candidate.".into())
}

impl CandidateRow {
    fn into_candidate(self) -> Result<PersonalCandidate, DenError> {
        if self.fact_type != "preferred_name"
            || self.evidence_kind != "explicit_user_request"
            || self.retention != "durable"
            || self.source_user_id <= 0
            || self.source_binding_generation <= 0
        {
            return Err(invalid_candidate());
        }
        let state = match self.state.as_str() {
            "pending" => CandidateState::Pending,
            "withdrawn" => CandidateState::Withdrawn,
            "quarantined" => CandidateState::Quarantined,
            "published" => CandidateState::Published,
            _ => return Err(invalid_candidate()),
        };
        let preferred_name = PreferredName::new(&self.value_text)?;
        if preferred_name.as_str() != self.value_text {
            return Err(invalid_candidate());
        }
        Ok(PersonalCandidate {
            id: PersonalCandidateId(
                Uuid::parse_str(&self.candidate_id).map_err(|_| invalid_candidate())?,
            ),
            person_entity_id: self.person_entity_id,
            preferred_name,
            state,
            source_conversation_id: Uuid::parse_str(&self.source_conversation_id)
                .map_err(|_| invalid_candidate())?,
            source_memory_id: Uuid::parse_str(&self.source_memory_id)
                .map_err(|_| invalid_candidate())?,
            source_bear_id: BearId::new(
                Uuid::parse_str(&self.source_bear_id).map_err(|_| invalid_candidate())?,
            ),
            source_user_id: UserId::new(self.source_user_id),
            source_binding_generation: self.source_binding_generation,
            created_at: self.created_at,
            withdrawn_at: self.withdrawn_at,
        })
    }
}

pub(crate) async fn get_inside(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person_entity_id: &str,
    id: PersonalCandidateId,
) -> Result<Option<PersonalCandidate>, DenError> {
    let id = id.to_string();
    sqlx::query_as!(
        CandidateRow,
        r#"
        SELECT candidate_id, person_entity_id, fact_type, value_text, evidence_kind, retention,
               CASE WHEN EXISTS (SELECT 1 FROM personal_memory_publications p
                    WHERE p.bear_id = personal_memory_candidates.bear_id
                      AND p.candidate_id = personal_memory_candidates.candidate_id)
                    THEN 'published' ELSE state END AS "state!: String",
               source_conversation_id, source_memory_id, source_bear_id,
               source_user_id AS "source_user_id!: i32", source_binding_generation, created_at, withdrawn_at
        FROM personal_memory_candidates WHERE bear_id = ? AND person_entity_id = ? AND candidate_id = ?
        "#,
        bear_id,
        person_entity_id,
        id,
    )
    .fetch_optional(connection)
    .await?
    .map(CandidateRow::into_candidate)
    .transpose()
}

pub async fn get_candidate(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    id: PersonalCandidateId,
) -> Result<Option<PersonalCandidate>, DenError> {
    let mut tx = store.pool().begin().await?;
    ensure_current(&mut tx, store, binding).await?;
    let candidate = get_inside(
        &mut tx,
        &store.bear_id().to_string(),
        binding.entity_id(),
        id,
    )
    .await?;
    tx.commit().await?;
    Ok(candidate)
}

pub async fn list_candidates(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    limit: u32,
) -> Result<Vec<PersonalCandidate>, DenError> {
    let mut tx = store.pool().begin().await?;
    ensure_current(&mut tx, store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let person_entity_id = binding.entity_id();
    let limit = i64::from(limit.clamp(1, 100));
    let rows = sqlx::query_as!(
        CandidateRow,
        r#"
        SELECT candidate_id, person_entity_id, fact_type, value_text, evidence_kind, retention,
               CASE WHEN EXISTS (SELECT 1 FROM personal_memory_publications p
                    WHERE p.bear_id = personal_memory_candidates.bear_id
                      AND p.candidate_id = personal_memory_candidates.candidate_id)
                    THEN 'published' ELSE state END AS "state!: String",
               source_conversation_id, source_memory_id, source_bear_id,
               source_user_id AS "source_user_id!: i32", source_binding_generation, created_at, withdrawn_at
        FROM personal_memory_candidates WHERE bear_id = ? AND person_entity_id = ?
        ORDER BY sequence_no DESC LIMIT ?
        "#,
        bear_id,
        person_entity_id,
        limit,
    )
    .fetch_all(&mut *tx)
    .await?;
    let candidates = rows
        .into_iter()
        .map(CandidateRow::into_candidate)
        .collect::<Result<_, _>>()?;
    tx.commit().await?;
    Ok(candidates)
}

/// Withdraw only the current person's candidate. Missing/other-person IDs are indistinguishable.
/// The minimized value and immutable provenance remain protected historical evidence, not reuse.
pub async fn withdraw_candidate(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    id: PersonalCandidateId,
) -> Result<bool, DenError> {
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let person_entity_id = binding.entity_id();
    let actor = binding.user_id().get();
    let id = id.to_string();
    let withdrawn_at = now_rfc3339()?;
    let changed = sqlx::query!(
        r"
        UPDATE personal_memory_candidates SET state = 'withdrawn', withdrawn_at = ?,
            withdrawn_on_bear_id = ?, withdrawn_by_user_id = ?
        WHERE bear_id = ? AND person_entity_id = ? AND candidate_id = ? AND state IN ('pending', 'quarantined')
                  AND NOT EXISTS (SELECT 1 FROM personal_memory_publications p
                      WHERE p.bear_id = personal_memory_candidates.bear_id
                        AND p.candidate_id = personal_memory_candidates.candidate_id)
        ",
        withdrawn_at,
        bear_id,
        actor,
        bear_id,
        person_entity_id,
        id,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(changed == 1)
}
