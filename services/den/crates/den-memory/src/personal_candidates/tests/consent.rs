use super::*;

#[tokio::test]
async fn capture_defaults_off_and_disabled_staging_has_no_side_effects() {
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let note = note(&store).await;
    let before = snapshot(&store).await;
    assert!(!capture_enabled(&store, &alice).await.unwrap());
    assert!(list_candidates(&store, &alice, 100)
        .await
        .unwrap()
        .is_empty());
    assert_denied(
        stage_preferred_name(
            &store,
            &note.proof(&alice),
            PreferredName::new("Alice").unwrap(),
        )
        .await,
    );
    assert_eq!(
        snapshot(&store).await,
        before,
        "a read/default must not manufacture consent"
    );
    assert_eq!(
        sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!: i64" FROM personal_memory_settings"#)
            .fetch_one(store.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn explicit_opt_in_records_current_epoch_and_minimized_typed_provenance() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let before = next_sequence(&store).await;
    let candidate = stage(&store, &alice, source, "  Alice  ").await;
    assert!(capture_enabled(&store, &alice).await.unwrap());
    assert_eq!(candidate.preferred_name().as_str(), "Alice");
    assert_eq!(candidate.person_entity_id(), alice.entity_id());
    assert_eq!(candidate.state(), CandidateState::Pending);
    assert_eq!(candidate.fact_type(), CandidateFactType::PreferredName);
    assert_eq!(candidate.evidence(), CandidateEvidence::ExplicitUserRequest);
    assert_eq!(candidate.retention(), CandidateRetention::Durable);
    assert_eq!(candidate.source_conversation_id(), source.conversation);
    assert_eq!(candidate.source_memory_id(), source.memory);
    assert_eq!(candidate.source_bear_id(), alice.bear_id());
    assert_eq!(candidate.source_user_id(), ALICE);
    assert_eq!(
        candidate.source_binding_generation(),
        alice.authority_generation
    );
    assert!(!candidate.created_at().is_empty());
    assert_eq!(candidate.withdrawn_at(), None);
    assert_eq!(candidate_sequence(&store, candidate.id()).await, before);
    assert_eq!(next_sequence(&store).await, before + 1);
    assert!(!format!("{candidate:?}").contains("Alice"));
    let bear_id = store.bear_id().to_string();
    let entity_id = alice.entity_id();
    let setting = sqlx::query!(
        r#"SELECT capture_enabled AS "enabled!: i64", authority_generation,
            updated_on_bear_id, updated_by_user_id AS "actor?: i32"
           FROM personal_memory_settings WHERE bear_id = ? AND person_entity_id = ?"#,
        bear_id,
        entity_id,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(setting.enabled, 1);
    assert_eq!(setting.authority_generation, alice.authority_generation);
    assert_eq!(setting.updated_on_bear_id, bear_id);
    assert_eq!(setting.actor, Some(ALICE.get()));
}

#[tokio::test]
async fn opt_out_withdraws_pending_and_quarantined_only_for_this_person_without_resurrection() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let pending = stage(&store, &alice, source, "Pending").await;
    let quarantined = stage(&store, &alice, source, "Quarantined").await;
    quarantine_candidate(&store, quarantined.id()).await;
    let withdrawn = stage(&store, &alice, source, "Withdrawn").await;
    assert!(withdraw_candidate(&store, &alice, withdrawn.id())
        .await
        .unwrap());
    let withdrawn_before = current_candidate(&store, &alice, withdrawn.id()).await;
    let bobs = stage(&store, &bob, source, "Bob").await;
    let sequence = next_sequence(&store).await;
    set_capture_enabled(&store, &alice, false).await.unwrap();
    assert!(!capture_enabled(&store, &alice).await.unwrap());
    for id in [pending.id(), quarantined.id(), withdrawn.id()] {
        let row = current_candidate(&store, &alice, id).await;
        assert_eq!(row.state(), CandidateState::Withdrawn);
        assert!(row.withdrawn_at().is_some());
    }
    let bear_id = store.bear_id().to_string();
    let pending_id = pending.id().to_string();
    let audit = sqlx::query!(
        r#"SELECT withdrawn_on_bear_id, withdrawn_by_user_id AS "actor?: i32"
           FROM personal_memory_candidates WHERE candidate_id = ?"#,
        pending_id,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(audit.withdrawn_on_bear_id, Some(bear_id));
    assert_eq!(audit.actor, Some(ALICE.get()));
    assert_eq!(
        current_candidate(&store, &alice, withdrawn.id())
            .await
            .withdrawn_at(),
        withdrawn_before.withdrawn_at()
    );
    assert_eq!(state(&store, bobs.id()).await, CandidateState::Pending);
    assert!(capture_enabled(&store, &bob).await.unwrap());
    set_capture_enabled(&store, &alice, true).await.unwrap();
    assert_eq!(next_sequence(&store).await, sequence);
    let history = list_candidates(&store, &alice, 100).await.unwrap();
    assert_eq!(history.len(), 3);
    assert!(history
        .iter()
        .all(|row| row.state() == CandidateState::Withdrawn));
    let fresh = stage(&store, &alice, source, "Pending").await;
    assert_ne!(fresh.id(), pending.id());
    assert_eq!(state(&store, pending.id()).await, CandidateState::Withdrawn);
}

#[tokio::test]
async fn withdrawal_is_owner_scoped_idempotent_and_keeps_history_and_consent() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "Alice").await;
    assert!(withdraw_candidate(&store, &alice, candidate.id())
        .await
        .unwrap());
    let once = snapshot(&store).await;
    assert!(!withdraw_candidate(&store, &alice, candidate.id())
        .await
        .unwrap());
    assert!(
        !withdraw_candidate(&store, &alice, PersonalCandidateId(Uuid::new_v4()))
            .await
            .unwrap()
    );
    assert_eq!(snapshot(&store).await, once);
    let history = current_candidate(&store, &alice, candidate.id()).await;
    assert_eq!(history.preferred_name(), candidate.preferred_name());
    assert_eq!(
        history.source_binding_generation(),
        candidate.source_binding_generation()
    );
    assert_eq!(history.created_at(), candidate.created_at());
    assert_eq!(history.state(), CandidateState::Withdrawn);
    assert!(capture_enabled(&store, &alice).await.unwrap());
}

#[tokio::test]
async fn lists_are_newest_first_bounded_and_include_protected_history() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let mut ids = Vec::new();
    for index in 0..103 {
        ids.push(
            stage(&store, &alice, source, &format!("Name {index}"))
                .await
                .id(),
        );
    }
    assert!(withdraw_candidate(&store, &alice, ids[101]).await.unwrap());
    quarantine_candidate(&store, ids[102]).await;
    assert_eq!(
        list_candidates(&store, &alice, 0).await.unwrap()[0].id(),
        ids[102]
    );
    let two = list_candidates(&store, &alice, 2).await.unwrap();
    assert_eq!(
        two.iter().map(PersonalCandidate::id).collect::<Vec<_>>(),
        vec![ids[102], ids[101]]
    );
    assert_eq!(two[0].state(), CandidateState::Quarantined);
    assert_eq!(two[1].state(), CandidateState::Withdrawn);
    let bounded = list_candidates(&store, &alice, u32::MAX).await.unwrap();
    assert_eq!(bounded.len(), 100);
    assert_eq!(bounded.last().unwrap().id(), ids[3]);
}
