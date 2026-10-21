use super::*;
use crate::test_support::inject_history_fixture;

#[tokio::test]
async fn malformed_receipt_history_is_skipped_without_grants_or_management_failure() {
    let store = new_test_store().await;
    let imported = person(&store, ALICE).await;
    let data = seed_context(&store, &imported, "malformed-history").await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let destination = resolve_authenticated_user_entity(&store, DESTINATION)
        .await
        .unwrap();
    let access = AccessContext::for_authenticated_user(&destination);
    let identities_before = serde_json::to_value(list_identities(&store).await.unwrap()).unwrap();
    inject_history_fixture(&store, false, 1).await.unwrap();
    inject_history_fixture(&store, true, 1).await.unwrap();
    let before = authority_snapshot(&store).await;
    let context_before = context_snapshot(&store).await;
    let history = receipt_history(&store, 100).await.unwrap();
    assert_eq!(history.receipts.len(), 1);
    assert_eq!(history.unreadable_entries, 1);
    assert_eq!(history.receipts[0].recorded_bear_id, "source-bear");
    assert_eq!(history.receipts[0].recorded_at, "9999-01-01");
    assert_eq!(history.receipts[0].prior_generation, 1);
    assert_eq!(history.receipts[0].resulting_generation, 2);
    assert_eq!(list_receipts(&store, 100).await.unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(list_identities(&store).await.unwrap()).unwrap(),
        identities_before
    );
    for denied in [
        access.clone(),
        access.with_audience([imported.clone()]),
        AccessContext::empty().with_audience([imported.clone()]),
    ] {
        assert_visible(&store, &data.active_memory_id, &denied, false).await;
    }
    let review = review_reconnection(&store, ACTOR, &imported, DESTINATION)
        .await
        .unwrap();
    assert_eq!(review.displaced_entity_id(), Some(destination.entity_id()));
    assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    assert_eq!(context_snapshot(&store).await, context_before);
    let receipt = commit_review(
        &store,
        &review,
        ReplacementConsent::PreserveDisplacedIdentity,
    )
    .await
    .unwrap();
    assert_receipt(&store, &review, &receipt).await;
    assert_eq!(person(&store, DESTINATION).await, imported);
    let after = receipt_history(&store, 100).await.unwrap();
    assert_eq!(after.receipts[0].receipt_id, receipt.receipt_id);
    assert_eq!(after.unreadable_entries, 1);
    assert_visible(
        &store,
        &data.active_memory_id,
        &context(&store, DESTINATION).await,
        true,
    )
    .await;
}

#[derive(Clone, Copy, Debug)]
enum MalformedReceiptField {
    ReceiptId,
    MissingReceiptId,
    Actor,
    Previous,
    Destination,
    PriorGeneration,
    ResultingGeneration,
}

