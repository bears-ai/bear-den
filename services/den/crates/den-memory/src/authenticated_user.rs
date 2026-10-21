//! Local authenticated-user authority over the existing `person` entity type (ADR-0042).
//!
//! Only `authenticated_user_entities` establishes this binding and permanently classifies
//! user identities, including quarantined imports. Portable canonical refs, display names,
//! and even asserted `den_user` handles are not authentication authority.

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use uuid::Uuid;

use crate::{clock::now_rfc3339, BearMemoryStore, EntityRow, EntityTrust, ResolutionState};

/// Permanent classification of a user entity, independent of its portable entity lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UserEntityAuthority {
    Active(UserId),
    Quarantined,
}

/// An immutable person binding established from a trusted, authenticated Den user id.
///
/// Construction is private so callers cannot turn arbitrary entity rows into authority.
/// The entity is a validated resolution-time snapshot; access checks must consult current
/// authority rather than treating a cached snapshot as an irrevocable grant.
#[derive(Clone, Debug)]
pub struct AuthenticatedUserEntity {
    bear_id: BearId,
    user_id: UserId,
    entity: EntityRow,
    pub(crate) authority_generation: i64,
}

impl AuthenticatedUserEntity {
    pub fn bear_id(&self) -> BearId {
        self.bear_id
    }

    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn entity(&self) -> &EntityRow {
        &self.entity
    }

    pub fn entity_id(&self) -> &str {
        &self.entity.entity_id
    }
}

/// Resolve a trusted authenticated user id to its immutable, bear-local person entity.
///
/// The caller must obtain `user_id` from authentication, never from model output or
/// user-supplied identity claims. No historical entity or handle is adopted. A new binding,
/// confirmed/asserted person, and strong `den_user` handle are committed atomically.
pub async fn resolve_authenticated_user_entity(
    store: &BearMemoryStore,
    user_id: UserId,
) -> Result<AuthenticatedUserEntity, DenError> {
    let bear_id = store.bear_id().to_string();
    let numeric_user_id = user_id.get();
    let canonical_ref = format!("den:user:{user_id}");
    let proposed_entity_id = Uuid::new_v4().to_string();
    let mut tx = store.pool().begin().await.map_err(|e| {
        DenError::System(format!("begin authenticated user resolution failed: {e}"))
    })?;

    // Write before reading: SQLite serializes contenders here, including across pools,
    // without a deferred read transaction needing an unsafe read-to-write lock upgrade.
    let inserted = sqlx::query!(
        r"
        INSERT INTO authenticated_user_entities (bear_id, user_id, entity_id, state)
        VALUES (?, ?, ?, 'active')
        ON CONFLICT (bear_id, user_id) WHERE state = 'active' DO NOTHING
        ",
        bear_id,
        numeric_user_id,
        proposed_entity_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| DenError::System(format!("bind authenticated user entity failed: {e}")))?
    .rows_affected();

    if inserted == 1 {
        sqlx::query!(
            "INSERT INTO user_identity_generations (bear_id, entity_id, generation) VALUES (?, ?, 1)",
            bear_id,
            proposed_entity_id,
        )
        .execute(&mut *tx)
        .await?;
        let sequence_no = sqlx::query_scalar!(
            r#"
            UPDATE bear_sequence SET next_sequence = next_sequence + 1 WHERE id = 1
            RETURNING next_sequence - 1 AS "sequence_no!: i64"
            "#,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            DenError::System(format!("allocate authenticated user sequence failed: {e}"))
        })?;
        let handle_id = Uuid::new_v4().to_string();
        let handle_value = user_id.to_string();
        let created_at = now_rfc3339()?;
        sqlx::query!(
            r"
            INSERT INTO entities (
                entity_id, bear_id, sequence_no, type, display_name, resolution, trust,
                canonical_ref, superseded_by_entity_id, metadata_json, created_at
            ) VALUES (?, ?, ?, 'person', NULL, 'confirmed', 'asserted', ?, NULL, '{}', ?)
            ",
            proposed_entity_id,
            bear_id,
            sequence_no,
            canonical_ref,
            created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| DenError::System(format!("create authenticated user person failed: {e}")))?;
        sqlx::query!(
            r"
            INSERT INTO entity_handles (
                handle_id, bear_id, entity_id, handle_type, handle_value, source,
                trust, state, created_at
            ) VALUES (?, ?, ?, 'den_user', ?, 'authentication', 'asserted', 'active', ?)
            ",
            handle_id,
            bear_id,
            proposed_entity_id,
            handle_value,
            created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| DenError::System(format!("create authenticated user handle failed: {e}")))?;
    }

    // Validate and load the same transactional snapshot. Never repair a corrupt binding,
    // follow its merge pointer, or substitute an entity based on a matching portable ref.
    let row = sqlx::query!(
        r#"
        SELECT e.entity_id AS "entity_id!: String", e.sequence_no AS "sequence_no!: i64",
               g.generation AS "generation!: i64",
               e.display_name AS "display_name?: String",
               e.canonical_ref AS "canonical_ref?: String",
               e.metadata_json AS "metadata_json!: String", e.created_at AS "created_at!: String"
        FROM authenticated_user_entities AS a
        INNER JOIN entities AS e ON e.bear_id = a.bear_id AND e.entity_id = a.entity_id
        INNER JOIN user_identity_generations AS g
            ON g.bear_id = a.bear_id AND g.entity_id = a.entity_id
        WHERE a.bear_id = ? AND a.user_id = ? AND a.state = 'active'
          AND e.type = 'person' AND e.resolution = 'confirmed' AND e.trust = 'asserted'
          AND e.superseded_by_entity_id IS NULL AND e.canonical_ref = ?
        "#,
        bear_id,
        numeric_user_id,
        canonical_ref,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| DenError::System(format!("read authenticated user binding failed: {e}")))?
    .ok_or_else(|| {
        DenError::System(format!(
            "authenticated user {user_id} binding has a missing or invalid person entity"
        ))
    })?;
    let entity = EntityRow {
        entity_id: row.entity_id,
        sequence_no: row.sequence_no,
        entity_type: "person".to_string(),
        display_name: row.display_name,
        resolution: ResolutionState::Confirmed,
        trust: EntityTrust::Asserted,
        canonical_ref: row.canonical_ref,
        superseded_by_entity_id: None,
        metadata_json: serde_json::from_str(&row.metadata_json)
            .unwrap_or_else(|_| serde_json::json!({})),
        created_at: row.created_at,
    };
    tx.commit().await.map_err(|e| {
        DenError::System(format!("commit authenticated user resolution failed: {e}"))
    })?;
    Ok(AuthenticatedUserEntity {
        bear_id: store.bear_id().into(),
        user_id,
        entity,
        authority_generation: row.generation,
    })
}

