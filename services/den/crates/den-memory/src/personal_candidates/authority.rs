use den_core::DenError;
use sqlx::{Sqlite, SqliteConnection, Transaction};

use crate::{AuthenticatedUserEntity, BearMemoryStore};

pub(super) fn denied() -> DenError {
    DenError::Authorization("Current authenticated personal-memory identity required.".into())
}

pub(crate) async fn ensure_current(
    connection: &mut SqliteConnection,
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
) -> Result<(), DenError> {
    if binding.bear_id().as_uuid() != store.bear_id() || binding.user_id().get() <= 0 {
        return Err(denied());
    }
    let bear_id = store.bear_id().to_string();
    let user_id = binding.user_id().get();
    let entity_id = binding.entity_id();
    let canonical_ref = format!("den:user:{}", binding.user_id());
    let current = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM authenticated_user_entities a
            JOIN user_identity_generations g ON g.bear_id = a.bear_id AND g.entity_id = a.entity_id
            JOIN entities e ON e.bear_id = a.bear_id AND e.entity_id = a.entity_id
            WHERE a.bear_id = ? AND a.entity_id = ? AND a.user_id = ? AND a.state = 'active'
              AND g.generation = ? AND g.generation > 0 AND e.type = 'person'
              AND e.resolution = 'confirmed' AND e.trust = 'asserted' AND e.canonical_ref = ?
              AND e.superseded_by_entity_id IS NULL
        ) AS "current!: bool"
        "#,
        bear_id,
        entity_id,
        user_id,
        binding.authority_generation,
        canonical_ref,
    )
    .fetch_one(connection)
    .await?;
    if !current {
        return Err(denied());
    }
    Ok(())
}

pub(crate) async fn write_transaction(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
) -> Result<Transaction<'static, Sqlite>, DenError> {
    let mut tx = store.pool().begin().await?;
    // Serialize before reading authority, including writers using independent pools.
    sqlx::query!("UPDATE bear_sequence SET next_sequence = next_sequence WHERE id = 1")
        .execute(&mut *tx)
        .await?;
    ensure_current(&mut tx, store, binding).await?;
    Ok(tx)
}
