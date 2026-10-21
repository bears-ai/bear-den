use super::*;
use crate::authenticated_user::{
    quarantine_imported_authenticated_users, resolve_authenticated_user_entity,
};
use crate::descriptors::{EntityTrust, ResolutionState};
use crate::entity::{merge_entities, set_resolution};
use crate::relations::{append_relation, RelationRow};
use crate::resolver::{resolve, Assertion, Resolution, Signal};
use crate::test_support::new_test_store;
use serde_json::json;

async fn confirmed_person(store: &BearMemoryStore, name: &str, email: &str) -> String {
    match resolve(
        store,
        "person",
        Some(name),
        &[Signal::new("email", email)],
        Assertion::Asserted,
    )
    .await
    .unwrap()
    {
        Resolution::Resolved(e) => e.entity_id,
        other => panic!("expected Resolved, got {other:?}"),
    }
}

async fn confirmed_surface(store: &BearMemoryStore, remote: &str) -> String {
    match resolve(
        store,
        "work_surface",
        Some("surface"),
        &[Signal::new("git_remote", remote)],
        Assertion::Asserted,
    )
    .await
    .unwrap()
    {
        Resolution::Resolved(e) => e.entity_id,
        other => panic!("expected Resolved, got {other:?}"),
    }
}

async fn rule(
    store: &BearMemoryStore,
    memory_id: &str,
    entity_id: &str,
    relation: &str,
) -> RelationRow {
    append_relation(
        store,
        memory_id,
        entity_id,
        relation,
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn record_without_access_rules_is_always_visible() {
    let store = new_test_store().await;
    assert!(record_visible(&store, "mem-open", &AccessContext::empty())
        .await
        .unwrap());

    let other_store = new_test_store().await;
    let binding = resolve_authenticated_user_entity(&other_store, UserId::new(1))
        .await
        .unwrap();
    assert!(record_visible(
        &store,
        "mem-open",
        &AccessContext::for_authenticated_user(&binding)
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn audience_gate_hides_unless_context_includes_addressee() {
    let store = new_test_store().await;
    let alice = confirmed_person(&store, "Alice", "alice@acme.com").await;
    let bob = confirmed_person(&store, "Bob", "bob@acme.com").await;
    rule(&store, "mem-a", &alice, "audience").await;

    assert!(!record_visible(&store, "mem-a", &AccessContext::empty())
        .await
        .unwrap());
    assert!(record_visible(
        &store,
        "mem-a",
        &AccessContext::empty().with_audience([alice])
    )
    .await
    .unwrap());
    assert!(!record_visible(
        &store,
        "mem-a",
        &AccessContext::empty().with_audience([bob])
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn confined_to_prevents_cross_surface_leak() {
    let store = new_test_store().await;
    let client_a = confirmed_surface(&store, "github.com/acme/client-a").await;
    let client_b = confirmed_surface(&store, "github.com/acme/client-b").await;
    rule(&store, "mem-c", &client_a, "confined_to").await;

    assert!(!record_visible(&store, "mem-c", &AccessContext::empty())
        .await
        .unwrap());
    assert!(record_visible(
        &store,
        "mem-c",
        &AccessContext::empty().with_confinement([client_a])
    )
    .await
    .unwrap());
    assert!(!record_visible(
        &store,
        "mem-c",
        &AccessContext::empty().with_confinement([client_b])
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn both_classes_must_pass() {
    let store = new_test_store().await;
    let alice = confirmed_person(&store, "Alice", "alice@acme.com").await;
    let surface = confirmed_surface(&store, "github.com/acme/app").await;
    rule(&store, "mem-d", &alice, "audience").await;
    rule(&store, "mem-d", &surface, "confined_to").await;

    assert!(!record_visible(
        &store,
        "mem-d",
        &AccessContext::empty().with_audience([alice.clone()])
    )
    .await
    .unwrap());
    assert!(record_visible(
        &store,
        "mem-d",
        &AccessContext::empty()
            .with_audience([alice])
            .with_confinement([surface])
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn audience_follows_merge_forward_pointer() {
    let store = new_test_store().await;
    let survivor = confirmed_person(&store, "Alice", "alice@acme.com").await;
    let loser = confirmed_person(&store, "Alice (dup)", "alice@personal.com").await;
    rule(&store, "mem-e", &loser, "audience").await;
    merge_entities(&store, &survivor, &loser).await.unwrap();

    assert!(record_visible(
        &store,
        "mem-e",
        &AccessContext::empty().with_audience([survivor])
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn bound_audience_requires_same_verified_principal_and_bear() {
    let store = new_test_store().await;
    let alice = resolve_authenticated_user_entity(&store, UserId::new(1))
        .await
        .unwrap();
    let bob = resolve_authenticated_user_entity(&store, UserId::new(2))
        .await
        .unwrap();
    rule(&store, "mem-private", alice.entity_id(), "audience").await;

    let verified = AccessContext::for_authenticated_user(&alice);
    assert!(!verified.is_empty());
    assert!(verified.audience.contains(alice.entity_id()));
    assert!(record_visible(&store, "mem-private", &verified)
        .await
        .unwrap());

    let same_bear = BearMemoryStore::new(store.bear_id(), store.pool().clone());
    let same_principal = resolve_authenticated_user_entity(&same_bear, UserId::new(1))
        .await
        .unwrap();
    assert!(record_visible(
        &store,
        "mem-private",
        &AccessContext::for_authenticated_user(&same_principal)
    )
    .await
    .unwrap());

    let mut raw = AccessContext::empty();
    raw.allow_audience(alice.entity_id());
    for ctx in [
        AccessContext::empty(),
        raw,
        AccessContext::empty().with_audience([alice.entity_id()]),
        AccessContext::for_authenticated_user(&bob),
        AccessContext::for_authenticated_user(&bob).with_audience([alice.entity_id()]),
    ] {
        assert!(!record_visible(&store, "mem-private", &ctx).await.unwrap());
    }
}

#[tokio::test]
async fn bound_audience_denies_cross_store_verified_contexts_even_with_raw_target() {
    let store = new_test_store().await;
    let alice = resolve_authenticated_user_entity(&store, UserId::new(1))
        .await
        .unwrap();
    rule(&store, "mem-private", alice.entity_id(), "audience").await;

    let other_store = new_test_store().await;
    let other_bear = resolve_authenticated_user_entity(&other_store, UserId::new(1))
        .await
        .unwrap();
    let wrong_bear =
        AccessContext::for_authenticated_user(&other_bear).with_audience([alice.entity_id()]);
    assert!(!record_visible(&store, "mem-private", &wrong_bear)
        .await
        .unwrap());

    // Even the same Bear/user ids from an independent store are not this entity binding.
    let independent_store = BearMemoryStore::new(store.bear_id(), other_store.pool().clone());
    let other_binding = resolve_authenticated_user_entity(&independent_store, UserId::new(1))
        .await
        .unwrap();
    assert_ne!(alice.entity_id(), other_binding.entity_id());
    let wrong_binding =
        AccessContext::for_authenticated_user(&other_binding).with_audience([alice.entity_id()]);
    assert!(!record_visible(&store, "mem-private", &wrong_binding)
        .await
        .unwrap());
}

#[tokio::test]
async fn wrong_bear_verified_context_denies_all_protected_records() {
    let store = new_test_store().await;
    let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
    let surface = confirmed_surface(&store, "github.com/acme/app").await;
    rule(&store, "mem-audience", &person, "audience").await;
    rule(&store, "mem-confined", &surface, "confined_to").await;
    let other_store = new_test_store().await;
    let other_user = resolve_authenticated_user_entity(&other_store, UserId::new(1))
        .await
        .unwrap();
    let ctx = AccessContext::for_authenticated_user(&other_user)
        .with_audience([person])
        .with_confinement([surface]);

    for memory_id in ["mem-audience", "mem-confined"] {
        assert!(!record_visible(&store, memory_id, &ctx).await.unwrap());
    }
}

#[tokio::test]
async fn audience_alternatives_do_not_bypass_verified_binding_or_confinement() {
    let store = new_test_store().await;
    let alice = resolve_authenticated_user_entity(&store, UserId::new(1))
        .await
        .unwrap();
    let bob = resolve_authenticated_user_entity(&store, UserId::new(2))
        .await
        .unwrap();
    let surface = confirmed_surface(&store, "github.com/acme/app").await;
    rule(&store, "mem-shared", alice.entity_id(), "audience").await;
    rule(&store, "mem-shared", bob.entity_id(), "audience").await;
    rule(&store, "mem-shared", &surface, "confined_to").await;

    for binding in [&alice, &bob] {
        assert!(!record_visible(
            &store,
            "mem-shared",
            &AccessContext::for_authenticated_user(binding)
        )
        .await
        .unwrap());
        assert!(record_visible(
            &store,
            "mem-shared",
            &AccessContext::for_authenticated_user(binding).with_confinement([surface.clone()])
        )
        .await
        .unwrap());
    }
    assert!(!record_visible(
        &store,
        "mem-shared",
        &AccessContext::empty()
            .with_audience([alice.entity_id(), bob.entity_id()])
            .with_confinement([surface])
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn quarantined_verified_context_cannot_use_raw_audience_or_confinement() {
    let store = new_test_store().await;
    let binding = resolve_authenticated_user_entity(&store, UserId::new(1))
        .await
        .unwrap();
    let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
    let surface = confirmed_surface(&store, "github.com/acme/app").await;
    rule(&store, "mem-private", binding.entity_id(), "audience").await;
    rule(&store, "mem-generic", &person, "audience").await;
    rule(&store, "mem-confined", &surface, "confined_to").await;
    let ctx = AccessContext::for_authenticated_user(&binding)
        .with_audience([binding.entity_id(), person.as_str()])
        .with_confinement([surface]);
    for memory_id in ["mem-private", "mem-generic", "mem-confined"] {
        assert!(record_visible(&store, memory_id, &ctx).await.unwrap());
    }

    let quarantined = quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert!(quarantined.iter().any(|id| id == binding.entity_id()));
    assert!(matches!(
        user_entity_authority(&store, binding.entity_id())
            .await
            .unwrap(),
        Some(UserEntityAuthority::Quarantined)
    ));
    for memory_id in ["mem-private", "mem-generic", "mem-confined"] {
        assert!(!record_visible(&store, memory_id, &ctx).await.unwrap());
    }
    // Reconstructing a context from the old immutable snapshot does not refresh authority.
    let reconstructed =
        AccessContext::for_authenticated_user(&binding).with_audience([person.clone()]);
    assert!(!record_visible(&store, "mem-generic", &reconstructed)
        .await
        .unwrap());
    assert!(record_visible(
        &store,
        "mem-generic",
        &AccessContext::empty().with_audience([person])
    )
    .await
    .unwrap());
    assert!(record_visible(&store, "mem-open", &ctx).await.unwrap());
}

#[tokio::test]
async fn verified_context_without_current_binding_cannot_use_raw_audience() {
    let source = new_test_store().await;
    let binding = resolve_authenticated_user_entity(&source, UserId::new(1))
        .await
        .unwrap();
    let independent_store = new_test_store().await;
    let store = BearMemoryStore::new(source.bear_id(), independent_store.pool().clone());
    let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
    rule(&store, "mem-generic", &person, "audience").await;
    assert!(user_entity_authority(&store, binding.entity_id())
        .await
        .unwrap()
        .is_none());

    let ctx = AccessContext::for_authenticated_user(&binding).with_audience([person.clone()]);
    assert!(!record_visible(&store, "mem-generic", &ctx).await.unwrap());
    assert!(record_visible(
        &store,
        "mem-generic",
        &AccessContext::empty().with_audience([person])
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn quarantined_rule_denies_raw_old_id_and_other_matching_audience() {
    let store = new_test_store().await;
    let binding = resolve_authenticated_user_entity(&store, UserId::new(1))
        .await
        .unwrap();
    let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
    rule(&store, "mem-old", binding.entity_id(), "audience").await;
    rule(&store, "mem-shared", binding.entity_id(), "audience").await;
    rule(&store, "mem-shared", &person, "audience").await;
    let raw_person = AccessContext::empty().with_audience([person.clone()]);
    assert!(record_visible(&store, "mem-shared", &raw_person)
        .await
        .unwrap());

    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    for ctx in [
        AccessContext::empty(),
        AccessContext::empty().with_audience([binding.entity_id()]),
        raw_person,
        AccessContext::empty().with_audience([binding.entity_id(), person.as_str()]),
    ] {
        for memory_id in ["mem-old", "mem-shared"] {
            assert!(!record_visible(&store, memory_id, &ctx).await.unwrap());
        }
    }
}

#[tokio::test]
async fn quarantined_identity_cannot_be_reconfirmed_or_merged_to_reopen_rules() {
    let store = new_test_store().await;
    let binding = resolve_authenticated_user_entity(&store, UserId::new(1))
        .await
        .unwrap();
    let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
    rule(&store, "mem-old", binding.entity_id(), "audience").await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();

    assert!(set_resolution(
        &store,
        binding.entity_id(),
        ResolutionState::Confirmed,
        Some(EntityTrust::Asserted),
    )
    .await
    .is_err());
    assert!(merge_entities(&store, &person, binding.entity_id())
        .await
        .is_err());
    assert!(merge_entities(&store, binding.entity_id(), &person)
        .await
        .is_err());
    let old_entity = get_entity(&store, binding.entity_id())
        .await
        .unwrap()
        .unwrap();
    assert!(!old_entity.resolution.can_bear_access());
    assert!(old_entity.superseded_by_entity_id.is_none());
    let ordinary_entity = get_entity(&store, &person).await.unwrap().unwrap();
    assert!(ordinary_entity.resolution.can_bear_access());
    assert!(ordinary_entity.superseded_by_entity_id.is_none());
    for ctx in [
        AccessContext::for_authenticated_user(&binding),
        AccessContext::empty().with_audience([binding.entity_id(), person.as_str()]),
    ] {
        assert!(!record_visible(&store, "mem-old", &ctx).await.unwrap());
    }
}

#[tokio::test]
async fn new_binding_for_same_account_cannot_access_quarantined_rules() {
    let store = new_test_store().await;
    let old = resolve_authenticated_user_entity(&store, UserId::new(1))
        .await
        .unwrap();
    rule(&store, "mem-old", old.entity_id(), "audience").await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let current = resolve_authenticated_user_entity(&store, old.user_id())
        .await
        .unwrap();
    assert_eq!(old.user_id(), current.user_id());
    assert_eq!(old.bear_id(), current.bear_id());
    assert_ne!(old.entity_id(), current.entity_id());
    rule(&store, "mem-current", current.entity_id(), "audience").await;
    let current_ctx =
        AccessContext::for_authenticated_user(&current).with_audience([old.entity_id()]);
    assert!(record_visible(&store, "mem-current", &current_ctx)
        .await
        .unwrap());
    assert!(!record_visible(&store, "mem-old", &current_ctx)
        .await
        .unwrap());

    let stale_ctx =
        AccessContext::for_authenticated_user(&old).with_audience([current.entity_id()]);
    for memory_id in ["mem-old", "mem-current"] {
        assert!(!record_visible(&store, memory_id, &stale_ctx).await.unwrap());
    }
}

#[tokio::test]
async fn rejected_provisional_and_observed_targets_deny_at_read_time() {
    for resolution in [
        ResolutionState::Rejected,
        ResolutionState::Provisional,
        ResolutionState::Observed,
        ResolutionState::Merged,
    ] {
        let store = new_test_store().await;
        let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
        let surface = confirmed_surface(&store, "github.com/acme/app").await;
        rule(&store, "mem-audience", &person, "audience").await;
        rule(&store, "mem-confined", &surface, "confined_to").await;
        let ctx = AccessContext::empty()
            .with_audience([person.clone()])
            .with_confinement([surface.clone()]);
        for target in [&person, &surface] {
            set_resolution(&store, target, resolution, None)
                .await
                .unwrap();
        }
        for memory_id in ["mem-audience", "mem-confined"] {
            assert!(!record_visible(&store, memory_id, &ctx).await.unwrap());
        }
    }
}

#[tokio::test]
async fn deleted_targets_deny_even_when_context_contains_raw_id() {
    let store = new_test_store().await;
    let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
    rule(&store, "mem-deleted", &person, "audience").await;
    let bear_id = store.bear_id().to_string();
    sqlx::query!(
        "DELETE FROM entities WHERE bear_id = ? AND entity_id = ?",
        bear_id,
        person,
    )
    .execute(store.pool())
    .await
    .unwrap();

    assert!(!record_visible(
        &store,
        "mem-deleted",
        &AccessContext::empty().with_audience([person])
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn unknown_or_descriptive_access_descriptor_denies_unconditionally() {
    let store = new_test_store().await;
    let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
    let access_rule = rule(&store, "mem-unknown", &person, "audience").await;
    // A valid audience rule must not mask the invalid rule either.
    rule(&store, "mem-unknown", &person, "audience").await;
    let ctx = AccessContext::empty()
        .with_audience([person.clone()])
        .with_confinement([person]);
    let bear_id = store.bear_id().to_string();
    for relation in ["den.memory.relation.future_policy", "subject"] {
        sqlx::query!(
            "UPDATE memory_access_rules SET relation = ? WHERE bear_id = ? AND link_id = ?",
            relation,
            bear_id,
            access_rule.link_id,
        )
        .execute(store.pool())
        .await
        .unwrap();
        assert!(!record_visible(&store, "mem-unknown", &ctx).await.unwrap());
    }
}

#[tokio::test]
async fn retracted_last_rule_stays_hidden_instead_of_becoming_public() {
    let store = new_test_store().await;
    let person = confirmed_person(&store, "Alice", "alice@acme.com").await;
    let access_rule = rule(&store, "mem-retracted", &person, "audience").await;
    let ctx = AccessContext::empty().with_audience([person]);
    assert!(record_visible(&store, "mem-retracted", &ctx).await.unwrap());

    let bear_id = store.bear_id().to_string();
    sqlx::query!(
        "UPDATE memory_access_rules SET state = 'retracted' WHERE bear_id = ? AND link_id = ?",
        bear_id,
        access_rule.link_id,
    )
    .execute(store.pool())
    .await
    .unwrap();

    assert!(list_access_rules_for_source(&store, "mem-retracted")
        .await
        .unwrap()
        .is_empty());
    for ctx in [AccessContext::empty(), ctx] {
        assert!(!record_visible(&store, "mem-retracted", &ctx).await.unwrap());
    }
    assert!(record_visible(&store, "mem-open", &AccessContext::empty())
        .await
        .unwrap());

    // History is Bear-scoped, even when stores share a SQLite pool.
    let other_bear = BearMemoryStore::new(uuid::Uuid::new_v4(), store.pool().clone());
    assert!(
        record_visible(&other_bear, "mem-retracted", &AccessContext::empty())
            .await
            .unwrap()
    );
}
