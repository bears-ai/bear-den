use super::support::{assert_denied, Fixture};
use crate::{bears::db, personal_memory as service};
use den_core::DenError;
use sqlx::PgPool;
use std::time::Duration;
use tokio::task::JoinHandle;

fn spawn_stage(fixture: &Fixture) -> JoinHandle<Result<service::PersonalCandidate, DenError>> {
    let pool = fixture.pool.clone();
    let stores = fixture.stores.clone();
    let bear = fixture.bear;
    let user = fixture.owner;
    let conversation = fixture.conversation;
    let source = fixture.source;
    tokio::spawn(async move {
        service::stage_preferred_name(
            &pool,
            &stores,
            bear,
            user,
            conversation,
            source,
            service::PreferredName::new("Alice").unwrap(),
        )
        .await
    })
}

async fn wait_for_query_lock(pool: &PgPool, query: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting = sqlx::query_scalar!(
                r#"SELECT EXISTS (
                    SELECT 1 FROM pg_stat_activity
                    WHERE datname = current_database() AND wait_event_type = 'Lock'
                      AND query = $1
                ) AS "waiting!""#,
                query,
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
    .expect("personal-memory operation must reach the expected authority lock");
}

fn assert_locked<T>(result: Result<T, sqlx::Error>) {
    assert!(matches!(result, Err(sqlx::Error::Database(error))
        if error.code().as_deref() == Some("55P03")));
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_lock_precedes_bear_lock(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let mut deletion = fixture.pool.begin().await.unwrap();
    sqlx::query_scalar!(
        "SELECT id FROM users WHERE id = $1 FOR UPDATE",
        fixture.owner.get(),
    )
    .fetch_one(&mut *deletion)
    .await
    .unwrap();
    let request = spawn_stage(&fixture);
    wait_for_query_lock(
        &fixture.pool,
        "SELECT id FROM users WHERE id = $1 FOR KEY SHARE",
    )
    .await;
    // A User-lock waiter must not already hold Bear and deadlock identity deletion.
    sqlx::query_scalar!(
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE NOWAIT",
        fixture.bear.as_uuid(),
    )
    .fetch_one(&mut *deletion)
    .await
    .unwrap();
    deletion.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn membership_is_rechecked_after_waiting_for_bear(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let mut revocation = fixture.pool.begin().await.unwrap();
    sqlx::query_scalar!(
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE",
        fixture.bear.as_uuid(),
    )
    .fetch_one(&mut *revocation)
    .await
    .unwrap();
    let request = spawn_stage(&fixture);
    wait_for_query_lock(
        &fixture.pool,
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE",
    )
    .await;
    sqlx::query!(
        "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        fixture.owner.get(),
        fixture.bear.as_uuid(),
    )
    .execute(&mut *revocation)
    .await
    .unwrap();
    revocation.commit().await.unwrap();
    assert_denied(
        tokio::time::timeout(Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap(),
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn cancellation_keeps_all_authority_and_source_locks_until_sqlite_finishes(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let store = fixture.store().await;
    // The process-owned per-Bear pool has one connection. Holding it pauses the
    // actual SQLite operation without runtime fixture SQL or production hooks.
    let sqlite = store.pool().acquire().await.unwrap();
    let request = spawn_stage(&fixture);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let probe = sqlx::query_scalar!(
                "SELECT id FROM conversations WHERE id = $1 FOR UPDATE NOWAIT",
                fixture.conversation,
            )
            .fetch_one(&fixture.pool)
            .await;
            match probe {
                Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
                    break
                }
                Ok(_) => {}
                Err(error) => panic!("unexpected conversation-lock probe failure: {error}"),
            }
            assert!(!request.is_finished(), "staging must wait for SQLite");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    request.abort();
    assert!(matches!(request.await, Err(error) if error.is_cancelled()));
    let mut revocation = {
        let pool = fixture.pool.clone();
        let bear = fixture.bear;
        let user = fixture.owner;
        tokio::spawn(async move { db::revoke_membership(&pool, user.get(), bear.as_uuid()).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut revocation)
            .await
            .is_err()
    );
    assert_locked(
        sqlx::query_scalar!(
            "SELECT id FROM users WHERE id = $1 FOR UPDATE NOWAIT",
            fixture.owner.get(),
        )
        .fetch_one(&fixture.pool)
        .await,
    );
    assert_locked(
        sqlx::query_scalar!(
            "SELECT id FROM bears WHERE id = $1 FOR UPDATE NOWAIT",
            fixture.bear.as_uuid(),
        )
        .fetch_one(&fixture.pool)
        .await,
    );
    assert_locked(
        sqlx::query_scalar!(
            "SELECT user_id FROM user_bear WHERE user_id = $1 AND bear_id = $2 FOR UPDATE NOWAIT",
            fixture.owner.get(),
            fixture.bear.as_uuid(),
        )
        .fetch_one(&fixture.pool)
        .await,
    );
    assert_locked(
        sqlx::query_scalar!(
            "SELECT id FROM conversations WHERE id = $1 FOR UPDATE NOWAIT",
            fixture.conversation,
        )
        .fetch_one(&fixture.pool)
        .await,
    );
    assert_locked(
        sqlx::query_scalar!(
            "SELECT id FROM bear_hats WHERE id = $1 FOR UPDATE NOWAIT",
            fixture.hat.as_uuid(),
        )
        .fetch_one(&fixture.pool)
        .await,
    );
    drop(sqlite);
    tokio::time::timeout(Duration::from_secs(5), revocation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_denied(
        fixture
            .stage_as(fixture.owner, fixture.conversation, fixture.source)
            .await,
    );
}
