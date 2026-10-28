use sqlx::SqliteConnection;

use super::{denied, AuthenticatedUserEntity, DenError, Expected, PersonalCandidateId, Uuid};
use crate::{personal_candidates::setting_inside, personal_facts::publication};

pub(super) async fn ensure_reviewed(
    connection: &mut SqliteConnection,
    bear_id: &str,
    binding: &AuthenticatedUserEntity,
    expected: Expected,
) -> Result<(), DenError> {
    let person = binding.entity_id();
    let head = publication(connection, bear_id, Some(person), None, None)
        .await?
        .map(|row| row.receipt().map(|receipt| receipt.fact_id))
        .transpose()?;
    // Include every terminal state and erased row: candidate identity is a reviewed head,
    // not a pending-list filter. No body is needed to compare the owner's snapshot.
    let latest = sqlx::query_scalar!(
        r"SELECT candidate_id FROM personal_memory_candidates
          WHERE bear_id = ? AND person_entity_id = ? AND fact_type = 'preferred_name'
          ORDER BY sequence_no DESC LIMIT 1",
        bear_id,
        person,
    )
    .fetch_optional(&mut *connection)
    .await?
    .map(|id| {
        Uuid::parse_str(&id)
            .map(PersonalCandidateId)
            .map_err(|_| denied())
    })
    .transpose()?;
    let capture = setting_inside(connection, bear_id, person, binding.authority_generation).await?;
    if head != expected.head
        || latest != expected.latest_candidate
        || capture.revision != expected.capture_revision
    {
        return Err(denied());
    }
    Ok(())
}

pub(super) async fn erase_bodies(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person: &str,
    created_at: &str,
) -> Result<(), DenError> {
    // Do not delete the classification, version chain, ACL history or audit IDs. Empty is
    // canonical absence of a body, never a substitute PreferredName. The ledger supplies
    // permanent typed erasure on reads even if lifecycle fields are later damaged/restored.
    sqlx::query!(
        r"UPDATE memory_records SET content_text = '', invalid_at = COALESCE(invalid_at, ?)
          WHERE bear_id = ? AND memory_id IN (
              SELECT memory_id FROM personal_memory_publications
              WHERE bear_id = ? AND person_entity_id = ? AND fact_type = 'preferred_name'
          )",
        created_at,
        bear_id,
        bear_id,
        person,
    )
    .execute(&mut *connection)
    .await?;
    // Move all raw rows out of the pending-value uniqueness index before clearing bodies;
    // otherwise two published conversation requests could collide on their empty values.
    // Historical withdrawal/review/publication audit is retained rather than rewritten.
    sqlx::query!(
        r"UPDATE personal_memory_candidates SET state = 'withdrawn', value_text = ''
          WHERE bear_id = ? AND person_entity_id = ? AND fact_type = 'preferred_name'",
        bear_id,
        person,
    )
    .execute(connection)
    .await?;
    Ok(())
}
