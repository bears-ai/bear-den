use std::str::FromStr;

use den_core::{ids::UserId, DenError};
use uuid::Uuid;

use super::{IdentityReceiptHistory, IdentityReconnectionReceipt};
use crate::BearMemoryStore;

/// Trusted management history, not authentication proof. Prefer decisions made for the
/// current Bear over imported timestamps, so hostile future dates cannot hide local recovery.
pub async fn receipt_history(
    store: &BearMemoryStore,
    limit: u32,
) -> Result<IdentityReceiptHistory, DenError> {
    let bear_id = store.bear_id().to_string();
    let limit = i64::from(limit.clamp(1, 100));
    // SQLite affinity does not validate integer fields or UTF-8. Read untrusted history as
    // bytes and parse each claim separately: one invalid row cannot hide valid local receipts.
    let rows = sqlx::query!(
        r#"
        SELECT CAST(receipt_id AS BLOB) AS "receipt_id?: Vec<u8>",
               CAST(recorded_bear_id AS BLOB) AS "recorded_bear_id!: Vec<u8>",
               CAST(actor_user_id AS BLOB) AS "actor_user_id!: Vec<u8>",
               CAST(previous_user_id AS BLOB) AS "previous_user_id!: Vec<u8>",
               CAST(entity_id AS BLOB) AS "entity_id!: Vec<u8>",
               CAST(destination_user_id AS BLOB) AS "destination_user_id?: Vec<u8>",
               CAST(displaced_entity_id AS BLOB) AS "displaced_entity_id?: Vec<u8>",
               CAST(prior_generation AS BLOB) AS "prior_generation!: Vec<u8>",
               CAST(resulting_generation AS BLOB) AS "resulting_generation!: Vec<u8>",
               CAST(recorded_at AS BLOB) AS "recorded_at!: Vec<u8>"
        FROM user_identity_receipts WHERE bear_id = ?
        ORDER BY (recorded_bear_id = ?) DESC, recorded_at DESC, receipt_id DESC LIMIT ?
        "#,
        bear_id,
        bear_id,
        limit,
    )
    .fetch_all(store.pool())
    .await?;
    let mut history = IdentityReceiptHistory {
        receipts: Vec::with_capacity(rows.len()),
        unreadable_entries: 0,
    };
    for row in rows {
        let parsed = (|| {
            let receipt_id = parse::<Uuid>(row.receipt_id.as_deref()?)?;
            let actor = user_id(&row.actor_user_id)?;
            let previous = user_id(&row.previous_user_id)?;
            let destination = match row.destination_user_id {
                Some(bytes) => Some(user_id(&bytes)?),
                None => None,
            };
            let prior = parse::<i64>(&row.prior_generation)?;
            let resulting = parse::<i64>(&row.resulting_generation)?;
            if prior <= 0 || resulting <= prior {
                return None;
            }
            Some(IdentityReconnectionReceipt {
                receipt_id,
                recorded_bear_id: String::from_utf8(row.recorded_bear_id).ok()?,
                actor_user_id: actor,
                previous_user_id: previous,
                prior_generation: prior,
                resulting_generation: resulting,
                entity_id: String::from_utf8(row.entity_id).ok()?,
                destination_user_id: destination,
                displaced_entity_id: row
                    .displaced_entity_id
                    .map(String::from_utf8)
                    .transpose()
                    .ok()?,
                recorded_at: String::from_utf8(row.recorded_at).ok()?,
            })
        })();
        match parsed {
            Some(receipt) => history.receipts.push(receipt),
            None => history.unreadable_entries += 1,
        }
    }
    Ok(history)
}

fn parse<T: FromStr>(bytes: &[u8]) -> Option<T> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn user_id(bytes: &[u8]) -> Option<UserId> {
    parse::<i32>(bytes).filter(|id| *id > 0).map(UserId::new)
}

/// Compatibility projection for trusted callers that only need readable receipts.
pub async fn list_receipts(
    store: &BearMemoryStore,
    limit: u32,
) -> Result<Vec<IdentityReconnectionReceipt>, DenError> {
    Ok(receipt_history(store, limit).await?.receipts)
}
