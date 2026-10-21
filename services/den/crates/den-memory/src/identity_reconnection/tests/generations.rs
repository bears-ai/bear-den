use super::*;
use crate::identity_reconnection::types::AuthorityState;

async fn set_generation(store: &BearMemoryStore, entity_id: &str, generation: i64) {
    let bear_id = store.bear_id().to_string();
    sqlx::query!(
        "UPDATE user_identity_generations SET generation = ? WHERE bear_id = ? AND entity_id = ?",
        generation,
        bear_id,
        entity_id,
    )
    .execute(store.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn imported_penultimate_epoch_cannot_activate_an_unrevokable_identity() {
    let store = new_test_store().await;
    let entity_id = person(&store, ALICE).await;
    let data = seed_context(&store, &entity_id, "imported-epoch-boundary").await;
    set_generation(&store, &entity_id, i64::MAX - 2).await;
    let binding = resolve_authenticated_user_entity(&store, ALICE)
        .await
        .unwrap();
    let old_access = AccessContext::for_authenticated_user(&binding);
    assert_visible(&store, &data.active_memory_id, &old_access, true).await;
    let mut review = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert_eq!(
        summary(&store, &entity_id).await.binding_version,
        i64::MAX - 1
    );
    let before = authority_snapshot(&store).await;
    let context_before = context_snapshot(&store).await;
    assert_validation(review_reconnection(&store, ACTOR, &entity_id, DESTINATION).await);
    // Model a server-side snapshot that predates the review-time safety check. Its
    // fields match the parked identity, so commit must enforce the epoch reserve itself.
    review.destination = Some(DESTINATION);
    review.selected.state = AuthorityState::Quarantined;
    review.selected.generation = i64::MAX - 1;
    assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    assert_eq!(context_snapshot(&store).await, context_before);
    assert_eq!(summary(&store, &entity_id).await.active_user_id, None);
    assert!(list_handles(&store, &entity_id).await.unwrap().is_empty());
    assert_eq!(receipt_count(&store).await, 0);
    assert_visible(&store, &data.active_memory_id, &old_access, false).await;
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
        &AccessContext::empty().with_audience([entity_id]),
        false,
    )
    .await;
}

#[tokio::test]
async fn direct_parked_identity_at_or_above_penultimate_epoch_cannot_activate() {
    for generation in [i64::MAX - 1, i64::MAX] {
        let store = new_test_store().await;
        let entity_id = person(&store, ALICE).await;
        let data = seed_context(&store, &entity_id, "parked-epoch-boundary").await;
        let binding = resolve_authenticated_user_entity(&store, ALICE)
            .await
            .unwrap();
        let revocation = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
        commit_review(&store, &revocation, ReplacementConsent::NoReplacement)
            .await
            .unwrap();
        let mut review = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
            .await
            .unwrap();
        set_generation(&store, &entity_id, generation).await;
        review.selected.generation = generation;
        let before = authority_snapshot(&store).await;
        let context_before = context_snapshot(&store).await;
        assert_validation(review_reconnection(&store, ACTOR, &entity_id, DESTINATION).await);
        assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
        assert_eq!(authority_snapshot(&store).await, before);
        assert_eq!(context_snapshot(&store).await, context_before);
        assert_eq!(summary(&store, &entity_id).await.active_user_id, None);
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
            &AccessContext::empty().with_audience([entity_id]),
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn last_safe_activation_reserves_the_final_epoch_for_a_successful_revocation() {
    let store = new_test_store().await;
    let entity_id = person(&store, ALICE).await;
    let data = seed_context(&store, &entity_id, "last-revokable-epoch").await;
    let old_binding = resolve_authenticated_user_entity(&store, ALICE)
        .await
        .unwrap();
    let revocation = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    commit_review(&store, &revocation, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    set_generation(&store, &entity_id, i64::MAX - 2).await;
    let context_before = context_snapshot(&store).await;
    let review = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
        .await
        .unwrap();
    let activation = commit_review(&store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_receipt(&store, &review, &activation).await;
    assert_eq!(activation.prior_generation, i64::MAX - 2);
    assert_eq!(activation.resulting_generation, i64::MAX - 1);
    let active = resolve_authenticated_user_entity(&store, DESTINATION)
        .await
        .unwrap();
    assert_eq!(active.entity_id(), entity_id);
    let active_access = AccessContext::for_authenticated_user(&active);
    assert_visible(&store, &data.active_memory_id, &active_access, true).await;
    assert_visible(
        &store,
        &data.active_memory_id,
        &AccessContext::for_authenticated_user(&old_binding),
        false,
    )
    .await;

    let review = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    let revoked = commit_review(&store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_receipt(&store, &review, &revoked).await;
    assert_eq!(revoked.prior_generation, i64::MAX - 1);
    assert_eq!(revoked.resulting_generation, i64::MAX);
    let parked = summary(&store, &entity_id).await;
    assert_eq!(parked.binding_version, i64::MAX);
    assert_eq!(parked.active_user_id, None);
    assert!(list_handles(&store, &entity_id).await.unwrap().is_empty());
    for denied in [
        active_access,
        AccessContext::for_authenticated_user(&active),
        AccessContext::empty().with_audience([entity_id.clone()]),
    ] {
        assert_visible(&store, &data.active_memory_id, &denied, false).await;
    }
    let after_revocation = authority_snapshot(&store).await;
    assert_validation(review_reconnection(&store, ACTOR, &entity_id, DESTINATION).await);
    assert_validation(review_revocation(&store, ACTOR, &entity_id).await);
    assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, after_revocation);
    assert_eq!(context_snapshot(&store).await, context_before);
}
