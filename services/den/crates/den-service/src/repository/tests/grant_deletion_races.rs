//! Deterministic FK/row-lock races on test-owned records, using deletion's
//! User -> Bear -> dependent order. No sleep-based race winner assumptions.
use super::{
    assert_denied,
    fixture::{fixture, Fixture},
};
use crate::{bears::hats::access, repository::grants};
use den_core::{DenError, TurnExecutionOrigin};
use sqlx::{PgPool, Postgres, Transaction};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Copy)]
enum Waiting {
    GrantUser,
    GrantBear,
    GrantHat,
    DeleteBear,
}
impl Waiting {
    fn code(self) -> i32 {
        match self {
            Self::GrantUser => 0,
            Self::GrantBear => 1,
            Self::GrantHat => 2,
            Self::DeleteBear => 3,
        }
    }
}

async fn wait_for_lock(pool: &PgPool, phase: Waiting) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting = sqlx::query_scalar!(
                r#"SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock' AND (
                    ($1=0 AND query='SELECT id FROM users WHERE id = $1 FOR KEY SHARE') OR
                    ($1=1 AND query='SELECT id FROM bears WHERE id = $1 FOR KEY SHARE') OR
                    ($1=2 AND query LIKE '%FOR UPDATE OF h, ub, hs, sb%') OR
                    ($1=3 AND query='SELECT id FROM bears WHERE id = $1 FOR UPDATE')
                )) AS "waiting!""#,
                phase.code()
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected lock barrier before dependent mutation");
}

async fn fresh_target(pool: &PgPool, f: &Fixture) -> String {
    access::revoke(pool, f.bear, f.hat, f.owner, f.grant)
        .await
        .unwrap();
    grants::choices(pool, f.bear, f.hat, f.owner)
        .await
        .unwrap()
        .remove(0)
        .target_key
}

fn grant_task(
    pool: &PgPool,
    f: &Fixture,
    target: String,
) -> tokio::task::JoinHandle<Result<Uuid, DenError>> {
    let pool = pool.clone();
    let (bear, hat, actor, surface) = (f.bear, f.hat, f.owner, f.surface);
    tokio::spawn(
        async move { grants::grant(&pool, bear, hat, actor, surface, &target, true).await },
    )
}

