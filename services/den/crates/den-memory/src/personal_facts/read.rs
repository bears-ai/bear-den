use den_core::DenError;
use sqlx::SqliteConnection;
use uuid::Uuid;

use super::{PersonalFactHead, PersonalFactId, PersonalFactState, PublicationReceipt};
use crate::{
    descriptors,
    personal_candidates::{ensure_current, PersonalCandidateId, PreferredName},
    AuthenticatedUserEntity, BearMemoryStore,
};

pub(super) fn invalid() -> DenError {
    DenError::Authorization("Personal-memory decision is stale or no longer authorized.".into())
}

pub(super) fn relation_ids() -> Result<(&'static str, &'static str), DenError> {
    let subject = descriptors::relation("subject").ok_or_else(invalid)?;
    let audience = descriptors::relation("audience").ok_or_else(invalid)?;
    Ok((subject.id, audience.id))
}

#[derive(sqlx::FromRow)]
pub(super) struct PublicationRow {
    pub memory_id: String,
    pub person_entity_id: String,
    pub candidate_id: String,
    pub previous_memory_id: Option<String>,
    pub retracted: bool,
    pub eligible: bool,
}

impl PublicationRow {
    pub fn receipt(&self) -> Result<PublicationReceipt, DenError> {
        Ok(PublicationReceipt {
            fact_id: PersonalFactId(Uuid::parse_str(&self.memory_id).map_err(|_| invalid())?),
            candidate_id: PersonalCandidateId(
                Uuid::parse_str(&self.candidate_id).map_err(|_| invalid())?,
            ),
            previous_fact_id: self
                .previous_memory_id
                .as_deref()
                .map(Uuid::parse_str)
                .transpose()
                .map_err(|_| invalid())?
                .map(PersonalFactId),
        })
    }
}

/// One shape covers current-head lookup, immutable receipt lookup, and classification checks.
/// No body is selected here. Ordering is the publication decision's sequence, so a missing
/// canonical head cannot make an older value current again.
pub(super) async fn publication(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person_entity_id: Option<&str>,
    memory_id: Option<&str>,
    candidate_id: Option<&str>,
) -> Result<Option<PublicationRow>, DenError> {
    let (subject, audience) = relation_ids()?;
    Ok(sqlx::query_as!(
        PublicationRow,
        r#"
        SELECT p.memory_id, p.person_entity_id, p.candidate_id, p.previous_memory_id,
            EXISTS (SELECT 1 FROM personal_memory_retractions t
                    WHERE t.bear_id = p.bear_id AND t.memory_id = p.memory_id) AS "retracted!: bool",
            (p.fact_type = 'preferred_name' AND r.memory_id IS NOT NULL
             AND r.visibility = 'normal' AND r.invalid_at IS NULL
             AND CASE WHEN json_valid(r.metadata_json)
                      THEN COALESCE(json_extract(r.metadata_json, '$.lifecycle.status'), 'active') = 'active'
                      ELSE 0 END
             AND NOT EXISTS (SELECT 1 FROM personal_memory_publications later
                 WHERE later.bear_id = p.bear_id AND later.person_entity_id = p.person_entity_id
                   AND later.sequence_no > p.sequence_no)
             AND NOT EXISTS (SELECT 1 FROM memory_records newer
                 WHERE newer.bear_id = p.bear_id AND newer.supersedes_memory_id = p.memory_id)
             AND EXISTS (SELECT 1 FROM memory_relations s
                 WHERE s.bear_id = p.bear_id AND s.src_memory_id = p.memory_id
                   AND s.entity_id = p.person_entity_id AND s.relation = ? AND s.state = 'active')
             AND EXISTS (SELECT 1 FROM memory_access_rules a
                 WHERE a.bear_id = p.bear_id AND a.src_memory_id = p.memory_id
                   AND a.entity_id = p.person_entity_id AND a.relation = ? AND a.state = 'active')
             AND NOT EXISTS (SELECT 1 FROM memory_access_rules a
                 WHERE a.bear_id = p.bear_id AND a.src_memory_id = p.memory_id AND a.state = 'active'
                   AND (a.entity_id <> p.person_entity_id OR a.relation <> ?))) AS "eligible!: bool"
        FROM personal_memory_publications p
        LEFT JOIN memory_records r ON r.bear_id = p.bear_id AND r.memory_id = p.memory_id
        WHERE p.bear_id = ? AND (? IS NULL OR p.person_entity_id = ?)
          AND (? IS NULL OR p.memory_id = ?) AND (? IS NULL OR p.candidate_id = ?)
        ORDER BY p.sequence_no DESC LIMIT 1
        "#,
        subject,
        audience,
        audience,
        bear_id,
        person_entity_id,
        person_entity_id,
        memory_id,
        memory_id,
        candidate_id,
        candidate_id,
    )
    .fetch_optional(connection)
    .await?)
}

pub async fn current_preferred_name(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
) -> Result<Option<PersonalFactHead>, DenError> {
    let mut tx = store.pool().begin().await?;
    ensure_current(&mut tx, store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let row = publication(&mut tx, &bear_id, Some(binding.entity_id()), None, None).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let receipt = row.receipt()?;
    let state = if row.retracted {
        PersonalFactState::Retracted
    } else if row.eligible {
        PersonalFactState::Active
    } else {
        PersonalFactState::Suppressed
    };
    let preferred_name = if state == PersonalFactState::Active {
        let value = sqlx::query_scalar!(
            "SELECT content_text FROM memory_records WHERE bear_id = ? AND memory_id = ?",
            bear_id,
            row.memory_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        let name = PreferredName::new(&value)?;
        if name.as_str() != value {
            return Err(invalid());
        }
        Some(name)
    } else {
        None
    };
    tx.commit().await?;
    Ok(Some(PersonalFactHead {
        fact_id: receipt.fact_id,
        candidate_id: receipt.candidate_id,
        state,
        preferred_name,
    }))
}

pub(crate) struct PublicationAccess {
    pub person_entity_id: String,
    pub eligible: bool,
}

pub(crate) async fn publication_access(
    store: &BearMemoryStore,
    memory_id: &str,
) -> Result<Option<PublicationAccess>, DenError> {
    let mut connection = store.pool().acquire().await?;
    Ok(publication(
        &mut connection,
        &store.bear_id().to_string(),
        None,
        Some(memory_id),
        None,
    )
    .await?
    .map(|row| PublicationAccess {
        person_entity_id: row.person_entity_id,
        eligible: row.eligible && !row.retracted,
    }))
}

pub(crate) async fn ensure_generic_mutation_allowed(
    store: &BearMemoryStore,
    memory_id: &str,
) -> Result<(), DenError> {
    let bear_id = store.bear_id().to_string();
    let protected = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM personal_memory_publications
           WHERE bear_id = ? AND memory_id = ?) AS "protected!: bool""#,
        bear_id,
        memory_id,
    )
    .fetch_one(store.pool())
    .await?;
    if protected {
        return Err(invalid());
    }
    Ok(())
}
