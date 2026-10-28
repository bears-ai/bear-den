use super::support::{assert_denied, Fixture};
use crate::{bears::db, personal_memory as service};
use den_core::{ids::UserId, DenError};
use den_memory::{
    identity_reconnection::{
        commit_review, review_reconnection, review_revocation, ReplacementConsent,
    },
    personal_candidates,
};
use service::{OwnerMemoryAction, OwnerMemoryReceipt, PersonalMemoryOwner, PreferredName};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

async fn inspect(f: &Fixture, user: UserId) -> service::PersonalMemoryInspection {
    service::inspect_personal_memory(&f.pool, &f.stores, f.bear, user)
        .await
        .unwrap()
}

async fn apply(
    f: &Fixture,
    owner: &PersonalMemoryOwner,
    action: OwnerMemoryAction,
) -> Result<OwnerMemoryReceipt, DenError> {
    service::apply_owner_memory_action(&f.pool, &f.stores, f.bear, f.owner, owner, action).await
}

fn request(name: &str) -> OwnerMemoryAction {
    OwnerMemoryAction::RequestPreferredName {
        request_id: Uuid::new_v4(),
        name: PreferredName::new(name).unwrap(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_management_stages_then_separately_publishes_without_exposing_another_owner(
    pool: PgPool,
) {
    let f = Fixture::new(pool).await;
    let initial = inspect(&f, f.owner).await;
    assert!(!initial.capture_enabled);
    assert!(initial.current.is_none());
    assert!(initial.candidates.is_empty());
    assert_eq!(
        format!("{:?}", initial.owner),
        "PersonalMemoryOwner([redacted])"
    );
    assert_denied(apply(&f, &initial.owner, request("Alice")).await);
    apply(&f, &initial.owner, OwnerMemoryAction::SetCapture(true))
        .await
        .unwrap();
    let opted_in = inspect(&f, f.owner).await;
    let receipt = apply(&f, &opted_in.owner, request("Alice")).await.unwrap();
    let OwnerMemoryReceipt::CandidateRequested(id) = receipt else {
        panic!("candidate receipt")
    };
    let pending = inspect(&f, f.owner).await;
    assert!(pending.current.is_none(), "requesting is not publication");
    assert_eq!(pending.candidates.len(), 1);
    assert_eq!(pending.candidates[0].id(), id);
    apply(
        &f,
        &pending.owner,
        OwnerMemoryAction::Publish {
            candidate_id: id,
            expected_head: None,
        },
    )
    .await
    .unwrap();
    let active = inspect(&f, f.owner).await;
    assert_eq!(
        active.current.unwrap().preferred_name.unwrap().as_str(),
        "Alice"
    );
    let other_admin = inspect(&f, f.other).await;
    assert!(other_admin.current.is_none());
    assert!(other_admin.candidates.is_empty());
    assert_denied(
        service::apply_owner_memory_action(
            &f.pool,
            &f.stores,
            f.bear,
            f.other,
            &other_admin.owner,
            OwnerMemoryAction::Publish {
                candidate_id: id,
                expected_head: None,
            },
        )
        .await,
    );
    assert_denied(service::inspect_personal_memory(&f.pool, &f.stores, f.bear, f.outsider).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn retained_owner_cannot_act_after_reconnection_generation_aba(pool: PgPool) {
    let f = Fixture::new(pool).await;
    let original = inspect(&f, f.owner).await;
    let binding = f.binding(f.owner).await;
    let store = f.store().await;
    let revoke = review_revocation(&store, f.other, binding.entity_id())
        .await
        .unwrap();
    commit_review(&store, &revoke, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let reconnect = review_reconnection(&store, f.other, binding.entity_id(), f.owner)
        .await
        .unwrap();
    commit_review(&store, &reconnect, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_denied(apply(&f, &original.owner, OwnerMemoryAction::SetCapture(true)).await);
    assert_denied(apply(&f, &original.owner, request("Stale name")).await);
    let fresh = inspect(&f, f.owner).await;
    assert!(!fresh.capture_enabled);
    assert!(fresh.candidates.is_empty());
    apply(&f, &fresh.owner, OwnerMemoryAction::SetCapture(true))
        .await
        .unwrap();
    assert!(inspect(&f, f.owner).await.capture_enabled);
}

#[sqlx::test(migrations = "../../migrations")]
async fn retained_owner_actions_recheck_membership_and_reject_mismatched_principals_before_io(
    pool: PgPool,
) {
    let f = Fixture::new(pool).await;
    let original = inspect(&f, f.owner).await;
    let closed = PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    closed.close().await;
    for (bear, user) in [(f.bear, f.other), (Uuid::new_v4().into(), f.owner)] {
        assert_denied(
            service::apply_owner_memory_action(
                &closed,
                &f.stores,
                bear,
                user,
                &original.owner,
                OwnerMemoryAction::SetCapture(true),
            )
            .await,
        );
    }
    db::revoke_membership(&f.pool, f.owner.get(), f.bear.as_uuid())
        .await
        .unwrap();
    assert_denied(apply(&f, &original.owner, OwnerMemoryAction::SetCapture(true)).await);
    assert!(
        !personal_candidates::capture_enabled(&f.store().await, &f.binding(f.owner).await)
            .await
            .unwrap()
    );
}
