use super::*;

fn labels(alice: UserId, bob: UserId) -> Vec<ExportUserLabel> {
    vec![
        ExportUserLabel {
            user_id: alice,
            username: Some("same-name".into()),
            display_name: Some("Alice".into()),
        },
        ExportUserLabel {
            user_id: bob,
            username: Some("same-name".into()),
            display_name: Some("Bob".into()),
        },
    ]
}

#[tokio::test]
async fn origins_are_source_qualified_and_exporter_is_recorded_per_entity() {
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let bob = person(&store, BOB).await;
    let missing_account = person(&store, UserId::new(33)).await;
    record_export_origins(&store, "https://source.example", ALICE, &labels(ALICE, BOB))
        .await
        .unwrap();
    let alice_summary = summary(&store, &alice).await;
    let bob_summary = summary(&store, &bob).await;
    let missing_summary = summary(&store, &missing_account).await;
    for identity in [&alice_summary, &bob_summary, &missing_summary] {
        assert_eq!(identity.origins.len(), 1);
        assert_eq!(identity.origins[0].source_host, "https://source.example");
        assert_eq!(
            identity.origins[0].source_bear_id,
            store.bear_id().to_string()
        );
        assert!(!identity.origins[0].recorded_at.is_empty());
    }
    assert_eq!(alice_summary.display_name.as_deref(), Some("Alice"));
    assert_eq!(bob_summary.display_name.as_deref(), Some("Bob"));
    assert_eq!(alice_summary.origins[0].source_user_id, ALICE);
    assert_eq!(bob_summary.origins[0].source_user_id, BOB);
    assert!(alice_summary.origins[0].was_exporter);
    assert!(!bob_summary.origins[0].was_exporter);
    assert!(!missing_summary.origins[0].was_exporter);
    assert_eq!(missing_summary.origins[0].source_user_id, UserId::new(33));
    assert!(missing_summary.origins[0].username.is_none());
    assert!(missing_summary.origins[0].display_name.is_none());
    assert!(missing_summary.display_name.is_none());
    let before = context_snapshot(&store).await;
    for _ in 0..3 {
        record_export_origins(&store, "https://source.example", ALICE, &labels(ALICE, BOB))
            .await
            .unwrap();
        assert_eq!(
            context_snapshot(&store).await,
            before,
            "identical capture preserves origin ids and timestamps"
        );
    }
    // A different exporter changes only that per-entity observation, not all labels.
    record_export_origins(&store, "https://source.example", BOB, &labels(ALICE, BOB))
        .await
        .unwrap();
    let alice_origins = summary(&store, &alice).await.origins;
    let bob_origins = summary(&store, &bob).await.origins;
    assert_eq!(alice_origins.len(), 2);
    assert_eq!(bob_origins.len(), 2);
    assert_eq!(
        alice_origins
            .iter()
            .filter(|origin| origin.was_exporter)
            .count(),
        1
    );
    assert_eq!(
        bob_origins
            .iter()
            .filter(|origin| origin.was_exporter)
            .count(),
        1
    );
    assert_eq!(summary(&store, &missing_account).await.origins.len(), 1);
}

