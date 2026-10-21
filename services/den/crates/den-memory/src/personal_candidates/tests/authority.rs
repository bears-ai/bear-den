use super::*;

#[tokio::test]
async fn other_people_get_none_and_cannot_withdraw_even_with_the_exact_raw_id() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = person(&store, BOB).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "Alice private name").await;
    let before = snapshot(&store).await;
    assert!(get_candidate(&store, &bob, candidate.id())
        .await
        .unwrap()
        .is_none());
    assert!(list_candidates(&store, &bob, 100).await.unwrap().is_empty());
    assert!(!withdraw_candidate(&store, &bob, candidate.id())
        .await
        .unwrap());
    assert!(!capture_enabled(&store, &bob).await.unwrap());
    assert_eq!(snapshot(&store).await, before);
    set_capture_enabled(&store, &bob, false).await.unwrap();
    assert!(capture_enabled(&store, &alice).await.unwrap());
    assert_eq!(state(&store, candidate.id()).await, CandidateState::Pending);
}

#[tokio::test]
async fn bear_and_exact_person_binding_are_required_not_just_numeric_user_or_portable_labels() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "Alice").await;
    let other_database = new_test_store().await;
    let other_bear = opted_in(&other_database, ALICE).await;
    let same_bear_other_database =
        BearMemoryStore::new(store.bear_id(), other_database.pool().clone());
    let impostor = opted_in(&same_bear_other_database, ALICE).await;
    assert_ne!(impostor.entity_id(), alice.entity_id());
    assert_eq!(impostor.user_id(), alice.user_id());
    assert_eq!(impostor.bear_id(), alice.bear_id());
    assert_eq!(
        impostor.entity().canonical_ref,
        alice.entity().canonical_ref
    );
    let before = snapshot(&store).await;
    assert_all_denied(&store, &other_bear, source, candidate.id()).await;
    assert_all_denied(&store, &impostor, source, candidate.id()).await;
    let wrong_store = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    assert_all_denied(&wrong_store, &alice, source, candidate.id()).await;
    assert_eq!(snapshot(&store).await, before);
}

#[derive(Clone, Copy, Debug)]
enum Corruption {
    EntityType,
    Resolution,
    Trust,
    CanonicalRef,
    Superseded,
    MissingEntity,
    MissingGeneration,
    DifferentGeneration,
    Quarantined,
    DifferentUser,
    MissingBinding,
}

