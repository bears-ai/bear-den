//! Durable local derived-copy cleanup work, not another erasure or a body-read grant.

use sqlx::SqliteConnection;

use super::{
    denied, ledger, write_transaction, AuthenticatedUserEntity, BearMemoryStore, DenError,
    PersonalErasureId, PersonalErasureReceipt, Uuid,
};
use crate::personal_candidates::ensure_current;

/// Return bounded, oldest-first local cleanup receipts for the current owner, without bodies.
/// Erasure/replay alone never completes cleanup. After a crash or failed PG transaction these
/// receipts remain pending until the service explicitly acknowledges successful cleanup.
pub async fn pending_erasure_cleanups(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    limit: u32,
) -> Result<Vec<PersonalErasureReceipt>, DenError> {
    let mut tx = store.pool().begin().await?;
    ensure_current(&mut tx, store, binding).await?;
    let receipts = pending_inside(&mut tx, store, binding, i64::from(limit.clamp(1, 100))).await?;
    tx.commit().await?;
    Ok(receipts)
}

async fn pending_inside(
    connection: &mut SqliteConnection,
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    limit: i64,
) -> Result<Vec<PersonalErasureReceipt>, DenError> {
    let bear_id = binding.bear_id().to_string();
    let local_bear_id = store.bear_id().to_string();
    let person = binding.entity_id();
    // Keep owning scope and recorded host separate: import may rebase the former, never
    // turn historical source cleanup into a new destination-host retry grant.
    let rows = sqlx::query!(
        r"SELECT erasure_id, sequence_no FROM personal_memory_erasures
          WHERE bear_id = ? AND person_entity_id = ? AND recorded_on_bear_id = ?
            AND cleanup_complete = 0
          ORDER BY sequence_no, erasure_id LIMIT ?",
        bear_id,
        person,
        local_bear_id,
        limit,
    )
    .fetch_all(&mut *connection)
    .await?;
    let mut receipts = Vec::with_capacity(rows.len());
    for row in rows {
        let id = PersonalErasureId(Uuid::parse_str(&row.erasure_id).map_err(|_| denied())?);
        receipts.push(ledger::receipt(connection, &bear_id, person, id, row.sequence_no).await?);
    }
    Ok(receipts)
}

/// Acknowledge only after the derived-copy PG transaction has committed, while the service
/// holds freshly checked membership authority. This is idempotent and permits a new current binding
/// generation/actor for the same person; it cannot erase newer names or grant body access.
/// Missing, other-owner and imported-source IDs are indistinguishable authorization failures.
pub async fn mark_erasure_cleanup_complete(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    erasure_id: PersonalErasureId,
) -> Result<(), DenError> {
    let mut tx = write_transaction(store, binding).await?;
    mark_inside(&mut tx, store, binding, erasure_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn mark_inside(
    connection: &mut SqliteConnection,
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    erasure_id: PersonalErasureId,
) -> Result<(), DenError> {
    let bear_id = binding.bear_id().to_string();
    let local_bear_id = store.bear_id().to_string();
    let person = binding.entity_id();
    let erasure_id = erasure_id.to_string();
    let changed = sqlx::query!(
        r"UPDATE personal_memory_erasures SET cleanup_complete = 1
          WHERE bear_id = ? AND person_entity_id = ? AND recorded_on_bear_id = ?
            AND erasure_id = ?",
        bear_id,
        person,
        local_bear_id,
        erasure_id,
    )
    .execute(connection)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(denied());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
