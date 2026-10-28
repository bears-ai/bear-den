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
    assert_eq!(
        candidate.source_conversation_id(),
        Some(source.conversation)
    );
    assert_eq!(candidate.source_memory_id(), Some(source.memory));
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
    assert_eq!(next_sequence(&store).await, sequence + 2);
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

#[tokio::test]
async fn capture_revision_cas_rejects_stale_forms_after_disabled_and_enabled_aba() {
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let initial = capture_setting(&store, &alice).await.unwrap();
    assert!(!initial.enabled);
    assert_eq!(initial.revision.get(), 0);
    assert!(PersonalCaptureRevision::try_from(-1).is_err());
    let before = snapshot(&store).await;
    assert_eq!(capture_setting(&store, &alice).await.unwrap(), initial);
    assert_eq!(snapshot(&store).await, before);

    set_capture_enabled_if_current(&store, &alice, true, initial.revision)
        .await
        .unwrap();
    let first_enabled = capture_setting(&store, &alice).await.unwrap();
    assert!(first_enabled.enabled);
    assert!(first_enabled.revision.get() > initial.revision.get());
    let before = snapshot(&store).await;
    assert_denied(set_capture_enabled_if_current(&store, &alice, true, initial.revision).await);
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(
        capture_setting(&store, &alice).await.unwrap(),
        first_enabled
    );

    set_capture_enabled_if_current(&store, &alice, false, first_enabled.revision)
        .await
        .unwrap();
    let disabled = capture_setting(&store, &alice).await.unwrap();
    assert!(!disabled.enabled);
    assert!(disabled.revision.get() > first_enabled.revision.get());
    assert_denied(set_capture_enabled_if_current(&store, &alice, true, initial.revision).await);
    assert_eq!(capture_setting(&store, &alice).await.unwrap(), disabled);
    set_capture_enabled_if_current(&store, &alice, true, disabled.revision)
        .await
        .unwrap();
    let reenabled = capture_setting(&store, &alice).await.unwrap();
    assert!(reenabled.enabled);
    assert!(reenabled.revision.get() > disabled.revision.get());
    let before = snapshot(&store).await;
    assert_denied(
        set_capture_enabled_if_current(&store, &alice, false, first_enabled.revision).await,
    );
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(capture_setting(&store, &alice).await.unwrap(), reenabled);
}

#[tokio::test]
async fn unconditional_capture_writes_always_advance_only_the_owners_revision() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let bobs_setting = capture_setting(&store, &bob).await.unwrap();
    let mut previous = capture_setting(&store, &alice).await.unwrap();
    for enabled in [true, false, false, true, true] {
        let sequence = next_sequence(&store).await;
        set_capture_enabled(&store, &alice, enabled).await.unwrap();
        let current = capture_setting(&store, &alice).await.unwrap();
        assert_eq!(current.enabled, enabled);
        assert_eq!(current.revision.get(), sequence);
        assert!(current.revision.get() > previous.revision.get());
        assert_eq!(next_sequence(&store).await, sequence + 1);
        assert_eq!(capture_setting(&store, &bob).await.unwrap(), bobs_setting);
        previous = current;
    }
}

#[tokio::test]
async fn concurrent_capture_cas_writers_cannot_both_consume_the_same_revision() {
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let initial = capture_setting(&store, &alice).await.unwrap();
    let sequence = next_sequence(&store).await;
    let (first, second) = tokio::join!(
        set_capture_enabled_if_current(&store, &alice, true, initial.revision),
        set_capture_enabled_if_current(&store, &alice, true, initial.revision),
    );
    match (first, second) {
        (Ok(()), rejected) | (rejected, Ok(())) => assert_denied(rejected),
        _ => panic!("exactly one consent write must consume a revision"),
    }
    assert!(capture_setting(&store, &alice).await.unwrap().enabled);
    assert_eq!(next_sequence(&store).await, sequence + 1);
}

#[tokio::test]
async fn old_generation_consent_is_disabled_without_resetting_its_head_revision() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let old = capture_setting(&store, &alice).await.unwrap();
    revoke(&store, &alice).await;
    assert_denied(capture_setting(&store, &alice).await);
    assert_denied(set_capture_enabled_if_current(&store, &alice, true, old.revision).await);
    let fresh = reconnect(
        &store,
        alice.entity_id(),
        ALICE,
        ReplacementConsent::NoReplacement,
    )
    .await;
    let current = capture_setting(&store, &fresh).await.unwrap();
    assert!(!current.enabled);
    assert_eq!(current.revision, old.revision);
    set_capture_enabled_if_current(&store, &fresh, true, current.revision)
        .await
        .unwrap();
    let renewed = capture_setting(&store, &fresh).await.unwrap();
    assert!(renewed.enabled);
    assert!(renewed.revision.get() > old.revision.get());
}
