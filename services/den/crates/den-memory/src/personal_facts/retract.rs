use den_core::DenError;
use sqlx::SqliteConnection;
use uuid::Uuid;

use super::{
    read::{invalid, publication, PublicationRow},
    PersonalFactId,
};
use crate::{
    clock::now_rfc3339, personal_candidates::write_transaction, AuthenticatedUserEntity,
    BearMemoryStore,
};

pub(super) enum RuleDisposition {
    Superseded,
    Withdrawn,
}

pub(super) async fn invalidate(
    connection: &mut SqliteConnection,
    bear_id: &str,
    memory_id: &str,
    created_at: &str,
    disposition: RuleDisposition,
) -> Result<(), DenError> {
    sqlx::query!(
        "UPDATE memory_records SET invalid_at = COALESCE(invalid_at, ?) WHERE bear_id = ? AND memory_id = ?",
        created_at,
        bear_id,
        memory_id,
    ).execute(&mut *connection).await?;
    let state = match disposition {
        RuleDisposition::Superseded => "superseded",
        RuleDisposition::Withdrawn => "withdrawn",
    };
    sqlx::query!(
        "UPDATE memory_access_rules SET state = ? WHERE bear_id = ? AND src_memory_id = ? AND state = 'active'",
        state,
        bear_id,
        memory_id,
    ).execute(connection).await?;
    Ok(())
}

enum RetractionReason {
    OwnerRequest,
    CaptureDisabled,
}

impl RetractionReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::OwnerRequest => "owner_request",
            Self::CaptureDisabled => "capture_disabled",
        }
    }
}

async fn retract_inside(
    connection: &mut SqliteConnection,
    bear_id: &str,
    row: PublicationRow,
    actor: i32,
    authority_generation: i64,
    reason: RetractionReason,
    created_at: &str,
) -> Result<bool, DenError> {
    if row.retracted {
        return Ok(false);
    }
    invalidate(
        connection,
        bear_id,
        &row.memory_id,
        created_at,
        RuleDisposition::Withdrawn,
    )
    .await?;
    let decision_id = Uuid::new_v4().to_string();
    let reason = reason.as_str();
    sqlx::query!(
        r"
        INSERT INTO personal_memory_retractions (
            memory_id, bear_id, decision_id, recorded_on_bear_id, actor_user_id,
            authority_generation, reason, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ",
        row.memory_id,
        bear_id,
        decision_id,
        bear_id,
        actor,
        authority_generation,
        reason,
        created_at,
    )
    .execute(connection)
    .await?;
    Ok(true)
}

/// Owner-requested removal from active reuse. This retains protected evidence; it is not
/// transcript erasure, index cleanup, or cancellation of an already-produced model context.
pub async fn retract_preferred_name(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    expected: PersonalFactId,
) -> Result<bool, DenError> {
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let row = publication(&mut tx, &bear_id, Some(binding.entity_id()), None, None)
        .await?
        .ok_or_else(invalid)?;
    if row.receipt()?.fact_id != expected {
        return Err(invalid());
    }
    let changed = retract_inside(
        &mut tx,
        &bear_id,
        row,
        binding.user_id().get(),
        binding.authority_generation,
        RetractionReason::OwnerRequest,
        &now_rfc3339()?,
    )
    .await?;
    tx.commit().await?;
    Ok(changed)
}

pub(crate) async fn retract_for_capture_disable(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person_entity_id: &str,
    actor: i32,
    authority_generation: i64,
    created_at: &str,
) -> Result<(), DenError> {
    if let Some(row) = publication(connection, bear_id, Some(person_entity_id), None, None).await? {
        retract_inside(
            connection,
            bear_id,
            row,
            actor,
            authority_generation,
            RetractionReason::CaptureDisabled,
            created_at,
        )
        .await?;
    }
    Ok(())
}
