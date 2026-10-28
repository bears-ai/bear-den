use super::*;

#[tokio::test]
async fn new_publication_invalidates_review_even_when_latest_candidate_is_unchanged() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let (candidate, proof) = owner_candidate(&store, &alice, "ReviewedCandidate").await;
    let form = Reviewed::read(&store, &alice).await;
    assert_eq!(form.latest, Some(candidate.id()));
    let fact = publish_preferred_name(&store, &proof, candidate.id(), None)
        .await
        .unwrap();
    let before = all_state(&store).await;
    denied(form.erase(&store, &alice).await);
    assert_eq!(all_state(&store).await, before);
    assert_head(
        &store,
        &alice,
        fact,
        PersonalFactState::Active,
        Some("ReviewedCandidate"),
    )
    .await;
    let fresh = Reviewed::read(&store, &alice).await;
    assert_eq!(fresh.latest, form.latest);
    fresh.erase(&store, &alice).await.unwrap();
}

#[tokio::test]
async fn new_candidate_invalidates_review_without_any_publication_or_consent_change() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    for terminal in [false, true] {
        let form = Reviewed::read(&store, &alice).await;
        let (candidate, _) = owner_candidate(&store, &alice, "UnreviewedNewCandidate").await;
        if terminal {
            review_candidate(
                &store,
                &alice,
                candidate.id(),
                CandidateReviewDecision::Reject,
                None,
            )
            .await
            .unwrap();
        }
        let before = all_state(&store).await;
        denied(form.erase(&store, &alice).await);
        assert_eq!(all_state(&store).await, before);
        assert_eq!(
            capture_setting(&store, &alice).await.unwrap().revision,
            form.capture
        );
    }
    Reviewed::read(&store, &alice)
        .await
        .erase(&store, &alice)
        .await
        .unwrap();
}

#[tokio::test]
async fn consent_aba_invalidates_review_even_with_identical_head_and_latest_candidate() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    owner_candidate(&store, &alice, "ConsentCandidate").await;
    let form = Reviewed::read(&store, &alice).await;
    set_capture_enabled_if_current(&store, &alice, false, form.capture)
        .await
        .unwrap();
    let disabled = capture_setting(&store, &alice).await.unwrap();
    set_capture_enabled_if_current(&store, &alice, true, disabled.revision)
        .await
        .unwrap();
    let before = all_state(&store).await;
    let current = capture_setting(&store, &alice).await.unwrap();
    denied(form.erase(&store, &alice).await);
    assert_eq!(all_state(&store).await, before);
    assert_eq!(capture_setting(&store, &alice).await.unwrap(), current);
    let fresh = Reviewed::read(&store, &alice).await;
    assert_eq!((fresh.head, fresh.latest), (form.head, form.latest));
    fresh.erase(&store, &alice).await.unwrap();
}

#[tokio::test]
async fn concurrent_owner_capture_and_erasure_cannot_silently_include_unreviewed_content() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let form = Reviewed::read(&store, &alice).await;
    let source = VerifiedOwnerSource::for_authorized_owner_request(&alice, Uuid::new_v4())
        .with_capture_revision(form.capture);
    let (erasure, capture) = tokio::join!(
        form.erase(&store, &alice),
        stage_owner_preferred_name(
            &store,
            &source,
            PreferredName::new("ConcurrentNewName").unwrap()
        ),
    );
    match (erasure, capture) {
        (Ok(receipt), Err(DenError::Authorization(_))) => {
            assert!(receipt.affected_fact_ids.is_empty());
            assert!(list_candidates(&store, &alice, 100)
                .await
                .unwrap()
                .is_empty());
        }
        (Err(DenError::Authorization(_)), Ok(candidate)) => {
            assert_eq!(candidate.state(), CandidateState::Pending);
            assert_eq!(
                candidate.preferred_name().unwrap().as_str(),
                "ConcurrentNewName"
            );
            assert!(capture_enabled(&store, &alice).await.unwrap());
        }
        other => panic!("unexpected capture/erasure results: {other:?}"),
    }
}
