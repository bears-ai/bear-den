use super::*;

#[tokio::test]
async fn optout_retracts_current_withdraws_pending_and_reenable_or_retry_cannot_revive() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "Former name", None).await;
    let current = publish(&store, &alice, source, "Current name", Some(first.fact_id)).await;
    let pending = stage(&store, &alice, source, "Unapproved name").await;
    let bobs = publish(&store, &bob, source, "Bob", None).await;
    set_capture_enabled(&store, &alice, false).await.unwrap();
    assert!(!capture_enabled(&store, &alice).await.unwrap());
    assert_head(&store, &alice, current, PersonalFactState::Retracted, None).await;
    assert_head(&store, &bob, bobs, PersonalFactState::Active, Some("Bob")).await;
    assert_eq!(
        candidate_state(&store, &alice, pending.id()).await,
        CandidateState::Withdrawn
    );
    for receipt in [first, current] {
        assert_eq!(
            candidate_state(&store, &alice, receipt.candidate_id).await,
            CandidateState::Published
        );
    }
    let decision = row(&snapshot(&store).await.0, "retractions", current.fact_id);
    assert_eq!(decision[6], "capture_disabled");
    let before = all_state(&store).await;
    set_capture_enabled(&store, &alice, false).await.unwrap();
    assert_eq!(
        row(&snapshot(&store).await.0, "retractions", current.fact_id),
        decision
    );
    // A repeat opt-out may refresh its settings audit, never its immutable decision.
    assert_eq!(generic_snapshot(&store).await, before.1);
    let disabled = all_state(&store).await;
    assert_eq!(
        publish_preferred_name(&store, &source.proof(&alice), current.candidate_id, None)
            .await
            .unwrap(),
        current
    );
    assert_eq!(all_state(&store).await, disabled);
    set_capture_enabled(&store, &alice, true).await.unwrap();
    let before = all_state(&store).await;
    for receipt in [first, current] {
        assert_eq!(
            publish_preferred_name(&store, &source.proof(&alice), receipt.candidate_id, None)
                .await
                .unwrap(),
            receipt
        );
        assert!(!crate::record_visible(
            &store,
            &receipt.fact_id.to_string(),
            &AccessContext::for_authenticated_user(&alice)
        )
        .await
        .unwrap());
    }
    denied(
        publish_preferred_name(
            &store,
            &source.proof(&alice),
            pending.id(),
            Some(current.fact_id),
        )
        .await,
    );
    assert_eq!(all_state(&store).await, before);
    assert_head(&store, &alice, current, PersonalFactState::Retracted, None).await;
    let fresh = publish(
        &store,
        &alice,
        source,
        "New explicit approval",
        Some(current.fact_id),
    )
    .await;
    assert_ne!(fresh.candidate_id, pending.id());
    assert_head(
        &store,
        &alice,
        fresh,
        PersonalFactState::Active,
        Some("New explicit approval"),
    )
    .await;
}

#[tokio::test]
async fn imported_consent_disable_preserves_facts_for_authorized_reconnection_with_capture_false() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "Portable preferred name", None).await;
    let pending = stage(&store, &alice, source, "Portable pending").await;
    let canonical = generic_snapshot(&store).await;
    for _ in 0..2 {
        crate::quarantine_imported_authenticated_users(&store)
            .await
            .unwrap();
        assert_eq!(generic_snapshot(&store).await, canonical);
        assert!(table(&snapshot(&store).await.0, "retractions").is_empty());
        denied(current_preferred_name(&store, &alice).await);
        assert!(!crate::record_visible(
            &store,
            &fact.fact_id.to_string(),
            &AccessContext::for_authenticated_user(&alice)
        )
        .await
        .unwrap());
    }
    let fresh = reconnect(&store, &alice).await;
    assert!(!capture_enabled(&store, &fresh).await.unwrap());
    assert_head(
        &store,
        &fresh,
        fact,
        PersonalFactState::Active,
        Some("Portable preferred name"),
    )
    .await;
    assert!(crate::record_visible(
        &store,
        &fact.fact_id.to_string(),
        &AccessContext::for_authenticated_user(&fresh)
    )
    .await
    .unwrap());
    assert_eq!(
        candidate_state(&store, &fresh, fact.candidate_id).await,
        CandidateState::Published
    );
    assert_eq!(
        candidate_state(&store, &fresh, pending.id()).await,
        CandidateState::Quarantined
    );
    let before = all_state(&store).await;
    denied(
        publish_preferred_name(
            &store,
            &source.proof(&fresh),
            pending.id(),
            Some(fact.fact_id),
        )
        .await,
    );
    assert_eq!(all_state(&store).await, before);
    assert!(retract_preferred_name(&store, &fresh, fact.fact_id)
        .await
        .unwrap());
    assert_head(&store, &fresh, fact, PersonalFactState::Retracted, None).await;
    assert_eq!(
        row(&snapshot(&store).await.0, "retractions", fact.fact_id)[6],
        "owner_request"
    );
}