/// Look up permanent user classification by exact entity id, including quarantined imports.
/// Callers must not treat a quarantined user identity as a generic entity.
pub(crate) async fn user_entity_authority(
    store: &BearMemoryStore,
    entity_id: &str,
) -> Result<Option<UserEntityAuthority>, DenError> {
    let bear_id = store.bear_id().to_string();
    let row = sqlx::query!(
        r#"
        SELECT user_id AS "user_id!: i32", state FROM authenticated_user_entities
        WHERE bear_id = ? AND entity_id = ?
        "#,
        bear_id,
        entity_id,
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("look up authenticated user authority failed: {e}")))?;
    row.map(|row| match row.state.as_str() {
        "active" => Ok(UserEntityAuthority::Active(UserId::new(row.user_id))),
        "quarantined" => Ok(UserEntityAuthority::Quarantined),
        _ => Err(DenError::System(
            "authenticated user entity has invalid authority state".to_string(),
        )),
    })
    .transpose()
}

/// Test-only projection of active authority; production callers must distinguish quarantine.
#[cfg(test)]
pub(crate) async fn authenticated_user_for_entity(
    store: &BearMemoryStore,
    entity_id: &str,
) -> Result<Option<UserId>, DenError> {
    Ok(match user_entity_authority(store, entity_id).await? {
        Some(UserEntityAuthority::Active(user_id)) => Some(user_id),
        Some(UserEntityAuthority::Quarantined) | None => None,
    })
}

