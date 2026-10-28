use den_core::DenError;
use sqlx::SqliteConnection;

use super::{
    authority::{ensure_current, write_transaction},
    CaptureSetting, PersonalCaptureRevision,
};
use crate::{clock::now_rfc3339, AuthenticatedUserEntity, BearMemoryStore};

pub(crate) async fn enabled(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person_entity_id: &str,
    authority_generation: i64,
) -> Result<bool, DenError> {
    Ok(
        setting_inside(connection, bear_id, person_entity_id, authority_generation)
            .await?
            .enabled,
    )
}

pub(super) async fn setting_inside(
    connection: &mut SqliteConnection,
    bear_id: &str,
    person_entity_id: &str,
    authority_generation: i64,
) -> Result<CaptureSetting, DenError> {
    // Do not filter by generation: old consent is disabled, but CAS must still see the
    // actual head revision rather than treating an existing setting as an absent row.
    let row = sqlx::query!(
        r#"SELECT capture_enabled AS "enabled!: i64", capture_revision AS "revision!: i64",
                  authority_generation
           FROM personal_memory_settings WHERE bear_id = ? AND person_entity_id = ?"#,
        bear_id,
        person_entity_id,
    )
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        return Ok(CaptureSetting {
            enabled: false,
            revision: PersonalCaptureRevision::try_from(0)?,
        });
    };
    let enabled = match row.enabled {
        0 => false,
        1 => true,
        _ => {
            return Err(DenError::Authorization(
                "Invalid personal-memory capture setting.".into(),
            ))
        }
    };
    Ok(CaptureSetting {
        enabled: enabled && row.authority_generation == authority_generation,
        revision: PersonalCaptureRevision::try_from(row.revision)?,
    })
}

pub async fn capture_enabled(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
) -> Result<bool, DenError> {
    Ok(capture_setting(store, binding).await?.enabled)
}

/// Read consent for the current binding, retaining the head revision even when its
/// consent belongs to an old identity generation. Missing settings are disabled at revision zero.
pub async fn capture_setting(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
) -> Result<CaptureSetting, DenError> {
    let mut tx = store.pool().begin().await?;
    ensure_current(&mut tx, store, binding).await?;
    let result = setting_inside(
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
/// Every successful write advances the revision, even when the enabled value is unchanged.
/// Management forms should use `set_capture_enabled_if_current`, not this trusted unconditional API.
pub async fn set_capture_enabled(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    capture_enabled: bool,
) -> Result<(), DenError> {
    set_capture(store, binding, capture_enabled, None).await
}

/// Apply an explicit owner setting only if its captured head is still current.
/// Comparison, revision allocation, consent, withdrawal and retraction share one write transaction.
/// A stale retry is rejected, including after a disable/reenable ABA cycle.
pub async fn set_capture_enabled_if_current(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    enabled: bool,
    expected_revision: PersonalCaptureRevision,
) -> Result<(), DenError> {
    set_capture(store, binding, enabled, Some(expected_revision)).await
}

async fn set_capture(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    capture_enabled: bool,
    expected_revision: Option<PersonalCaptureRevision>,
) -> Result<(), DenError> {
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let person_entity_id = binding.entity_id();
    if let Some(expected) = expected_revision {
        let current = setting_inside(
            &mut tx,
            &bear_id,
            person_entity_id,
            binding.authority_generation,
        )
        .await?;
        if current.revision != expected {
            return Err(super::authority::denied());
        }
    }
    let revision = sqlx::query_scalar!(
        r#"UPDATE bear_sequence SET next_sequence = next_sequence + 1 WHERE id = 1
           RETURNING next_sequence - 1 AS "revision!: i64""#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let actor = binding.user_id().get();
    let updated_at = now_rfc3339()?;
    sqlx::query!(
        r"
        INSERT INTO personal_memory_settings (
            bear_id, person_entity_id, capture_enabled, capture_revision, authority_generation, updated_at, updated_on_bear_id, updated_by_user_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT (bear_id, person_entity_id) DO UPDATE SET
            capture_enabled = excluded.capture_enabled, capture_revision = excluded.capture_revision,
            authority_generation = excluded.authority_generation,
            updated_at = excluded.updated_at,
            updated_on_bear_id = excluded.updated_on_bear_id, updated_by_user_id = excluded.updated_by_user_id
        ",
        bear_id,
        person_entity_id,
        capture_enabled,
        revision,
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
              AND NOT EXISTS (SELECT 1 FROM personal_memory_publications p
                  WHERE p.bear_id = personal_memory_candidates.bear_id
                    AND p.candidate_id = personal_memory_candidates.candidate_id)
            ",
            updated_at,
            bear_id,
            actor,
            bear_id,
            person_entity_id,
        )
        .execute(&mut *tx)
        .await?;
        crate::personal_facts::retract_for_capture_disable(
            &mut tx,
            &bear_id,
            person_entity_id,
            actor,
            binding.authority_generation,
            &updated_at,
        )
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
        r"UPDATE personal_memory_candidates SET state = 'quarantined'
          WHERE bear_id = ? AND person_entity_id = ? AND state = 'pending'
            AND NOT EXISTS (SELECT 1 FROM personal_memory_publications p
                WHERE p.bear_id = personal_memory_candidates.bear_id
                  AND p.candidate_id = personal_memory_candidates.candidate_id)",
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
        r"UPDATE personal_memory_candidates SET state = 'quarantined'
          WHERE bear_id = ? AND state = 'pending'
            AND NOT EXISTS (SELECT 1 FROM personal_memory_publications p
                WHERE p.bear_id = personal_memory_candidates.bear_id
                  AND p.candidate_id = personal_memory_candidates.candidate_id)",
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
