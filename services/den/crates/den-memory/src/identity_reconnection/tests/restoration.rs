use super::*;

#[tokio::test]
async fn alice_and_bob_reconnect_to_different_local_ids_without_rewriting_portable_context() {
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let bob = person(&store, BOB).await;
    let alice_data = seed_context(&store, &alice, "alice").await;
    let bob_data = seed_context(&store, &bob, "bob").await;
    record_export_origins(
        &store,
        "https://source.example",
        ALICE,
        &[
            ExportUserLabel {
                user_id: ALICE,
                username: Some("alice".into()),
                display_name: Some("Alice".into()),
            },
            ExportUserLabel {
                user_id: BOB,
                username: Some("bob".into()),
                display_name: Some("Bob".into()),
            },
        ],
    )
    .await
    .unwrap();
    let alice_entity = get_entity(&store, &alice).await.unwrap().unwrap();
    let bob_entity = get_entity(&store, &bob).await.unwrap().unwrap();
    let original_context = context_snapshot(&store).await;
    let alice_handles = historical_handles(&store, &alice).await;
    let bob_handles = historical_handles(&store, &bob).await;
    let old_alice = context(&store, ALICE).await;
    let old_bob = context(&store, BOB).await;
    assert_visible(&store, &alice_data.active_memory_id, &old_alice, true).await;
    assert_visible(&store, &bob_data.active_memory_id, &old_bob, true).await;

    let mut quarantined = quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    quarantined.sort();
    let mut expected = vec![alice.clone(), bob.clone()];
    expected.sort();
    assert_eq!(quarantined, expected);
    let before_review = authority_snapshot(&store).await;
    for data in [&alice_data, &bob_data] {
        for denied in [
            AccessContext::empty(),
            old_alice.clone(),
            old_bob.clone(),
            AccessContext::empty().with_audience([alice.clone(), bob.clone()]),
        ] {
            assert_visible(&store, &data.active_memory_id, &denied, false).await;
        }
    }
    let alice_review = review_reconnection(&store, ACTOR, &alice, DESTINATION)
        .await
        .unwrap();
    let bob_destination = UserId::new(202);
    let bob_review = review_reconnection(&store, ACTOR, &bob, bob_destination)
        .await
        .unwrap();
    assert_eq!(
        authority_snapshot(&store).await,
        before_review,
        "review must not grant or mutate authority"
    );
    assert_eq!(alice_review.actor_user_id(), ACTOR);
    assert_eq!(alice_review.bear_id(), store.bear_id().into());
    assert!(alice_review.displaced_entity_id().is_none());
    for review in [&alice_review, &bob_review] {
        let receipt = commit_review(&store, review, ReplacementConsent::NoReplacement)
            .await
            .unwrap();
        assert_receipt(&store, review, &receipt).await;
    }
    assert_eq!(receipt_count(&store).await, 2);
    assert_eq!(person(&store, DESTINATION).await, alice);
    assert_eq!(person(&store, bob_destination).await, bob);
    let new_alice = context(&store, DESTINATION).await;
    let new_bob = context(&store, bob_destination).await;
    for (data, allowed, other) in [
        (&alice_data, &new_alice, &new_bob),
        (&bob_data, &new_bob, &new_alice),
    ] {
        for memory_id in &data.memory_ids {
            assert_visible(&store, memory_id, allowed, true).await;
            assert_visible(&store, memory_id, other, false).await;
            assert_visible(
                &store,
                memory_id,
                &AccessContext::empty().with_audience([alice.clone(), bob.clone()]),
                false,
            )
            .await;
            assert_visible(&store, memory_id, &old_alice, false).await;
            assert_visible(&store, memory_id, &old_bob, false).await;
        }
    }
    for (before, user) in [(&alice_entity, DESTINATION), (&bob_entity, bob_destination)] {
        let after = get_entity(&store, &before.entity_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.entity_id, before.entity_id);
        assert_eq!(after.sequence_no, before.sequence_no);
        assert_eq!(after.entity_type, before.entity_type);
        assert_eq!(after.display_name, before.display_name);
        assert_eq!(after.metadata_json, before.metadata_json);
        assert_eq!(after.created_at, before.created_at);
        assert_eq!(
            after.superseded_by_entity_id,
            before.superseded_by_entity_id
        );
        assert_eq!(after.resolution, ResolutionState::Confirmed);
        assert_eq!(after.trust, EntityTrust::Asserted);
        assert_eq!(after.canonical_ref, Some(format!("den:user:{user}")));
        let handles = list_handles(&store, &after.entity_id).await.unwrap();
        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].handle_type, "den_user");
        assert_eq!(handles[0].handle_value, user.to_string());
        assert_eq!(handles[0].source.as_deref(), Some("authorized_restore"));
    }
    assert_historical_handles_detached(&store, &alice, &alice_handles).await;
    assert_historical_handles_detached(&store, &bob, &bob_handles).await;
    assert_eq!(context_snapshot(&store).await, original_context);
    // The actual recall projection must keep archived/candidate/superseded records hidden.
    let grant = crate::scoped::MemoryReadGrant::new(
        crate::MemorySource::Conversation(Uuid::new_v4()),
        None,
    );
    let recalled = crate::scoped::read_path(&store, grant, &new_alice, &alice_data.active_path, 50)
        .await
        .unwrap();
    assert!(recalled
        .iter()
        .any(|row| row.memory_id == alice_data.active_memory_id));
    for hidden in &alice_data.memory_ids[2..5] {
        assert!(!recalled.iter().any(|row| &row.memory_id == hidden));
    }
    assert!(
        crate::scoped::read_path(&store, grant, &new_bob, &alice_data.active_path, 50)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn source_numeric_collision_cannot_adopt_or_read_the_imported_identity() {
    let store = new_test_store().await;
    let imported = person(&store, ALICE).await;
    let data = seed_context(&store, &imported, "collision").await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let local = resolve_authenticated_user_entity(&store, ALICE)
        .await
        .unwrap();
    assert_ne!(local.entity_id(), imported);
    let local_context =
        AccessContext::for_authenticated_user(&local).with_audience([imported.clone()]);
    assert_visible(&store, &data.active_memory_id, &local_context, false).await;
    reconnect(&store, &imported, DESTINATION).await;
    assert_eq!(person(&store, ALICE).await, local.entity_id());
    assert_eq!(person(&store, DESTINATION).await, imported);
    assert_visible(&store, &data.active_memory_id, &local_context, false).await;
    assert_visible(
        &store,
        &data.active_memory_id,
        &context(&store, DESTINATION).await,
        true,
    )
    .await;
    assert_eq!(
        summary(&store, &imported).await.active_user_id,
        Some(DESTINATION)
    );
    assert_eq!(
        summary(&store, local.entity_id()).await.active_user_id,
        Some(ALICE)
    );
}

#[tokio::test]
async fn revoke_and_same_tuple_reconnect_never_revive_old_bindings_or_context_clones() {
    let store = new_test_store().await;
    let initial = resolve_authenticated_user_entity(&store, ALICE)
        .await
        .unwrap();
    let entity_id = initial.entity_id().to_owned();
    let data = seed_context(&store, &entity_id, "epochs").await;
    let original_context = context_snapshot(&store).await;
    let initial_version = summary(&store, &entity_id).await.binding_version;
    assert_eq!(
        initial_version, 1,
        "resolver initializes the authority epoch"
    );
    let mut old_bindings = vec![initial];
    let mut old_contexts = Vec::new();
    for cycle in 0..3 {
        let current = AccessContext::for_authenticated_user(old_bindings.last().unwrap());
        assert_visible(&store, &data.active_memory_id, &current, true).await;
        old_contexts.push(current.clone());
        let review = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
        let receipt = commit_review(&store, &review, ReplacementConsent::NoReplacement)
            .await
            .unwrap();
        assert_receipt(&store, &review, &receipt).await;
        assert_eq!(
            summary(&store, &entity_id).await.binding_version,
            initial_version + cycle * 2 + 1
        );
        for stale in &old_contexts {
            assert_visible(&store, &data.active_memory_id, stale, false).await;
        }
        reconnect(&store, &entity_id, ALICE).await;
        assert_eq!(
            summary(&store, &entity_id).await.binding_version,
            initial_version + cycle * 2 + 2
        );
        for binding in &old_bindings {
            // Constructing a *new* context from a stale authenticated binding is also denied.
            assert_visible(
                &store,
                &data.active_memory_id,
                &AccessContext::for_authenticated_user(binding),
                false,
            )
            .await;
        }
        for stale in &old_contexts {
            assert_visible(&store, &data.active_memory_id, stale, false).await;
        }
        let fresh = resolve_authenticated_user_entity(&store, ALICE)
            .await
            .unwrap();
        assert_eq!(fresh.entity_id(), entity_id);
        assert_visible(
            &store,
            &data.active_memory_id,
            &AccessContext::for_authenticated_user(&fresh),
            true,
        )
        .await;
        old_bindings.push(fresh);
    }
    assert_eq!(context_snapshot(&store).await, original_context);
}

#[tokio::test]
async fn quarantine_bumps_epochs_even_when_repeated_and_same_tuple_is_restored() {
    let store = new_test_store().await;
    let binding = resolve_authenticated_user_entity(&store, ALICE)
        .await
        .unwrap();
    let entity_id = binding.entity_id();
    let data = seed_context(&store, entity_id, "quarantine-epochs").await;
    let stale = AccessContext::for_authenticated_user(&binding);
    for expected_generation in [2, 3] {
        quarantine_imported_authenticated_users(&store)
            .await
            .unwrap();
        assert_eq!(
            summary(&store, entity_id).await.binding_version,
            expected_generation
        );
        assert_visible(&store, &data.active_memory_id, &stale, false).await;
    }
    reconnect(&store, entity_id, ALICE).await;
    assert_eq!(summary(&store, entity_id).await.binding_version, 4);
    assert_visible(&store, &data.active_memory_id, &stale, false).await;
    assert_visible(
        &store,
        &data.active_memory_id,
        &AccessContext::for_authenticated_user(&binding),
        false,
    )
    .await;
    assert_visible(
        &store,
        &data.active_memory_id,
        &context(&store, ALICE).await,
        true,
    )
    .await;
}

#[tokio::test]
async fn generic_mutations_remain_guarded_after_reconnect_and_after_revoke() {
    let store = new_test_store().await;
    let entity_id = person(&store, ALICE).await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert_identity_guarded(&store, &entity_id).await;
    reconnect(&store, &entity_id, DESTINATION).await;
    assert_identity_guarded(&store, &entity_id).await;
    let review = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    commit_review(&store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_identity_guarded(&store, &entity_id).await;
}
