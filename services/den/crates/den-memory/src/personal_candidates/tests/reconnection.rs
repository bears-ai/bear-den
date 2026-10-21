use super::*;

#[tokio::test]
async fn replacement_parks_only_displaced_persons_pending_candidates_and_invalidates_their_consent()
{
    let store = new_test_store().await;
    let portable = opted_in(&store, ALICE).await;
    let portable_source = note(&store).await;
    let imported = stage(&store, &portable, portable_source, "Portable Alice").await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let bob = opted_in(&store, BOB).await;
    let carol = opted_in(&store, UserId::new(33)).await;
    let source = note(&store).await;
    let bobs_pending = stage(&store, &bob, source, "Bob pending").await;
    let bobs_quarantined = stage(&store, &bob, source, "Bob historical").await;
    quarantine_candidate(&store, bobs_quarantined.id()).await;
    let bobs_withdrawn = stage(&store, &bob, source, "Bob withdrawn").await;
    withdraw_candidate(&store, &bob, bobs_withdrawn.id())
        .await
        .unwrap();
    let carols = stage(&store, &carol, source, "Carol").await;
    let generic_before = generic_snapshot(&store).await;
    let replacement = reconnect(
        &store,
        portable.entity_id(),
        BOB,
        ReplacementConsent::PreserveDisplacedIdentity,
    )
    .await;
    assert_eq!(
        state(&store, bobs_pending.id()).await,
        CandidateState::Quarantined
    );
    assert_eq!(
        state(&store, bobs_quarantined.id()).await,
        CandidateState::Quarantined
    );
    assert_eq!(
        state(&store, bobs_withdrawn.id()).await,
        CandidateState::Withdrawn
    );
    assert_eq!(
        state(&store, imported.id()).await,
        CandidateState::Quarantined
    );
    assert_eq!(state(&store, carols.id()).await, CandidateState::Pending);
    assert!(capture_enabled(&store, &carol).await.unwrap());
    assert!(!capture_enabled(&store, &replacement).await.unwrap());
    assert_all_denied(&store, &bob, source, bobs_pending.id()).await;
    assert!(get_candidate(&store, &replacement, bobs_pending.id())
        .await
        .unwrap()
        .is_none());
    assert!(!withdraw_candidate(&store, &replacement, bobs_pending.id())
        .await
        .unwrap());
    assert_eq!(generic_snapshot(&store).await, generic_before);
    let bob_restored = reconnect(
        &store,
        bob.entity_id(),
        UserId::new(44),
        ReplacementConsent::NoReplacement,
    )
    .await;
    assert!(
        !capture_enabled(&store, &bob_restored).await.unwrap(),
        "parking invalidates epoch-bound consent without reusing a stored true flag"
    );
    assert_eq!(
        list_candidates(&store, &bob_restored, 100)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        state(&store, bobs_withdrawn.id()).await,
        CandidateState::Withdrawn
    );
}

#[tokio::test]
async fn import_quarantine_is_bear_scoped_and_repeatable_without_resurrecting_history() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let pending = stage(&store, &alice, source, "Pending").await;
    let quarantined = stage(&store, &alice, source, "Quarantined").await;
    quarantine_candidate(&store, quarantined.id()).await;
    let withdrawn = stage(&store, &alice, source, "Withdrawn").await;
    withdraw_candidate(&store, &alice, withdrawn.id())
        .await
        .unwrap();
    let bobs = stage(&store, &bob, source, "Bob").await;
    let foreign = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let foreign_alice = opted_in(&foreign, ALICE).await;
    let foreign_source = note(&foreign).await;
    let foreign_pending = stage(&foreign, &foreign_alice, foreign_source, "Other Bear").await;
    let generic_before = generic_snapshot(&store).await;
    for _ in 0..2 {
        let marked = quarantine_imported_authenticated_users(&store)
            .await
            .unwrap();
        assert!(marked.contains(&alice.entity_id().to_owned()));
        assert!(marked.contains(&bob.entity_id().to_owned()));
        assert_eq!(marked.len(), 2);
        for id in [pending.id(), quarantined.id(), bobs.id()] {
            assert_eq!(state(&store, id).await, CandidateState::Quarantined);
        }
        assert_eq!(
            state(&store, withdrawn.id()).await,
            CandidateState::Withdrawn
        );
        assert_eq!(
            state(&foreign, foreign_pending.id()).await,
            CandidateState::Pending
        );
        assert!(capture_enabled(&foreign, &foreign_alice).await.unwrap());
        assert_eq!(generic_snapshot(&store).await, generic_before);
    }
    let fresh = reconnect(
        &store,
        alice.entity_id(),
        ALICE,
        ReplacementConsent::NoReplacement,
    )
    .await;
    assert!(!capture_enabled(&store, &fresh).await.unwrap());
    assert_eq!(list_candidates(&store, &fresh, 100).await.unwrap().len(), 3);
    assert_eq!(
        current_candidate(&store, &fresh, quarantined.id())
            .await
            .state(),
        CandidateState::Quarantined
    );
    // Human opt-out applies to imported quarantine too; opting back in does not undo it.
    set_capture_enabled(&store, &fresh, false).await.unwrap();
    set_capture_enabled(&store, &fresh, true).await.unwrap();
    for id in [pending.id(), quarantined.id(), withdrawn.id()] {
        assert_eq!(state(&store, id).await, CandidateState::Withdrawn);
    }
    let canonical_new_source = note(&store).await;
    let fresh_candidate = stage(&store, &fresh, canonical_new_source, "Pending").await;
    assert_ne!(fresh_candidate.id(), pending.id());
    assert_eq!(fresh_candidate.state(), CandidateState::Pending);
}
