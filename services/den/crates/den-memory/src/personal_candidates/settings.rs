use den_core::DenError;
use sqlx::SqliteConnection;

use super::authority::{ensure_current, write_transaction};
use crate::{clock::now_rfc3339, AuthenticatedUserEntity, BearMemoryStore};

pub(super) async fn enabled(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person_entity_id: &str,
    authority_generation: i64,
) -> Result<bool, DenError> {
    let value = sqlx::query_scalar!(
        r#"SELECT capture_enabled AS "enabled!: i64" FROM personal_memory_settings
           WHERE bear_id = ? AND person_entity_id = ? AND authority_generation = ?"#,
        bear_id,
        person_entity_id,
        authority_generation,
    )
    .fetch_optional(connection)
    .await?;
    match value {
        None | Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(_) => Err(DenError::Authorization(
            "Invalid personal-memory capture setting.".into(),
        )),
    }
}

pub async fn capture_enabled(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
) -> Result<bool, DenError> {
    let mut tx = store.pool().begin().await?;
    ensure_current(&mut tx, store, binding).await?;
    let result = enabled(
        &mut tx,
        &store.bear_id().to_string(),
        binding.entity_id(),
        binding.authority_generation,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

/// Trusted human-settings boundary only, never a model-selected consent flag. Disabling
/// capture atomically withdraws every pending candidate; reenabling never resurrects one.
pub async fn set_capture_enabled(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    capture_enabled: bool,
) -> Result<(), DenError> {
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let person_entity_id = binding.entity_id();
    let actor = binding.user_id().get();
    let updated_at = now_rfc3339()?;
    sqlx::query!(
        r"
        INSERT INTO personal_memory_settings (
            bear_id, person_entity_id, capture_enabled, authority_generation, updated_at, updated_on_bear_id, updated_by_user_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT (bear_id, person_entity_id) DO UPDATE SET
            capture_enabled = excluded.capture_enabled, authority_generation = excluded.authority_generation,
            updated_at = excluded.updated_at,
            updated_on_bear_id = excluded.updated_on_bear_id, updated_by_user_id = excluded.updated_by_user_id
        ",
        bear_id,
        person_entity_id,
        capture_enabled,
        binding.authority_generation,
        updated_at,
        bear_id,
        actor,
    )
    .execute(&mut *tx)
    .await?;
    if !capture_enabled {
        sqlx::query!(
            r"
            UPDATE personal_memory_candidates SET state = 'withdrawn', withdrawn_at = ?,
                withdrawn_on_bear_id = ?, withdrawn_by_user_id = ?
            WHERE bear_id = ? AND person_entity_id = ? AND state IN ('pending', 'quarantined')
            ",
            updated_at,
            bear_id,
            actor,
            bear_id,
            person_entity_id,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Parked identity changes cannot leave old pending requests eligible for future curation.
pub(crate) async fn quarantine_person(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person_entity_id: &str,
) -> Result<(), DenError> {
    sqlx::query!(
        "UPDATE personal_memory_candidates SET state = 'quarantined' WHERE bear_id = ? AND person_entity_id = ? AND state = 'pending'",
        bear_id,
        person_entity_id,
    )
    .execute(connection)
    .await?;
    Ok(())
}

/// Called inside the same import transaction as source authority quarantine.
/// Imported consent is not destination consent; pending content requires fresh review.
pub(crate) async fn quarantine_imported(
    connection: &mut SqliteConnection,
    bear_id: &str,
) -> Result<(), DenError> {
    let updated_at = now_rfc3339()?;
    sqlx::query!(
        "UPDATE personal_memory_candidates SET state = 'quarantined' WHERE bear_id = ? AND state = 'pending'",
        bear_id,
    )
    .execute(&mut *connection)
    .await?;
    sqlx::query!(
        r"
        UPDATE personal_memory_settings SET capture_enabled = 0, updated_at = ?,
            updated_on_bear_id = ?, updated_by_user_id = NULL WHERE bear_id = ?
        ",
        updated_at,
        bear_id,
        bear_id,
    )
    .execute(connection)
    .await?;
    Ok(())
}
