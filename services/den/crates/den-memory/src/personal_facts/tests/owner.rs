use super::*;
use crate::personal_candidates::{
    review_candidate, stage_owner_preferred_name, CandidateReviewDecision, CandidateSource,
    VerifiedOwnerSource,
};

async fn owner_candidate(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    name: &str,
) -> (PersonalCandidate, VerifiedPersonalSource) {
    let owner = VerifiedOwnerSource::for_authorized_owner_request(binding, Uuid::new_v4());
    let candidate = stage_owner_preferred_name(store, &owner, PreferredName::new(name).unwrap())
        .await
        .unwrap();
    (candidate, VerifiedPersonalSource::from(&owner))
}

#[tokio::test]
async fn owner_approval_and_correction_require_separate_decisions_and_exact_head() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let (first, proof) = owner_candidate(&store, &alice, "Alice").await;
    assert!(current_preferred_name(&store, &alice)
        .await
        .unwrap()
        .is_none());
    let CandidateSource::OwnerRequest { request_id } = first.source() else {
        panic!("owner provenance")
    };
    let bob_proof = VerifiedPersonalSource::from(
        &VerifiedOwnerSource::for_authorized_owner_request(&bob, request_id),
    );
    denied(publish_preferred_name(&store, &bob_proof, first.id(), None).await);
    let receipt = publish_preferred_name(&store, &proof, first.id(), None)
        .await
        .unwrap();
    assert_head(
        &store,
        &alice,
        receipt,
        PersonalFactState::Active,
        Some("Alice"),
    )
    .await;
    let (correction, correction_proof) = owner_candidate(&store, &alice, "Alicia").await;
    denied(publish_preferred_name(&store, &correction_proof, correction.id(), None).await);
    assert_head(
        &store,
        &alice,
        receipt,
        PersonalFactState::Active,
        Some("Alice"),
    )
    .await;
    let corrected = publish_preferred_name(
        &store,
        &correction_proof,
        correction.id(),
        Some(receipt.fact_id),
    )
    .await
    .unwrap();
    assert_eq!(corrected.previous_fact_id, Some(receipt.fact_id));
    assert_head(
        &store,
        &alice,
        corrected,
        PersonalFactState::Active,
        Some("Alicia"),
    )
    .await;
    assert_eq!(
        publish_preferred_name(&store, &proof, first.id(), None)
            .await
            .unwrap(),
        receipt
    );
    assert_head(
        &store,
        &alice,
        corrected,
        PersonalFactState::Active,
        Some("Alicia"),
    )
    .await;
    assert!(current_preferred_name(&store, &bob)
        .await
        .unwrap()
        .is_none());
    assert!(
        crate::fetch_record_by_id(&store, &corrected.fact_id.to_string())
            .await
            .unwrap()
            .is_none()
    );
    set_capture_enabled(&store, &alice, false).await.unwrap();
    assert_eq!(
        publish_preferred_name(&store, &correction_proof, correction.id(), None)
            .await
            .unwrap(),
        corrected
    );
    assert_head(
        &store,
        &alice,
        corrected,
        PersonalFactState::Retracted,
        None,
    )
    .await;
}

#[tokio::test]
async fn source_variant_and_request_locator_must_match_the_protected_candidate() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let conversation = note(&store).await;
    let from_conversation = stage(&store, &alice, conversation, "Conversation name").await;
    let (from_owner, owner_proof) = owner_candidate(&store, &alice, "Owner name").await;
    let wrong_request = VerifiedPersonalSource::from(
        &VerifiedOwnerSource::for_authorized_owner_request(&alice, Uuid::new_v4()),
    );
    let before = all_state(&store).await;
    denied(publish_preferred_name(&store, &owner_proof, from_conversation.id(), None).await);
    denied(
        publish_preferred_name(&store, &conversation.proof(&alice), from_owner.id(), None).await,
    );
    denied(publish_preferred_name(&store, &wrong_request, from_owner.id(), None).await);
    assert_eq!(all_state(&store).await, before);
    // Adding owner-looking claims to generic metadata does not change protected provenance.
    let CandidateSource::OwnerRequest { request_id } = from_owner.source() else {
        panic!("owner provenance")
    };
    let forged = crate::append_memory_record(
        &store, &conversation.path(), "note", "pair", None, "Not a human settings request",
        &json!({"source_kind": "owner_request", "request_id": request_id, "owner_user_id": ALICE.get()}),
    ).await.unwrap();
    let forged_proof = VerifiedPersonalSource::for_authorized_conversation(
        &alice,
        conversation.conversation,
        Uuid::parse_str(&forged.memory_id).unwrap(),
    );
    denied(publish_preferred_name(&store, &forged_proof, from_owner.id(), None).await);
    assert_eq!(
        get_candidate(&store, &alice, from_owner.id())
            .await
            .unwrap()
            .unwrap()
            .source(),
        from_owner.source()
    );
    publish_preferred_name(&store, &owner_proof, from_owner.id(), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn new_owner_approval_rechecks_consent_review_state_and_identity_generation() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let (candidate, proof) = owner_candidate(&store, &alice, "Alice").await;
    review_candidate(
        &store,
        &alice,
        candidate.id(),
        CandidateReviewDecision::Expire,
        None,
    )
    .await
    .unwrap();
    denied(publish_preferred_name(&store, &proof, candidate.id(), None).await);
    let (pending, pending_proof) = owner_candidate(&store, &alice, "Pending").await;
    set_capture_enabled(&store, &alice, false).await.unwrap();
    denied(publish_preferred_name(&store, &pending_proof, pending.id(), None).await);
    set_capture_enabled(&store, &alice, true).await.unwrap();
    denied(publish_preferred_name(&store, &pending_proof, pending.id(), None).await);
    let (imported, imported_proof) = owner_candidate(&store, &alice, "Imported").await;
    crate::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    denied(publish_preferred_name(&store, &imported_proof, imported.id(), None).await);
    let restored = reconnect(&store, &alice).await;
    set_capture_enabled(&store, &restored, true).await.unwrap();
    let CandidateSource::OwnerRequest { request_id } = imported.source() else {
        panic!("owner provenance")
    };
    let restored_proof = VerifiedPersonalSource::from(
        &VerifiedOwnerSource::for_authorized_owner_request(&restored, request_id),
    );
    denied(publish_preferred_name(&store, &restored_proof, imported.id(), None).await);
    assert!(current_preferred_name(&store, &restored)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn reopening_schema_preserves_mixed_sources_and_review_publication_import_history() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let conversation = note(&store).await;
    let historical = stage(&store, &alice, conversation, "Old evidence").await;
    review_candidate(
        &store,
        &alice,
        historical.id(),
        CandidateReviewDecision::Defer,
        None,
    )
    .await
    .unwrap();
    let (owner, proof) = owner_candidate(&store, &alice, "Owner approved").await;
    publish_preferred_name(&store, &proof, owner.id(), None)
        .await
        .unwrap();
    crate::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let binding = reconnect(&store, &alice).await;
    let before = all_state(&store).await;
    for _ in 0..2 {
        crate::migrate::migrate_bear_sqlite_schema(store.pool())
            .await
            .unwrap();
        assert_eq!(all_state(&store).await, before);
        assert_eq!(
            get_candidate(&store, &binding, historical.id())
                .await
                .unwrap()
                .unwrap()
                .source(),
            historical.source()
        );
        assert_eq!(
            get_candidate(&store, &binding, owner.id())
                .await
                .unwrap()
                .unwrap()
                .source(),
            owner.source()
        );
    }
}
