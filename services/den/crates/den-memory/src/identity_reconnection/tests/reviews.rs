use std::sync::Arc;

use sqlx::sqlite::SqlitePoolOptions;
use tokio::sync::Barrier;

use super::*;

#[tokio::test]
async fn selected_generation_prevents_stale_review_after_an_aba_authority_change() {
    let store = new_test_store().await;
    let entity_id = parked_person(&store, ALICE).await;
    let stale = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
        .await
        .unwrap();
    reconnect(&store, &entity_id, ALICE).await;
    let revoke = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    commit_review(&store, &revoke, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let fresh = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
        .await
        .unwrap();
    assert_eq!(stale.selected.entity_id, fresh.selected.entity_id);
    assert_eq!(stale.selected.user_id, fresh.selected.user_id);
    assert_eq!(stale.selected.state, fresh.selected.state);
    assert!(fresh.selected.generation > stale.selected.generation);
    let before = authority_snapshot(&store).await;
    assert_validation(commit_review(&store, &stale, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    commit_review(&store, &fresh, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
}

#[tokio::test]
async fn newly_created_destination_identity_invalidates_review_even_with_replacement_consent() {
    let store = new_test_store().await;
    let entity_id = parked_person(&store, ALICE).await;
    let review = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
        .await
        .unwrap();
    assert!(review.displaced_entity_id().is_none());
    let destination = person(&store, DESTINATION).await;
    let before = authority_snapshot(&store).await;
    for consent in [
        ReplacementConsent::NoReplacement,
        ReplacementConsent::PreserveDisplacedIdentity,
    ] {
        assert_validation(commit_review(&store, &review, consent).await);
        assert_eq!(authority_snapshot(&store).await, before);
    }
    assert_eq!(person(&store, DESTINATION).await, destination);
}

#[tokio::test]
async fn displaced_generation_invalidates_review_even_if_destination_tuple_is_identical() {
    let store = new_test_store().await;
    let portable = parked_person(&store, ALICE).await;
    let displaced = person(&store, DESTINATION).await;
    let stale = review_reconnection(&store, ACTOR, &portable, DESTINATION)
        .await
        .unwrap();
    let revoke = review_revocation(&store, ACTOR, &displaced).await.unwrap();
    commit_review(&store, &revoke, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    reconnect(&store, &displaced, DESTINATION).await;
    let before = authority_snapshot(&store).await;
    assert_validation(
        commit_review(
            &store,
            &stale,
            ReplacementConsent::PreserveDisplacedIdentity,
        )
        .await,
    );
    assert_eq!(authority_snapshot(&store).await, before);
    assert_eq!(person(&store, DESTINATION).await, displaced);
}

#[tokio::test]
async fn reconnect_and_revocation_reviews_are_single_use_including_after_same_tuple_restore() {
    let store = new_test_store().await;
    let entity_id = parked_person(&store, ALICE).await;
    let connect = review_reconnection(&store, ACTOR, &entity_id, ALICE)
        .await
        .unwrap();
    commit_review(&store, &connect, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let before = authority_snapshot(&store).await;
    assert_validation(commit_review(&store, &connect, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    let revoke = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    commit_review(&store, &revoke, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let before = authority_snapshot(&store).await;
    assert_validation(commit_review(&store, &revoke, ReplacementConsent::NoReplacement).await);
    assert_validation(commit_review(&store, &connect, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    reconnect(&store, &entity_id, ALICE).await;
    let before = authority_snapshot(&store).await;
    assert_validation(commit_review(&store, &revoke, ReplacementConsent::NoReplacement).await);
    assert_validation(commit_review(&store, &connect, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
}

#[tokio::test]
async fn a_consumed_receipt_id_cannot_be_reused_with_a_fresh_authority_snapshot() {
    let store = new_test_store().await;
    let entity_id = parked_person(&store, ALICE).await;
    let first = review_reconnection(&store, ACTOR, &entity_id, ALICE)
        .await
        .unwrap();
    commit_review(&store, &first, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let revoke = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    commit_review(&store, &revoke, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let fresh = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
        .await
        .unwrap();
    let mut reused_id = fresh.clone();
    reused_id.id = first.id();
    let before = authority_snapshot(&store).await;
    assert_validation(commit_review(&store, &reused_id, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    commit_review(&store, &fresh, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
}

#[tokio::test]
async fn expired_and_implausibly_future_reviews_are_rejected_without_writes() {
    let store = new_test_store().await;
    let entity_id = parked_person(&store, ALICE).await;
    let review = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
        .await
        .unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(review.expires_at_unix() > now);
    assert!(review.expires_at_unix() <= now + 15 * 60);
    let before = authority_snapshot(&store).await;
    for expires_at in [now - 1, now, now + 16 * 60, i64::MAX] {
        let mut expired = review.clone();
        // Simulate elapsed time/corrupt trusted session storage, never browser authority.
        expired.expires_at = expires_at;
        assert_validation(commit_review(&store, &expired, ReplacementConsent::NoReplacement).await);
        assert_eq!(authority_snapshot(&store).await, before);
    }
    // Server-side session serialization preserves the snapshot, not just its labels.
    let restored: IdentityReconnectionReview =
        serde_json::from_value(serde_json::to_value(&review).unwrap()).unwrap();
    let receipt = commit_review(&store, &restored, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_receipt(&store, &restored, &receipt).await;
}

#[tokio::test]
async fn reviews_and_verified_audiences_do_not_cross_bear_boundaries() {
    let store = new_test_store().await;
    let other = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let entity_id = person(&store, ALICE).await;
    let data = seed_context(&store, &entity_id, "cross-bear").await;
    let other_entity = person(&other, ALICE).await;
    let other_data = seed_context(&other, &other_entity, "other-bear").await;
    let other_context = context(&other, ALICE).await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let review = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
        .await
        .unwrap();
    let before = authority_snapshot(&store).await;
    assert_validation(review_reconnection(&other, ACTOR, &entity_id, DESTINATION).await);
    assert_validation(review_revocation(&other, ACTOR, &entity_id).await);
    assert_validation(commit_review(&other, &review, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    commit_review(&store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_eq!(person(&other, ALICE).await, other_entity);
    assert_visible(&other, &other_data.active_memory_id, &other_context, true).await;
    assert_visible(
        &store,
        &data.active_memory_id,
        &other_context.with_audience([entity_id.clone()]),
        false,
    )
    .await;
    assert_visible(
        &other,
        &other_data.active_memory_id,
        &context(&store, DESTINATION)
            .await
            .with_audience([other_entity]),
        false,
    )
    .await;
    assert_eq!(list_identities(&store).await.unwrap().len(), 1);
    assert_eq!(list_identities(&other).await.unwrap().len(), 1);
}

#[tokio::test]
async fn lifecycle_and_principal_preconditions_fail_closed() {
    let store = new_test_store().await;
    let entity_id = person(&store, ALICE).await;
    assert_validation(review_reconnection(&store, ACTOR, &entity_id, DESTINATION).await);
    for invalid in [UserId::new(0), UserId::new(-1)] {
        assert_validation(review_revocation(&store, invalid, &entity_id).await);
    }
    let revocation = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    commit_review(&store, &revocation, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let before = authority_snapshot(&store).await;
    assert_validation(review_revocation(&store, ACTOR, &entity_id).await);
    assert_validation(review_reconnection(&store, ACTOR, "missing", DESTINATION).await);
    for invalid in [UserId::new(0), UserId::new(-1)] {
        assert_validation(review_reconnection(&store, invalid, &entity_id, DESTINATION).await);
        assert_validation(review_reconnection(&store, ACTOR, &entity_id, invalid).await);
    }
    let generic = create_entity(
        &store,
        "person",
        Some("not authenticated"),
        ResolutionState::Confirmed,
        EntityTrust::Asserted,
        Some("den:user:101"),
        &json!({}),
    )
    .await
    .unwrap();
    let after_generic = authority_snapshot(&store).await;
    assert_validation(review_reconnection(&store, ACTOR, &generic.entity_id, DESTINATION).await);
    assert_eq!(authority_snapshot(&store).await, after_generic);
    // The only deliberate change since the precondition checks was creating the generic row.
    assert_ne!(after_generic, before);
}

#[derive(Clone, Copy, Debug)]
enum ClaimRace {
    Destination,
    Selected,
    Replay,
}

async fn race_claims(kind: ClaimRace) {
    for independent_pools in [false, true] {
        let store = new_test_store().await;
        let alice = person(&store, ALICE).await;
        let bob = person(&store, BOB).await;
        let alice_data = seed_context(&store, &alice, "racing-alice").await;
        let bob_data = seed_context(&store, &bob, "racing-bob").await;
        quarantine_imported_authenticated_users(&store)
            .await
            .unwrap();
        let before_context = context_snapshot(&store).await;
        let first = review_reconnection(&store, ACTOR, &alice, DESTINATION)
            .await
            .unwrap();
        let second = match kind {
            ClaimRace::Destination => review_reconnection(&store, ACTOR, &bob, DESTINATION)
                .await
                .unwrap(),
            ClaimRace::Selected => review_reconnection(&store, ACTOR, &alice, UserId::new(202))
                .await
                .unwrap(),
            ClaimRace::Replay => first.clone(),
        };
        let other_pool = if independent_pools {
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(store.pool().connect_options().as_ref().clone())
                .await
                .unwrap()
        } else {
            store.pool().clone()
        };
        let other = BearMemoryStore::new(store.bear_id(), other_pool);
        let barrier = Arc::new(Barrier::new(2));
        let (left, right) = tokio::join!(
            async {
                barrier.wait().await;
                commit_review(&store, &first, ReplacementConsent::NoReplacement).await
            },
            async {
                barrier.wait().await;
                commit_review(&other, &second, ReplacementConsent::NoReplacement).await
            },
        );
        let (winner, winning_review) = match (left, right) {
            (Ok(receipt), Err(error)) => { assert_validation::<()>(Err(error)); (receipt, &first) },
            (Err(error), Ok(receipt)) => { assert_validation::<()>(Err(error)); (receipt, &second) },
            result => panic!("exactly one claim should win ({kind:?}, independent={independent_pools}): {result:?}"),
        };
        assert_receipt(&store, winning_review, &winner).await;
        assert_eq!(receipt_count(&store).await, 1);
        let identities = list_identities(&store).await.unwrap();
        assert_eq!(identities.len(), 2);
        assert_eq!(
            identities
                .iter()
                .filter(|identity| identity.active_user_id.is_some())
                .count(),
            1
        );
        let destination = winner.destination_user_id.unwrap();
        assert_eq!(person(&store, destination).await, winner.entity_id);
        let access = context(&store, destination).await;
        assert_visible(
            &store,
            &alice_data.active_memory_id,
            &access,
            winner.entity_id == alice,
        )
        .await;
        assert_visible(
            &store,
            &bob_data.active_memory_id,
            &access,
            winner.entity_id == bob,
        )
        .await;
        assert_eq!(context_snapshot(&store).await, before_context);
        let after = authority_snapshot(&store).await;
        assert_validation(commit_review(&store, &first, ReplacementConsent::NoReplacement).await);
        assert_validation(commit_review(&other, &second, ReplacementConsent::NoReplacement).await);
        assert_eq!(authority_snapshot(&store).await, after);
    }
}

#[tokio::test]
async fn competing_people_cannot_claim_one_destination_across_independent_sqlite_pools() {
    race_claims(ClaimRace::Destination).await;
}

#[tokio::test]
async fn competing_destinations_cannot_claim_one_person_across_independent_sqlite_pools() {
    race_claims(ClaimRace::Selected).await;
}

#[tokio::test]
async fn simultaneous_replay_commits_once_across_independent_sqlite_pools() {
    race_claims(ClaimRace::Replay).await;
}

#[derive(Clone, Copy, Debug)]
enum Corruption {
    WrongType,
    Rejected,
    Merged,
    Superseded,
    RawSuperseded,
    UnknownResolution,
    UnknownTrust,
    MissingEntity,
    MissingAuthority,
    MissingGeneration,
    ExhaustedGeneration,
    WrongCanonicalRef,
    WrongTrust,
    ProvisionalActive,
}

async fn corrupt(store: &BearMemoryStore, entity_id: &str, corruption: Corruption) {
    let bear_id = store.bear_id().to_string();
    match corruption {
        Corruption::RawSuperseded | Corruption::UnknownResolution => {
            let resolution = match corruption {
                Corruption::RawSuperseded => "superseded",
                Corruption::UnknownResolution => "unknown-resolution",
                _ => unreachable!(),
            };
            // Preserve the actual SQLite spelling: EntityRow maps superseded to Merged
            // and may fall back when parsing unknown values.
            sqlx::query!(
                "UPDATE entities SET resolution = ? WHERE bear_id = ? AND entity_id = ?",
                resolution,
                bear_id,
                entity_id,
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        Corruption::UnknownTrust => {
            sqlx::query!(
                "UPDATE entities SET trust = 'unknown-trust' WHERE bear_id = ? AND entity_id = ?",
                bear_id,
                entity_id,
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        Corruption::MissingEntity => {
            sqlx::query!(
                "DELETE FROM entities WHERE bear_id = ? AND entity_id = ?",
                bear_id,
                entity_id
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        Corruption::MissingAuthority => {
            sqlx::query!(
                "DELETE FROM authenticated_user_entities WHERE bear_id = ? AND entity_id = ?",
                bear_id,
                entity_id
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        Corruption::MissingGeneration => {
            sqlx::query!(
                "DELETE FROM user_identity_generations WHERE bear_id = ? AND entity_id = ?",
                bear_id,
                entity_id
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        Corruption::ExhaustedGeneration => {
            sqlx::query!("UPDATE user_identity_generations SET generation = 9223372036854775807 WHERE bear_id = ? AND entity_id = ?", bear_id, entity_id)
                .execute(store.pool()).await.unwrap();
        }
        other => {
            let mut entity = get_entity(store, entity_id).await.unwrap().unwrap();
            match other {
                Corruption::WrongType => entity.entity_type = "work_surface".into(),
                Corruption::Rejected => entity.resolution = ResolutionState::Rejected,
                Corruption::Merged => entity.resolution = ResolutionState::Merged,
                Corruption::Superseded => {
                    entity.superseded_by_entity_id = Some("another-person".into());
                }
                Corruption::WrongCanonicalRef => entity.canonical_ref = Some("den:user:999".into()),
                Corruption::WrongTrust => entity.trust = EntityTrust::Inferred,
                Corruption::ProvisionalActive => entity.resolution = ResolutionState::Provisional,
                _ => unreachable!(),
            }
            let resolution = entity.resolution.as_str();
            let trust = entity.trust.as_str();
            sqlx::query!(
                "UPDATE entities SET type = ?, resolution = ?, trust = ?, canonical_ref = ?, superseded_by_entity_id = ? WHERE bear_id = ? AND entity_id = ?",
                entity.entity_type, resolution, trust, entity.canonical_ref, entity.superseded_by_entity_id, bear_id, entity_id,
            ).execute(store.pool()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn import_quarantine_preserves_terminal_and_unknown_resolution_spellings() {
    for (resolution, live) in [
        ("observed", true),
        ("provisional", true),
        ("resolved", true),
        ("confirmed", true),
        ("rejected", false),
        ("merged", false),
        ("superseded", false),
        ("unknown-resolution", false),
    ] {
        let store = new_test_store().await;
        let binding = resolve_authenticated_user_entity(&store, ALICE)
            .await
            .unwrap();
        let entity_id = binding.entity_id();
        let data = seed_context(&store, entity_id, "raw-import-state").await;
        let old_context = AccessContext::for_authenticated_user(&binding);
        let old_review = review_revocation(&store, ACTOR, entity_id).await.unwrap();
        let handles = historical_handles(&store, entity_id).await;
        let context_before = context_snapshot(&store).await;
        let bear_id = store.bear_id().to_string();
        sqlx::query!(
            "UPDATE entities SET resolution = ? WHERE bear_id = ? AND entity_id = ?",
            resolution,
            bear_id,
            entity_id,
        )
        .execute(store.pool())
        .await
        .unwrap();
        for expected_generation in [2, 3] {
            assert_eq!(
                quarantine_imported_authenticated_users(&store)
                    .await
                    .unwrap(),
                vec![entity_id.to_owned()]
            );
            let raw = sqlx::query!(
                "SELECT resolution, trust, canonical_ref FROM entities WHERE bear_id = ? AND entity_id = ?",
                bear_id, entity_id,
            ).fetch_one(store.pool()).await.unwrap();
            assert_eq!(raw.resolution, if live { "provisional" } else { resolution },
                "quarantine must not normalize terminal or unknown states into reconnectable people");
            assert_eq!(raw.trust, "inferred");
            assert!(raw.canonical_ref.is_none());
            let identity = summary(&store, entity_id).await;
            assert_eq!(identity.active_user_id, None);
            assert_eq!(identity.binding_version, expected_generation);
            assert_historical_handles_detached(&store, entity_id, &handles).await;
            assert_visible(&store, &data.active_memory_id, &old_context, false).await;
            assert_visible(
                &store,
                &data.active_memory_id,
                &AccessContext::empty().with_audience([entity_id]),
                false,
            )
            .await;
            let before_review = authority_snapshot(&store).await;
            let reviewed = review_reconnection(&store, ACTOR, entity_id, DESTINATION).await;
            if live {
                assert!(
                    reviewed.is_ok(),
                    "known live state {resolution} should remain reconnectable: {reviewed:?}"
                );
            } else {
                assert_validation(reviewed);
            }
            assert_validation(
                commit_review(&store, &old_review, ReplacementConsent::NoReplacement).await,
            );
            assert_eq!(authority_snapshot(&store).await, before_review);
            assert_eq!(context_snapshot(&store).await, context_before);
        }
    }
}

#[tokio::test]
async fn raw_terminal_or_unknown_values_after_quarantine_fail_core_review_and_commit() {
    for corruption in [
        Corruption::RawSuperseded,
        Corruption::UnknownResolution,
        Corruption::UnknownTrust,
    ] {
        let store = new_test_store().await;
        let entity_id = person(&store, ALICE).await;
        let data = seed_context(&store, &entity_id, "raw-quarantined-state").await;
        quarantine_imported_authenticated_users(&store)
            .await
            .unwrap();
        let review = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
            .await
            .unwrap();
        // Quarantine deliberately lowers trust to inferred. Corrupt the stored value
        // afterward so the core review must parse it, not rely on import sanitization.
        corrupt(&store, &entity_id, corruption).await;
        let before = authority_snapshot(&store).await;
        let context_before = context_snapshot(&store).await;
        assert_validation(review_reconnection(&store, ACTOR, &entity_id, DESTINATION).await);
        assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
        assert_visible(
            &store,
            &data.active_memory_id,
            &AccessContext::empty().with_audience([entity_id]),
            false,
        )
        .await;
        assert_eq!(
            authority_snapshot(&store).await,
            before,
            "{corruption:?} must not be repaired or adopted"
        );
        assert_eq!(context_snapshot(&store).await, context_before);
    }
}

#[tokio::test]
async fn malformed_and_terminal_parked_identities_cannot_be_reviewed_or_committed() {
    for corruption in [
        Corruption::WrongType,
        Corruption::Rejected,
        Corruption::Merged,
        Corruption::Superseded,
        Corruption::RawSuperseded,
        Corruption::UnknownResolution,
        Corruption::UnknownTrust,
        Corruption::MissingEntity,
        Corruption::MissingAuthority,
        Corruption::MissingGeneration,
        Corruption::ExhaustedGeneration,
    ] {
        let store = new_test_store().await;
        let entity_id = parked_person(&store, ALICE).await;
        let review = review_reconnection(&store, ACTOR, &entity_id, DESTINATION)
            .await
            .unwrap();
        corrupt(&store, &entity_id, corruption).await;
        let before = authority_snapshot(&store).await;
        assert_validation(review_reconnection(&store, ACTOR, &entity_id, DESTINATION).await);
        assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
        assert_eq!(
            authority_snapshot(&store).await,
            before,
            "{corruption:?} must not be repaired or adopted"
        );
    }
}

#[tokio::test]
async fn malformed_active_identities_cannot_be_revoked_or_displaced_by_restore() {
    for corruption in [
        Corruption::WrongType,
        Corruption::Rejected,
        Corruption::Merged,
        Corruption::Superseded,
        Corruption::RawSuperseded,
        Corruption::UnknownResolution,
        Corruption::UnknownTrust,
        Corruption::MissingEntity,
        Corruption::MissingGeneration,
        Corruption::ExhaustedGeneration,
        Corruption::WrongCanonicalRef,
        Corruption::WrongTrust,
        Corruption::ProvisionalActive,
    ] {
        let store = new_test_store().await;
        let portable = parked_person(&store, ALICE).await;
        let active = person(&store, DESTINATION).await;
        let revocation = review_revocation(&store, ACTOR, &active).await.unwrap();
        let replacement = review_reconnection(&store, ACTOR, &portable, DESTINATION)
            .await
            .unwrap();
        corrupt(&store, &active, corruption).await;
        let before = authority_snapshot(&store).await;
        assert_validation(review_revocation(&store, ACTOR, &active).await);
        assert_validation(review_reconnection(&store, ACTOR, &portable, DESTINATION).await);
        assert_validation(
            commit_review(&store, &revocation, ReplacementConsent::NoReplacement).await,
        );
        assert_validation(
            commit_review(
                &store,
                &replacement,
                ReplacementConsent::PreserveDisplacedIdentity,
            )
            .await,
        );
        assert_eq!(
            authority_snapshot(&store).await,
            before,
            "{corruption:?} must fail closed"
        );
    }
}
