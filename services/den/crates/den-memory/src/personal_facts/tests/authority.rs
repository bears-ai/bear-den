use super::*;

#[tokio::test]
async fn exact_owner_bear_and_epoch_are_required_for_reads_decisions_and_receipt_replay() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "Alice private", None).await;
    let pending = stage(&store, &alice, source, "Alice correction").await;
    let other_store = new_test_store().await;
    let foreign = opted_in(&other_store, ALICE).await;
    let same_bear_database = BearMemoryStore::new(store.bear_id(), other_store.pool().clone());
    let impostor = opted_in(&same_bear_database, ALICE).await;
    let wrong_bear = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let mut stale = alice.clone();
    stale.authority_generation += 1;
    let before = all_state(&store).await;
    assert!(current_preferred_name(&store, &bob)
        .await
        .unwrap()
        .is_none());
    for owner in [&bob, &foreign, &impostor, &stale] {
        for id in [pending.id(), first.candidate_id] {
            denied(
                publish_preferred_name(&store, &source.proof(owner), id, Some(first.fact_id)).await,
            );
        }
        denied(retract_preferred_name(&store, owner, first.fact_id).await);
        let access =
            AccessContext::for_authenticated_user(owner).with_audience([alice.entity_id()]);
        assert!(
            !crate::record_visible(&store, &first.fact_id.to_string(), &access)
                .await
                .unwrap()
        );
    }
    for owner in [&foreign, &impostor, &stale] {
        denied(current_preferred_name(&store, owner).await);
    }
    denied(current_preferred_name(&wrong_bear, &alice).await);
    denied(
        publish_preferred_name(
            &wrong_bear,
            &source.proof(&alice),
            pending.id(),
            Some(first.fact_id),
        )
        .await,
    );
    denied(retract_preferred_name(&wrong_bear, &alice, first.fact_id).await);
    for access in [
        AccessContext::empty(),
        AccessContext::empty().with_audience([alice.entity_id()]),
    ] {
        assert!(
            !crate::record_visible(&store, &first.fact_id.to_string(), &access)
                .await
                .unwrap()
        );
    }
    assert!(crate::record_visible(
        &store,
        &first.fact_id.to_string(),
        &AccessContext::for_authenticated_user(&alice)
    )
    .await
    .unwrap());
    assert_eq!(all_state(&store).await, before);
    let wrong_source = note(&store).await;
    let before_source_denial = all_state(&store).await;
    denied(
        publish_preferred_name(
            &store,
            &wrong_source.proof(&alice),
            pending.id(),
            Some(first.fact_id),
        )
        .await,
    );
    let mismatched_conversation =
        VerifiedPersonalSource::for_authorized_conversation(&alice, Uuid::new_v4(), source.memory);
    denied(
        publish_preferred_name(
            &store,
            &mismatched_conversation,
            pending.id(),
            Some(first.fact_id),
        )
        .await,
    );
    assert_eq!(all_state(&store).await, before_source_denial);

    use crate::identity_reconnection::{commit_review, review_revocation, ReplacementConsent};
    let review = review_revocation(&store, UserId::new(99), alice.entity_id())
        .await
        .unwrap();
    commit_review(&store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let fresh = reconnect(&store, &alice).await;
    assert!(fresh.authority_generation > alice.authority_generation);
    let revoked = all_state(&store).await;
    denied(current_preferred_name(&store, &alice).await);
    denied(publish_preferred_name(&store, &source.proof(&alice), first.candidate_id, None).await);
    denied(retract_preferred_name(&store, &alice, first.fact_id).await);
    assert!(!crate::record_visible(
        &store,
        &first.fact_id.to_string(),
        &AccessContext::for_authenticated_user(&alice)
    )
    .await
    .unwrap());
    assert_eq!(all_state(&store).await, revoked);
    assert_head(
        &store,
        &fresh,
        first,
        PersonalFactState::Active,
        Some("Alice private"),
    )
    .await;
}

#[tokio::test]
async fn consent_pending_state_and_current_source_privacy_are_rechecked_before_publication() {
    // Source state is canonical; an earlier proof is not a permanent publication permission.
    for (visibility, metadata, invalid_at) in [
        ("private", "{}", None),
        ("hidden", "{}", None),
        ("quarantined", "{}", None),
        ("withdrawn", "{}", None),
        ("normal", r#"{"lifecycle":{"status":"archived"}}"#, None),
        ("normal", "not json", None),
        ("normal", "{}", Some("2026-01-01T00:00:00Z")),
    ] {
        let store = new_test_store().await;
        let alice = opted_in(&store, ALICE).await;
        let source = note(&store).await;
        let candidate = stage(&store, &alice, source, "Never publish private source").await;
        fixtures::alter_record(
            &store,
            &source.memory.to_string(),
            &source.path(),
            visibility,
            metadata,
            invalid_at,
        )
        .await;
        let before = all_state(&store).await;
        denied(publish_preferred_name(&store, &source.proof(&alice), candidate.id(), None).await);
        assert_eq!(all_state(&store).await, before);
        assert!(current_preferred_name(&store, &alice)
            .await
            .unwrap()
            .is_none());
    }
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let access_gated = stage(&store, &alice, source, "Access-gated evidence").await;
    crate::append_relation(
        &store,
        &source.memory.to_string(),
        alice.entity_id(),
        "audience",
        &json!({}),
        "pair",
        None,
        None,
    )
    .await
    .unwrap();
    let before = all_state(&store).await;
    denied(publish_preferred_name(&store, &source.proof(&alice), access_gated.id(), None).await);
    assert_eq!(all_state(&store).await, before);
    // Owning an audience is not permission to publish from a classified source.
    let source = note(&store).await;
    let withdrawn = stage(&store, &alice, source, "Withdrawn").await;
    assert!(withdraw_candidate(&store, &alice, withdrawn.id())
        .await
        .unwrap());
    let pending = stage(&store, &alice, source, "Opted out").await;
    set_capture_enabled(&store, &alice, false).await.unwrap();
    let before = all_state(&store).await;
    for id in [
        withdrawn.id(),
        pending.id(),
        PersonalCandidateId(Uuid::new_v4()),
    ] {
        denied(publish_preferred_name(&store, &source.proof(&alice), id, None).await);
        assert_eq!(all_state(&store).await, before);
    }
    set_capture_enabled(&store, &alice, true).await.unwrap();
    denied(publish_preferred_name(&store, &source.proof(&alice), pending.id(), None).await);
}
