use super::*;

#[tokio::test]
async fn pending_cleanup_survives_erasure_retries_and_migration_until_explicit_ack() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "PendingCleanupName", None).await;
    let form = Reviewed::read(&store, &alice).await;
    assert!(pending_erasure_cleanups(&store, &alice, 100)
        .await
        .unwrap()
        .is_empty());
    let receipt = form.erase(&store, &alice).await.unwrap();
    for _ in 0..2 {
        assert_eq!(form.erase(&store, &alice).await.unwrap(), receipt);
        crate::migrate::migrate_bear_sqlite_schema(store.pool())
            .await
            .unwrap();
        assert_eq!(
            pending_erasure_cleanups(&store, &alice, 100).await.unwrap(),
            vec![receipt.clone()]
        );
    }
    assert_eq!(receipt.affected_fact_ids, vec![fact.fact_id]);
    let before = all_state(&store).await;
    let capture = capture_setting(&store, &alice).await.unwrap();
    for _ in 0..2 {
        mark_erasure_cleanup_complete(&store, &alice, receipt.erasure_id)
            .await
            .unwrap();
        assert!(pending_erasure_cleanups(&store, &alice, 100)
            .await
            .unwrap()
            .is_empty());
        // Exact erasure replay preserves complete status; it does not enqueue work again.
        assert_eq!(form.erase(&store, &alice).await.unwrap(), receipt);
        assert!(pending_erasure_cleanups(&store, &alice, 100)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(all_state(&store).await, before);
        assert_eq!(capture_setting(&store, &alice).await.unwrap(), capture);
    }
}

#[tokio::test]
async fn ack_and_pending_reads_require_current_owner_and_local_bear() {
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let bob = person(&store, BOB).await;
    let receipt = Reviewed::read(&store, &alice)
        .await
        .erase(&store, &alice)
        .await
        .unwrap();
    let foreign = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let foreign_owner = person(&foreign, ALICE).await;
    for (target, owner) in [(&store, &bob), (&foreign, &foreign_owner)] {
        assert!(pending_erasure_cleanups(target, owner, 100)
            .await
            .unwrap()
            .is_empty());
        denied(mark_erasure_cleanup_complete(target, owner, receipt.erasure_id).await);
    }
    denied(pending_erasure_cleanups(&foreign, &alice, 100).await);
    denied(mark_erasure_cleanup_complete(&foreign, &alice, receipt.erasure_id).await);
    denied(mark_erasure_cleanup_complete(&store, &alice, PersonalErasureId(Uuid::new_v4())).await);
    assert_eq!(
        pending_erasure_cleanups(&store, &alice, 100).await.unwrap(),
        vec![receipt]
    );
}

#[tokio::test]
async fn cleanup_ack_never_erases_later_names_or_changes_their_capture_revision() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "ErasedCleanupName", None).await;
    let receipt = Reviewed::read(&store, &alice)
        .await
        .erase(&store, &alice)
        .await
        .unwrap();
    set_capture_enabled(&store, &alice, true).await.unwrap();
    let (candidate, proof) = owner_candidate(&store, &alice, "LaterCleanupName").await;
    let current = publish_preferred_name(&store, &proof, candidate.id(), Some(first.fact_id))
        .await
        .unwrap();
    let before = all_state(&store).await;
    let capture = capture_setting(&store, &alice).await.unwrap();
    assert_eq!(
        pending_erasure_cleanups(&store, &alice, 100).await.unwrap(),
        vec![receipt.clone()]
    );
    mark_erasure_cleanup_complete(&store, &alice, receipt.erasure_id)
        .await
        .unwrap();
    assert_eq!(all_state(&store).await, before);
    assert_eq!(capture_setting(&store, &alice).await.unwrap(), capture);
    assert_head(
        &store,
        &alice,
        current,
        PersonalFactState::Active,
        Some("LaterCleanupName"),
    )
    .await;
    assert_eq!(
        candidate_state(&store, &alice, candidate.id()).await,
        CandidateState::Published
    );
    assert!(canonical_record_contents_match(
        &store,
        &[
            (&first.fact_id.to_string(), ""),
            (&current.fact_id.to_string(), "LaterCleanupName"),
        ]
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn cleanup_after_generation_and_actor_change_is_allowed_only_for_the_current_same_person() {
    use crate::identity_reconnection::{commit_review, review_reconnection, ReplacementConsent};

    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let form = Reviewed::read(&store, &alice).await;
    let receipt = form.erase(&store, &alice).await.unwrap();
    crate::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    denied(pending_erasure_cleanups(&store, &alice, 100).await);
    denied(mark_erasure_cleanup_complete(&store, &alice, receipt.erasure_id).await);
    let review = review_reconnection(&store, UserId::new(99), alice.entity_id(), BOB)
        .await
        .unwrap();
    commit_review(&store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let current = person(&store, BOB).await;
    assert_eq!(current.entity_id(), alice.entity_id());
    assert!(current.authority_generation > alice.authority_generation);
    denied(form.erase(&store, &current).await);
    assert_eq!(
        pending_erasure_cleanups(&store, &current, 100)
            .await
            .unwrap(),
        vec![receipt.clone()]
    );
    mark_erasure_cleanup_complete(&store, &current, receipt.erasure_id)
        .await
        .unwrap();
    assert!(pending_erasure_cleanups(&store, &current, 100)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn pending_cleanup_is_bounded_oldest_first_and_ack_changes_only_the_selected_operation() {
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let first_form = Reviewed::read(&store, &alice).await;
    let first = first_form.erase(&store, &alice).await.unwrap();
    let second_form = Reviewed::read(&store, &alice).await;
    let second = second_form.erase(&store, &alice).await.unwrap();
    assert_eq!(
        pending_erasure_cleanups(&store, &alice, 100).await.unwrap(),
        vec![first.clone(), second.clone()]
    );
    assert_eq!(
        pending_erasure_cleanups(&store, &alice, 1).await.unwrap(),
        vec![first.clone()]
    );
    mark_erasure_cleanup_complete(&store, &alice, first.erasure_id)
        .await
        .unwrap();
    assert_eq!(
        pending_erasure_cleanups(&store, &alice, 1).await.unwrap(),
        vec![second.clone()]
    );
    assert_eq!(first_form.erase(&store, &alice).await.unwrap(), first);
    assert_eq!(
        pending_erasure_cleanups(&store, &alice, 100).await.unwrap(),
        vec![second.clone()]
    );
    mark_erasure_cleanup_complete(&store, &alice, second.erasure_id)
        .await
        .unwrap();
    assert!(pending_erasure_cleanups(&store, &alice, 100)
        .await
        .unwrap()
        .is_empty());
}
