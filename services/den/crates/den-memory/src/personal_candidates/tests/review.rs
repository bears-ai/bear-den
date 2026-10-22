use super::*;
use crate::personal_facts::{publish_preferred_name, retract_preferred_name};

use CandidateReviewDecision::{Defer, Expire, Reject, Resume};

const ACTIONS: [CandidateReviewDecision; 4] = [Reject, Defer, Expire, Resume];

async fn deny_actions(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    id: PersonalCandidateId,
    expected: Option<CandidateReviewId>,
) {
    let before = snapshot(store).await;
    let generic_before = generic_snapshot(store).await;
    for action in ACTIONS {
        assert_denied(review_candidate(store, binding, id, action, expected).await);
        assert_eq!(snapshot(store).await, before);
        assert_eq!(generic_snapshot(store).await, generic_before);
    }
}

async fn supersede_source(store: &BearMemoryStore, source: Note) {
    store
        .append_record_with_options(
            &LogicalMemoryPath::source_local(
                MemorySource::Conversation(source.conversation),
                "note",
            ),
            "note",
            "pair",
            None,
            "Replacement source; historical evidence is no longer eligible.",
            &serde_json::json!({}),
            "normal",
            "normal",
            Some(&source.memory.to_string()),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn reviews_require_current_exact_owner_bear_and_generation_even_for_receipt_replay() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = person(&store, BOB).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "Private Alice").await;
    let receipt = review_candidate(&store, &alice, candidate.id(), Reject, None)
        .await
        .unwrap();
    let foreign = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let foreign_alice = opted_in(&foreign, ALICE).await;
    let mut stale = alice.clone();
    stale.authority_generation += 1;
    for (target, binding) in [
        (&store, &bob),
        (&store, &foreign_alice),
        (&foreign, &alice),
        (&foreign, &foreign_alice),
        (&store, &stale),
    ] {
        deny_actions(target, binding, candidate.id(), Some(receipt.review_id)).await;
    }
    deny_actions(&store, &alice, PersonalCandidateId(Uuid::new_v4()), None).await;
    revoke(&store, &alice).await;
    let fresh = reconnect(
        &store,
        alice.entity_id(),
        ALICE,
        ReplacementConsent::NoReplacement,
    )
    .await;
    deny_actions(&store, &alice, candidate.id(), Some(receipt.review_id)).await;
    assert_eq!(
        review_candidate(&store, &fresh, candidate.id(), Reject, None)
            .await
            .unwrap(),
        receipt,
        "current ownership, not historical review authority, permits receipt replay"
    );
}

