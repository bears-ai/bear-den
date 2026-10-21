use super::*;

async fn fail_receipts(store: &BearMemoryStore) {
    sqlx::query!(
        r"CREATE TRIGGER fail_identity_receipt
          BEFORE INSERT ON user_identity_receipts
          BEGIN
              SELECT RAISE(ABORT, 'injected identity receipt failure');
          END",
    )
    .execute(store.pool())
    .await
    .unwrap();
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Reconnect,
    Replace,
    Revoke,
}

async fn receipt_failure_rolls_back(operation: Operation) {
    let store = new_test_store().await;
    let entity_id = person(&store, ALICE).await;
    let data = seed_context(&store, &entity_id, "rollback-portable").await;
    let old_binding = resolve_authenticated_user_entity(&store, ALICE)
        .await
        .unwrap();
    let old_context = AccessContext::for_authenticated_user(&old_binding);
    record_export_origins(&store, "https://source.example", ALICE, &[])
        .await
        .unwrap();
    let (review, consent, displaced) = match operation {
        Operation::Revoke => (
            review_revocation(&store, ACTOR, &entity_id).await.unwrap(),
            ReplacementConsent::NoReplacement,
            None,
        ),
        Operation::Reconnect | Operation::Replace => {
            quarantine_imported_authenticated_users(&store)
                .await
                .unwrap();
            let displaced = if matches!(operation, Operation::Replace) {
                let binding = resolve_authenticated_user_entity(&store, DESTINATION)
                    .await
                    .unwrap();
                let context = AccessContext::for_authenticated_user(&binding);
                let data = seed_context(&store, binding.entity_id(), "rollback-displaced").await;
                Some((binding, context, data))
            } else {
                None
            };
            let review = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
                .await
                .unwrap();
            let consent = if displaced.is_some() {
                ReplacementConsent::PreserveDisplacedIdentity
            } else {
                ReplacementConsent::NoReplacement
            };
            (review, consent, displaced)
        }
    };
    fail_receipts(&store).await;
    let before = authority_snapshot(&store).await;
    let context_before = context_snapshot(&store).await;
    for _ in 0..2 {
        let result = commit_review(&store, &review, consent).await;
        assert!(
            matches!(result, Err(DenError::Database(_))),
            "{operation:?}: {result:?}"
        );
        assert_eq!(authority_snapshot(&store).await, before,
            "{operation:?}: receipt failure rolls back authority, entity state, handle insertion/detachment, generations and receipt consumption");
        assert_eq!(context_snapshot(&store).await, context_before);
        assert_visible(
            &store,
            &data.active_memory_id,
            &old_context,
            matches!(operation, Operation::Revoke),
        )
        .await;
        if let Some((binding, access, data)) = &displaced {
            assert_eq!(person(&store, DESTINATION).await, binding.entity_id());
            assert_visible(&store, &data.active_memory_id, access, true).await;
            assert_visible(
                &store,
                &data.active_memory_id,
                &AccessContext::for_authenticated_user(binding),
                true,
            )
            .await;
        }
    }
    // A failed audit insert does not consume the review or its authority epochs.
    sqlx::query!("DROP TRIGGER IF EXISTS fail_identity_receipt")
        .execute(store.pool())
        .await
        .unwrap();
    let receipt = commit_review(&store, &review, consent).await.unwrap();
    assert_receipt(&store, &review, &receipt).await;
    assert_eq!(receipt_count(&store).await, 1);
    assert_eq!(context_snapshot(&store).await, context_before);
    assert_visible(&store, &data.active_memory_id, &old_context, false).await;
    if !matches!(operation, Operation::Revoke) {
        assert_eq!(person(&store, DESTINATION).await, entity_id);
        assert_visible(
            &store,
            &data.active_memory_id,
            &context(&store, DESTINATION).await,
            true,
        )
        .await;
    }
    if let Some((binding, access, data)) = &displaced {
        assert_eq!(
            summary(&store, binding.entity_id()).await.active_user_id,
            None
        );
        assert_visible(&store, &data.active_memory_id, access, false).await;
    }
}

#[tokio::test]
async fn audit_receipt_failure_rolls_back_reconnection_including_new_handle_and_epoch() {
    receipt_failure_rolls_back(Operation::Reconnect).await;
}

#[tokio::test]
async fn audit_receipt_failure_rolls_back_both_sides_of_replacement() {
    receipt_failure_rolls_back(Operation::Replace).await;
}

#[tokio::test]
async fn audit_receipt_failure_rolls_back_revocation_and_keeps_existing_context_valid() {
    receipt_failure_rolls_back(Operation::Revoke).await;
}

