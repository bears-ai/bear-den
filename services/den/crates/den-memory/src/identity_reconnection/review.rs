use den_core::{ids::UserId, DenError};
use sqlx::SqliteConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    origins::origins_for_identity,
    types::{AuthorityState, IdentityReconnectionReview, IdentitySnapshot, UserIdentitySummary},
};
use crate::{BearMemoryStore, EntityTrust, ResolutionState};

pub(super) fn stale_review() -> DenError {
    DenError::ValidationError(
        "Identity mapping changed, expired, or was already confirmed. Start a new identity review."
            .into(),
    )
}

pub(super) async fn snapshot(
    connection: &mut SqliteConnection,
    bear_id: &str,
    entity_id: &str,
) -> Result<IdentitySnapshot, DenError> {
    let row = sqlx::query!(
        r#"
        SELECT a.user_id AS "user_id!: i32", a.state,
               g.generation AS "generation!: i64", e.type AS entity_type,
               e.resolution, e.trust, e.canonical_ref, e.superseded_by_entity_id, e.display_name
        FROM authenticated_user_entities a
        JOIN user_identity_generations g ON g.bear_id = a.bear_id AND g.entity_id = a.entity_id
        JOIN entities e ON e.bear_id = a.bear_id AND e.entity_id = a.entity_id
        WHERE a.bear_id = ? AND a.entity_id = ?
        "#,
        bear_id,
        entity_id,
    )
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(stale_review)?;
    let resolution = ResolutionState::parse(&row.resolution).ok_or_else(stale_review)?;
    let trust = EntityTrust::parse(&row.trust).ok_or_else(stale_review)?;
    if row.entity_type != "person"
        || !resolution.is_live()
        || row.superseded_by_entity_id.is_some()
        || row.generation <= 0
        || row.generation == i64::MAX
    {
        return Err(stale_review());
    }
    let user_id = UserId::new(row.user_id);
    let state = match row.state.as_str() {
        "active"
            if resolution == ResolutionState::Confirmed
                && trust == EntityTrust::Asserted
                && row.canonical_ref.as_deref() == Some(&format!("den:user:{user_id}")) =>
        {
            AuthorityState::Active
        }
        "quarantined" => AuthorityState::Quarantined,
        _ => return Err(stale_review()),
    };
    Ok(IdentitySnapshot {
        entity_id: entity_id.into(),
        user_id,
        state,
        generation: row.generation,
        display_name: row.display_name,
        origins: origins_for_identity(connection, bear_id, entity_id).await?,
    })
}

pub(super) async fn destination_snapshot(
    connection: &mut SqliteConnection,
    bear_id: &str,
    user_id: UserId,
) -> Result<Option<IdentitySnapshot>, DenError> {
    let user_id_value = user_id.get();
    let entity_id = sqlx::query_scalar!(
        "SELECT entity_id FROM authenticated_user_entities WHERE bear_id = ? AND user_id = ? AND state = 'active'",
        bear_id,
        user_id_value,
    )
    .fetch_optional(&mut *connection)
    .await?;
    match entity_id {
        Some(entity_id) => snapshot(connection, bear_id, &entity_id).await.map(Some),
        None => Ok(None),
    }
}

/// Trusted management inspection: source labels are not proof of a destination identity.
pub async fn list_identities(
    store: &BearMemoryStore,
) -> Result<Vec<UserIdentitySummary>, DenError> {
    let bear_id = store.bear_id().to_string();
    let mut tx = store.pool().begin().await?;
    let rows = sqlx::query!(
        r#"
        SELECT a.entity_id, a.user_id AS "user_id!: i32", a.state, e.display_name,
               g.generation AS "generation!: i64"
        FROM authenticated_user_entities a
        JOIN entities e ON e.bear_id = a.bear_id AND e.entity_id = a.entity_id
        JOIN user_identity_generations g ON g.bear_id = a.bear_id AND g.entity_id = a.entity_id
        WHERE a.bear_id = ? ORDER BY e.sequence_no, a.entity_id
        "#,
        bear_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut identities = Vec::with_capacity(rows.len());
    for row in rows {
        let origins = origins_for_identity(&mut tx, &bear_id, &row.entity_id).await?;
        let display_name = row.display_name.or_else(|| {
            origins.iter().rev().find_map(|origin| {
                origin
                    .display_name
                    .clone()
                    .or_else(|| origin.username.clone())
            })
        });
        let active_user_id = match row.state.as_str() {
            "active" => Some(UserId::new(row.user_id)),
            "quarantined" => None,
            _ => return Err(stale_review()),
        };
        identities.push(UserIdentitySummary {
            entity_id: row.entity_id,
            display_name,
            active_user_id,
            binding_version: row.generation,
            origins,
        });
    }
    tx.commit().await?;
    Ok(identities)
}

/// Prepare a quarantined person's explicit attachment to a verified destination account.
/// The trusted caller authorizes both principals; neither source IDs nor names are matched.
pub async fn review_reconnection(
    store: &BearMemoryStore,
    actor: UserId,
    entity_id: &str,
    destination: UserId,
) -> Result<IdentityReconnectionReview, DenError> {
    make_review(store, actor, entity_id, Some(destination)).await
}

pub async fn review_revocation(
    store: &BearMemoryStore,
    actor: UserId,
    entity_id: &str,
) -> Result<IdentityReconnectionReview, DenError> {
    make_review(store, actor, entity_id, None).await
}

async fn make_review(
    store: &BearMemoryStore,
    actor: UserId,
    entity_id: &str,
    destination: Option<UserId>,
) -> Result<IdentityReconnectionReview, DenError> {
    if actor.get() <= 0 || destination.is_some_and(|user| user.get() <= 0) {
        return Err(stale_review());
    }
    let bear_id = store.bear_id().to_string();
    let mut tx = store.pool().begin().await?;
    let selected = snapshot(&mut tx, &bear_id, entity_id).await?;
    let expected_state = if destination.is_some() {
        AuthorityState::Quarantined
    } else {
        AuthorityState::Active
    };
    if destination.is_some() && selected.generation >= i64::MAX - 1 {
        // Activation must leave a valid next epoch for revocation, even for hostile imports.
        return Err(stale_review());
    }
    if selected.state != expected_state {
        return Err(DenError::ValidationError(
            "Reconnect only a parked identity. Revoke an existing mapping in a separate reviewed operation before changing its account."
                .into(),
        ));
    }
    let displaced = match destination {
        Some(user_id) => destination_snapshot(&mut tx, &bear_id, user_id).await?,
        None => None,
    };
    tx.commit().await?;
    Ok(IdentityReconnectionReview {
        id: Uuid::new_v4(),
        bear_id: store.bear_id().into(),
        actor,
        destination,
        selected,
        displaced,
        expires_at: OffsetDateTime::now_utc().unix_timestamp() + 15 * 60,
    })
}
