use super::*;

#[tokio::test]
async fn erases_all_versions_and_candidate_states_preserving_audit_and_classification() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "PriorBodySentinel", None).await;
    let second = publish(
        &store,
        &alice,
        source,
        "RetractedBodySentinel",
        Some(first.fact_id),
    )
    .await;
    retract_preferred_name(&store, &alice, second.fact_id)
        .await
        .unwrap();
    let (third_candidate, third_proof) =
        owner_candidate(&store, &alice, "SuppressedBodySentinel").await;
    let third = publish_preferred_name(
        &store,
        &third_proof,
        third_candidate.id(),
        Some(second.fact_id),
    )
    .await
    .unwrap();
    fixtures::lose_all_acl(&store, &third.fact_id.to_string()).await;
    assert_head(&store, &alice, third, PersonalFactState::Suppressed, None).await;
    let pending = stage(&store, &alice, source, "PendingBodySentinel").await;
    let withdrawn = stage(&store, &alice, source, "WithdrawnBodySentinel").await;
    withdraw_candidate(&store, &alice, withdrawn.id())
        .await
        .unwrap();
    let mut reviewed = Vec::new();
    for (decision, name, state) in [
        (
            CandidateReviewDecision::Reject,
            "RejectedBodySentinel",
            CandidateState::Rejected,
        ),
        (
            CandidateReviewDecision::Defer,
            "DeferredBodySentinel",
            CandidateState::Deferred,
        ),
        (
            CandidateReviewDecision::Expire,
            "ExpiredBodySentinel",
            CandidateState::Expired,
        ),
    ] {
        let (candidate, _) = owner_candidate(&store, &alice, name).await;
        let receipt = review_candidate(&store, &alice, candidate.id(), decision, None)
            .await
            .unwrap();
        assert_eq!(candidate_state(&store, &alice, candidate.id()).await, state);
        reviewed.push(receipt);
    }
    // Parking/reconnection gives a real quarantined candidate, not a new fixture query.
    crate::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let alice = reconnect(&store, &alice).await;
    assert_eq!(
        candidate_state(&store, &alice, pending.id()).await,
        CandidateState::Quarantined
    );
    assert!(!capture_enabled(&store, &alice).await.unwrap());
    let before = snapshot(&store).await;
    let form = Reviewed::read(&store, &alice).await;
    let receipt = form.erase(&store, &alice).await.unwrap();
    assert_eq!(
        receipt.affected_fact_ids,
        vec![first.fact_id, second.fact_id, third.fact_id]
    );
    let ids = [
        first.fact_id.to_string(),
        second.fact_id.to_string(),
        third.fact_id.to_string(),
    ];
    assert!(canonical_record_contents_match(
        &store,
        &[(&ids[0], ""), (&ids[1], ""), (&ids[2], ""),]
    )
    .await
    .unwrap());
    let candidates = list_candidates(&store, &alice, 100).await.unwrap();
    assert_eq!(candidates.len(), 8);
    assert!(candidates
        .iter()
        .all(|c| c.state() == CandidateState::Erased && c.preferred_name().is_none()));
    for candidate in &candidates {
        assert_erased(&store, &alice, candidate.id()).await;
    }
    for review in reviewed {
        assert_eq!(
            get_candidate(&store, &alice, review.candidate_id)
                .await
                .unwrap()
                .unwrap()
                .latest_review_id(),
            Some(review.review_id)
        );
    }
    let after = snapshot(&store).await;
    assert_eq!(
        table(&before.0, "publications"),
        table(&after.0, "publications")
    );
    assert_eq!(
        table(&before.0, "candidate_reviews"),
        table(&after.0, "candidate_reviews")
    );
    assert!(!after.0.contains("BodySentinel"));
    assert_head(&store, &alice, third, PersonalFactState::Retracted, None).await;
    assert_eq!(
        current_preferred_name_version(&store, &alice)
            .await
            .unwrap(),
        Some(PersonalFactVersion {
            fact_id: third.fact_id,
            state: PersonalFactState::Retracted,
        })
    );
    for id in ids {
        assert!(crate::fetch_record_by_id(&store, &id)
            .await
            .unwrap()
            .is_none());
        assert!(
            !crate::personal_facts::publication_access(&store, &id)
                .await
                .unwrap()
                .unwrap()
                .eligible
        );
    }
    let disabled = capture_setting(&store, &alice).await.unwrap();
    assert!(!disabled.enabled);
    assert!(disabled.revision.get() > form.capture.get());
}

