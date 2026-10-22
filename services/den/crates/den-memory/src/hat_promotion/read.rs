use den_core::{ids::HatId, DenError};
use uuid::Uuid;

use super::{decode_candidate, HatHead, ReviewCandidate, ReviewCandidateSqlRow};

pub(super) async fn load_candidate<'e, E>(
    executor: E,
    bear_id: Uuid,
    memory_id: Uuid,
) -> Result<ReviewCandidate, DenError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let bear_id = bear_id.to_string();
    let memory_id = memory_id.to_string();
    let row = sqlx::query_as!(
        ReviewCandidateSqlRow,
        r#"
        SELECT m.memory_id AS "memory_id!", m.scope_source_kind AS "scope_source_kind!",
               m.scope_source_id AS "scope_source_id!", m.kind AS "kind!",
               m.content_text AS "content_text!", m.sequence_no AS "sequence_no!"
        FROM generic_memory_records m
        WHERE m.bear_id = ? AND m.scope_type = 'source_local' AND m.memory_id = ?
          AND m.visibility = 'normal' AND m.invalid_at IS NULL
          AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                AND newer.supersedes_memory_id = m.memory_id
          )
        "#,
        bear_id,
        memory_id,
    )
    .fetch_optional(executor)
    .await
    .map_err(|err| DenError::System(format!("load source-local review candidate: {err}")))?
    .ok_or_else(|| DenError::NotFound("reviewable source note not found".into()))?;
    decode_candidate(row)
}

// Resolve the physical head before reading a generic body. Filtering first can
// resurrect an older version, or let promotion overwrite a classified head.
pub(super) async fn load_hat_head(
    connection: &mut sqlx::SqliteConnection,
    bear_id: Uuid,
    hat_id: HatId,
    path: &str,
) -> Result<Option<HatHead>, DenError> {
    let bear_id = bear_id.to_string();
    let hat_id = hat_id.to_string();
    let head_id = sqlx::query_scalar!(
        r#"SELECT memory_id AS "memory_id!" FROM memory_records
           WHERE bear_id = ? AND scope_type = 'hat' AND scope_hat_id = ? AND logical_path = ?
           ORDER BY sequence_no DESC LIMIT 1"#,
        bear_id,
        hat_id,
        path,
    )
    .fetch_optional(&mut *connection)
    .await?;
    let Some(head_id) = head_id else {
        return Ok(None);
    };
    let head = sqlx::query!(
        r#"
        SELECT m.memory_id AS "memory_id!", m.content_text AS "content_text!"
        FROM generic_memory_records m
        WHERE m.bear_id = ? AND m.memory_id = ?
          AND m.visibility = 'normal' AND m.invalid_at IS NULL
          AND COALESCE(json_extract(m.metadata_json, '$.lifecycle.status'), 'active')
              NOT IN ('archived', 'archive-candidate')
          AND NOT EXISTS (
              SELECT 1 FROM memory_records newer WHERE newer.bear_id = m.bear_id
                AND newer.supersedes_memory_id = m.memory_id
          )
        "#,
        bear_id,
        head_id,
    )
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| {
        DenError::Authorization("hat target is not an active, generic-readable head".into())
    })?;
    Ok(Some(HatHead {
        memory_id: Uuid::parse_str(&head.memory_id)
            .map_err(|err| DenError::System(format!("invalid hat head ID: {err}")))?,
        content_text: head.content_text,
    }))
}
