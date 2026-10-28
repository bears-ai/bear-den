use super::*;

async fn owner_stage(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    request: Uuid,
    value: &str,
) -> Result<PersonalCandidate, DenError> {
    stage_owner_preferred_name(
        store,
        &VerifiedOwnerSource::for_authorized_owner_request(binding, request),
        PreferredName::new(value).unwrap(),
    )
    .await
}

#[tokio::test]
async fn owner_intake_requires_opt_in_and_never_creates_source_or_generic_content() {
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let request = Uuid::new_v4();
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    assert_denied(owner_stage(&store, &alice, request, "Alice").await);
    assert_eq!(snapshot(&store).await, before);
    set_capture_enabled(&store, &alice, true).await.unwrap();
    let candidate = owner_stage(&store, &alice, request, " Alice ")
        .await
        .unwrap();
    assert_eq!(candidate.state(), CandidateState::Pending);
    assert_eq!(
        candidate.source(),
        CandidateSource::OwnerRequest {
            request_id: request
        }
    );
    assert_eq!(candidate.source_conversation_id(), None);
    assert_eq!(candidate.source_memory_id(), None);
    assert_eq!(candidate.source_user_id(), ALICE);
    assert_eq!(candidate.source_bear_id(), alice.bear_id());
    assert_eq!(
        candidate.source_binding_generation(),
        alice.authority_generation
    );
    assert_eq!(candidate.preferred_name().unwrap().as_str(), "Alice");
    assert_eq!(generic_snapshot(&store).await, generic_before);
    assert!(
        crate::personal_facts::current_preferred_name(&store, &alice)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        crate::fetch_record_by_id(&store, &candidate.id().to_string())
            .await
            .unwrap()
            .is_none()
    );
    assert!(crate::list_reviewable_memory_proposals(&store)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn owner_request_retries_are_immutable_actor_scoped_and_bear_isolated() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let request = Uuid::new_v4();
    let original = owner_stage(&store, &alice, request, "Alice").await.unwrap();
    let before = snapshot(&store).await;
    let replay = owner_stage(&store, &alice, request, " Alice ")
        .await
        .unwrap();
    assert_eq!(replay.id(), original.id());
    assert_eq!(replay.created_at(), original.created_at());
    assert_denied(owner_stage(&store, &alice, request, "Alicia").await);
    assert_denied(owner_stage(&store, &bob, request, "Alice").await);
    assert!(get_candidate(&store, &bob, original.id())
        .await
        .unwrap()
        .is_none());
    assert_eq!(snapshot(&store).await, before);
    let foreign = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    assert_denied(owner_stage(&foreign, &alice, request, "Alice").await);
    let foreign_owner = opted_in(&foreign, ALICE).await;
    let other = owner_stage(&foreign, &foreign_owner, request, "Different Bear")
        .await
        .unwrap();
    assert_ne!(other.id(), original.id());
    assert_eq!(other.source_bear_id(), foreign_owner.bear_id());
    assert!(get_candidate(&foreign, &foreign_owner, original.id())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn concurrent_owner_retries_allocate_one_candidate_and_one_sequence() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let request = Uuid::new_v4();
    let before = next_sequence(&store).await;
    let (first, second) = tokio::join!(
        owner_stage(&store, &alice, request, "Alice"),
        owner_stage(&store, &alice, request, "Alice"),
    );
    assert_eq!(first.unwrap().id(), second.unwrap().id());
    assert_eq!(next_sequence(&store).await, before + 1);
    assert_eq!(list_candidates(&store, &alice, 100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn owner_reviews_and_retries_do_not_resurrect_after_opt_out_or_closure() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let request = Uuid::new_v4();
    let candidate = owner_stage(&store, &alice, request, "Alice").await.unwrap();
    let deferred = review_candidate(
        &store,
        &alice,
        candidate.id(),
        CandidateReviewDecision::Defer,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        current_candidate(&store, &alice, candidate.id())
            .await
            .state(),
        CandidateState::Deferred
    );
    let resumed = review_candidate(
        &store,
        &alice,
        candidate.id(),
        CandidateReviewDecision::Resume,
        Some(deferred.review_id),
    )
    .await
    .unwrap();
    set_capture_enabled(&store, &alice, false).await.unwrap();
    assert_denied(owner_stage(&store, &alice, request, "Alice").await);
    set_capture_enabled(&store, &alice, true).await.unwrap();
    let replay = owner_stage(&store, &alice, request, "Alice").await.unwrap();
    assert_eq!(replay.id(), candidate.id());
    assert_eq!(replay.state(), CandidateState::Withdrawn);
    assert_denied(
        review_candidate(
            &store,
            &alice,
            candidate.id(),
            CandidateReviewDecision::Defer,
            Some(resumed.review_id),
        )
        .await,
    );
    let new_request = Uuid::new_v4();
    let new_candidate = owner_stage(&store, &alice, new_request, "Alice")
        .await
        .unwrap();
    review_candidate(
        &store,
        &alice,
        new_candidate.id(),
        CandidateReviewDecision::Reject,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        owner_stage(&store, &alice, new_request, "Alice")
            .await
            .unwrap()
            .state(),
        CandidateState::Rejected
    );
    assert_denied(owner_stage(&store, &alice, new_request, "Changed after rejection").await);
}

#[tokio::test]
async fn import_and_old_generations_cannot_reissue_or_resume_an_owner_request() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let request = Uuid::new_v4();
    let original = owner_stage(&store, &alice, request, "Alice").await.unwrap();
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert_denied(owner_stage(&store, &alice, request, "Alice").await);
    let fresh = reconnect(
        &store,
        alice.entity_id(),
        ALICE,
        ReplacementConsent::NoReplacement,
    )
    .await;
    assert!(!capture_enabled(&store, &fresh).await.unwrap());
    set_capture_enabled(&store, &fresh, true).await.unwrap();
    let imported = current_candidate(&store, &fresh, original.id()).await;
    assert_eq!(imported.state(), CandidateState::Quarantined);
    assert_eq!(imported.source(), original.source());
    assert_eq!(
        imported.source_binding_generation(),
        original.source_binding_generation()
    );
    assert_denied(owner_stage(&store, &fresh, request, "Alice").await);
    assert_denied(
        review_candidate(
            &store,
            &fresh,
            original.id(),
            CandidateReviewDecision::Resume,
            None,
        )
        .await,
    );
    assert_denied(
        crate::personal_facts::publish_preferred_name(
            &store,
            &VerifiedPersonalSource::from(&VerifiedOwnerSource::for_authorized_owner_request(
                &fresh, request,
            )),
            original.id(),
            None,
        )
        .await,
    );
    let next = owner_stage(&store, &fresh, Uuid::new_v4(), "Alice")
        .await
        .unwrap();
    assert_ne!(next.id(), original.id());
    assert_eq!(next.source_binding_generation(), fresh.authority_generation);
    revoke(&store, &fresh).await;
    assert_denied(owner_stage(&store, &fresh, Uuid::new_v4(), "No stale authority").await);
}

#[tokio::test]
async fn owner_source_cannot_bypass_the_existing_conversation_intake_boundary() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let owner = VerifiedOwnerSource::for_authorized_owner_request(&alice, Uuid::new_v4());
    let before = snapshot(&store).await;
    assert_denied(
        stage_preferred_name(
            &store,
            &VerifiedPersonalSource::from(&owner),
            PreferredName::new("Alice").unwrap(),
        )
        .await,
    );
    let missing_note =
        VerifiedPersonalSource::for_authorized_conversation(&alice, Uuid::new_v4(), Uuid::new_v4());
    assert_denied(
        stage_preferred_name(&store, &missing_note, PreferredName::new("Alice").unwrap()).await,
    );
    assert_eq!(snapshot(&store).await, before);
}

#[tokio::test]
async fn retained_owner_source_rejects_stale_consent_before_insert_and_deduplication() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let capture = capture_setting(&store, &alice).await.unwrap();
    let request = Uuid::new_v4();
    let source = VerifiedOwnerSource::for_authorized_owner_request(&alice, request)
        .with_capture_revision(capture.revision);
    let unsubmitted = VerifiedOwnerSource::for_authorized_owner_request(&alice, Uuid::new_v4())
        .with_capture_revision(capture.revision);
    let original =
        stage_owner_preferred_name(&store, &source, PreferredName::new("Alice").unwrap())
            .await
            .unwrap();
    assert_eq!(
        stage_owner_preferred_name(&store, &source, PreferredName::new("Alice").unwrap())
            .await
            .unwrap()
            .id(),
        original.id(),
    );
    set_capture_enabled_if_current(&store, &alice, false, capture.revision)
        .await
        .unwrap();
    let disabled = capture_setting(&store, &alice).await.unwrap();
    set_capture_enabled_if_current(&store, &alice, true, disabled.revision)
        .await
        .unwrap();
    let current = capture_setting(&store, &alice).await.unwrap();
    assert_ne!(current.revision, capture.revision);
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    for retained in [&source, &unsubmitted] {
        assert_denied(
            stage_owner_preferred_name(&store, retained, PreferredName::new("Alice").unwrap())
                .await,
        );
        assert_eq!(snapshot(&store).await, before);
        assert_eq!(capture_setting(&store, &alice).await.unwrap(), current);
    }
    assert_eq!(generic_snapshot(&store).await, generic_before);
    assert_eq!(
        current_candidate(&store, &alice, original.id())
            .await
            .state(),
        CandidateState::Withdrawn
    );
    let fresh = VerifiedOwnerSource::for_authorized_owner_request(&alice, Uuid::new_v4())
        .with_capture_revision(current.revision);
    let candidate =
        stage_owner_preferred_name(&store, &fresh, PreferredName::new("Alice").unwrap())
            .await
            .unwrap();
    assert_eq!(candidate.state(), CandidateState::Pending);
}