#[tokio::test]
async fn candidate_only_and_empty_owner_erasure_have_no_fake_fact_or_name() {
    for candidate_only in [false, true] {
        let store = new_test_store().await;
        let alice = person(&store, ALICE).await;
        let candidate = if candidate_only {
            set_capture_enabled(&store, &alice, true).await.unwrap();
            Some(
                owner_candidate(&store, &alice, "UnapprovedOnlySentinel")
                    .await
                    .0,
            )
        } else {
            None
        };
        let form = Reviewed::read(&store, &alice).await;
        let receipt = form.erase(&store, &alice).await.unwrap();
        assert!(receipt.affected_fact_ids.is_empty());
        assert!(current_preferred_name(&store, &alice)
            .await
            .unwrap()
            .is_none());
        assert!(!capture_enabled(&store, &alice).await.unwrap());
        if let Some(candidate) = candidate {
            assert_erased(&store, &alice, candidate.id()).await;
        }
        assert_eq!(form.erase(&store, &alice).await.unwrap(), receipt);
    }
}

#[tokio::test]
async fn original_human_source_survives_but_exact_erased_evidence_cannot_be_harvested_again() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let conversation = Uuid::new_v4();
    let original = "Please remember my preferred name is HistoryNameSentinel.";
    let row = store
        .append_record(
            &LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "note"),
            "note",
            "pair",
            None,
            original,
            &json!({}),
            "normal",
        )
        .await
        .unwrap();
    let source = Note {
        conversation,
        memory: Uuid::parse_str(&row.memory_id).unwrap(),
    };
    let fact = publish(&store, &alice, source, "HistoryNameSentinel", None).await;
    let form = Reviewed::read(&store, &alice).await;
    form.erase(&store, &alice).await.unwrap();
    assert!(canonical_record_contents_match(
        &store,
        &[(&row.memory_id, original), (&fact.fact_id.to_string(), ""),]
    )
    .await
    .unwrap());
    assert!(crate::fetch_record_by_id(&store, &row.memory_id)
        .await
        .unwrap()
        .is_none());

    // Refreshing the canonical view preserves the evidence body and its generic exclusion.
    crate::migrate::migrate_bear_sqlite_schema(store.pool())
        .await
        .unwrap();
    assert!(crate::fetch_record_by_id(&store, &row.memory_id)
        .await
        .unwrap()
        .is_none());
    assert!(
        canonical_record_contents_match(&store, &[(&row.memory_id, original)])
            .await
            .unwrap()
    );
    set_capture_enabled(&store, &alice, true).await.unwrap();
    // The boundary follows exact source provenance, not a substring of either submitted name.
    for name in ["HistoryNameSentinel", "DifferentStringSameEvidence"] {
        denied(
            stage_preferred_name(
                &store,
                &source.proof(&alice),
                PreferredName::new(name).unwrap(),
            )
            .await,
        );
    }
    assert_eq!(
        publish_preferred_name(&store, &source.proof(&alice), fact.candidate_id, None)
            .await
            .unwrap(),
        fact
    );
    assert_head(&store, &alice, fact, PersonalFactState::Retracted, None).await;
    // An explicit new owner request may intentionally remember the same value again.
    let (fresh, proof) = owner_candidate(&store, &alice, "HistoryNameSentinel").await;
    let current = publish_preferred_name(&store, &proof, fresh.id(), Some(fact.fact_id))
        .await
        .unwrap();
    assert_head(
        &store,
        &alice,
        current,
        PersonalFactState::Active,
        Some("HistoryNameSentinel"),
    )
    .await;
}

#[tokio::test]
async fn erased_head_never_falls_back_when_lifecycle_fields_are_restored() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "OlderName", None).await;
    let current = publish(&store, &alice, source, "LatestName", Some(first.fact_id)).await;
    Reviewed::read(&store, &alice)
        .await
        .erase(&store, &alice)
        .await
        .unwrap();
    fixtures::alter_record(
        &store,
        &current.fact_id.to_string(),
        &LogicalMemoryPath::shared_core("restored"),
        "normal",
        "{}",
        None,
    )
    .await;
    assert_head(&store, &alice, current, PersonalFactState::Retracted, None).await;
    assert!(
        !crate::personal_facts::publication_access(&store, &current.fact_id.to_string())
            .await
            .unwrap()
            .unwrap()
            .eligible
    );
    for id in [first.fact_id, current.fact_id] {
        assert!(crate::fetch_record_by_id(&store, &id.to_string())
            .await
            .unwrap()
            .is_none());
    }
}
