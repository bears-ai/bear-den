use super::*;
use crate::bears::db::{self, BearParams};
use std::{sync::Arc, time::Duration};
use tokio::sync::Barrier;

async fn bear(pool: &PgPool) -> Uuid {
    db::create_bear(
        pool,
        BearParams {
            slug: "membership-guard-bear",
            name: "Membership guard Bear",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap()
}

async fn user(pool: &PgPool, username: &str) -> i32 {
    let email = format!("{username}@example.test");
    sqlx::query_scalar!(
        "INSERT INTO users (username, email, display_name, passhash) VALUES ($1, $2, $1, 'membership-test-hash') RETURNING id",
        username, email
    ).fetch_one(pool).await.unwrap()
}

fn assert_last_admin(result: Result<(), DenError>) {
    assert!(
        matches!(result, Err(DenError::ValidationError(ref message)) if message == LAST_BEAR_ADMIN_MESSAGE)
    );
}

#[test]
fn role_boundary_accepts_legacy_member_defaults_and_canonicalizes_known_roles() {
    for value in [None, Some(""), Some(" "), Some("member"), Some(" MEMBER ")] {
        assert_eq!(
            BearMembershipRole::try_from(value).unwrap(),
            BearMembershipRole::Member
        );
    }
    for value in [Some("admin"), Some(" ADMIN "), Some("AdMiN")] {
        assert_eq!(
            BearMembershipRole::try_from(value).unwrap(),
            BearMembershipRole::Admin
        );
    }
    assert_eq!(BearMembershipRole::Admin.as_str(), "admin");
    assert_eq!(BearMembershipRole::Member.as_str(), "member");
    assert!(matches!(
        BearMembershipRole::try_from(Some("owner")),
        Err(DenError::ValidationError(_))
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn sole_admin_demotion_and_revocation_leave_membership_unchanged(pool: PgPool) {
    let bear = bear(&pool).await;
    let admin = user(&pool, "soleadmin").await;
    grant_membership(&pool, admin, bear, Some("admin"))
        .await
        .unwrap();
    for role in [Some("member"), None, Some(" ")] {
        assert_last_admin(grant_membership(&pool, admin, bear, role).await);
        assert_eq!(
            db::membership_role_for_user(&pool, admin, bear)
                .await
                .unwrap(),
            Some(Some("admin".into()))
        );
        assert_eq!(db::count_bear_admins(&pool, bear).await.unwrap(), 1);
    }
    assert_last_admin(revoke_membership(&pool, admin, bear).await);
    assert_eq!(db::count_bear_members(&pool, bear).await.unwrap(), 1);
    grant_membership(&pool, admin, bear, Some(" ADMIN "))
        .await
        .unwrap();
    assert_eq!(
        db::membership_role_for_user(&pool, admin, bear)
            .await
            .unwrap(),
        Some(Some("admin".into()))
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_role_changes_do_not_modify_or_create_memberships(pool: PgPool) {
    let bear = bear(&pool).await;
    let admin = user(&pool, "validationadmin").await;
    let member = user(&pool, "validationmember").await;
    let unassigned = user(&pool, "validationnew").await;
    grant_membership(&pool, admin, bear, Some("admin"))
        .await
        .unwrap();
    grant_membership(&pool, member, bear, None).await.unwrap();
    for id in [admin, member, unassigned] {
        for role in ["owner", "viewer", "administrator", "admin member"] {
            assert!(matches!(
                grant_membership(&pool, id, bear, Some(role)).await,
                Err(DenError::ValidationError(_))
            ));
        }
    }
    assert_eq!(
        db::membership_role_for_user(&pool, admin, bear)
            .await
            .unwrap(),
        Some(Some("admin".into()))
    );
    assert_eq!(
        db::membership_role_for_user(&pool, member, bear)
            .await
            .unwrap(),
        Some(Some("member".into()))
    );
    assert_eq!(
        db::membership_role_for_user(&pool, unassigned, bear)
            .await
            .unwrap(),
        None
    );
    assert_eq!(db::count_bear_admins(&pool, bear).await.unwrap(), 1);
    assert_eq!(db::count_bear_members(&pool, bear).await.unwrap(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn grant_another_admin_then_demotion_or_revocation_succeeds(pool: PgPool) {
    let bear = bear(&pool).await;
    let first = user(&pool, "handofffirst").await;
    let second = user(&pool, "handoffsecond").await;
    grant_membership(&pool, first, bear, Some("admin"))
        .await
        .unwrap();
    grant_membership(&pool, second, bear, Some("member"))
        .await
        .unwrap();
    assert_last_admin(revoke_membership(&pool, first, bear).await);
    grant_membership(&pool, second, bear, Some("admin"))
        .await
        .unwrap();
    grant_membership(&pool, first, bear, Some("member"))
        .await
        .unwrap();
    assert_eq!(db::count_bear_admins(&pool, bear).await.unwrap(), 1);
    grant_membership(&pool, first, bear, Some("admin"))
        .await
        .unwrap();
    revoke_membership(&pool, second, bear).await.unwrap();
    assert_eq!(
        db::membership_role_for_user(&pool, second, bear)
            .await
            .unwrap(),
        None
    );
    assert_eq!(db::count_bear_admins(&pool, bear).await.unwrap(), 1);
    assert!(matches!(
        revoke_membership(&pool, second, bear).await,
        Err(DenError::NotFound(_))
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn guard_counts_legacy_case_and_whitespace_admin_roles(pool: PgPool) {
    let bear = bear(&pool).await;
    let legacy = user(&pool, "legacyadmin").await;
    sqlx::query!(
        "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
        legacy,
        bear,
        " AdMiN "
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_last_admin(grant_membership(&pool, legacy, bear, Some("member")).await);
    assert_last_admin(revoke_membership(&pool, legacy, bear).await);
    assert_eq!(
        db::membership_role_for_user(&pool, legacy, bear)
            .await
            .unwrap(),
        Some(Some(" AdMiN ".into()))
    );
    let next = user(&pool, "nextadmin").await;
    grant_membership(&pool, next, bear, Some("admin"))
        .await
        .unwrap();
    revoke_membership(&pool, next, bear).await.unwrap();
    assert_eq!(db::count_bear_admins(&pool, bear).await.unwrap(), 1);
}

#[derive(Clone, Copy)]
enum Attempt {
    Demote,
    Revoke,
}

async fn attempt(
    pool: PgPool,
    barrier: Arc<Barrier>,
    bear: Uuid,
    user: i32,
    action: Attempt,
) -> Result<(), DenError> {
    barrier.wait().await;
    match action {
        Attempt::Demote => grant_membership(&pool, user, bear, Some("member")).await,
        Attempt::Revoke => revoke_membership(&pool, user, bear).await,
    }
}

async fn race(pool: PgPool, first_action: Attempt, second_action: Attempt) {
    let bear = bear(&pool).await;
    let first = user(&pool, "racefirst").await;
    let second = user(&pool, "racesecond").await;
    grant_membership(&pool, first, bear, Some("admin"))
        .await
        .unwrap();
    grant_membership(&pool, second, bear, Some("admin"))
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let first_task = tokio::spawn(attempt(
        pool.clone(),
        barrier.clone(),
        bear,
        first,
        first_action,
    ));
    let second_task = tokio::spawn(attempt(
        pool.clone(),
        barrier.clone(),
        bear,
        second,
        second_action,
    ));
    let (first_result, second_result) = tokio::time::timeout(Duration::from_secs(10), async {
        barrier.wait().await;
        (first_task.await.unwrap(), second_task.await.unwrap())
    })
    .await
    .expect("membership mutation race must finish without deadlock");
    assert_eq!(
        usize::from(first_result.is_ok()) + usize::from(second_result.is_ok()),
        1
    );
    let (winner, loser, winner_action) = if first_result.is_ok() {
        assert_last_admin(second_result);
        (first, second, first_action)
    } else {
        assert_last_admin(first_result);
        (second, first, second_action)
    };
    assert_eq!(db::count_bear_admins(&pool, bear).await.unwrap(), 1);
    assert_eq!(
        db::membership_role_for_user(&pool, loser, bear)
            .await
            .unwrap(),
        Some(Some("admin".into()))
    );
    let expected = match winner_action {
        Attempt::Demote => Some(Some("member".into())),
        Attempt::Revoke => None,
    };
    assert_eq!(
        db::membership_role_for_user(&pool, winner, bear)
            .await
            .unwrap(),
        expected
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_demotions_cannot_remove_both_admins(pool: PgPool) {
    race(pool, Attempt::Demote, Attempt::Demote).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_revocations_cannot_remove_both_admins(pool: PgPool) {
    race(pool, Attempt::Revoke, Attempt::Revoke).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_demotion_and_revocation_share_one_guard(pool: PgPool) {
    race(pool, Attempt::Demote, Attempt::Revoke).await;
}