#[tokio::test]
async fn defer_resume_cas_and_latest_only_replay_never_resurrect_withdrawal_or_opt_out() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "Alice").await;
    assert_eq!(candidate.latest_review_id(), None);
    let generic_before = generic_snapshot(&store).await;
    let mut head = None;
    let mut first = None;
    for (action, state) in [
        (Defer, CandidateState::Deferred),
        (Resume, CandidateState::Pending),
        (Defer, CandidateState::Deferred),
    ] {
        let before = snapshot(&store).await;
        assert_denied(
            review_candidate(
                &store,
                &alice,
                candidate.id(),
                action,
                Some(CandidateReviewId(Uuid::new_v4())),
            )
            .await,
        );
        assert_eq!(snapshot(&store).await, before);
        let sequence = next_sequence(&store).await;
        let receipt = review_candidate(&store, &alice, candidate.id(), action, head)
            .await
            .unwrap();
        assert_eq!(next_sequence(&store).await, sequence + 1);
        assert_eq!(receipt.candidate_id, candidate.id());
        assert_eq!(receipt.decision, action);
        first.get_or_insert(receipt.review_id);
        head = Some(receipt.review_id);
        let current = current_candidate(&store, &alice, candidate.id()).await;
        assert_eq!(current.state(), state);
        assert_eq!(current.latest_review_id(), head);
        let before = snapshot(&store).await;
        assert_eq!(
            review_candidate(&store, &alice, candidate.id(), action, None)
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(snapshot(&store).await, before);
    }
    assert_ne!(head, first);
    let before = snapshot(&store).await;
    assert_denied(review_candidate(&store, &alice, candidate.id(), Resume, first).await);
    assert_eq!(snapshot(&store).await, before);
    let listed = list_candidates(&store, &alice, 10).await.unwrap();
    assert_eq!(listed[0].state(), CandidateState::Deferred);
    assert_eq!(listed[0].latest_review_id(), head);
    assert_eq!(
        stage(&store, &alice, source, "Alice").await.id(),
        candidate.id()
    );
    assert!(withdraw_candidate(&store, &alice, candidate.id())
        .await
        .unwrap());
    let before = snapshot(&store).await;
    assert_eq!(
        review_candidate(&store, &alice, candidate.id(), Defer, None)
            .await
            .unwrap()
            .review_id,
        head.unwrap()
    );
    for action in [Resume, Reject, Expire] {
        assert_denied(review_candidate(&store, &alice, candidate.id(), action, head).await);
    }
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(
        current_candidate(&store, &alice, candidate.id())
            .await
            .state(),
        CandidateState::Withdrawn
    );
    let second = stage(&store, &alice, source, "Alice").await;
    let deferred = review_candidate(&store, &alice, second.id(), Defer, None)
        .await
        .unwrap();
    set_capture_enabled(&store, &alice, false).await.unwrap();
    set_capture_enabled(&store, &alice, true).await.unwrap();
    let before = snapshot(&store).await;
    assert_eq!(
        review_candidate(&store, &alice, second.id(), Defer, None)
            .await
            .unwrap(),
        deferred
    );
    assert_denied(
        review_candidate(
            &store,
            &alice,
            second.id(),
            Resume,
            Some(deferred.review_id),
        )
        .await,
    );
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(
        current_candidate(&store, &alice, second.id()).await.state(),
        CandidateState::Withdrawn
    );
    assert_eq!(generic_snapshot(&store).await, generic_before);
}

#[tokio::test]
async fn terminal_closure_matrix_releases_pending_unique_key_but_keeps_private_body_and_audit() {
    // Six transitions cover each eligible base/review state for both terminal decisions.
    for action in [Reject, Expire] {
        for initial in [
            CandidateState::Pending,
            CandidateState::Deferred,
            CandidateState::Quarantined,
        ] {
            let store = new_test_store().await;
            let alice = opted_in(&store, ALICE).await;
            let source = note(&store).await;
            let candidate = stage(&store, &alice, source, "Retained privately").await;
            let head = match initial {
                CandidateState::Deferred => Some(
                    review_candidate(&store, &alice, candidate.id(), Defer, None)
                        .await
                        .unwrap()
                        .review_id,
                ),
                CandidateState::Quarantined => {
                    quarantine_candidate(&store, candidate.id()).await;
                    None
                }
                _ => None,
            };
            let generic_before = generic_snapshot(&store).await;
            let receipt = review_candidate(&store, &alice, candidate.id(), action, head)
                .await
                .unwrap();
            let current = current_candidate(&store, &alice, candidate.id()).await;
            let terminal = if action == Reject {
                CandidateState::Rejected
            } else {
                CandidateState::Expired
            };
            assert_eq!(current.state(), terminal);
            assert_eq!(
                state(&store, candidate.id()).await,
                CandidateState::Withdrawn
            );
            assert_eq!(current.preferred_name(), candidate.preferred_name());
            assert_eq!(current.created_at(), candidate.created_at());
            assert!(current.withdrawn_at().is_some());
            let before = snapshot(&store).await;
            assert_eq!(
                review_candidate(&store, &alice, candidate.id(), action, head)
                    .await
                    .unwrap(),
                receipt
            );
            for other in ACTIONS.into_iter().filter(|other| *other != action) {
                assert_denied(
                    review_candidate(
                        &store,
                        &alice,
                        candidate.id(),
                        other,
                        Some(receipt.review_id),
                    )
                    .await,
                );
            }
            assert!(!withdraw_candidate(&store, &alice, candidate.id())
                .await
                .unwrap());
            assert_denied(
                publish_preferred_name(&store, &source.proof(&alice), candidate.id(), None).await,
            );
            assert_eq!(snapshot(&store).await, before);
            let audit: serde_json::Value = serde_json::from_str(&before.0).unwrap();
            let raw = &audit["candidates"][0];
            assert_eq!(raw[16], store.bear_id().to_string());
            assert_eq!(raw[17], ALICE.get());
            let review = audit["candidate_reviews"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row[0] == receipt.review_id.to_string())
                .unwrap();
            assert_eq!(review[6], store.bear_id().to_string());
            assert_eq!(review[7], ALICE.get());
            assert_eq!(review[8], alice.authority_generation);
            let fresh = stage(&store, &alice, source, "Retained privately").await;
            assert_ne!(fresh.id(), candidate.id());
            assert_eq!(fresh.state(), CandidateState::Pending);
            assert_eq!(fresh.latest_review_id(), None);
            assert_eq!(
                current_candidate(&store, &alice, candidate.id())
                    .await
                    .state(),
                terminal
            );
            assert_eq!(generic_snapshot(&store).await, generic_before);
        }
    }
}

