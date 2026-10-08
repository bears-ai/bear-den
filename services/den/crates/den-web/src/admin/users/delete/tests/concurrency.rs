use super::*;
use std::time::Duration;
use tokio::sync::Barrier;

#[derive(Clone, Copy)]
enum Action {
    Delete,
    Demote,
    Revoke,
    GrantAdmin,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Applied,
    LastAdmin,
    NotFound,
}

async fn attempt(
    pool: PgPool,
    start: Arc<Barrier>,
    user: UserId,
    bear: BearId,
    action: Action,
) -> Outcome {
    start.wait().await;
    match action {
        Action::Delete => match deletion::delete_user(&pool, user).await {
            Ok(()) => Outcome::Applied,
            Err(UserDeletionError::LastBearAdmin(_)) => Outcome::LastAdmin,
            Err(UserDeletionError::NotFound) => Outcome::NotFound,
            result => panic!("unexpected deletion result (including deadlock): {result:?}"),
        },
        action => {
            let result = match action {
                Action::Demote => {
                    bears_db::grant_membership(&pool, user.get(), bear.as_uuid(), Some("member"))
                        .await
                }
                Action::Revoke => {
                    bears_db::revoke_membership(&pool, user.get(), bear.as_uuid()).await
                }
                Action::GrantAdmin => {
                    bears_db::grant_membership(&pool, user.get(), bear.as_uuid(), Some("admin"))
                        .await
                }
                Action::Delete => unreachable!(),
            };
            match result {
                Ok(()) => Outcome::Applied,
                Err(DenError::ValidationError(_)) => Outcome::LastAdmin,
                Err(DenError::NotFound(_)) => Outcome::NotFound,
                result => panic!("unexpected membership result (including deadlock): {result:?}"),
            }
        }
    }
}

async fn race(pool: &PgPool, attempts: &[(UserId, BearId, Action)]) -> Vec<Outcome> {
    let start = Arc::new(Barrier::new(attempts.len() + 1));
    let tasks: Vec<_> = attempts
        .iter()
        .map(|&(user, bear, action)| {
            tokio::spawn(attempt(pool.clone(), start.clone(), user, bear, action))
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(10), async {
        start.wait().await;
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap());
        }
        results
    })
    .await
    .expect("identity/membership race must finish without deadlock")
}

