use super::*;

#[derive(Clone, Copy, Debug)]
enum StageFailure {
    CandidateInsert,
    SequenceAllocation,
}

async fn staging_rollback(failure: StageFailure) {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let existing = stage(&store, &alice, source, "Existing").await;
    let generic_before = generic_snapshot(&store).await;
    match failure {
        StageFailure::CandidateInsert => {
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_candidate_insert BEFORE INSERT ON personal_memory_candidates
                  BEGIN SELECT RAISE(ABORT, 'injected candidate insertion failure'); END",
            ).execute(store.pool()).await.unwrap();
        }
        StageFailure::SequenceAllocation => {
            // The authority serialization no-op remains permitted; fail the real allocation.
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_sequence BEFORE UPDATE ON bear_sequence
                  WHEN NEW.next_sequence <> OLD.next_sequence
                  BEGIN SELECT RAISE(ABORT, 'injected candidate sequence failure'); END",
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
    }
    let before = snapshot(&store).await;
    let sequence = next_sequence(&store).await;
    for _ in 0..2 {
        assert_database_error(
            stage_preferred_name(
                &store,
                &source.proof(&alice),
                PreferredName::new("New request").unwrap(),
            )
            .await,
        );
        assert_eq!(
            snapshot(&store).await,
            before,
            "{failure:?} must not consume a sequence or partly stage"
        );
        assert_eq!(generic_snapshot(&store).await, generic_before);
    }
    // A duplicate does not need either an INSERT or a sequence allocation.
    assert_eq!(
        stage(&store, &alice, source, "Existing").await.id(),
        existing.id()
    );
    match failure {
        StageFailure::CandidateInsert => {
            sqlx::query!("DROP TRIGGER IF EXISTS fail_personal_candidate_insert")
                .execute(store.pool())
                .await
                .unwrap();
        }
        StageFailure::SequenceAllocation => {
            sqlx::query!("DROP TRIGGER IF EXISTS fail_personal_sequence")
                .execute(store.pool())
                .await
                .unwrap();
        }
    }
    let fresh = stage(&store, &alice, source, "New request").await;
    assert_eq!(candidate_sequence(&store, fresh.id()).await, sequence);
    assert_eq!(next_sequence(&store).await, sequence + 1);
}

#[tokio::test]
async fn candidate_insert_failure_rolls_back_the_allocated_sequence_and_all_candidate_state() {
    staging_rollback(StageFailure::CandidateInsert).await;
}

#[tokio::test]
async fn sequence_failure_cannot_create_a_candidate_or_change_consent() {
    staging_rollback(StageFailure::SequenceAllocation).await;
}

#[tokio::test]
async fn opt_out_withdrawal_failure_rolls_back_consent_and_every_candidate_transition() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let pending = stage(&store, &alice, source, "Pending").await;
    let quarantined = stage(&store, &alice, source, "Block withdrawal").await;
    quarantine_candidate(&store, quarantined.id()).await;
    let withdrawn = stage(&store, &alice, source, "Already withdrawn").await;
    withdraw_candidate(&store, &alice, withdrawn.id())
        .await
        .unwrap();
    let bobs = stage(&store, &bob, source, "Bob").await;
    sqlx::query!(
        r"CREATE TRIGGER fail_personal_withdrawal BEFORE UPDATE ON personal_memory_candidates
          WHEN NEW.state = 'withdrawn' AND OLD.value_text = 'Block withdrawal'
          BEGIN SELECT RAISE(ABORT, 'injected candidate withdrawal failure'); END",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let before = snapshot(&store).await;
    for _ in 0..2 {
        assert_database_error(set_capture_enabled(&store, &alice, false).await);
        assert_eq!(snapshot(&store).await, before);
        assert!(capture_enabled(&store, &alice).await.unwrap());
    }
    sqlx::query!("DROP TRIGGER IF EXISTS fail_personal_withdrawal")
        .execute(store.pool())
        .await
        .unwrap();
    set_capture_enabled(&store, &alice, false).await.unwrap();
    for id in [pending.id(), quarantined.id(), withdrawn.id()] {
        assert_eq!(state(&store, id).await, CandidateState::Withdrawn);
    }
    assert_eq!(state(&store, bobs.id()).await, CandidateState::Pending);
    assert!(capture_enabled(&store, &bob).await.unwrap());
}

#[tokio::test]
async fn single_withdrawal_failure_preserves_pending_body_and_audit_fields() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let pending = stage(&store, &alice, source, "Alice").await;
    sqlx::query!(
        r"CREATE TRIGGER fail_single_personal_withdrawal BEFORE UPDATE ON personal_memory_candidates
          WHEN NEW.state = 'withdrawn'
          BEGIN SELECT RAISE(ABORT, 'injected single withdrawal failure'); END",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let before = snapshot(&store).await;
    assert_database_error(withdraw_candidate(&store, &alice, pending.id()).await);
    assert_eq!(snapshot(&store).await, before);
    sqlx::query!("DROP TRIGGER IF EXISTS fail_single_personal_withdrawal")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(withdraw_candidate(&store, &alice, pending.id())
        .await
        .unwrap());
}

#[derive(Clone, Copy, Debug)]
enum IdentityOperation {
    Revoke,
    Replace,
}