/// Model only the identity-table re-key performed by a trusted import. Source-qualified
/// labels and recorded Bear ids deliberately remain historical; this is not an auth grant.
async fn move_identity_fixture(source: &BearMemoryStore) -> BearMemoryStore {
    let old_bear = source.bear_id().to_string();
    let new_bear = Uuid::new_v4();
    let new_bear_text = new_bear.to_string();
    let mut tx = source.pool().begin().await.unwrap();
    sqlx::query!(
        "UPDATE entities SET bear_id = ? WHERE bear_id = ?",
        new_bear_text,
        old_bear
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE authenticated_user_entities SET bear_id = ? WHERE bear_id = ?",
        new_bear_text,
        old_bear
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE user_identity_generations SET bear_id = ? WHERE bear_id = ?",
        new_bear_text,
        old_bear
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE entity_handles SET bear_id = ? WHERE bear_id = ?",
        new_bear_text,
        old_bear
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE user_identity_origins SET bear_id = ? WHERE bear_id = ?",
        new_bear_text,
        old_bear
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE user_identity_receipts SET bear_id = ? WHERE bear_id = ?",
        new_bear_text,
        old_bear
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let destination = BearMemoryStore::new(new_bear, source.pool().clone());
    quarantine_imported_authenticated_users(&destination)
        .await
        .unwrap();
    destination
}

#[tokio::test]
async fn repeated_moves_accumulate_qualified_origins_without_duplication_or_label_matching() {
    let source = new_test_store().await;
    let alice = person(&source, ALICE).await;
    let bob = person(&source, BOB).await;
    let source_bear = source.bear_id().to_string();
    record_export_origins(&source, "https://one.example", ALICE, &labels(ALICE, BOB))
        .await
        .unwrap();
    let second = move_identity_fixture(&source).await;
    let second_bear = second.bear_id().to_string();
    let before = context_snapshot(&second).await;
    // Capturing a quarantined import cannot invent local labels or exporter flags.
    record_export_origins(&second, "https://two.example", ALICE, &labels(ALICE, BOB))
        .await
        .unwrap();
    assert_eq!(context_snapshot(&second).await, before);
    reconnect(&second, &alice, DESTINATION).await;
    let second_bob = UserId::new(202);
    reconnect(&second, &bob, second_bob).await;
    for _ in 0..3 {
        record_export_origins(
            &second,
            "https://two.example",
            second_bob,
            &labels(DESTINATION, second_bob),
        )
        .await
        .unwrap();
    }
    for (entity_id, old_id, local_id, was_exporter) in [
        (&alice, ALICE, DESTINATION, false),
        (&bob, BOB, second_bob, true),
    ] {
        let identity = summary(&second, entity_id).await;
        assert_eq!(identity.origins.len(), 2);
        let first = identity
            .origins
            .iter()
            .find(|origin| origin.source_host == "https://one.example")
            .unwrap();
        assert_eq!(first.source_bear_id, source_bear);
        assert_eq!(first.source_user_id, old_id);
        let local = identity
            .origins
            .iter()
            .find(|origin| origin.source_host == "https://two.example")
            .unwrap();
        assert_eq!(local.source_bear_id, second_bear);
        assert_eq!(local.source_user_id, local_id);
        assert_eq!(local.was_exporter, was_exporter);
    }
    let third = move_identity_fixture(&second).await;
    // Source id 101 now collides numerically, but the person chosen for it is still explicit.
    reconnect(&third, &alice, UserId::new(404)).await;
    reconnect(&third, &bob, DESTINATION).await;
    for _ in 0..3 {
        record_export_origins(
            &third,
            "https://three.example",
            UserId::new(404),
            &labels(UserId::new(404), DESTINATION),
        )
        .await
        .unwrap();
    }
    for entity_id in [&alice, &bob] {
        let identity = summary(&third, entity_id).await;
        assert_eq!(identity.origins.len(), 3);
        let mut sources: Vec<_> = identity
            .origins
            .iter()
            .map(|origin| origin.source_host.as_str())
            .collect();
        sources.sort_unstable();
        assert_eq!(
            sources,
            [
                "https://one.example",
                "https://three.example",
                "https://two.example"
            ]
        );
        assert_eq!(
            identity
                .origins
                .iter()
                .find(|origin| origin.source_host == "https://one.example")
                .unwrap()
                .source_bear_id,
            source_bear
        );
    }
    assert_eq!(person(&third, DESTINATION).await, bob);
    assert_eq!(person(&third, UserId::new(404)).await, alice);
}

#[tokio::test]
async fn capture_validates_host_labels_and_never_allocates_people_from_account_labels() {
    let store = new_test_store().await;
    let before = authority_snapshot(&store).await;
    record_export_origins(&store, "https://source.example", ALICE, &labels(ALICE, BOB))
        .await
        .unwrap();
    assert_eq!(authority_snapshot(&store).await, before);
    assert!(list_identities(&store).await.unwrap().is_empty());
    let entity_id = person(&store, ALICE).await;
    let authority_before = authority_snapshot(&store).await;
    let context_before = context_snapshot(&store).await;
    for invalid in [String::new(), " \n\t".into(), "x".repeat(2049)] {
        assert_validation(
            record_export_origins(&store, &invalid, ALICE, &labels(ALICE, BOB)).await,
        );
        assert_eq!(authority_snapshot(&store).await, authority_before);
        assert_eq!(context_snapshot(&store).await, context_before);
    }
    record_export_origins(
        &store,
        "https://source.example",
        BOB,
        &[ExportUserLabel {
            user_id: BOB,
            username: Some("not Alice's label".into()),
            display_name: None,
        }],
    )
    .await
    .unwrap();
    let identity = summary(&store, &entity_id).await;
    assert_eq!(identity.origins.len(), 1);
    assert!(identity.origins[0].username.is_none());
    assert!(!identity.origins[0].was_exporter);
    assert_eq!(authority_snapshot(&store).await, authority_before);
}