#[sqlx::test(migrations = "../../migrations")]
async fn three_concurrent_deletes_lock_overlapping_bears_in_uuid_order_and_leave_one_admin(
    pool: PgPool,
) {
    let atlas = bear(&pool, "delete-race-atlas", "Atlas").await;
    let birch = bear(&pool, "delete-race-birch", "Birch").await;
    let mut attempts = Vec::new();
    for username in [
        "delete-race-first",
        "delete-race-second",
        "delete-race-third",
    ] {
        let user = user(&pool, username, false).await;
        // Seed in reverse orders; locking must not follow membership insertion order.
        let bears = if attempts.len() == 1 {
            [birch, atlas]
        } else {
            [atlas, birch]
        };
        for bear in bears {
            grant(&pool, user, bear, "admin").await;
        }
        attempts.push((user, atlas, Action::Delete));
    }
    let results = race(&pool, &attempts).await;
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Outcome::Applied)
            .count(),
        2
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Outcome::LastAdmin)
            .count(),
        1
    );
    for bear in [atlas, birch] {
        assert_eq!(admins(&pool, bear).await, 1);
    }
    for ((user, _, _), result) in attempts.iter().zip(results) {
        assert_eq!(exists(&pool, *user).await, result == Outcome::LastAdmin);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_deletes_of_same_identity_recheck_existence_after_user_lock(pool: PgPool) {
    let target = user(&pool, "same-delete-target", false).await;
    let next = user(&pool, "same-delete-next", false).await;
    let atlas = bear(&pool, "same-delete-atlas", "Atlas").await;
    for user in [target, next] {
        grant(&pool, user, atlas, "admin").await;
    }
    let outcomes = race(
        &pool,
        &[
            (target, atlas, Action::Delete),
            (target, atlas, Action::Delete),
        ],
    )
    .await;
    assert!(outcomes.contains(&Outcome::Applied));
    assert!(outcomes.contains(&Outcome::NotFound));
    assert_eq!(admins(&pool, atlas).await, 1);
}

async fn delete_vs_other_removal(pool: PgPool, removal: Action) {
    let target = user(&pool, "delete-vs-removal-target", false).await;
    let other = user(&pool, "delete-vs-removal-other", false).await;
    let atlas = bear(&pool, "delete-vs-removal-atlas", "Atlas").await;
    for user in [target, other] {
        grant(&pool, user, atlas, "admin").await;
    }
    let outcomes = race(
        &pool,
        &[(target, atlas, Action::Delete), (other, atlas, removal)],
    )
    .await;
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Outcome::Applied)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Outcome::LastAdmin)
            .count(),
        1
    );
    assert_eq!(admins(&pool, atlas).await, 1);
    if outcomes[0] == Outcome::LastAdmin {
        assert!(exists(&pool, target).await);
        assert_eq!(role(&pool, target, atlas).await, Some(Some("admin".into())));
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn deletion_and_other_admin_demotion_share_fresh_guard(pool: PgPool) {
    delete_vs_other_removal(pool, Action::Demote).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn deletion_and_other_admin_revocation_share_fresh_guard(pool: PgPool) {
    delete_vs_other_removal(pool, Action::Revoke).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn deletion_and_new_admin_grant_allow_only_safe_serial_outcomes(pool: PgPool) {
    let target = user(&pool, "delete-vs-grant-target", false).await;
    let next = user(&pool, "delete-vs-grant-next", false).await;
    let atlas = bear(&pool, "delete-vs-grant-atlas", "Atlas").await;
    grant(&pool, target, atlas, "admin").await;
    let outcomes = race(
        &pool,
        &[
            (target, atlas, Action::Delete),
            (next, atlas, Action::GrantAdmin),
        ],
    )
    .await;
    assert_eq!(outcomes[1], Outcome::Applied);
    assert!(matches!(outcomes[0], Outcome::Applied | Outcome::LastAdmin));
    assert_eq!(
        admins(&pool, atlas).await,
        if outcomes[0] == Outcome::Applied {
            1
        } else {
            2
        }
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn deletion_and_same_user_grant_demotion_or_revocation_cannot_deadlock(pool: PgPool) {
    for (index, action) in [Action::GrantAdmin, Action::Demote, Action::Revoke]
        .into_iter()
        .enumerate()
    {
        let target = user(&pool, &format!("same-user-target-{index}"), false).await;
        let next = user(&pool, &format!("same-user-next-{index}"), false).await;
        let atlas = bear(&pool, &format!("same-user-atlas-{index}"), "Atlas").await;
        for user in [target, next] {
            grant(&pool, user, atlas, "admin").await;
        }
        let outcomes = race(
            &pool,
            &[(target, atlas, Action::Delete), (target, atlas, action)],
        )
        .await;
        assert_eq!(outcomes[0], Outcome::Applied);
        assert!(matches!(outcomes[1], Outcome::Applied | Outcome::NotFound));
        assert_eq!(admins(&pool, atlas).await, 1);
        assert!(!exists(&pool, target).await);
    }
}

// Poll actual lock waits, rather than relying on timing to establish adversarial schedules.
async fn waiting(pool: &PgPool, query: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting = sqlx::query_scalar!(
                r#"SELECT EXISTS (
                    SELECT 1 FROM pg_stat_activity
                    WHERE datname = current_database() AND wait_event_type = 'Lock' AND query = $1
                ) AS "waiting!""#,
                query
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
    .expect("expected database lock waiter");
}

#[sqlx::test(migrations = "../../migrations")]
async fn fk_insert_started_before_delete_cannot_slip_past_membership_snapshot(pool: PgPool) {
    let target = user(&pool, "fk-delete-target", false).await;
    let atlas = bear(&pool, "fk-delete-atlas", "Atlas").await;
    // An uncommitted insert holds the FK's User KEY SHARE lock. Deletion must wait
    // before enumerating Bears and then see this newly committed sole-admin membership.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query!(
        "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
        target.get(),
        atlas.as_uuid(),
        "admin"
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    let task = tokio::spawn({
        let pool = pool.clone();
        async move { deletion::delete_user(&pool, target).await }
    });
    waiting(&pool, "SELECT username FROM users WHERE id = $1 FOR UPDATE").await;
    tx.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(UserDeletionError::LastBearAdmin(_))));
    assert_eq!(admins(&pool, atlas).await, 1);
    assert!(exists(&pool, target).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn fk_insert_started_after_delete_user_lock_cannot_create_an_unchecked_membership(
    pool: PgPool,
) {
    let target = user(&pool, "late-fk-target", false).await;
    let next = user(&pool, "late-fk-next", false).await;
    let atlas = bear(&pool, "late-fk-atlas", "Atlas").await;
    let birch = bear(&pool, "late-fk-birch", "Birch").await;
    for user in [target, next] {
        grant(&pool, user, atlas, "admin").await;
    }
    let mut blocker = pool.begin().await.unwrap();
    let blocker_pid = sqlx::query_scalar!("SELECT pg_backend_pid() AS \"pid!\"")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query!(
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE",
        atlas.as_uuid()
    )
    .fetch_one(&mut *blocker)
    .await
    .unwrap();
    let delete = tokio::spawn({
        let pool = pool.clone();
        async move { deletion::delete_user(&pool, target).await }
    });
    // Deletion's Bear wait proves it has already acquired the exclusive User lock.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting = sqlx::query_scalar!(
                r#"SELECT EXISTS (
                    SELECT 1 FROM pg_stat_activity
                    WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid))
                ) AS "waiting!""#,
                blocker_pid
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("deletion should be waiting on Bear after locking User");
    let insert = tokio::spawn({
        let pool = pool.clone();
        async move {
            sqlx::query!(
                "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
                target.get(),
                birch.as_uuid(),
                "admin"
            )
            .execute(&pool)
            .await
        }
    });
    waiting(
        &pool,
        "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
    )
    .await;
    blocker.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        delete.await.unwrap().unwrap();
        let error = insert.await.unwrap().unwrap_err();
        assert!(
            matches!(error, sqlx::Error::Database(ref error) if error.is_foreign_key_violation())
        );
    })
    .await
    .expect("delete and FK insert must finish without deadlock");
    assert!(!exists(&pool, target).await);
    assert_eq!(role(&pool, target, birch).await, None);
    assert_eq!(admins(&pool, atlas).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn new_grant_waits_on_target_user_before_taking_any_bear_lock(pool: PgPool) {
    let target = user(&pool, "lock-order-target", false).await;
    let next = user(&pool, "lock-order-next", false).await;
    let atlas = bear(&pool, "lock-order-atlas", "Atlas").await;
    grant(&pool, next, atlas, "admin").await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query!(
        "SELECT id FROM users WHERE id = $1 FOR UPDATE",
        target.get()
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let task = tokio::spawn({
        let pool = pool.clone();
        async move {
            bears_db::grant_membership(&pool, target.get(), atlas.as_uuid(), Some("admin")).await
        }
    });
    waiting(&pool, "SELECT id FROM users WHERE id = $1 FOR KEY SHARE").await;
    // NOWAIT proves the waiting membership mutation did not acquire Bear first.
    sqlx::query!(
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE NOWAIT",
        atlas.as_uuid()
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query!("DELETE FROM users WHERE id = $1", target.get())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(DenError::NotFound(_))));
    assert_eq!(role(&pool, target, atlas).await, None);
    assert_eq!(admins(&pool, atlas).await, 1);
}
