use super::*;

#[tokio::test]
async fn exact_retry_recovers_original_locators_without_erasing_future_names_or_consent() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "OriginalNameSentinel", None).await;
    let original = Reviewed::read(&store, &alice).await;
    let receipt = original.erase(&store, &alice).await.unwrap();
    assert_eq!(receipt.affected_fact_ids, vec![first.fact_id]);
    assert_eq!(format!("{receipt:?}"), "PersonalErasureReceipt([redacted])");
    assert_eq!(receipt.clone(), receipt);
    let erased_state = all_state(&store).await;
    assert_eq!(original.erase(&store, &alice).await.unwrap(), receipt);
    assert_eq!(all_state(&store).await, erased_state);
    let disabled = capture_setting(&store, &alice).await.unwrap();
    set_capture_enabled_if_current(&store, &alice, true, disabled.revision)
        .await
        .unwrap();
    let (candidate, proof) = owner_candidate(&store, &alice, "FutureNameSentinel").await;
    let next = publish_preferred_name(&store, &proof, candidate.id(), Some(first.fact_id))
        .await
        .unwrap();
    let consent = capture_setting(&store, &alice).await.unwrap();
    let before = all_state(&store).await;
    // Deliberately reuse the original reviewed heads/revision, not the current UI snapshot.
    assert_eq!(original.erase(&store, &alice).await.unwrap(), receipt);
    assert_eq!(all_state(&store).await, before);
    assert_eq!(capture_setting(&store, &alice).await.unwrap(), consent);
    assert_head(
        &store,
        &alice,
        next,
        PersonalFactState::Active,
        Some("FutureNameSentinel"),
    )
    .await;
    assert_erased(&store, &alice, first.candidate_id).await;
    let fresh = Reviewed::read(&store, &alice).await;
    let next_erasure = fresh.erase(&store, &alice).await.unwrap();
    assert_ne!(next_erasure.erasure_id, receipt.erasure_id);
    assert_eq!(
        next_erasure.affected_fact_ids,
        vec![first.fact_id, next.fact_id]
    );
    assert_eq!(original.erase(&store, &alice).await.unwrap(), receipt);
    assert_eq!(fresh.erase(&store, &alice).await.unwrap(), next_erasure);
    assert_head(&store, &alice, next, PersonalFactState::Retracted, None).await;
}

#[tokio::test]
async fn request_id_reuse_rejects_each_changed_parameter_and_another_owner() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "ReviewSpecName", None).await;
    let form = Reviewed::read(&store, &alice).await;
    let receipt = form.erase(&store, &alice).await.unwrap();
    let current_capture = capture_setting(&store, &alice).await.unwrap().revision;
    let before = all_state(&store).await;
    for changed in [
        Reviewed { head: None, ..form },
        Reviewed {
            head: Some(PersonalFactId(Uuid::new_v4())),
            ..form
        },
        Reviewed {
            latest: None,
            ..form
        },
        Reviewed {
            latest: Some(PersonalCandidateId(Uuid::new_v4())),
            ..form
        },
        Reviewed {
            capture: current_capture,
            ..form
        },
    ] {
        denied(changed.erase(&store, &alice).await);
        assert_eq!(all_state(&store).await, before);
    }
    denied(form.erase(&store, &bob).await);
    let bobs_form = Reviewed {
        request: form.request,
        ..Reviewed::read(&store, &bob).await
    };
    denied(bobs_form.erase(&store, &bob).await);
    assert_eq!(all_state(&store).await, before);
    assert_eq!(form.erase(&store, &alice).await.unwrap(), receipt);
    assert_head(&store, &alice, fact, PersonalFactState::Retracted, None).await;
}

#[tokio::test]
async fn stale_capture_forms_and_owner_request_replay_cannot_restore_erased_candidates() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let capture = capture_setting(&store, &alice).await.unwrap();
    let owner = VerifiedOwnerSource::for_authorized_owner_request(&alice, Uuid::new_v4())
        .with_capture_revision(capture.revision);
    let candidate = stage_owner_preferred_name(
        &store,
        &owner,
        PreferredName::new("ErasedRequestName").unwrap(),
    )
    .await
    .unwrap();
    let review = review_candidate(
        &store,
        &alice,
        candidate.id(),
        CandidateReviewDecision::Defer,
        None,
    )
    .await
    .unwrap();
    let form = Reviewed::read(&store, &alice).await;
    form.erase(&store, &alice).await.unwrap();
    denied(set_capture_enabled_if_current(&store, &alice, true, capture.revision).await);
    denied(
        stage_owner_preferred_name(
            &store,
            &owner,
            PreferredName::new("ErasedRequestName").unwrap(),
        )
        .await,
    );
    set_capture_enabled(&store, &alice, true).await.unwrap();
    let before = all_state(&store).await;
    denied(
        stage_owner_preferred_name(
            &store,
            &owner,
            PreferredName::new("ErasedRequestName").unwrap(),
        )
        .await,
    );
    // Even the trusted revision-less retry cannot compare/recover a deleted value.
    let current_proof = VerifiedOwnerSource::for_authorized_owner_request(&alice, owner.request_id);
    denied(
        stage_owner_preferred_name(
            &store,
            &current_proof,
            PreferredName::new("ErasedRequestName").unwrap(),
        )
        .await,
    );
    denied(
        publish_preferred_name(
            &store,
            &VerifiedPersonalSource::from(&owner),
            candidate.id(),
            None,
        )
        .await,
    );
    denied(
        review_candidate(
            &store,
            &alice,
            candidate.id(),
            CandidateReviewDecision::Resume,
            Some(review.review_id),
        )
        .await,
    );
    denied(
        review_candidate(
            &store,
            &alice,
            candidate.id(),
            CandidateReviewDecision::Reject,
            Some(review.review_id),
        )
        .await,
    );
    // A value-free historical review receipt remains replayable but never changes Erased.
    assert_eq!(
        review_candidate(
            &store,
            &alice,
            candidate.id(),
            CandidateReviewDecision::Defer,
            None
        )
        .await
        .unwrap(),
        review
    );
    assert_eq!(all_state(&store).await, before);
    assert_erased(&store, &alice, candidate.id()).await;
    assert!(!withdraw_candidate(&store, &alice, candidate.id())
        .await
        .unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_exact_retries_across_independent_pools_make_one_erasure_and_revision() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "ConcurrentErasureName", None).await;
    let form = Reviewed::read(&store, &alice).await;
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(store.pool().connect_options().as_ref().clone())
        .await
        .unwrap();
    let other = BearMemoryStore::new(store.bear_id(), pool);
    let (first, second) = tokio::join!(form.erase(&store, &alice), form.erase(&other, &alice));
    let receipt = first.unwrap();
    assert_eq!(second.unwrap(), receipt);
    assert_eq!(receipt.affected_fact_ids, vec![fact.fact_id]);
    let after = all_state(&store).await;
    let capture = capture_setting(&store, &alice).await.unwrap();
    assert_eq!(form.erase(&other, &alice).await.unwrap(), receipt);
    assert_eq!(all_state(&store).await, after);
    assert_eq!(capture_setting(&store, &alice).await.unwrap(), capture);
    other.pool().close().await;
}