#[tokio::test]
async fn imported_receipts_and_origin_labels_are_history_not_destination_grants() {
    let store = new_test_store().await;
    let imported = person(&store, ALICE).await;
    let data = seed_context(&store, &imported, "imported-receipt").await;
    record_export_origins(
        &store,
        "https://source.example",
        ALICE,
        &[ExportUserLabel {
            user_id: ALICE,
            username: Some("destination-admin".into()),
            display_name: Some("Destination Admin".into()),
        }],
    )
    .await
    .unwrap();
    let historical = reconnect_after_quarantine(&store, &imported).await;
    let old_binding = resolve_authenticated_user_entity(&store, DESTINATION)
        .await
        .unwrap();
    let old_context = AccessContext::for_authenticated_user(&old_binding);
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert_eq!(
        receipt_count(&store).await,
        1,
        "quarantine preserves historical receipts"
    );
    let fresh = resolve_authenticated_user_entity(&store, DESTINATION)
        .await
        .unwrap();
    assert_ne!(fresh.entity_id(), imported);
    for denied in [
        old_context,
        AccessContext::for_authenticated_user(&old_binding),
        AccessContext::for_authenticated_user(&fresh).with_audience([imported.clone()]),
        AccessContext::empty().with_audience([imported.clone()]),
    ] {
        assert_visible(&store, &data.active_memory_id, &denied, false).await;
    }
    let identities = list_identities(&store).await.unwrap();
    assert_eq!(identities.len(), 2);
    let imported_summary = identities
        .iter()
        .find(|identity| identity.entity_id == imported)
        .unwrap();
    assert_eq!(imported_summary.active_user_id, None);
    assert_eq!(
        imported_summary.origins[0].display_name.as_deref(),
        Some("Destination Admin")
    );
    let before = authority_snapshot(&store).await;
    let review = review_reconnection(&store, ACTOR, &imported, DESTINATION)
        .await
        .unwrap();
    assert_ne!(review.id(), historical.receipt_id);
    assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    commit_review(
        &store,
        &review,
        ReplacementConsent::PreserveDisplacedIdentity,
    )
    .await
    .unwrap();
    assert_eq!(person(&store, DESTINATION).await, imported);
    assert_visible(
        &store,
        &data.active_memory_id,
        &AccessContext::for_authenticated_user(&old_binding),
        false,
    )
    .await;
    assert_visible(
        &store,
        &data.active_memory_id,
        &context(&store, DESTINATION).await,
        true,
    )
    .await;
    assert_eq!(receipt_count(&store).await, 2);
}

async fn reconnect_after_quarantine(
    store: &BearMemoryStore,
    entity_id: &str,
) -> IdentityReconnectionReceipt {
    quarantine_imported_authenticated_users(store)
        .await
        .unwrap();
    reconnect(store, entity_id, DESTINATION).await
}

#[tokio::test]
async fn schema_startup_epoch_backfill_initializes_missing_epochs_without_resetting_existing_ones()
{
    let store = new_test_store().await;
    let alice = person(&store, ALICE).await;
    let bob = person(&store, BOB).await;
    let bear_id = store.bear_id().to_string();
    sqlx::query!(
        "DELETE FROM user_identity_generations WHERE bear_id = ? AND entity_id = ?",
        bear_id,
        alice
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE user_identity_generations SET generation = 7 WHERE bear_id = ? AND entity_id = ?",
        bear_id,
        bob
    )
    .execute(store.pool())
    .await
    .unwrap();
    // Exercise the exact startup fragment without constructing unrelated service Config.
    // The source assertion keeps this test tied to the schema actually applied on startup.
    macro_rules! backfill_from_schema {
        ($sql:literal) => {{
            assert!(include_str!("../../schema.sql").contains($sql));
            sqlx::query!($sql).execute(store.pool()).await.unwrap();
        }};
    }
    backfill_from_schema!("INSERT OR IGNORE INTO user_identity_generations (bear_id, entity_id, generation)\n    SELECT bear_id, entity_id, 1 FROM authenticated_user_entities");
    assert_eq!(summary(&store, &alice).await.binding_version, 1);
    assert_eq!(summary(&store, &bob).await.binding_version, 7);
    assert_eq!(person(&store, ALICE).await, alice);
    assert_eq!(person(&store, BOB).await, bob);
    let before = authority_snapshot(&store).await;
    backfill_from_schema!("INSERT OR IGNORE INTO user_identity_generations (bear_id, entity_id, generation)\n    SELECT bear_id, entity_id, 1 FROM authenticated_user_entities");
    assert_eq!(authority_snapshot(&store).await, before);
}