#[tokio::test]
async fn defer_resume_and_explicit_approval_require_live_source_but_terminal_closure_does_not() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let pending = stage(&store, &alice, source, "Pending").await;
    let deferred = stage(&store, &alice, source, "Deferred").await;
    let receipt = review_candidate(&store, &alice, deferred.id(), Defer, None)
        .await
        .unwrap();
    supersede_source(&store, source).await;
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    assert_denied(review_candidate(&store, &alice, pending.id(), Defer, None).await);
    assert_denied(review_candidate(&store, &alice, pending.id(), Resume, None).await);
    assert_denied(
        review_candidate(
            &store,
            &alice,
            deferred.id(),
            Resume,
            Some(receipt.review_id),
        )
        .await,
    );
    assert_denied(publish_preferred_name(&store, &source.proof(&alice), deferred.id(), None).await);
    assert_eq!(snapshot(&store).await, before);
    review_candidate(&store, &alice, pending.id(), Reject, None)
        .await
        .unwrap();
    review_candidate(
        &store,
        &alice,
        deferred.id(),
        Expire,
        Some(receipt.review_id),
    )
    .await
    .unwrap();
    assert_eq!(generic_snapshot(&store).await, generic_before);
}

#[tokio::test]
async fn deferred_explicit_approval_and_published_dedup_are_unchanged_by_historical_reviews() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "Alice").await;
    let review = review_candidate(&store, &alice, candidate.id(), Defer, None)
        .await
        .unwrap();
    let publication = publish_preferred_name(&store, &source.proof(&alice), candidate.id(), None)
        .await
        .unwrap();
    assert_eq!(
        current_candidate(&store, &alice, candidate.id())
            .await
            .state(),
        CandidateState::Published
    );
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    for action in [Resume, Reject, Expire] {
        assert_denied(
            review_candidate(
                &store,
                &alice,
                candidate.id(),
                action,
                Some(review.review_id),
            )
            .await,
        );
    }
    assert_eq!(
        review_candidate(&store, &alice, candidate.id(), Defer, None)
            .await
            .unwrap(),
        review
    );
    assert_eq!(
        stage(&store, &alice, source, "Alice").await.id(),
        candidate.id()
    );
    assert_eq!(
        publish_preferred_name(&store, &source.proof(&alice), candidate.id(), None)
            .await
            .unwrap(),
        publication
    );
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(generic_snapshot(&store).await, generic_before);
    retract_preferred_name(&store, &alice, publication.fact_id)
        .await
        .unwrap();
    assert_eq!(
        stage(&store, &alice, source, "Alice").await.state(),
        CandidateState::Published
    );
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    for action in [Resume, Reject, Expire] {
        assert_denied(
            review_candidate(
                &store,
                &alice,
                candidate.id(),
                action,
                Some(review.review_id),
            )
            .await,
        );
    }
    assert_eq!(
        review_candidate(&store, &alice, candidate.id(), Defer, None)
            .await
            .unwrap(),
        review
    );
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(generic_snapshot(&store).await, generic_before);
}

