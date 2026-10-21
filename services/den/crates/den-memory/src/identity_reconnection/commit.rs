use den_core::{ids::UserId, DenError};
use sqlx::SqliteConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    review::{destination_snapshot, snapshot, stale_review},
    types::{
        AuthorityState, IdentityReconnectionReceipt, IdentityReconnectionReview, ReplacementConsent,
    },
};
use crate::{clock::now_rfc3339, BearMemoryStore};

/// Trusted human restore boundary only: the caller holds fresh actor/target eligibility
/// locks through completion. Atomically changes attachment, derived resolution/handles,
/// revocation epochs, and an append-only receipt. Never changes memory or relation rows.
pub async fn commit_review(
    store: &BearMemoryStore,
    review: &IdentityReconnectionReview,
    consent: ReplacementConsent,
) -> Result<IdentityReconnectionReceipt, DenError> {
    if review.bear_id != store.bear_id().into()
        || review.expires_at <= OffsetDateTime::now_utc().unix_timestamp()
        || review.expires_at > OffsetDateTime::now_utc().unix_timestamp() + 15 * 60
    {
        return Err(stale_review());
    }
    if review.displaced.is_some() && matches!(consent, ReplacementConsent::NoReplacement) {
        return Err(DenError::ValidationError(
            "Explicitly confirm parking the destination's existing identity with all records preserved. No facts will be merged or discarded."
                .into(),
        ));
    }
    let bear_id = store.bear_id().to_string();
    let mut tx = store.pool().begin().await?;
    // Write before any reads: contenders serialize across independent SQLite pools.
    sqlx::query!("UPDATE bear_sequence SET next_sequence = next_sequence WHERE id = 1")
        .execute(&mut *tx)
        .await?;
    let selected = snapshot(&mut tx, &bear_id, review.entity_id()).await?;
    if selected != review.selected {
        return Err(stale_review());
    }
    if let Some(destination) = review.destination {
        if selected.state != AuthorityState::Quarantined
            || selected.generation >= i64::MAX - 1
            || destination_snapshot(&mut tx, &bear_id, destination).await? != review.displaced
        {
            return Err(stale_review());
        }
    } else if selected.state != AuthorityState::Active || review.displaced.is_some() {
        return Err(stale_review());
    }
    let receipt_id = review.id.to_string();
    let consumed = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM user_identity_receipts WHERE receipt_id = ?) AS "consumed!: bool""#,
        receipt_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    if consumed || review.expires_at <= OffsetDateTime::now_utc().unix_timestamp() {
        return Err(stale_review());
    }
    if let Some(displaced) = &review.displaced {
        park(&mut tx, &bear_id, &displaced.entity_id).await?;
    }
    let recorded_at = now_rfc3339()?;
    match review.destination {
        Some(destination) => {
            activate(
                &mut tx,
                &bear_id,
                review.entity_id(),
                destination,
                &recorded_at,
            )
            .await?;
        }
        None => park(&mut tx, &bear_id, review.entity_id()).await?,
    }
    let actor = review.actor.get();
    let destination = review.destination.map(UserId::get);
    let previous_user_id = review.selected.user_id.get();
    let displaced_entity_id = review.displaced_entity_id();
    let resulting_generation = selected.generation + 1;
    sqlx::query!(
        r"
        INSERT INTO user_identity_receipts (
            receipt_id, bear_id, recorded_bear_id, actor_user_id, entity_id,
            destination_user_id, previous_user_id, displaced_entity_id,
            prior_generation, resulting_generation, recorded_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ",
        receipt_id,
        bear_id,
        bear_id,
        actor,
        selected.entity_id,
        destination,
        previous_user_id,
        displaced_entity_id,
        selected.generation,
        resulting_generation,
        recorded_at,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(IdentityReconnectionReceipt {
        receipt_id: review.id,
        recorded_bear_id: bear_id,
        actor_user_id: review.actor,
        previous_user_id: selected.user_id,
        prior_generation: selected.generation,
        resulting_generation,
        entity_id: selected.entity_id,
        destination_user_id: review.destination,
        displaced_entity_id: displaced_entity_id.map(str::to_owned),
        recorded_at,
    })
}

async fn bump_generation(
    connection: &mut SqliteConnection,
    bear_id: &str,
    entity_id: &str,
) -> Result<(), DenError> {
    let changed = sqlx::query!(
        "UPDATE user_identity_generations SET generation = generation + 1 WHERE bear_id = ? AND entity_id = ? AND generation < 9223372036854775807",
        bear_id,
        entity_id,
    )
    .execute(connection)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(stale_review());
    }
    Ok(())
}

async fn detach_handles(
    connection: &mut SqliteConnection,
    bear_id: &str,
    entity_id: &str,
) -> Result<(), DenError> {
    sqlx::query!(
        "UPDATE entity_handles SET state = 'detached' WHERE bear_id = ? AND entity_id = ?",
        bear_id,
        entity_id,
    )
    .execute(connection)
    .await?;
    Ok(())
}

async fn park(
    connection: &mut SqliteConnection,
    bear_id: &str,
    entity_id: &str,
) -> Result<(), DenError> {
    bump_generation(connection, bear_id, entity_id).await?;
    sqlx::query!(
        "UPDATE authenticated_user_entities SET state = 'quarantined' WHERE bear_id = ? AND entity_id = ?",
        bear_id,
        entity_id,
    )
    .execute(&mut *connection)
    .await?;
    sqlx::query!(
        "UPDATE entities SET resolution = 'provisional', trust = 'inferred', canonical_ref = NULL WHERE bear_id = ? AND entity_id = ?",
        bear_id,
        entity_id,
    )
    .execute(&mut *connection)
    .await?;
    detach_handles(connection, bear_id, entity_id).await?;
    crate::personal_candidates::quarantine_person(connection, bear_id, entity_id).await
}

async fn activate(
    connection: &mut SqliteConnection,
    bear_id: &str,
    entity_id: &str,
    destination: UserId,
    recorded_at: &str,
) -> Result<(), DenError> {
    bump_generation(connection, bear_id, entity_id).await?;
    let destination_value = destination.get();
    sqlx::query!(
        "UPDATE authenticated_user_entities SET state = 'active', user_id = ? WHERE bear_id = ? AND entity_id = ?",
        destination_value,
        bear_id,
        entity_id,
    )
    .execute(&mut *connection)
    .await?;
    let canonical_ref = format!("den:user:{destination}");
    sqlx::query!(
        "UPDATE entities SET resolution = 'confirmed', trust = 'asserted', canonical_ref = ? WHERE bear_id = ? AND entity_id = ?",
        canonical_ref,
        bear_id,
        entity_id,
    )
    .execute(&mut *connection)
    .await?;
    detach_handles(connection, bear_id, entity_id).await?;
    // Historical source handles remain detached; only this new local attachment is asserted.
    let handle_id = Uuid::new_v4().to_string();
    let handle_value = destination.to_string();
    sqlx::query!(
        r"
        INSERT INTO entity_handles (
            handle_id, bear_id, entity_id, handle_type, handle_value, source, trust, state, created_at
        ) VALUES (?, ?, ?, 'den_user', ?, 'authorized_restore', 'asserted', 'active', ?)
        ",
        handle_id,
        bear_id,
        entity_id,
        handle_value,
        recorded_at,
    )
    .execute(connection)
    .await?;
    Ok(())
}