/// Reject ordinary mutations of user entities, including quarantined identities forever.
pub(crate) async fn ensure_entity_mutable(
    store: &BearMemoryStore,
    entity_id: &str,
) -> Result<(), DenError> {
    if user_entity_authority(store, entity_id).await?.is_some() {
        return Err(DenError::ValidationError(
            "authenticated user entities are immutable".to_string(),
        ));
    }
    Ok(())
}

/// Reject mutations of any user entity handle, including detached/quarantined handles.
/// The argument is a handle id, not a handle value or entity id.
pub(crate) async fn ensure_handle_mutable(
    store: &BearMemoryStore,
    handle_id: &str,
) -> Result<(), DenError> {
    let bear_id = store.bear_id().to_string();
    let bound_entity = sqlx::query_scalar!(
        r"
        SELECT h.entity_id FROM entity_handles AS h
        INNER JOIN authenticated_user_entities AS a
            ON a.bear_id = h.bear_id AND a.entity_id = h.entity_id
        WHERE h.bear_id = ? AND h.handle_id = ?
        ",
        bear_id,
        handle_id,
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|e| {
        DenError::System(format!(
            "look up authenticated user handle authority failed: {e}"
        ))
    })?;
    if bound_entity.is_some() {
        return Err(DenError::ValidationError(
            "authenticated user entity handles are immutable".to_string(),
        ));
    }
    Ok(())
}

/// Revoke imported authentication authority without losing permanent user classification.
///
/// **Trusted import boundary only:** call on a newly imported database, before existing
/// entity quarantine and before resolving any authenticated users. Source-host numeric user
/// ids cannot establish destination-host authority. This is not a normal lifecycle/reset
/// operation. In one transaction, retain quarantined markers, demote nonterminal entities,
/// clear canonical refs, lower trust, and detach their handles. Subsequent resolution creates
/// a fresh person unless a human-authorized reconnection has established a new binding.
/// Imported audiences stay fail-closed until that dedicated operation succeeds.
///
/// Returns every marked entity id for this Bear, including already quarantined identities.
/// The import loop must skip these permanently immutable rows during generic quarantine.
pub async fn quarantine_imported_authenticated_users(
    store: &BearMemoryStore,
) -> Result<Vec<String>, DenError> {
    let bear_id = store.bear_id().to_string();
    let mut tx = store
        .pool()
        .begin()
        .await
        .map_err(|e| DenError::System(format!("begin imported user quarantine failed: {e}")))?;
    // Write before the snapshot read to serialize with resolution across independent pools.
    // Every marker remains present, so revoked identities can never become generic entities.
    sqlx::query!(
        r"
        INSERT INTO user_identity_generations (bear_id, entity_id, generation)
        SELECT bear_id, entity_id, 1 FROM authenticated_user_entities WHERE bear_id = ?
        ON CONFLICT (bear_id, entity_id) DO UPDATE SET generation = generation + 1
        ",
        bear_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE authenticated_user_entities SET state = 'quarantined' WHERE bear_id = ?",
        bear_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| DenError::System(format!("quarantine imported user authority failed: {e}")))?;
    let entity_ids = sqlx::query_scalar!(
        "SELECT entity_id FROM authenticated_user_entities WHERE bear_id = ? ORDER BY entity_id",
        bear_id,
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| DenError::System(format!("list imported user identities failed: {e}")))?;
    sqlx::query!(
        r"
        UPDATE entities
        SET resolution = CASE WHEN resolution IN ('observed', 'provisional', 'resolved', 'confirmed')
                              THEN 'provisional' ELSE resolution END,
            canonical_ref = NULL, trust = 'inferred'
        WHERE bear_id = ? AND entity_id IN (
            SELECT entity_id FROM authenticated_user_entities WHERE bear_id = ?
        )
        ",
        bear_id,
        bear_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| DenError::System(format!("demote imported user entities failed: {e}")))?;
    sqlx::query!(
        r"
        UPDATE entity_handles SET state = 'detached'
        WHERE bear_id = ? AND entity_id IN (
            SELECT entity_id FROM authenticated_user_entities WHERE bear_id = ?
        )
        ",
        bear_id,
        bear_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| DenError::System(format!("detach imported user handles failed: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| DenError::System(format!("commit imported user quarantine failed: {e}")))?;
    Ok(entity_ids)
}

#[cfg(test)]
mod tests;
