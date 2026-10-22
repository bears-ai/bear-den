use super::*;

#[tokio::test]
async fn publication_minimizes_body_and_atomically_classifies_subject_audience_and_consumption() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "  Alice preferred  ").await;
    let before = snapshot(&store).await;
    let generic_before = generic_snapshot(&store).await;
    let receipt = publish_preferred_name(&store, &source.proof(&alice), candidate.id(), None)
        .await
        .unwrap();
    assert_eq!(receipt.candidate_id, candidate.id());
    assert_eq!(receipt.previous_fact_id, None);
    assert_head(
        &store,
        &alice,
        receipt,
        PersonalFactState::Active,
        Some("Alice preferred"),
    )
    .await;
    assert_eq!(
        candidate_state(&store, &alice, candidate.id()).await,
        CandidateState::Published
    );
    assert_eq!(
        list_candidates(&store, &alice, 10).await.unwrap()[0].state(),
        CandidateState::Published
    );
    assert!(!withdraw_candidate(&store, &alice, candidate.id())
        .await
        .unwrap());

    let after = snapshot(&store).await;
    // The old CHECK-constrained candidate row is byte-for-byte unchanged. Published is derived.
    assert_eq!(
        table(&after.0, "candidates"),
        table(&before.0, "candidates")
    );
    assert_eq!(table(&after.0, "publications").len(), 1);
    let decision = row(&after.0, "publications", receipt.fact_id);
    assert_eq!(decision[1], store.bear_id().to_string());
    assert_eq!(decision[3], alice.entity_id());
    assert_eq!(decision[4], candidate.id().to_string());
    assert_eq!(decision[5], "preferred_name");
    assert!(decision[6].is_null());
    assert_eq!(decision[7], store.bear_id().to_string());
    assert_eq!(decision[8], ALICE.get());
    assert_eq!(decision[9], alice.authority_generation);
    let generic = generic_snapshot(&store).await;
    let body = row(&generic.0, "records", receipt.fact_id);
    // Column order belongs to the shared exact snapshot, not a second inspection query.
    assert_eq!(body[3], "shared");
    assert_eq!(body[8], "personal_preferred_name");
    assert_eq!(body[12], "Alice preferred");
    assert_eq!(body[13], "{}");
    for index in [4, 5, 6, 7, 10, 14, 16, 17, 19] {
        assert!(body[index].is_null(), "unexpected canonical field {index}");
    }
    assert_eq!(body[15], "normal");
    assert_eq!(body[18], body[11]);
    assert_eq!(
        row(&generic.0, "records", source.memory),
        row(&generic_before.0, "records", source.memory)
    );
    for (table_name, relation) in [("relations", "subject"), ("rules", "audience")] {
        let links = table(&generic.0, table_name);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0][3], receipt.fact_id.to_string());
        assert_eq!(links[0][4], alice.entity_id());
        assert_eq!(
            links[0][5],
            crate::descriptors::relation(relation).unwrap().id
        );
        assert_eq!(links[0][10], "active");
    }
    let original = all_state(&store).await;
    assert_eq!(
        publish_preferred_name(&store, &source.proof(&alice), candidate.id(), None)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(all_state(&store).await, original);
}

#[tokio::test]
async fn correction_cas_includes_retracted_tombstone_and_replays_never_reactivate() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "First", None).await;
    let second = stage(&store, &alice, source, "Second").await;
    let before = all_state(&store).await;
    for expected in [None, Some(PersonalFactId(Uuid::new_v4()))] {
        denied(publish_preferred_name(&store, &source.proof(&alice), second.id(), expected).await);
        assert_eq!(all_state(&store).await, before);
    }
    let second = publish_preferred_name(
        &store,
        &source.proof(&alice),
        second.id(),
        Some(first.fact_id),
    )
    .await
    .unwrap();
    assert_eq!(second.previous_fact_id, Some(first.fact_id));
    let generic = generic_snapshot(&store).await;
    assert!(row(&generic.0, "records", first.fact_id)[19].is_string());
    assert_eq!(
        row(&generic.0, "records", second.fact_id)[14],
        first.fact_id.to_string()
    );
    let old_rule = table(&generic.0, "rules")
        .into_iter()
        .find(|r| r[3] == first.fact_id.to_string())
        .unwrap();
    assert_eq!(old_rule[10], "superseded");
    denied(retract_preferred_name(&store, &alice, first.fact_id).await);
    assert!(retract_preferred_name(&store, &alice, second.fact_id)
        .await
        .unwrap());
    assert!(!retract_preferred_name(&store, &alice, second.fact_id)
        .await
        .unwrap());
    let third = stage(&store, &alice, source, "Third").await;
    let tombstone = all_state(&store).await;
    for expected in [None, Some(first.fact_id)] {
        denied(publish_preferred_name(&store, &source.proof(&alice), third.id(), expected).await);
        assert_eq!(all_state(&store).await, tombstone);
    }
    for receipt in [first, second] {
        assert_eq!(
            publish_preferred_name(&store, &source.proof(&alice), receipt.candidate_id, None)
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(all_state(&store).await, tombstone);
    }
    assert_head(&store, &alice, second, PersonalFactState::Retracted, None).await;
    let third = publish_preferred_name(
        &store,
        &source.proof(&alice),
        third.id(),
        Some(second.fact_id),
    )
    .await
    .unwrap();
    assert_eq!(third.previous_fact_id, Some(second.fact_id));
    assert_head(
        &store,
        &alice,
        third,
        PersonalFactState::Active,
        Some("Third"),
    )
    .await;
    let before = all_state(&store).await;
    assert_eq!(
        publish_preferred_name(
            &store,
            &source.proof(&alice),
            first.candidate_id,
            Some(third.fact_id)
        )
        .await
        .unwrap(),
        first
    );
    assert_eq!(all_state(&store).await, before);
}

#[tokio::test]
async fn concurrent_same_candidate_retries_and_competing_corrections_have_one_decision() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "First").await;
    // Independent handles, shared pool: every call must obtain its own transaction.
    let other = BearMemoryStore::new(store.bear_id(), store.pool().clone());
    let proof = source.proof(&alice);
    let (left, right) = tokio::join!(
        publish_preferred_name(&store, &proof, candidate.id(), None),
        publish_preferred_name(&other, &proof, candidate.id(), None),
    );
    let first = left.unwrap();
    assert_eq!(right.unwrap(), first);
    assert_eq!(table(&snapshot(&store).await.0, "publications").len(), 1);
    let a = stage(&store, &alice, source, "Correction A").await;
    let b = stage(&store, &alice, source, "Correction B").await;
    let (left, right) = tokio::join!(
        publish_preferred_name(&store, &proof, a.id(), Some(first.fact_id)),
        publish_preferred_name(&other, &proof, b.id(), Some(first.fact_id)),
    );
    let (winner, loser) = match (left, right) {
        (Ok(receipt), error @ Err(_)) => {
            denied(error);
            (receipt, b.id())
        }
        (error @ Err(_), Ok(receipt)) => {
            denied(error);
            (receipt, a.id())
        }
        results => panic!("expected exactly one CAS winner: {results:?}"),
    };
    assert_eq!(table(&snapshot(&store).await.0, "publications").len(), 2);
    assert_eq!(
        candidate_state(&store, &alice, winner.candidate_id).await,
        CandidateState::Published
    );
    assert_eq!(
        candidate_state(&store, &alice, loser).await,
        CandidateState::Pending
    );
    assert_eq!(
        current_preferred_name(&store, &alice)
            .await
            .unwrap()
            .unwrap()
            .fact_id,
        winner.fact_id
    );
}