async fn lock_dependents_without_waiting(tx: &mut Transaction<'_, Postgres>, f: &Fixture) {
    sqlx::query!(
        "SELECT id FROM provider_connections WHERE id=$1 FOR UPDATE NOWAIT",
        f.connection.0
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap();
    sqlx::query!(
        "SELECT id FROM bear_hats WHERE id=$1 FOR UPDATE NOWAIT",
        f.hat.as_uuid()
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_deletion_wins_before_a_new_grant_takes_dependent_locks(pool: PgPool) {
    let f = fixture(&pool).await;
    let target = fresh_target(&pool, &f).await;
    let mut deletion = pool.begin().await.unwrap();
    sqlx::query!("SELECT id FROM users WHERE id=$1 FOR UPDATE", f.owner.get())
        .fetch_one(&mut *deletion)
        .await
        .unwrap();
    let grant = grant_task(&pool, &f, target);
    wait_for_lock(&pool, Waiting::GrantUser).await;
    sqlx::query!(
        "SELECT id FROM bears WHERE id=$1 FOR UPDATE NOWAIT",
        f.bear.as_uuid()
    )
    .fetch_one(&mut *deletion)
    .await
    .unwrap();
    lock_dependents_without_waiting(&mut deletion, &f).await;
    // Explicitly retire this fixture's owned Bear and repository/account before
    // deleting its User; production identity deletion must also respect these FKs.
    sqlx::query!("DELETE FROM bears WHERE id=$1", f.bear.as_uuid())
        .execute(&mut *deletion)
        .await
        .unwrap();
    sqlx::query!("DELETE FROM work_surfaces WHERE id=$1", f.surface.0)
        .execute(&mut *deletion)
        .await
        .unwrap();
    sqlx::query!(
        "DELETE FROM provider_connections WHERE id=$1",
        f.connection.0
    )
    .execute(&mut *deletion)
    .await
    .unwrap();
    sqlx::query!("DELETE FROM users WHERE id=$1", f.owner.get())
        .execute(&mut *deletion)
        .await
        .unwrap();
    deletion.commit().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), grant)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    assert_eq!(
        sqlx::query_scalar!(
            "SELECT count(*) FROM bear_hat_access_grants WHERE bear_id=$1",
            f.bear.as_uuid()
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        Some(0)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_wins_before_a_new_grant_takes_dependent_locks(pool: PgPool) {
    let f = fixture(&pool).await;
    let target = fresh_target(&pool, &f).await;
    let mut deletion = pool.begin().await.unwrap();
    sqlx::query!(
        "SELECT id FROM users WHERE id = $1 FOR KEY SHARE",
        f.owner.get()
    )
    .fetch_one(&mut *deletion)
    .await
    .unwrap();
    sqlx::query!(
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE",
        f.bear.as_uuid()
    )
    .fetch_one(&mut *deletion)
    .await
    .unwrap();
    let grant = grant_task(&pool, &f, target);
    wait_for_lock(&pool, Waiting::GrantBear).await;
    lock_dependents_without_waiting(&mut deletion, &f).await;
    sqlx::query!("DELETE FROM bears WHERE id=$1", f.bear.as_uuid())
        .execute(&mut *deletion)
        .await
        .unwrap();
    deletion.commit().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), grant)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_partial_retirement_bear_lock_forces_retry_before_dependent_locks(pool: PgPool) {
    let f = fixture(&pool).await;
    let target = fresh_target(&pool, &f).await;
    let mut retirement = pool.begin().await.unwrap();
    sqlx::query!(
        "SELECT id FROM users WHERE id = $1 FOR KEY SHARE",
        f.owner.get()
    )
    .fetch_one(&mut *retirement)
    .await
    .unwrap();
    sqlx::query!(
        "SELECT id FROM bears WHERE id=$1 FOR NO KEY UPDATE",
        f.bear.as_uuid()
    )
    .fetch_one(&mut *retirement)
    .await
    .unwrap();
    sqlx::query!(
        "SELECT user_id FROM user_bear WHERE user_id=$1 AND bear_id=$2 FOR SHARE",
        f.owner.get(),
        f.bear.as_uuid()
    )
    .fetch_one(&mut *retirement)
    .await
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        grants::grant(&pool, f.bear, f.hat, f.owner, f.surface, &target, true),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(DenError::ValidationError(_))));
    lock_dependents_without_waiting(&mut retirement, &f).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query!(
            "SELECT id FROM bears WHERE id=$1 FOR UPDATE",
            f.bear.as_uuid()
        )
        .fetch_one(&mut *retirement),
    )
    .await
    .unwrap()
    .unwrap();
    retirement.rollback().await.unwrap();
    assert!(grants::list(&pool, f.bear, f.hat).await.unwrap().is_empty());
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_grant_that_wins_is_removed_by_subsequent_bear_deletion(pool: PgPool) {
    let f = fixture(&pool).await;
    let target = fresh_target(&pool, &f).await;
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query!(
        "SELECT id FROM bear_hats WHERE id=$1 FOR UPDATE",
        f.hat.as_uuid()
    )
    .fetch_one(&mut *barrier)
    .await
    .unwrap();
    let grant = grant_task(&pool, &f, target);
    wait_for_lock(&pool, Waiting::GrantHat).await;
    let delete = tokio::spawn({
        let pool = pool.clone();
        let (actor, bear) = (f.owner, f.bear);
        async move {
            let mut tx = pool.begin().await.unwrap();
            sqlx::query!(
                "SELECT id FROM users WHERE id = $1 FOR KEY SHARE",
                actor.get()
            )
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            sqlx::query!(
                "SELECT id FROM bears WHERE id = $1 FOR UPDATE",
                bear.as_uuid()
            )
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            sqlx::query!("DELETE FROM bears WHERE id=$1", bear.as_uuid())
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    });
    wait_for_lock(&pool, Waiting::DeleteBear).await;
    barrier.commit().await.unwrap();
    let id = tokio::time::timeout(Duration::from_secs(5), grant)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), delete)
        .await
        .unwrap()
        .unwrap();
    assert!(!sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM bear_hat_access_grants WHERE id=$1) AS \"exists!\"",
        id
    )
    .fetch_one(&pool)
    .await
    .unwrap());
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
}
