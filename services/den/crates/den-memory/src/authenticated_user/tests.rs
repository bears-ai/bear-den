use std::sync::Arc;

use sqlx::sqlite::SqlitePoolOptions;
use tokio::sync::Barrier;

use super::*;
use crate::{
    append_relation, attach_handle, create_entity, descriptors, detach_handle, get_entity,
    list_entities, list_handles, merge_entities, record_visible, set_canonical_ref, set_resolution,
    test_support::new_test_store, AccessContext, EntityTrust, HandleStrength, ResolutionState,
};

async fn historical_person(store: &BearMemoryStore, user_id: UserId) -> EntityRow {
    let canonical_ref = format!("den:user:{user_id}");
    let entity = create_entity(
        store,
        "person",
        Some("Same display name"),
        ResolutionState::Confirmed,
        EntityTrust::Asserted,
        Some(&canonical_ref),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    attach_handle(
        store,
        &entity.entity_id,
        "den_user",
        &user_id.to_string(),
        Some("historical import"),
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    attach_handle(
        store,
        &entity.entity_id,
        "alias",
        "same_username",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    entity
}

async fn assert_counts(store: &BearMemoryStore, bindings: i64, entities: i64, handles: i64) {
    let bear_id = store.bear_id().to_string();
    let counts = sqlx::query!(
        r#"
        SELECT
            (SELECT COUNT(*) FROM authenticated_user_entities
             WHERE bear_id = ? AND state = 'active') AS "bindings!: i64",
            (SELECT COUNT(*) FROM entities WHERE bear_id = ?) AS "entities!: i64",
            (SELECT COUNT(*) FROM entity_handles WHERE bear_id = ?) AS "handles!: i64"
        "#,
        bear_id,
        bear_id,
        bear_id,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(counts.bindings, bindings);
    assert_eq!(counts.entities, entities);
    assert_eq!(counts.handles, handles);
}

// Deliberately bypass mutation guards to model corrupted/imported database contents.
async fn persist_entity_snapshot(store: &BearMemoryStore, entity: &EntityRow) {
    let bear_id = store.bear_id().to_string();
    let resolution = entity.resolution.as_str();
    let trust = entity.trust.as_str();
    let metadata_json = entity.metadata_json.to_string();
    sqlx::query!(
        r"
        UPDATE entities
        SET type = ?, display_name = ?, resolution = ?, trust = ?, canonical_ref = ?,
            superseded_by_entity_id = ?, metadata_json = ?
        WHERE bear_id = ? AND entity_id = ?
        ",
        entity.entity_type,
        entity.display_name,
        resolution,
        trust,
        entity.canonical_ref,
        entity.superseded_by_entity_id,
        metadata_json,
        bear_id,
        entity.entity_id,
    )
    .execute(store.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn same_user_is_idempotent_and_creates_a_confirmed_asserted_person() {
    let store = new_test_store().await;
    let user_id = UserId::new(42);
    let first = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    let second = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();

    assert_eq!(first.bear_id(), BearId::from(store.bear_id()));
    assert_eq!(first.user_id(), user_id);
    assert_eq!(
        user_entity_authority(&store, first.entity_id())
            .await
            .unwrap(),
        Some(UserEntityAuthority::Active(user_id))
    );
    assert_eq!(first.entity_id(), second.entity_id());
    assert_eq!(first.entity_id(), first.clone().entity_id());
    let entity = first.entity();
    assert_eq!(entity.entity_type, "person");
    assert_eq!(entity.resolution, ResolutionState::Confirmed);
    assert_eq!(entity.trust, EntityTrust::Asserted);
    assert_eq!(entity.canonical_ref.as_deref(), Some("den:user:42"));
    assert!(entity.display_name.is_none());
    assert!(entity.superseded_by_entity_id.is_none());
    assert_eq!(entity.sequence_no, second.entity().sequence_no);
    let handles = list_handles(&store, first.entity_id()).await.unwrap();
    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].handle_type, "den_user");
    assert_eq!(handles[0].handle_value, "42");
    assert_eq!(handles[0].trust, EntityTrust::Asserted);
    assert_eq!(
        descriptors::handle_strength(&handles[0].handle_type),
        HandleStrength::Strong
    );
    assert_counts(&store, 1, 1, 1).await;
}

#[tokio::test]
async fn different_users_and_bears_have_separate_bindings() {
    let store = new_test_store().await;
    // Sharing a pool makes the test check SQL bear scoping, not just file separation.
    let other_bear = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let alice = resolve_authenticated_user_entity(&store, UserId::new(42))
        .await
        .unwrap();
    let bob = resolve_authenticated_user_entity(&store, UserId::new(43))
        .await
        .unwrap();
    let elsewhere = resolve_authenticated_user_entity(&other_bear, UserId::new(42))
        .await
        .unwrap();

    assert_ne!(alice.entity_id(), bob.entity_id());
    assert_ne!(alice.entity_id(), elsewhere.entity_id());
    assert_ne!(bob.entity_id(), elsewhere.entity_id());
    assert_ne!(alice.bear_id(), elsewhere.bear_id());
    assert_eq!(alice.user_id(), elsewhere.user_id());
    assert_eq!(
        authenticated_user_for_entity(&store, elsewhere.entity_id())
            .await
            .unwrap(),
        None
    );
    ensure_entity_mutable(&store, elsewhere.entity_id())
        .await
        .unwrap();
    let elsewhere_handles = list_handles(&other_bear, elsewhere.entity_id())
        .await
        .unwrap();
    ensure_handle_mutable(&store, &elsewhere_handles[0].handle_id)
        .await
        .unwrap();
    assert_counts(&store, 2, 2, 2).await;
    assert_counts(&other_bear, 1, 1, 1).await;
}

#[tokio::test]
async fn identical_labels_refs_and_historical_handles_are_not_authority() {
    let store = new_test_store().await;
    let user_id = UserId::new(42);
    let first = historical_person(&store, user_id).await;
    let second = historical_person(&store, user_id).await;
    let authenticated = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();

    assert_ne!(authenticated.entity_id(), first.entity_id);
    assert_ne!(authenticated.entity_id(), second.entity_id);
    for historical in [first, second] {
        assert_eq!(
            authenticated_user_for_entity(&store, &historical.entity_id)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            user_entity_authority(&store, &historical.entity_id)
                .await
                .unwrap(),
            None
        );
        ensure_entity_mutable(&store, &historical.entity_id)
            .await
            .unwrap();
        for handle in list_handles(&store, &historical.entity_id).await.unwrap() {
            ensure_handle_mutable(&store, &handle.handle_id)
                .await
                .unwrap();
        }
    }
    assert_counts(&store, 1, 3, 5).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_resolution_across_independent_pools_creates_exactly_once() {
    let store = Arc::new(new_test_store().await);
    let other_pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(store.pool().connect_options().as_ref().clone())
        .await
        .unwrap();
    let other_store = Arc::new(BearMemoryStore::new(store.bear_id(), other_pool));
    let barrier = Arc::new(Barrier::new(24));
    let mut tasks = Vec::new();
    for index in 0..24 {
        let contender = Arc::clone(if index % 2 == 0 { &store } else { &other_store });
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            resolve_authenticated_user_entity(&contender, UserId::new(42))
                .await
                .unwrap()
        }));
    }
    let mut expected = None;
    for task in tasks {
        let resolved = task.await.unwrap();
        if let Some(entity_id) = &expected {
            assert_eq!(resolved.entity_id(), entity_id);
        } else {
            expected = Some(resolved.entity_id().to_string());
        }
    }
    assert_counts(&store, 1, 1, 1).await;
}

#[tokio::test]
async fn guard_helpers_reject_bound_entities_and_handles_only() {
    let store = new_test_store().await;
    let user_id = UserId::new(42);
    let authenticated = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    assert_eq!(
        authenticated_user_for_entity(&store, authenticated.entity_id())
            .await
            .unwrap(),
        Some(user_id)
    );
    assert!(matches!(
        ensure_entity_mutable(&store, authenticated.entity_id()).await,
        Err(DenError::ValidationError(_))
    ));
    let handles = list_handles(&store, authenticated.entity_id())
        .await
        .unwrap();
    assert!(matches!(
        ensure_handle_mutable(&store, &handles[0].handle_id).await,
        Err(DenError::ValidationError(_))
    ));
    ensure_entity_mutable(&store, "unknown-entity")
        .await
        .unwrap();
    ensure_handle_mutable(&store, "unknown-handle")
        .await
        .unwrap();
    assert_eq!(
        authenticated_user_for_entity(&store, "unknown-entity")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn failed_creation_rolls_back_binding_entity_and_handle() {
    let store = new_test_store().await;
    // A missing sequence allocator forces failure after the binding reservation.
    sqlx::query!("DELETE FROM bear_sequence WHERE id = 1")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(resolve_authenticated_user_entity(&store, UserId::new(42))
        .await
        .is_err());
    assert_counts(&store, 0, 0, 0).await;
    sqlx::query!("INSERT INTO bear_sequence (id, next_sequence) VALUES (1, 1)")
        .execute(store.pool())
        .await
        .unwrap();
    resolve_authenticated_user_entity(&store, UserId::new(42))
        .await
        .unwrap();
    assert_counts(&store, 1, 1, 1).await;
}

#[tokio::test]
async fn import_quarantine_retains_immutable_demoted_identities_and_never_rebinds_them() {
    let store = new_test_store().await;
    let other_bear = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let user_id = UserId::new(42);
    let imported = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    let imported_handles = list_handles(&store, imported.entity_id()).await.unwrap();
    let unrelated = resolve_authenticated_user_entity(&other_bear, user_id)
        .await
        .unwrap();

    let quarantined = quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert_eq!(quarantined, vec![imported.entity_id().to_string()]);
    assert_eq!(
        quarantine_imported_authenticated_users(&store)
            .await
            .unwrap(),
        quarantined
    );
    assert_counts(&store, 0, 1, 1).await;
    assert_counts(&other_bear, 1, 1, 1).await;
    assert_eq!(
        authenticated_user_for_entity(&store, imported.entity_id())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        user_entity_authority(&store, imported.entity_id())
            .await
            .unwrap(),
        Some(UserEntityAuthority::Quarantined)
    );
    let demoted = get_entity(&store, imported.entity_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(demoted.entity_type, "person");
    assert_eq!(demoted.resolution, ResolutionState::Provisional);
    assert_eq!(demoted.trust, EntityTrust::Inferred);
    assert!(demoted.canonical_ref.is_none());
    assert!(demoted.superseded_by_entity_id.is_none());
    assert!(list_handles(&store, imported.entity_id())
        .await
        .unwrap()
        .is_empty());
    let bear_id = store.bear_id().to_string();
    let imported_entity_id = imported.entity_id();
    let states = sqlx::query_scalar!(
        "SELECT state FROM entity_handles WHERE bear_id = ? AND entity_id = ?",
        bear_id,
        imported_entity_id,
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(states, vec!["detached".to_string()]);
    assert!(ensure_entity_mutable(&store, imported.entity_id())
        .await
        .is_err());
    assert!(
        ensure_handle_mutable(&store, &imported_handles[0].handle_id)
            .await
            .is_err()
    );
    assert!(detach_handle(&store, &imported_handles[0].handle_id)
        .await
        .is_err());
    assert!(set_resolution(
        &store,
        imported.entity_id(),
        ResolutionState::Confirmed,
        Some(EntityTrust::Asserted),
    )
    .await
    .is_err());
    assert!(
        set_canonical_ref(&store, imported.entity_id(), Some("den:user:42"))
            .await
            .is_err()
    );
    assert!(attach_handle(
        &store,
        imported.entity_id(),
        "den_user",
        "42",
        None,
        EntityTrust::Asserted,
    )
    .await
    .is_err());

    let destination = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    assert_ne!(destination.entity_id(), imported.entity_id());
    let again = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    assert_eq!(again.entity_id(), destination.entity_id());
    assert_eq!(
        user_entity_authority(&store, imported.entity_id())
            .await
            .unwrap(),
        Some(UserEntityAuthority::Quarantined)
    );
    assert_eq!(
        user_entity_authority(&store, destination.entity_id())
            .await
            .unwrap(),
        Some(UserEntityAuthority::Active(user_id))
    );
    assert_eq!(
        resolve_authenticated_user_entity(&other_bear, user_id)
            .await
            .unwrap()
            .entity_id(),
        unrelated.entity_id()
    );
    assert_eq!(
        list_entities(&store, Some("person"), 10)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_counts(&store, 1, 2, 2).await;
}

#[tokio::test]
async fn resolver_returns_the_current_valid_entity_snapshot() {
    let store = new_test_store().await;
    let user_id = UserId::new(42);
    let original = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    let mut current = original.entity().clone();
    current.display_name = Some("Current database snapshot".to_string());
    current.metadata_json = serde_json::json!({"snapshot": 2});
    persist_entity_snapshot(&store, &current).await;

    let resolved = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    assert_eq!(resolved.entity_id(), original.entity_id());
    assert_eq!(
        serde_json::to_value(resolved.entity()).unwrap(),
        serde_json::to_value(&current).unwrap()
    );
    assert!(original.entity().display_name.is_none());
    assert_counts(&store, 1, 1, 1).await;
}

#[tokio::test]
async fn corrupted_existing_bindings_fail_closed_without_repair_or_adoption() {
    let store = new_test_store().await;
    let user_id = UserId::new(42);
    let original = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    let mut corruptions = Vec::new();
    for resolution in [
        ResolutionState::Observed,
        ResolutionState::Provisional,
        ResolutionState::Resolved,
        ResolutionState::Rejected,
        ResolutionState::Merged,
    ] {
        let mut corrupted = original.entity().clone();
        corrupted.resolution = resolution;
        corruptions.push(corrupted);
    }
    for entity_type in ["org", "PERSON"] {
        let mut corrupted = original.entity().clone();
        corrupted.entity_type = entity_type.to_string();
        corruptions.push(corrupted);
    }
    let mut inferred = original.entity().clone();
    inferred.trust = EntityTrust::Inferred;
    corruptions.push(inferred);
    for pointer in [original.entity_id().to_string(), Uuid::new_v4().to_string()] {
        let mut corrupted = original.entity().clone();
        corrupted.superseded_by_entity_id = Some(pointer);
        corruptions.push(corrupted);
    }
    for canonical_ref in [None, Some("den:user:43".to_string())] {
        let mut corrupted = original.entity().clone();
        corrupted.canonical_ref = canonical_ref;
        corruptions.push(corrupted);
    }

    for corrupted in corruptions {
        persist_entity_snapshot(&store, &corrupted).await;
        assert!(
            resolve_authenticated_user_entity(&store, user_id)
                .await
                .is_err(),
            "accepted corrupt bound entity: {corrupted:?}"
        );
        assert_eq!(
            user_entity_authority(&store, original.entity_id())
                .await
                .unwrap(),
            Some(UserEntityAuthority::Active(user_id))
        );
        let stored = get_entity(&store, original.entity_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(stored).unwrap(),
            serde_json::to_value(corrupted).unwrap()
        );
        assert_counts(&store, 1, 1, 1).await;
    }
    // Restoring valid storage explicitly is distinct from a resolver silently repairing it.
    persist_entity_snapshot(&store, original.entity()).await;
    assert_eq!(
        resolve_authenticated_user_entity(&store, user_id)
            .await
            .unwrap()
            .entity_id(),
        original.entity_id()
    );
}

#[tokio::test]
async fn quarantine_preserves_terminal_lifecycle_and_only_demotes_marked_entities() {
    let store = new_test_store().await;
    let generic = historical_person(&store, UserId::new(999)).await;
    let mut imported = Vec::new();
    for (user_id, resolution) in (100..).zip([
        ResolutionState::Rejected,
        ResolutionState::Merged,
        ResolutionState::Observed,
        ResolutionState::Provisional,
        ResolutionState::Resolved,
        ResolutionState::Confirmed,
    ]) {
        let binding = resolve_authenticated_user_entity(&store, UserId::new(user_id))
            .await
            .unwrap();
        let mut snapshot = binding.entity().clone();
        snapshot.resolution = resolution;
        if resolution == ResolutionState::Merged {
            snapshot.superseded_by_entity_id = Some(generic.entity_id.clone());
        }
        persist_entity_snapshot(&store, &snapshot).await;
        imported.push(snapshot);
    }
    let quarantined = quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let mut expected: Vec<_> = imported
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect();
    expected.sort();
    assert_eq!(quarantined, expected);
    for before in imported {
        let after = get_entity(&store, &before.entity_id)
            .await
            .unwrap()
            .unwrap();
        let expected_resolution = match before.resolution {
            ResolutionState::Rejected | ResolutionState::Merged => before.resolution,
            _ => ResolutionState::Provisional,
        };
        assert_eq!(after.resolution, expected_resolution);
        assert_eq!(
            after.superseded_by_entity_id,
            before.superseded_by_entity_id
        );
        assert_eq!(after.trust, EntityTrust::Inferred);
        assert!(after.canonical_ref.is_none());
        assert!(list_handles(&store, &before.entity_id)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            user_entity_authority(&store, &before.entity_id)
                .await
                .unwrap(),
            Some(UserEntityAuthority::Quarantined)
        );
        assert!(ensure_entity_mutable(&store, &before.entity_id)
            .await
            .is_err());
    }
    assert_eq!(
        serde_json::to_value(
            get_entity(&store, &generic.entity_id)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&generic).unwrap()
    );
    assert_eq!(
        list_handles(&store, &generic.entity_id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        user_entity_authority(&store, &generic.entity_id)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn quarantined_markers_never_grant_raw_audiences_after_reconfirmation_or_merge() {
    let store = new_test_store().await;
    let user_id = UserId::new(42);
    let binding = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    append_relation(
        &store,
        "private-user-record",
        binding.entity_id(),
        "audience",
        &serde_json::json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    let raw = AccessContext::empty().with_audience([binding.entity_id()]);
    let verified = AccessContext::for_authenticated_user(&binding);
    assert!(!record_visible(&store, "private-user-record", &raw)
        .await
        .unwrap());
    assert!(record_visible(&store, "private-user-record", &verified)
        .await
        .unwrap());
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert!(!record_visible(&store, "private-user-record", &raw)
        .await
        .unwrap());
    assert!(!record_visible(&store, "private-user-record", &verified)
        .await
        .unwrap());

    // Even a direct DB re-confirmation cannot erase the permanent quarantined marker.
    persist_entity_snapshot(&store, binding.entity()).await;
    assert_eq!(
        user_entity_authority(&store, binding.entity_id())
            .await
            .unwrap(),
        Some(UserEntityAuthority::Quarantined)
    );
    assert!(ensure_entity_mutable(&store, binding.entity_id())
        .await
        .is_err());
    assert!(!record_visible(&store, "private-user-record", &raw)
        .await
        .unwrap());
    assert!(!record_visible(&store, "private-user-record", &verified)
        .await
        .unwrap());
    let destination = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    assert_ne!(destination.entity_id(), binding.entity_id());
    assert!(!record_visible(
        &store,
        "private-user-record",
        &AccessContext::for_authenticated_user(&destination),
    )
    .await
    .unwrap());

    let generic = historical_person(&store, UserId::new(43)).await;
    assert!(
        merge_entities(&store, &generic.entity_id, binding.entity_id())
            .await
            .is_err()
    );
    assert!(
        merge_entities(&store, binding.entity_id(), &generic.entity_id)
            .await
            .is_err()
    );
    // Model a corrupt imported merge pointer; access must classify the original rule target
    // before following the pointer to an otherwise grantable generic entity.
    let mut merged = binding.entity().clone();
    merged.resolution = ResolutionState::Merged;
    merged.superseded_by_entity_id = Some(generic.entity_id.clone());
    persist_entity_snapshot(&store, &merged).await;
    let raw_survivor =
        AccessContext::empty().with_audience([generic.entity_id.as_str(), binding.entity_id()]);
    assert!(
        !record_visible(&store, "private-user-record", &raw_survivor)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn quarantine_failure_rolls_back_marker_and_entity_changes() {
    let store = new_test_store().await;
    let user_id = UserId::new(42);
    let binding = resolve_authenticated_user_entity(&store, user_id)
        .await
        .unwrap();
    // Make the last quarantine update fail, after authority and entity updates execute.
    sqlx::query!("DROP TABLE entity_handles")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(quarantine_imported_authenticated_users(&store)
        .await
        .is_err());
    assert_eq!(
        user_entity_authority(&store, binding.entity_id())
            .await
            .unwrap(),
        Some(UserEntityAuthority::Active(user_id))
    );
    assert_eq!(
        serde_json::to_value(
            get_entity(&store, binding.entity_id())
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(binding.entity()).unwrap()
    );
}