async fn receipt_rollback(operation: IdentityOperation) {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let alice_pending = stage(&store, &alice, source, "Alice").await;
    let (review, consent, parked, parked_candidate) = match operation {
        IdentityOperation::Revoke => (
            review_revocation(&store, ACTOR, alice.entity_id())
                .await
                .unwrap(),
            ReplacementConsent::NoReplacement,
            alice.clone(),
            alice_pending.id(),
        ),
        IdentityOperation::Replace => {
            quarantine_imported_authenticated_users(&store)
                .await
                .unwrap();
            let bob = opted_in(&store, BOB).await;
            let bobs_source = note(&store).await;
            let bobs = stage(&store, &bob, bobs_source, "Bob").await;
            (
                review_reconnection(&store, ACTOR, alice.entity_id(), BOB)
                    .await
                    .unwrap(),
                ReplacementConsent::PreserveDisplacedIdentity,
                bob,
                bobs.id(),
            )
        }
    };
    sqlx::query!(
        r"CREATE TRIGGER fail_personal_identity_receipt BEFORE INSERT ON user_identity_receipts
          BEGIN SELECT RAISE(ABORT, 'injected personal identity receipt failure'); END",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    for _ in 0..2 {
        assert_database_error(commit_review(&store, &review, consent).await);
        assert_eq!(
            snapshot(&store).await,
            before,
            "{operation:?}: pending quarantine and epochs must roll back with receipt failure"
        );
        assert_eq!(generic_snapshot(&store).await, generic_before);
        assert_eq!(
            current_candidate(&store, &parked, parked_candidate)
                .await
                .state(),
            CandidateState::Pending
        );
        assert!(capture_enabled(&store, &parked).await.unwrap());
    }
    sqlx::query!("DROP TRIGGER IF EXISTS fail_personal_identity_receipt")
        .execute(store.pool())
        .await
        .unwrap();
    commit_review(&store, &review, consent).await.unwrap();
    assert_eq!(
        state(&store, parked_candidate).await,
        CandidateState::Quarantined
    );
    assert_denied(capture_enabled(&store, &parked).await);
}

#[tokio::test]
async fn revocation_receipt_failure_rolls_back_pending_quarantine_and_keeps_old_consent_valid() {
    receipt_rollback(IdentityOperation::Revoke).await;
}

#[tokio::test]
async fn reconnection_receipt_failure_rolls_back_displaced_pending_quarantine_and_both_authorities()
{
    receipt_rollback(IdentityOperation::Replace).await;
}

#[derive(Clone, Copy, Debug)]
enum ImportFailure {
    CandidateQuarantine,
    ConsentDisable,
}

async fn import_rollback(failure: ImportFailure) {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let pending = stage(&store, &alice, source, "Alice").await;
    let bobs = stage(&store, &bob, source, "Bob").await;
    let quarantined = stage(&store, &alice, source, "Old quarantine").await;
    quarantine_candidate(&store, quarantined.id()).await;
    let withdrawn = stage(&store, &alice, source, "Old withdrawal").await;
    withdraw_candidate(&store, &alice, withdrawn.id())
        .await
        .unwrap();
    match failure {
        ImportFailure::CandidateQuarantine => {
            sqlx::query!(
                r"CREATE TRIGGER fail_imported_personal_quarantine BEFORE UPDATE ON personal_memory_candidates
                  WHEN OLD.state = 'pending' AND NEW.state = 'quarantined'
                  BEGIN SELECT RAISE(ABORT, 'injected imported candidate quarantine failure'); END",
            ).execute(store.pool()).await.unwrap();
        }
        ImportFailure::ConsentDisable => {
            // Runs after authority, handles, generations AND candidate quarantine were changed.
            sqlx::query!(
                r"CREATE TRIGGER fail_imported_personal_settings BEFORE UPDATE ON personal_memory_settings
                  WHEN NEW.capture_enabled = 0
                  BEGIN SELECT RAISE(ABORT, 'injected imported consent disable failure'); END",
            ).execute(store.pool()).await.unwrap();
        }
    }
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    for _ in 0..2 {
        assert_database_error(quarantine_imported_authenticated_users(&store).await);
        assert_eq!(
            snapshot(&store).await,
            before,
            "{failure:?}: import is one authority/candidate/settings transaction"
        );
        assert_eq!(generic_snapshot(&store).await, generic_before);
        assert!(capture_enabled(&store, &alice).await.unwrap());
        assert!(capture_enabled(&store, &bob).await.unwrap());
    }
    match failure {
        ImportFailure::CandidateQuarantine => {
            sqlx::query!("DROP TRIGGER IF EXISTS fail_imported_personal_quarantine")
                .execute(store.pool())
                .await
                .unwrap();
        }
        ImportFailure::ConsentDisable => {
            sqlx::query!("DROP TRIGGER IF EXISTS fail_imported_personal_settings")
                .execute(store.pool())
                .await
                .unwrap();
        }
    }
    let imported = quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert_eq!(imported.len(), 2);
    for id in [pending.id(), bobs.id(), quarantined.id()] {
        assert_eq!(state(&store, id).await, CandidateState::Quarantined);
    }
    assert_eq!(
        state(&store, withdrawn.id()).await,
        CandidateState::Withdrawn
    );
    assert_all_denied(&store, &alice, source, pending.id()).await;
    assert_all_denied(&store, &bob, source, bobs.id()).await;
    assert_eq!(sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!: i64" FROM personal_memory_settings WHERE capture_enabled <> 0"#)
        .fetch_one(store.pool()).await.unwrap(), 0);
}

#[tokio::test]
async fn imported_candidate_quarantine_failure_rolls_back_all_authentication_and_personal_state() {
    import_rollback(ImportFailure::CandidateQuarantine).await;
}

#[tokio::test]
async fn imported_consent_failure_rolls_back_authority_handles_epochs_and_candidate_quarantine() {
    import_rollback(ImportFailure::ConsentDisable).await;
}
