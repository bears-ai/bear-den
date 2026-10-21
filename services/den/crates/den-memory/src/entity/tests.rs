use super::*;
use crate::authenticated_user::{resolve_authenticated_user_entity, AuthenticatedUserEntity};
use crate::test_support::new_test_store;
use den_core::ids::UserId;

async fn new_person(store: &BearMemoryStore, name: &str) -> EntityRow {
    create_entity(
        store,
        "person",
        Some(name),
        ResolutionState::Resolved,
        EntityTrust::Inferred,
        None,
        &Value::Object(Default::default()),
    )
    .await
    .unwrap()
}

async fn bound_person(store: &BearMemoryStore) -> AuthenticatedUserEntity {
    resolve_authenticated_user_entity(store, UserId::new(1))
        .await
        .unwrap()
}

fn assert_mutation_rejected<T>(result: Result<T, DenError>) {
    assert!(
        result.is_err(),
        "generic mutation of a bound identity must fail"
    );
}

#[tokio::test]
async fn create_get_and_handle_lookup_round_trip() {
    let store = new_test_store().await;
    let ryan = new_person(&store, "Ryan").await;
    attach_handle(
        &store,
        &ryan.entity_id,
        "email",
        "ryan@acme.com",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();

    let fetched = get_entity(&store, &ryan.entity_id).await.unwrap().unwrap();
    assert_eq!(fetched.display_name.as_deref(), Some("Ryan"));

    let by_handle = find_entity_by_handle(&store, "email", "ryan@acme.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_handle.entity_id, ryan.entity_id);

    let handles = list_handles(&store, &ryan.entity_id).await.unwrap();
    assert_eq!(handles.len(), 1);
}

#[tokio::test]
async fn unknown_entity_type_rejected() {
    let store = new_test_store().await;
    let err = create_entity(
        &store,
        "not_a_type",
        None,
        ResolutionState::Observed,
        EntityTrust::Inferred,
        None,
        &Value::Object(Default::default()),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DenError::ValidationError(_)));
}

#[tokio::test]
async fn merge_forward_points_and_rehomes_handles() {
    let store = new_test_store().await;
    let survivor = new_person(&store, "Ryan").await;
    let loser = new_person(&store, "Ryan (dup)").await;
    attach_handle(
        &store,
        &loser.entity_id,
        "slack_user",
        "T01:U07",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();

    let returned = merge_entities(&store, &survivor.entity_id, &loser.entity_id)
        .await
        .unwrap();
    assert_eq!(returned.entity_id, survivor.entity_id);

    let dead = get_entity(&store, &loser.entity_id).await.unwrap().unwrap();
    assert_eq!(dead.resolution, ResolutionState::Merged);
    assert_eq!(
        dead.superseded_by_entity_id.as_deref(),
        Some(survivor.entity_id.as_str())
    );
    let live = resolve_live_entity(&store, &loser.entity_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.entity_id, survivor.entity_id);

    let by_handle = find_entity_by_handle(&store, "slack_user", "T01:U07")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_handle.entity_id, survivor.entity_id);
}

#[tokio::test]
async fn split_rehomes_offending_handle_to_new_entity() {
    let store = new_test_store().await;
    let merged = new_person(&store, "Ryan").await;
    let h_email = attach_handle(
        &store,
        &merged.entity_id,
        "email",
        "ryan@acme.com",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    let h_slack = attach_handle(
        &store,
        &merged.entity_id,
        "slack_user",
        "T01:U07",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();

    let split = split_entity(
        &store,
        "person",
        Some("Other Ryan"),
        std::slice::from_ref(&h_slack.handle_id),
        ResolutionState::Provisional,
        EntityTrust::Inferred,
    )
    .await
    .unwrap();

    let slack_owner = find_entity_by_handle(&store, "slack_user", "T01:U07")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(slack_owner.entity_id, split.entity_id);

    let email_owner = find_entity_by_handle(&store, "email", "ryan@acme.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(email_owner.entity_id, merged.entity_id);
    assert_ne!(h_email.handle_id, h_slack.handle_id);
}

#[tokio::test]
async fn bound_entity_resolution_and_trust_mutations_are_rejected() {
    let store = new_test_store().await;
    let bound = bound_person(&store).await;
    for resolution in [
        ResolutionState::Rejected,
        ResolutionState::Merged,
        ResolutionState::Observed,
        ResolutionState::Provisional,
        ResolutionState::Resolved,
        ResolutionState::Confirmed,
    ] {
        for trust in [
            None,
            Some(EntityTrust::Inferred),
            Some(EntityTrust::Asserted),
        ] {
            assert_mutation_rejected(
                set_resolution(&store, bound.entity_id(), resolution, trust).await,
            );
        }
    }
    let unchanged = get_entity(&store, bound.entity_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.resolution, bound.entity().resolution);
    assert_eq!(unchanged.trust, bound.entity().trust);
}

#[tokio::test]
async fn bound_entity_canonical_ref_mutations_are_rejected() {
    let store = new_test_store().await;
    let bound = bound_person(&store).await;
    for canonical_ref in [
        None,
        Some("den:user:2"),
        bound.entity().canonical_ref.as_deref(),
    ] {
        assert_mutation_rejected(set_canonical_ref(&store, bound.entity_id(), canonical_ref).await);
    }
    let unchanged = get_entity(&store, bound.entity_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.canonical_ref, bound.entity().canonical_ref);
}

#[tokio::test]
async fn bound_entity_cannot_be_either_merge_participant() {
    let store = new_test_store().await;
    let bound = bound_person(&store).await;
    let other = new_person(&store, "Unbound").await;
    let handle = attach_handle(
        &store,
        &other.entity_id,
        "email",
        "unbound@acme.com",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    assert_mutation_rejected(merge_entities(&store, bound.entity_id(), &other.entity_id).await);
    assert_mutation_rejected(merge_entities(&store, &other.entity_id, bound.entity_id()).await);

    for entity_id in [bound.entity_id(), other.entity_id.as_str()] {
        let unchanged = get_entity(&store, entity_id).await.unwrap().unwrap();
        assert!(unchanged.superseded_by_entity_id.is_none());
        assert!(unchanged.resolution.can_bear_access());
    }
    assert_eq!(
        list_handles(&store, &other.entity_id).await.unwrap()[0].handle_id,
        handle.handle_id
    );
}

#[tokio::test]
async fn merge_rejects_bound_resolved_survivor_behind_forward_pointer() {
    let store = new_test_store().await;
    let bound = bound_person(&store).await;
    let alias = new_person(&store, "Legacy alias").await;
    let loser = new_person(&store, "Unbound").await;
    // Simulate a pre-existing pointer; the public merge path must not create one now.
    let bear_id = store.bear_id().to_string();
    let bound_entity_id = bound.entity_id();
    sqlx::query!(
        "UPDATE entities SET resolution = 'merged', superseded_by_entity_id = ? WHERE bear_id = ? AND entity_id = ?",
        bound_entity_id,
        bear_id,
        alias.entity_id,
    )
    .execute(store.pool())
    .await
    .unwrap();

    assert_mutation_rejected(merge_entities(&store, &alias.entity_id, &loser.entity_id).await);
    let unchanged = get_entity(&store, &loser.entity_id).await.unwrap().unwrap();
    assert_eq!(unchanged.resolution, ResolutionState::Resolved);
    assert!(unchanged.superseded_by_entity_id.is_none());
}

#[tokio::test]
async fn bound_entity_handle_attach_and_detach_are_rejected() {
    let store = new_test_store().await;
    let bound = bound_person(&store).await;
    let handles = list_handles(&store, bound.entity_id()).await.unwrap();
    assert!(
        !handles.is_empty(),
        "authenticated binding must have a handle"
    );
    assert_mutation_rejected(
        attach_handle(
            &store,
            bound.entity_id(),
            "email",
            "injected@acme.com",
            None,
            EntityTrust::Asserted,
        )
        .await,
    );
    for handle in &handles {
        // Even an otherwise idempotent attachment is not a generic user mutation API.
        assert_mutation_rejected(
            attach_handle(
                &store,
                bound.entity_id(),
                &handle.handle_type,
                &handle.handle_value,
                handle.source.as_deref(),
                handle.trust,
            )
            .await,
        );
        assert_mutation_rejected(detach_handle(&store, &handle.handle_id).await);
    }
    let unchanged = list_handles(&store, bound.entity_id()).await.unwrap();
    assert_eq!(unchanged.len(), handles.len());
    for handle in handles {
        assert!(unchanged
            .iter()
            .any(|row| row.handle_id == handle.handle_id));
    }
}

#[tokio::test]
async fn split_rejects_bound_handles_before_creating_entity_or_moving_any_handle() {
    let store = new_test_store().await;
    let bound = bound_person(&store).await;
    let bound_handles = list_handles(&store, bound.entity_id()).await.unwrap();
    assert!(!bound_handles.is_empty());
    let other = new_person(&store, "Unbound").await;
    let ordinary_handle = attach_handle(
        &store,
        &other.entity_id,
        "email",
        "unbound@acme.com",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    let before = list_entities(&store, None, 100).await.unwrap();

    for handle in &bound_handles {
        for handle_ids in [
            vec![handle.handle_id.clone()],
            vec![ordinary_handle.handle_id.clone(), handle.handle_id.clone()],
        ] {
            assert_mutation_rejected(
                split_entity(
                    &store,
                    "person",
                    Some("Attempted split"),
                    &handle_ids,
                    ResolutionState::Resolved,
                    EntityTrust::Asserted,
                )
                .await,
            );
        }
    }
    assert_eq!(
        list_entities(&store, None, 100).await.unwrap().len(),
        before.len()
    );
    let bound_handles_after = list_handles(&store, bound.entity_id()).await.unwrap();
    assert_eq!(bound_handles_after.len(), bound_handles.len());
    for handle in bound_handles {
        assert!(bound_handles_after
            .iter()
            .any(|row| row.handle_id == handle.handle_id));
    }
    assert_eq!(
        list_handles(&store, &other.entity_id).await.unwrap()[0].handle_id,
        ordinary_handle.handle_id
    );
}

#[tokio::test]
async fn unbound_entity_resolution_canonical_and_handle_mutations_remain_available() {
    let store = new_test_store().await;
    let person = new_person(&store, "Unbound").await;
    set_resolution(
        &store,
        &person.entity_id,
        ResolutionState::Confirmed,
        Some(EntityTrust::Asserted),
    )
    .await
    .unwrap();
    set_canonical_ref(&store, &person.entity_id, Some("cabinet:people/unbound"))
        .await
        .unwrap();
    let handle = attach_handle(
        &store,
        &person.entity_id,
        "email",
        "unbound@acme.com",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    detach_handle(&store, &handle.handle_id).await.unwrap();

    let updated = get_entity(&store, &person.entity_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.resolution, ResolutionState::Confirmed);
    assert_eq!(updated.trust, EntityTrust::Asserted);
    assert_eq!(
        updated.canonical_ref.as_deref(),
        Some("cabinet:people/unbound")
    );
    assert!(list_handles(&store, &person.entity_id)
        .await
        .unwrap()
        .is_empty());
}
