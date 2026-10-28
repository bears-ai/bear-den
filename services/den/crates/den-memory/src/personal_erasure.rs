//! Explicit owner erasure of preferred-name memory, not original human history or automatic
//! forgetting. The service fences body changes and derived-copy cleanup with current authority,
//! then freshly authorizes ledger acknowledgement after the derived-copy transaction commits.
//! This SQLite operation makes no forensic, backup, provider, or cross-database atomicity claim.

use std::fmt;

use den_core::DenError;
use uuid::Uuid;

use crate::{
    clock::now_rfc3339,
    personal_candidates::{
        write_capture_inside, write_transaction, PersonalCandidateId, PersonalCaptureRevision,
    },
    personal_facts::PersonalFactId,
    AuthenticatedUserEntity, BearMemoryStore,
};

mod cleanup;
mod ledger;
mod write;

pub use cleanup::{mark_erasure_cleanup_complete, pending_erasure_cleanups};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersonalErasureId(pub Uuid);

impl fmt::Display for PersonalErasureId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Content-free cleanup locators, not a claim that derived-copy cleanup has completed.
/// No generic serde: the parent must choose an explicit authorized projection.
#[derive(Clone, PartialEq, Eq)]
pub struct PersonalErasureReceipt {
    pub erasure_id: PersonalErasureId,
    pub affected_fact_ids: Vec<PersonalFactId>,
}

impl fmt::Debug for PersonalErasureReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PersonalErasureReceipt([redacted])")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Expected {
    head: Option<PersonalFactId>,
    latest_candidate: Option<PersonalCandidateId>,
    capture_revision: PersonalCaptureRevision,
}

fn denied() -> DenError {
    DenError::Authorization("Personal-memory erasure is stale or no longer authorized.".into())
}

/// Erase every preferred-name candidate and published body version owned by this person.
/// All three reviewed heads are compared inside the same SQLite write transaction as erasure.
/// Exact retries check current binding and the original actor/generation/spec, then recover the
/// original receipt without rechecking the changed heads or erasing any later names. A request ID
/// cannot be reused by another person, actor, generation, host, or reviewed specification.
/// Imported ledger ownership may be rebased, but its historical actor/host is never a retry grant.
pub async fn erase_preferred_name_memory(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    request_id: Uuid,
    expected_head: Option<PersonalFactId>,
    expected_latest_candidate: Option<PersonalCandidateId>,
    expected_capture_revision: PersonalCaptureRevision,
) -> Result<PersonalErasureReceipt, DenError> {
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let expected = Expected {
        head: expected_head,
        latest_candidate: expected_latest_candidate,
        capture_revision: expected_capture_revision,
    };
    if let Some((erasure_id, boundary)) =
        ledger::recover(&mut tx, &bear_id, binding, request_id, expected).await?
    {
        let receipt =
            ledger::receipt(&mut tx, &bear_id, binding.entity_id(), erasure_id, boundary).await?;
        tx.commit().await?;
        return Ok(receipt);
    }
    write::ensure_reviewed(&mut tx, &bear_id, binding, expected).await?;
    let boundary = sqlx::query_scalar!(
        r#"UPDATE bear_sequence SET next_sequence = next_sequence + 1 WHERE id = 1
           RETURNING next_sequence - 1 AS "sequence_no!: i64""#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let created_at = now_rfc3339()?;
    write::erase_bodies(&mut tx, &bear_id, binding.entity_id(), &created_at).await?;
    // Reuse the canonical opt-out writer: fresh consent revision, head retraction and rule
    // withdrawal commit with the body writes and permanent erasure marker, or not at all.
    write_capture_inside(&mut tx, &bear_id, binding, false, &created_at).await?;
    let erasure_id = PersonalErasureId(Uuid::new_v4());
    ledger::record(
        &mut tx,
        binding,
        request_id,
        expected,
        erasure_id,
        boundary,
        &created_at,
    )
    .await?;
    let receipt =
        ledger::receipt(&mut tx, &bear_id, binding.entity_id(), erasure_id, boundary).await?;
    tx.commit().await?;
    Ok(receipt)
}
