use super::*;

#[tokio::test]
async fn owner_erasure_is_person_and_bear_isolated_even_in_a_shared_database() {
    let store = new_test_store().await;
    let foreign = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let foreign_alice = opted_in(&foreign, ALICE).await;
    let source = note(&store).await;
    let bob_source = note(&store).await;
    let foreign_source = note(&foreign).await;
    let first = publish(&store, &alice, source, "SameName", None).await;
    let bobs = publish(&store, &bob, bob_source, "SameName", None).await;
    let foreign_fact = publish(&foreign, &foreign_alice, foreign_source, "SameName", None).await;
    let bobs_capture = capture_setting(&store, &bob).await.unwrap();
    let foreign_capture = capture_setting(&foreign, &foreign_alice).await.unwrap();
    let form = Reviewed::read(&store, &alice).await;
    let before = all_state(&store).await;
    denied(form.erase(&foreign, &alice).await);
    denied(form.erase(&store, &foreign_alice).await);
    denied(form.erase(&store, &bob).await);
    assert_eq!(all_state(&store).await, before);
    let receipt = form.erase(&store, &alice).await.unwrap();
    assert_eq!(receipt.affected_fact_ids, vec![first.fact_id]);
    assert_head(
        &store,
        &bob,
        bobs,
        PersonalFactState::Active,
        Some("SameName"),
    )
    .await;
    assert_head(
        &foreign,
        &foreign_alice,
        foreign_fact,
        PersonalFactState::Active,
        Some("SameName"),
    )
    .await;
    assert_eq!(capture_setting(&store, &bob).await.unwrap(), bobs_capture);
    assert_eq!(
        capture_setting(&foreign, &foreign_alice).await.unwrap(),
        foreign_capture
    );
    assert_eq!(
        candidate_state(&store, &bob, bobs.candidate_id).await,
        CandidateState::Published
    );
    assert!(get_candidate(&store, &bob, first.candidate_id)
        .await
        .unwrap()
        .is_none());
    // The same request UUID in another Bear is not the first Bear's operation.
    let other_form = Reviewed {
        request: form.request,
        ..Reviewed::read(&foreign, &foreign_alice).await
    };
    let other_receipt = other_form.erase(&foreign, &foreign_alice).await.unwrap();
    assert_ne!(other_receipt.erasure_id, receipt.erasure_id);
    assert_eq!(other_receipt.affected_fact_ids, vec![foreign_fact.fact_id]);
    assert_eq!(form.erase(&store, &alice).await.unwrap(), receipt);
}

#[tokio::test]
async fn revocation_and_generation_aba_reject_old_review_and_receipt_recovery() {
    use crate::identity_reconnection::{commit_review, review_revocation, ReplacementConsent};

    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "GenerationName", None).await;
    let form = Reviewed::read(&store, &alice).await;
    form.erase(&store, &alice).await.unwrap();
    let revocation = review_revocation(&store, UserId::new(99), alice.entity_id())
        .await
        .unwrap();
    commit_review(&store, &revocation, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let before = all_state(&store).await;
    denied(form.erase(&store, &alice).await);
    assert_eq!(all_state(&store).await, before);
    let fresh = reconnect(&store, &alice).await;
    assert!(fresh.authority_generation > alice.authority_generation);
    denied(form.erase(&store, &fresh).await);
    let current_form = Reviewed::read(&store, &fresh).await;
    let changed_spec = Reviewed {
        request: form.request,
        ..current_form
    };
    denied(changed_spec.erase(&store, &fresh).await);
    // The current owner can issue a new operation; historical receipt actor is not authority.
    let receipt = current_form.erase(&store, &fresh).await.unwrap();
    assert_eq!(receipt.affected_fact_ids, vec![fact.fact_id]);
    assert_erased(&store, &fresh, fact.candidate_id).await;
}

#[tokio::test]
async fn imported_quarantine_and_reconnection_do_not_revive_erased_bodies_or_capture() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "ImportedErasedName", None).await;
    let form = Reviewed::read(&store, &alice).await;
    form.erase(&store, &alice).await.unwrap();
    crate::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    denied(form.erase(&store, &alice).await);
    let fresh = reconnect(&store, &alice).await;
    assert!(!capture_enabled(&store, &fresh).await.unwrap());
    assert_erased(&store, &fresh, fact.candidate_id).await;
    assert_head(&store, &fresh, fact, PersonalFactState::Retracted, None).await;
    denied(form.erase(&store, &fresh).await);
    let before = all_state(&store).await;
    let capture = capture_setting(&store, &fresh).await.unwrap();
    for _ in 0..2 {
        crate::migrate::migrate_bear_sqlite_schema(store.pool())
            .await
            .unwrap();
        assert_eq!(all_state(&store).await, before);
        assert_erased(&store, &fresh, fact.candidate_id).await;
        assert_eq!(capture_setting(&store, &fresh).await.unwrap(), capture);
    }
    assert!(
        canonical_record_contents_match(&store, &[(&fact.fact_id.to_string(), "")])
            .await
            .unwrap()
    );
    let receipt = Reviewed::read(&store, &fresh)
        .await
        .erase(&store, &fresh)
        .await
        .unwrap();
    assert_eq!(receipt.affected_fact_ids, vec![fact.fact_id]);
}

#[tokio::test]
async fn new_owner_after_explicit_reconnection_cannot_replay_the_previous_actors_erasure() {
    use crate::identity_reconnection::{commit_review, review_reconnection, ReplacementConsent};

    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "MovedOwnerName", None).await;
    let form = Reviewed::read(&store, &alice).await;
    form.erase(&store, &alice).await.unwrap();
    crate::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let reconnection = review_reconnection(&store, UserId::new(99), alice.entity_id(), BOB)
        .await
        .unwrap();
    commit_review(&store, &reconnection, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let bob = person(&store, BOB).await;
    assert_eq!(bob.entity_id(), alice.entity_id());
    denied(form.erase(&store, &bob).await);
    assert_erased(&store, &bob, fact.candidate_id).await;
    let receipt = Reviewed::read(&store, &bob)
        .await
        .erase(&store, &bob)
        .await
        .unwrap();
    assert_eq!(receipt.affected_fact_ids, vec![fact.fact_id]);
}
