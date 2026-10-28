use super::*;

#[tokio::test]
async fn late_retraction_failure_rolls_back_all_body_erasure_consent_audit_and_sequence() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let bob_source = note(&store).await;
    let first = publish(&store, &alice, source, "RollbackOldName", None).await;
    let head = publish(
        &store,
        &alice,
        source,
        "RollbackCurrentName",
        Some(first.fact_id),
    )
    .await;
    let pending = stage(&store, &alice, source, "RollbackCandidateName").await;
    let bobs = publish(&store, &bob, bob_source, "RollbackOtherOwner", None).await;
    let form = Reviewed::read(&store, &alice).await;
    // Existing fixture fails after erase_bodies and the consent revision write, before the
    // permanent ledger can commit. No additional test query or failure-only SQL is needed.
    fixtures::fail_late(&store, fixtures::Failure::Retraction).await;
    let before = all_state(&store).await;
    let capture = capture_setting(&store, &alice).await.unwrap();
    for _ in 0..2 {
        database_error(form.erase(&store, &alice).await);
        assert!(pending_erasure_cleanups(&store, &alice, 100)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(all_state(&store).await, before);
        assert_eq!(capture_setting(&store, &alice).await.unwrap(), capture);
        assert_eq!(
            candidate_state(&store, &alice, pending.id()).await,
            CandidateState::Pending
        );
        assert_head(
            &store,
            &alice,
            head,
            PersonalFactState::Active,
            Some("RollbackCurrentName"),
        )
        .await;
        assert!(canonical_record_contents_match(
            &store,
            &[
                (&first.fact_id.to_string(), "RollbackOldName"),
                (&head.fact_id.to_string(), "RollbackCurrentName"),
            ]
        )
        .await
        .unwrap());
    }
    fixtures::clear_failure(&store).await;
    let receipt = form.erase(&store, &alice).await.unwrap();
    assert_eq!(receipt.affected_fact_ids, vec![first.fact_id, head.fact_id]);
    assert_erased(&store, &alice, pending.id()).await;
    assert_head(&store, &alice, head, PersonalFactState::Retracted, None).await;
    assert_head(
        &store,
        &bob,
        bobs,
        PersonalFactState::Active,
        Some("RollbackOtherOwner"),
    )
    .await;
    assert!(canonical_record_contents_match(
        &store,
        &[
            (&first.fact_id.to_string(), ""),
            (&head.fact_id.to_string(), ""),
        ]
    )
    .await
    .unwrap());
    assert_eq!(form.erase(&store, &alice).await.unwrap(), receipt);
}
