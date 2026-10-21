use crate::BearMemoryStore;

#[derive(Clone, Copy, Debug)]
pub(super) enum FailurePoint {
    CandidateInsert,
    SequenceAllocation,
    CandidateWithdrawal,
    IdentityReceipt,
    ImportedCandidateQuarantine,
    ImportedConsentDisable,
}

// Each rollback fixture installs one failure at a time in its own database.
pub(super) async fn inject_failure(store: &BearMemoryStore, failure: FailurePoint) {
    match failure {
        FailurePoint::CandidateInsert => {
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_operation BEFORE INSERT ON personal_memory_candidates
                  BEGIN SELECT RAISE(ABORT, 'injected candidate insertion failure'); END",
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        FailurePoint::SequenceAllocation => {
            // The authority serialization no-op remains permitted; fail the real allocation.
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_operation BEFORE UPDATE ON bear_sequence
                  WHEN NEW.next_sequence <> OLD.next_sequence
                  BEGIN SELECT RAISE(ABORT, 'injected candidate sequence failure'); END",
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        FailurePoint::CandidateWithdrawal => {
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_operation BEFORE UPDATE ON personal_memory_candidates
                  WHEN NEW.state = 'withdrawn' AND OLD.value_text = 'Block withdrawal'
                  BEGIN SELECT RAISE(ABORT, 'injected candidate withdrawal failure'); END",
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        FailurePoint::IdentityReceipt => {
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_operation BEFORE INSERT ON user_identity_receipts
                  BEGIN SELECT RAISE(ABORT, 'injected personal identity receipt failure'); END",
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        FailurePoint::ImportedCandidateQuarantine => {
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_operation BEFORE UPDATE ON personal_memory_candidates
                  WHEN OLD.state = 'pending' AND NEW.state = 'quarantined'
                  BEGIN SELECT RAISE(ABORT, 'injected imported candidate quarantine failure'); END",
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        FailurePoint::ImportedConsentDisable => {
            // Runs after authority, handles, generations AND candidate quarantine were changed.
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_operation BEFORE UPDATE ON personal_memory_settings
                  WHEN NEW.capture_enabled = 0
                  BEGIN SELECT RAISE(ABORT, 'injected imported consent disable failure'); END",
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
    }
}

pub(super) async fn clear_failure(store: &BearMemoryStore) {
    sqlx::query!("DROP TRIGGER IF EXISTS fail_personal_operation")
        .execute(store.pool())
        .await
        .unwrap();
}