#[tokio::test]
async fn invalid_uuid_and_integer_history_fields_are_counted_unreadable_not_collection_errors() {
    let store = new_test_store().await;
    let imported = person(&store, ALICE).await;
    let data = seed_context(&store, &imported, "unreadable-history-fields").await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let destination_access = context(&store, DESTINATION).await;
    let identities_before = serde_json::to_value(list_identities(&store).await.unwrap()).unwrap();
    inject_history_fixture(&store, false, 1).await.unwrap();
    let good = receipt_history(&store, 100)
        .await
        .unwrap()
        .receipts
        .remove(0);
    let bear_id = store.bear_id().to_string();
    let cases = [
        (MalformedReceiptField::ReceiptId, "not-a-uuid"),
        (MalformedReceiptField::ReceiptId, ""),
        (MalformedReceiptField::MissingReceiptId, ""),
        (MalformedReceiptField::Actor, "not-an-integer"),
        (MalformedReceiptField::Actor, "0"),
        (MalformedReceiptField::Actor, "-1"),
        (MalformedReceiptField::Actor, "2147483648"),
        (MalformedReceiptField::Previous, "not-an-integer"),
        (MalformedReceiptField::Previous, "0"),
        (MalformedReceiptField::Previous, "-1"),
        (MalformedReceiptField::Previous, "2147483648"),
        (MalformedReceiptField::Destination, "not-an-integer"),
        (MalformedReceiptField::Destination, "0"),
        (MalformedReceiptField::Destination, "-1"),
        (MalformedReceiptField::Destination, "2147483648"),
        (MalformedReceiptField::PriorGeneration, "not-an-integer"),
        (MalformedReceiptField::PriorGeneration, "0"),
        (MalformedReceiptField::PriorGeneration, "-1"),
        (
            MalformedReceiptField::PriorGeneration,
            "9223372036854775808",
        ),
        (MalformedReceiptField::ResultingGeneration, "not-an-integer"),
        (MalformedReceiptField::ResultingGeneration, "0"),
        (MalformedReceiptField::ResultingGeneration, "-1"),
        (MalformedReceiptField::ResultingGeneration, "1"),
        (MalformedReceiptField::ResultingGeneration, "1.5"),
        (
            MalformedReceiptField::ResultingGeneration,
            "9223372036854775808",
        ),
    ];
    for (field, invalid) in cases {
        let uuid = Uuid::new_v4().to_string();
        let mut receipt_id = Some(uuid.as_str());
        let mut actor = "900";
        let mut previous = "11";
        let mut destination = Some("101");
        let mut prior = "1";
        let mut resulting = "2";
        match field {
            MalformedReceiptField::ReceiptId => receipt_id = Some(invalid),
            MalformedReceiptField::MissingReceiptId => receipt_id = None,
            MalformedReceiptField::Actor => actor = invalid,
            MalformedReceiptField::Previous => previous = invalid,
            MalformedReceiptField::Destination => destination = Some(invalid),
            MalformedReceiptField::PriorGeneration => prior = invalid,
            MalformedReceiptField::ResultingGeneration => resulting = invalid,
        }
        // SQLite's integer affinity permits hostile imported text, floats and oversized
        // numbers. The history boundary must validate each row without granting anything.
        sqlx::query!(
            "INSERT INTO user_identity_receipts (receipt_id, bear_id, recorded_bear_id, actor_user_id, entity_id, destination_user_id, previous_user_id, displaced_entity_id, prior_generation, resulting_generation, recorded_at) VALUES (?, ?, 'source-bear', ?, ?, ?, ?, NULL, ?, ?, '9999-01-01')",
            receipt_id, bear_id, actor, imported, destination, previous, prior, resulting,
        ).execute(store.pool()).await.unwrap();
    }
    let before = authority_snapshot(&store).await;
    let context_before = context_snapshot(&store).await;
    let history = receipt_history(&store, 100).await.unwrap();
    assert_eq!(history.unreadable_entries, cases.len());
    assert_eq!(history.receipts.len(), 1);
    assert_eq!(history.receipts[0].receipt_id, good.receipt_id);
    assert_eq!(
        list_receipts(&store, 100).await.unwrap()[0].receipt_id,
        good.receipt_id
    );
    assert_eq!(
        serde_json::to_value(list_identities(&store).await.unwrap()).unwrap(),
        identities_before
    );
    assert_visible(&store, &data.active_memory_id, &destination_access, false).await;
    assert_visible(
        &store,
        &data.active_memory_id,
        &AccessContext::empty().with_audience([imported.clone()]),
        false,
    )
    .await;
    let review = review_reconnection(&store, ACTOR, &imported, DESTINATION)
        .await
        .unwrap();
    assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    assert_eq!(context_snapshot(&store).await, context_before);
}

