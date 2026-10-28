use sqlx::SqliteConnection;

use super::{
    denied, AuthenticatedUserEntity, DenError, Expected, PersonalCandidateId,
    PersonalCaptureRevision, PersonalErasureId, PersonalErasureReceipt, PersonalFactId, Uuid,
};

pub(super) async fn recover(
    connection: &mut SqliteConnection,
    bear_id: &str,
    binding: &AuthenticatedUserEntity,
    request_id: Uuid,
    expected: Expected,
) -> Result<Option<(PersonalErasureId, i64)>, DenError> {
    let request_id = request_id.to_string();
    // Bear-wide lookup prevents another person's request ID from minting a new operation.
    // No candidate/fact body is selected, including for rejected recovery attempts.
    let row = sqlx::query!(
        r#"SELECT erasure_id, person_entity_id, sequence_no, expected_head_memory_id,
                  expected_latest_candidate_id, expected_capture_revision,
                  recorded_on_bear_id, actor_user_id AS "actor_user_id!: i32", authority_generation
           FROM personal_memory_erasures WHERE bear_id = ? AND request_id = ?"#,
        bear_id,
        request_id,
    )
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let original = Expected {
        head: row
            .expected_head_memory_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| denied())?
            .map(PersonalFactId),
        latest_candidate: row
            .expected_latest_candidate_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| denied())?
            .map(PersonalCandidateId),
        capture_revision: PersonalCaptureRevision::try_from(row.expected_capture_revision)?,
    };
    if row.person_entity_id != binding.entity_id()
        || Uuid::parse_str(&row.recorded_on_bear_id).map_err(|_| denied())?
            != binding.bear_id().as_uuid()
        || row.actor_user_id != binding.user_id().get()
        || row.authority_generation != binding.authority_generation
        || original != expected
    {
        return Err(denied());
    }
    Ok(Some((
        PersonalErasureId(Uuid::parse_str(&row.erasure_id).map_err(|_| denied())?),
        row.sequence_no,
    )))
}

pub(super) async fn record(
    connection: &mut SqliteConnection,
    binding: &AuthenticatedUserEntity,
    request_id: Uuid,
    expected: Expected,
    erasure_id: PersonalErasureId,
    boundary: i64,
    created_at: &str,
) -> Result<(), DenError> {
    let bear_id = binding.bear_id().to_string();
    let erasure_id = erasure_id.to_string();
    let request_id = request_id.to_string();
    let head = expected.head.map(|id| id.to_string());
    let candidate = expected.latest_candidate.map(|id| id.to_string());
    let capture_revision = expected.capture_revision.get();
    let person = binding.entity_id();
    let actor = binding.user_id().get();
    sqlx::query!(
        r"INSERT INTO personal_memory_erasures (
            erasure_id, bear_id, person_entity_id, request_id, sequence_no,
            expected_head_memory_id, expected_latest_candidate_id, expected_capture_revision,
            recorded_on_bear_id, actor_user_id, authority_generation, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        erasure_id,
        bear_id,
        person,
        request_id,
        boundary,
        head,
        candidate,
        capture_revision,
        bear_id,
        actor,
        binding.authority_generation,
        created_at,
    )
    .execute(connection)
    .await?;
    Ok(())
}

pub(super) async fn receipt(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person: &str,
    erasure_id: PersonalErasureId,
    boundary: i64,
) -> Result<PersonalErasureReceipt, DenError> {
    let ids = sqlx::query_scalar!(
        r"SELECT memory_id FROM personal_memory_publications
          WHERE bear_id = ? AND person_entity_id = ? AND fact_type = 'preferred_name'
            AND sequence_no < ? ORDER BY sequence_no, memory_id",
        bear_id,
        person,
        boundary,
    )
    .fetch_all(connection)
    .await?;
    let affected_fact_ids = ids
        .iter()
        .map(|id| {
            Uuid::parse_str(id)
                .map(PersonalFactId)
                .map_err(|_| denied())
        })
        .collect::<Result<_, _>>()?;
    Ok(PersonalErasureReceipt {
        erasure_id,
        affected_fact_ids,
    })
}
