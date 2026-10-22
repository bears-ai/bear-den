use super::*;

async fn assert_version(
    store: &BearMemoryStore,
    owner: &AuthenticatedUserEntity,
    expected: PersonalFactVersion,
) {
    assert_eq!(
        current_preferred_name_version(store, owner).await.unwrap(),
        Some(expected)
    );
    let head = current_preferred_name(store, owner).await.unwrap().unwrap();
    assert_eq!(head.fact_id, expected.fact_id);
    assert_eq!(head.state, expected.state);
}

#[tokio::test]
async fn no_publication_has_no_version() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    assert_eq!(
        current_preferred_name_version(&store, &alice)
            .await
            .unwrap(),
        None
    );
    assert!(current_preferred_name(&store, &alice)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn active_version_matches_full_head() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "Alice", None).await;
    assert_version(
        &store,
        &alice,
        PersonalFactVersion {
            fact_id: fact.fact_id,
            state: PersonalFactState::Active,
        },
    )
    .await;
}

#[tokio::test]
async fn correction_replaces_version_and_rejects_prior_id() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "First", None).await;
    let prior = current_preferred_name_version(&store, &alice)
        .await
        .unwrap()
        .unwrap();
    let corrected = publish(&store, &alice, source, "Corrected", Some(prior.fact_id)).await;
    assert_eq!(prior.fact_id, first.fact_id);
    assert_ne!(corrected.fact_id, prior.fact_id);
    let expected = PersonalFactVersion {
        fact_id: corrected.fact_id,
        state: PersonalFactState::Active,
    };
    assert_version(&store, &alice, expected).await;
    let pending = stage(&store, &alice, source, "Stale correction").await;
    denied(
        publish_preferred_name(
            &store,
            &source.proof(&alice),
            pending.id(),
            Some(prior.fact_id),
        )
        .await,
    );
    assert_version(&store, &alice, expected).await;
}

#[tokio::test]
async fn retraction_retains_version_id_and_changes_state() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "Alice", None).await;
    let active = current_preferred_name_version(&store, &alice)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.state, PersonalFactState::Active);
    assert!(retract_preferred_name(&store, &alice, active.fact_id)
        .await
        .unwrap());
    assert_eq!(active.fact_id, fact.fact_id);
    assert_version(
        &store,
        &alice,
        PersonalFactVersion {
            fact_id: active.fact_id,
            state: PersonalFactState::Retracted,
        },
    )
    .await;
}

#[tokio::test]
async fn suppressed_head_retains_current_version_without_falling_back() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let first = publish(&store, &alice, source, "First", None).await;
    let current = publish(&store, &alice, source, "Current", Some(first.fact_id)).await;
    fixtures::lose_all_acl(&store, &current.fact_id.to_string()).await;
    assert_version(
        &store,
        &alice,
        PersonalFactVersion {
            fact_id: current.fact_id,
            state: PersonalFactState::Suppressed,
        },
    )
    .await;
}

#[tokio::test]
async fn reconnected_binding_can_read_version_but_stale_generation_cannot() {
    use crate::identity_reconnection::{commit_review, review_revocation, ReplacementConsent};

    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, "Alice", None).await;
    let expected = PersonalFactVersion {
        fact_id: fact.fact_id,
        state: PersonalFactState::Active,
    };
    assert_version(&store, &alice, expected).await;
    let review = review_revocation(&store, UserId::new(99), alice.entity_id())
        .await
        .unwrap();
    commit_review(&store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    denied(current_preferred_name_version(&store, &alice).await);
    let fresh = reconnect(&store, &alice).await;
    assert_eq!(fresh.entity_id(), alice.entity_id());
    assert_eq!(fresh.user_id(), alice.user_id());
    assert!(fresh.authority_generation > alice.authority_generation);
    denied(current_preferred_name_version(&store, &alice).await);
    assert_version(&store, &fresh, expected).await;
}