#[tokio::test]
async fn import_quarantine_overrides_defer_resume_but_not_terminals_and_new_owner_can_close() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let mut candidates = Vec::new();
    for (action, name) in [
        (Defer, "Deferred"),
        (Resume, "Resumed"),
        (Reject, "Rejected"),
        (Expire, "Expired"),
    ] {
        let candidate = stage(&store, &alice, source, name).await;
        let receipt = review_candidate(&store, &alice, candidate.id(), action, None)
            .await
            .unwrap();
        candidates.push((candidate.id(), receipt));
    }
    supersede_source(&store, source).await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let fresh = reconnect(
        &store,
        alice.entity_id(),
        BOB,
        ReplacementConsent::NoReplacement,
    )
    .await;
    assert!(!capture_enabled(&store, &fresh).await.unwrap());
    let generic_before = generic_snapshot(&store).await;
    for ((id, receipt), expected) in candidates.iter().zip([
        CandidateState::Quarantined,
        CandidateState::Quarantined,
        CandidateState::Rejected,
        CandidateState::Expired,
    ]) {
        assert_eq!(
            current_candidate(&store, &fresh, *id).await.state(),
            expected
        );
        let before = snapshot(&store).await;
        assert_eq!(
            review_candidate(&store, &fresh, *id, receipt.decision, None)
                .await
                .unwrap(),
            *receipt
        );
        assert_eq!(snapshot(&store).await, before);
    }
    for (id, receipt) in candidates.iter().take(2) {
        let other = if receipt.decision == Defer {
            Resume
        } else {
            Defer
        };
        let before = snapshot(&store).await;
        assert_denied(review_candidate(&store, &fresh, *id, other, Some(receipt.review_id)).await);
        assert_eq!(snapshot(&store).await, before);
    }
    for ((id, previous), action) in candidates.iter().take(2).zip([Reject, Expire]) {
        review_candidate(&store, &fresh, *id, action, Some(previous.review_id))
            .await
            .unwrap();
    }
    set_capture_enabled(&store, &fresh, true).await.unwrap();
    for (id, _) in &candidates {
        let current = current_candidate(&store, &fresh, *id).await;
        let before = snapshot(&store).await;
        for action in [Defer, Resume] {
            assert_denied(
                review_candidate(&store, &fresh, *id, action, current.latest_review_id()).await,
            );
        }
        assert_eq!(snapshot(&store).await, before);
    }
    assert_eq!(generic_snapshot(&store).await, generic_before);
}

#[tokio::test]
async fn failed_review_insert_rolls_back_terminal_withdrawal_audit_and_all_sequence_allocations() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let pending = stage(&store, &alice, source, "Pending").await;
    let deferred = stage(&store, &alice, source, "Deferred").await;
    let head = review_candidate(&store, &alice, deferred.id(), Defer, None)
        .await
        .unwrap();
    // The only new test SQL shape: fail the final immutable write for every transition.
    sqlx::query!(
        r"CREATE TRIGGER fail_candidate_review BEFORE INSERT ON personal_memory_candidate_reviews
          BEGIN SELECT RAISE(ABORT, 'injected candidate review failure'); END"
    )
    .execute(store.pool())
    .await
    .unwrap();
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    for action in ACTIONS {
        let (id, expected) = if action == Resume {
            (deferred.id(), Some(head.review_id))
        } else {
            (pending.id(), None)
        };
        assert_database_error(review_candidate(&store, &alice, id, action, expected).await);
        assert_eq!(snapshot(&store).await, before);
        assert_eq!(generic_snapshot(&store).await, generic_before);
    }
}

#[test]
fn review_decision_boundary_rejects_unknown_values_instead_of_treating_them_as_pending() {
    for action in ACTIONS {
        assert_eq!(
            CandidateReviewDecision::parse(action.as_str()).unwrap(),
            action
        );
    }
    for unknown in ["", "pending", "withdrawn", "published", "DEFER", "unknown"] {
        assert!(CandidateReviewDecision::parse(unknown).is_err());
    }
}
