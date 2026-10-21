use super::*;

async fn replacement_requires_consent_and_preserves_displaced_identity(populated: bool) {
    let store = new_test_store().await;
    let portable = person(&store, ALICE).await;
    let portable_data = seed_context(&store, &portable, "portable").await;
    record_export_origins(
        &store,
        "https://source.example",
        ALICE,
        &[ExportUserLabel {
            user_id: ALICE,
            username: Some("alice".into()),
            display_name: Some("Portable Alice".into()),
        }],
    )
    .await
    .unwrap();
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let displaced_binding = resolve_authenticated_user_entity(&store, DESTINATION)
        .await
        .unwrap();
    let displaced = displaced_binding.entity_id().to_owned();
    let displaced_context = AccessContext::for_authenticated_user(&displaced_binding);
    let displaced_data = if populated {
        Some(seed_context(&store, &displaced, "destination").await)
    } else {
        None
    };
    record_export_origins(
        &store,
        "https://destination.example",
        DESTINATION,
        &[
            // Even identical labels are not a request to merge these people.
            ExportUserLabel {
                user_id: DESTINATION,
                username: Some("alice".into()),
                display_name: Some("Portable Alice".into()),
            },
        ],
    )
    .await
    .unwrap();
    let portable_before = summary(&store, &portable).await;
    let displaced_before = summary(&store, &displaced).await;
    let portable_handles = historical_handles(&store, &portable).await;
    let displaced_handles = historical_handles(&store, &displaced).await;
    let displaced_entity_before = get_entity(&store, &displaced).await.unwrap().unwrap();
    let context_before = context_snapshot(&store).await;
    let authority_before = authority_snapshot(&store).await;
    let review = review_reconnection(&store, ACTOR, &portable, DESTINATION)
        .await
        .unwrap();
    assert_eq!(review.entity_id(), portable);
    assert_eq!(review.displaced_entity_id(), Some(displaced.as_str()));
    assert_eq!(review.destination_user_id(), Some(DESTINATION));
    assert_eq!(authority_snapshot(&store).await, authority_before);
    assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
    assert_eq!(
        authority_snapshot(&store).await,
        authority_before,
        "refusal must not partially park either person"
    );
    assert_eq!(context_snapshot(&store).await, context_before);
    if let Some(data) = &displaced_data {
        assert_visible(&store, &data.active_memory_id, &displaced_context, true).await;
    }

    // The same unconsumed review can be explicitly confirmed after the refusal.
    let receipt = commit_review(
        &store,
        &review,
        ReplacementConsent::PreserveDisplacedIdentity,
    )
    .await
    .unwrap();
    assert_receipt(&store, &review, &receipt).await;
    assert_eq!(
        person(&store, DESTINATION).await,
        portable,
        "select the original portable identity, never the fresh person"
    );
    let portable_after = summary(&store, &portable).await;
    let displaced_after = summary(&store, &displaced).await;
    assert_eq!(portable_after.active_user_id, Some(DESTINATION));
    assert_eq!(
        portable_after.binding_version,
        portable_before.binding_version + 1
    );
    assert_eq!(displaced_after.active_user_id, None);
    assert_eq!(
        displaced_after.binding_version,
        displaced_before.binding_version + 1
    );
    assert_eq!(list_identities(&store).await.unwrap().len(), 2);
    for entity_id in [&portable, &displaced] {
        let entity = get_entity(&store, entity_id).await.unwrap().unwrap();
        assert!(
            entity.superseded_by_entity_id.is_none(),
            "replacement is not a merge"
        );
        assert_ne!(entity.resolution, ResolutionState::Merged);
    }
    assert!(list_handles(&store, &displaced).await.unwrap().is_empty());
    assert_historical_handles_detached(&store, &portable, &portable_handles).await;
    assert_historical_handles_detached(&store, &displaced, &displaced_handles).await;
    let parked = get_entity(&store, &displaced).await.unwrap().unwrap();
    assert_eq!(parked.sequence_no, displaced_entity_before.sequence_no);
    assert_eq!(parked.display_name, displaced_entity_before.display_name);
    assert_eq!(parked.metadata_json, displaced_entity_before.metadata_json);
    assert_eq!(parked.created_at, displaced_entity_before.created_at);
    assert_eq!(parked.resolution, ResolutionState::Provisional);
    assert_eq!(parked.trust, EntityTrust::Inferred);
    assert!(parked.canonical_ref.is_none());
    let new_context = context(&store, DESTINATION).await;
    assert_visible(&store, &portable_data.active_memory_id, &new_context, true).await;
    assert_visible(
        &store,
        &portable_data.active_memory_id,
        &displaced_context,
        false,
    )
    .await;
    if let Some(data) = &displaced_data {
        for denied in [
            new_context.clone(),
            displaced_context.clone(),
            AccessContext::for_authenticated_user(&displaced_binding),
            AccessContext::empty().with_audience([displaced.clone(), portable.clone()]),
        ] {
            assert_visible(&store, &data.active_memory_id, &denied, false).await;
        }
    }
    assert_eq!(
        context_snapshot(&store).await,
        context_before,
        "records, links, candidates, lifecycle and origins are untouched"
    );

    // Parking is lossless and reversible, even for an otherwise unused destination person.
    let later_user = UserId::new(303);
    let later_receipt = reconnect(&store, &displaced, later_user).await;
    assert_eq!(later_receipt.entity_id, displaced);
    assert_eq!(person(&store, later_user).await, displaced);
    assert_historical_handles_detached(&store, &displaced, &displaced_handles).await;
    assert_eq!(person(&store, DESTINATION).await, portable);
    let restored_context = context(&store, later_user).await;
    if let Some(data) = &displaced_data {
        for memory_id in &data.memory_ids {
            assert_visible(&store, memory_id, &restored_context, true).await;
            assert_visible(&store, memory_id, &new_context, false).await;
            assert_visible(&store, memory_id, &displaced_context, false).await;
        }
    }
    assert_visible(
        &store,
        &portable_data.active_memory_id,
        &restored_context,
        false,
    )
    .await;
    assert_visible(&store, &portable_data.active_memory_id, &new_context, true).await;
    assert_eq!(context_snapshot(&store).await, context_before);
    assert_eq!(list_identities(&store).await.unwrap().len(), 2);
}

#[tokio::test]
async fn even_a_fresh_destination_person_requires_explicit_lossless_replacement() {
    replacement_requires_consent_and_preserves_displaced_identity(false).await;
}

#[tokio::test]
async fn destination_with_records_relations_lifecycle_and_provenance_is_parked_not_merged() {
    replacement_requires_consent_and_preserves_displaced_identity(true).await;
}