#[tokio::test]
async fn one_hundred_future_source_receipts_cannot_mask_subsequent_local_recovery() {
    let store = new_test_store().await;
    let imported = person(&store, ALICE).await;
    let data = seed_context(&store, &imported, "future-source-history").await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    inject_history_fixture(&store, false, 100).await.unwrap();
    let source_history = receipt_history(&store, 100).await.unwrap();
    assert_eq!(source_history.receipts.len(), 100);
    assert_eq!(source_history.unreadable_entries, 0);
    assert!(source_history
        .receipts
        .iter()
        .all(|receipt| receipt.recorded_bear_id == "source-bear"
            && receipt.recorded_at == "9999-01-01"));
    let restored = reconnect(&store, &imported, DESTINATION).await;
    let active_access = context(&store, DESTINATION).await;
    assert_visible(&store, &data.active_memory_id, &active_access, true).await;
    let before_read = authority_snapshot(&store).await;
    for (limit, expected_count) in [(0, 1), (1, 1), (10, 10), (100, 100), (u32::MAX, 100)] {
        let history = receipt_history(&store, limit).await.unwrap();
        assert_eq!(history.receipts.len(), expected_count);
        assert_eq!(history.unreadable_entries, 0);
        assert_eq!(history.receipts[0].receipt_id, restored.receipt_id);
        assert_eq!(
            history.receipts[0].recorded_bear_id,
            store.bear_id().to_string()
        );
        assert_eq!(
            list_receipts(&store, limit).await.unwrap()[0].receipt_id,
            restored.receipt_id
        );
    }
    assert_eq!(authority_snapshot(&store).await, before_read);
    let review = review_revocation(&store, ACTOR, &imported).await.unwrap();
    let revoked = commit_review(&store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_receipt(&store, &review, &revoked).await;
    let latest = receipt_history(&store, 2).await.unwrap();
    assert_eq!(latest.receipts.len(), 2);
    assert!(latest
        .receipts
        .iter()
        .any(|receipt| receipt.receipt_id == restored.receipt_id));
    assert!(latest
        .receipts
        .iter()
        .any(|receipt| receipt.receipt_id == revoked.receipt_id
            && receipt.destination_user_id.is_none()));
    assert!(latest
        .receipts
        .iter()
        .all(|receipt| receipt.recorded_bear_id == store.bear_id().to_string()));
    assert_visible(&store, &data.active_memory_id, &active_access, false).await;
}

#[tokio::test]
async fn receipt_history_remains_bear_scoped_with_a_shared_sqlite_pool() {
    let store = new_test_store().await;
    let other = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    inject_history_fixture(&store, false, 2).await.unwrap();
    inject_history_fixture(&other, false, 3).await.unwrap();
    inject_history_fixture(&other, true, 1).await.unwrap();
    let before = authority_snapshot(&store).await;
    let current = receipt_history(&store, 100).await.unwrap();
    let foreign = receipt_history(&other, 100).await.unwrap();
    assert_eq!(current.receipts.len(), 2);
    assert_eq!(current.unreadable_entries, 0);
    assert_eq!(foreign.receipts.len(), 3);
    assert_eq!(foreign.unreadable_entries, 1);
    assert!(current.receipts.iter().all(|receipt| !foreign
        .receipts
        .iter()
        .any(|other| receipt.receipt_id == other.receipt_id)));
    assert_eq!(authority_snapshot(&store).await, before);
    assert!(list_identities(&store).await.unwrap().is_empty());
    assert!(list_identities(&other).await.unwrap().is_empty());
}

#[tokio::test]
async fn history_fixture_is_bounded_and_zero_count_is_a_no_op() {
    let store = new_test_store().await;
    let before = authority_snapshot(&store).await;
    inject_history_fixture(&store, false, 0).await.unwrap();
    inject_history_fixture(&store, true, 0).await.unwrap();
    for count in [101, u32::MAX] {
        for malformed in [false, true] {
            assert_validation(inject_history_fixture(&store, malformed, count).await);
        }
    }
    assert_eq!(authority_snapshot(&store).await, before);
    inject_history_fixture(&store, true, 100).await.unwrap();
    let history = receipt_history(&store, 100).await.unwrap();
    assert_eq!(history.unreadable_entries, 100);
    assert!(history.receipts.is_empty());
    assert_eq!(receipt_count(&store).await, 100);
}

#[tokio::test]
async fn invalid_utf8_in_any_imported_receipt_field_cannot_hide_local_recovery() {
    let store = new_test_store().await;
    let imported = parked_person(&store, ALICE).await;
    let local = reconnect(&store, &imported, DESTINATION).await;
    let before = receipt_history(&store, 100).await.unwrap();
    let bear_id = store.bear_id().to_string();
    for invalid_column in 0..10 {
        let mut fields = [
            Uuid::new_v4().to_string().into_bytes(),
            b"source-bear".to_vec(),
            b"900".to_vec(),
            imported.as_bytes().to_vec(),
            b"101".to_vec(),
            b"11".to_vec(),
            b"old-person".to_vec(),
            b"1".to_vec(),
            b"2".to_vec(),
            b"9999-01-01".to_vec(),
        ];
        fields[invalid_column] = vec![0x80];
        sqlx::query!(
            "INSERT INTO user_identity_receipts (receipt_id, bear_id, recorded_bear_id, actor_user_id, entity_id, destination_user_id, previous_user_id, displaced_entity_id, prior_generation, resulting_generation, recorded_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            fields[0], bear_id, fields[1], fields[2], fields[3], fields[4], fields[5], fields[6], fields[7], fields[8], fields[9],
        )
        .execute(store.pool())
        .await
        .unwrap();
    }
    let history = receipt_history(&store, 100).await.unwrap();
    assert_eq!(history.unreadable_entries, 10);
    assert_eq!(
        serde_json::to_value(&history.receipts).unwrap(),
        serde_json::to_value(&before.receipts).unwrap(),
        "both the initial revocation and reconnection receipts remain recoverable"
    );
    assert_eq!(history.receipts[0].receipt_id, local.receipt_id);
    assert_eq!(
        list_receipts(&store, 1).await.unwrap()[0].receipt_id,
        local.receipt_id
    );
    assert_eq!(person(&store, DESTINATION).await, imported);
}