async fn corrupt(store: &BearMemoryStore, alice: &AuthenticatedUserEntity, corruption: Corruption) {
    let bear_id = store.bear_id().to_string();
    let entity_id = alice.entity_id();
    match corruption {
        Corruption::EntityType
        | Corruption::Resolution
        | Corruption::Trust
        | Corruption::CanonicalRef
        | Corruption::Superseded => {
            let kind = if matches!(corruption, Corruption::EntityType) {
                "organization"
            } else {
                "person"
            };
            let resolution = if matches!(corruption, Corruption::Resolution) {
                "provisional"
            } else {
                "confirmed"
            };
            let trust = if matches!(corruption, Corruption::Trust) {
                "inferred"
            } else {
                "asserted"
            };
            let canonical = if matches!(corruption, Corruption::CanonicalRef) {
                "den:user:22".to_owned()
            } else {
                format!("den:user:{}", alice.user_id())
            };
            let superseder =
                matches!(corruption, Corruption::Superseded).then(|| Uuid::new_v4().to_string());
            sqlx::query!(
                "UPDATE entities SET type = ?, resolution = ?, trust = ?, canonical_ref = ?, superseded_by_entity_id = ? WHERE bear_id = ? AND entity_id = ?",
                kind, resolution, trust, canonical, superseder, bear_id, entity_id,
            ).execute(store.pool()).await.unwrap();
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
        Corruption::DifferentGeneration => {
            let generation = alice.authority_generation + 1;
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
        Corruption::Quarantined | Corruption::DifferentUser => {
            let (state, user) = match corruption {
                Corruption::Quarantined => ("quarantined", alice.user_id()),
                Corruption::DifferentUser => ("active", BOB),
                _ => unreachable!(),
            };
            let user_id = user.get();
            sqlx::query!(
                "UPDATE authenticated_user_entities SET state = ?, user_id = ? WHERE bear_id = ? AND entity_id = ?",
                state,
                user_id,
                bear_id,
                entity_id,
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        Corruption::MissingBinding => {
            sqlx::query!(
                "DELETE FROM authenticated_user_entities WHERE bear_id = ? AND entity_id = ?",
                bear_id,
                entity_id
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
    }
}

#[tokio::test]
async fn every_api_revalidates_malformed_existing_bindings_without_repairing_them() {
    for corruption in [
        Corruption::EntityType,
        Corruption::Resolution,
        Corruption::Trust,
        Corruption::CanonicalRef,
        Corruption::Superseded,
        Corruption::MissingEntity,
        Corruption::MissingGeneration,
        Corruption::DifferentGeneration,
        Corruption::Quarantined,
        Corruption::DifferentUser,
        Corruption::MissingBinding,
    ] {
        let store = new_test_store().await;
        let alice = opted_in(&store, ALICE).await;
        let source = note(&store).await;
        let candidate = stage(&store, &alice, source, "Alice").await;
        corrupt(&store, &alice, corruption).await;
        let before = snapshot(&store).await;
        assert_all_denied(&store, &alice, source, candidate.id()).await;
        assert_eq!(
            snapshot(&store).await,
            before,
            "{corruption:?} must not be repaired by candidate APIs"
        );
    }
}

#[tokio::test]
async fn nonpositive_users_and_stale_snapshot_generations_cannot_grant_authority() {
    let store = new_test_store().await;
    let source = note(&store).await;
    let missing_id = PersonalCandidateId(Uuid::new_v4());
    for user in [UserId::new(0), UserId::new(-1)] {
        let binding = person(&store, user).await;
        let before = snapshot(&store).await;
        assert_all_denied(&store, &binding, source, missing_id).await;
        assert_eq!(snapshot(&store).await, before);
    }
    let alice = opted_in(&store, ALICE).await;
    for generation in [0, -1, alice.authority_generation + 1] {
        let mut stale = alice.clone();
        stale.authority_generation = generation;
        assert_all_denied(&store, &stale, source, missing_id).await;
    }
}

#[tokio::test]
async fn person_filter_precedes_body_decoding_and_revoked_read_authority_precedes_body_queries() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = person(&store, BOB).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "Alice").await;
    let id = candidate.id().to_string();
    sqlx::query!(
        "UPDATE personal_memory_candidates SET value_text = '   ' WHERE candidate_id = ?",
        id
    )
    .execute(store.pool())
    .await
    .unwrap();
    assert!(matches!(
        get_candidate(&store, &alice, candidate.id()).await,
        Err(DenError::ValidationError(_))
    ));
    assert!(get_candidate(&store, &bob, candidate.id())
        .await
        .unwrap()
        .is_none());
    assert!(list_candidates(&store, &bob, 100).await.unwrap().is_empty());
    assert!(!withdraw_candidate(&store, &bob, candidate.id())
        .await
        .unwrap());
    revoke(&store, &alice).await;
    // If get/list attempted their bodies query first, this yields Database, not Authorization.
    sqlx::query!("DROP TABLE personal_memory_candidates")
        .execute(store.pool())
        .await
        .unwrap();
    assert_denied(get_candidate(&store, &alice, candidate.id()).await);
    assert_denied(list_candidates(&store, &alice, 100).await);
}

#[tokio::test]
async fn revoke_reconnect_aba_invalidates_all_old_apis_and_old_consent_for_the_same_tuple() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let old = stage(&store, &alice, source, "Alice").await;
    revoke(&store, &alice).await;
    assert_all_denied(&store, &alice, source, old.id()).await;
    let fresh = reconnect(
        &store,
        alice.entity_id(),
        ALICE,
        ReplacementConsent::NoReplacement,
    )
    .await;
    assert_eq!(fresh.bear_id(), alice.bear_id());
    assert_eq!(fresh.user_id(), alice.user_id());
    assert_eq!(fresh.entity_id(), alice.entity_id());
    assert!(fresh.authority_generation > alice.authority_generation);
    let before = snapshot(&store).await;
    assert_all_denied(&store, &alice, source, old.id()).await;
    assert_eq!(snapshot(&store).await, before);
    assert!(!capture_enabled(&store, &fresh).await.unwrap());
    assert_eq!(
        current_candidate(&store, &fresh, old.id()).await.state(),
        CandidateState::Quarantined
    );
    let new_source = note(&store).await;
    assert_denied(
        stage_preferred_name(
            &store,
            &new_source.proof(&fresh),
            PreferredName::new("Alice").unwrap(),
        )
        .await,
    );
    set_capture_enabled(&store, &fresh, true).await.unwrap();
    let new = stage(&store, &fresh, new_source, "Alice").await;
    assert_ne!(new.id(), old.id());
    assert_eq!(new.source_binding_generation(), fresh.authority_generation);
    assert_eq!(state(&store, old.id()).await, CandidateState::Quarantined);
    assert_all_denied(&store, &alice, new_source, new.id()).await;
}
