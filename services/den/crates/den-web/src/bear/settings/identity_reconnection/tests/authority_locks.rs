use super::super::authority;
use super::*;
use std::time::Duration;

#[sqlx::test(migrations = "../../migrations")]
async fn authority_locks_freeze_users_verification_and_membership_until_release(pool: PgPool) {
    let fixture = fixture(pool).await;
    let locked = authority::lock(
        &fixture.pool,
        fixture.bear_id,
        fixture.actor,
        Some(fixture.destination),
    )
    .await
    .unwrap();
    let deletion_lock = {
        let pool = fixture.pool.clone();
        let user = fixture.destination;
        tokio::spawn(async move {
            sqlx::query_scalar!("SELECT id FROM users WHERE id = $1 FOR UPDATE", user.get())
                .fetch_one(&pool)
                .await
                .unwrap()
        })
    };
    let verification = {
        let pool = fixture.pool.clone();
        let user = fixture.destination;
        tokio::spawn(async move {
            den_http::user::email_settings::set_admin_email_verified(&pool, user.get(), false)
                .await
                .unwrap();
        })
    };
    let membership = {
        let pool = fixture.pool.clone();
        let user = fixture.destination;
        let bear = fixture.bear_id;
        tokio::spawn(async move {
            bears_db::revoke_membership(&pool, user.get(), bear.as_uuid())
                .await
                .unwrap();
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!deletion_lock.is_finished());
    assert!(!verification.is_finished());
    assert!(!membership.is_finished());
    drop(locked);
    tokio::time::timeout(Duration::from_secs(5), async {
        deletion_lock.await.unwrap();
        verification.await.unwrap();
        membership.await.unwrap();
    })
    .await
    .unwrap();
    assert!(authority::lock(
        &fixture.pool,
        fixture.bear_id,
        fixture.actor,
        Some(fixture.destination)
    )
    .await
    .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn cancelled_http_confirmation_keeps_authority_locked_until_sqlite_commit(pool: PgPool) {
    let fixture = fixture(pool).await;
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let form = fixture.reviewed_form(&location, false).await;
    let store = fixture.store().await;
    den_memory::test_support::inject_history_fixture(&store, false, 100)
        .await
        .unwrap();
    let writer = den_memory::test_support::lock_identity_test_store(&store)
        .await
        .unwrap();
    let http = {
        let app = fixture.app.clone();
        let cookie = fixture.cookie.clone();
        tokio::spawn(async move {
            request(&app, &cookie, "POST", &format!("{LIST}/confirm"), &form).await
        })
    };
    // Detect the real authority fence rather than assuming a sleep acquired it.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let probe = sqlx::query_scalar!(
                "SELECT id FROM users WHERE id = $1 FOR UPDATE NOWAIT",
                fixture.actor.get()
            )
            .fetch_one(&fixture.pool)
            .await;
            if let Err(sqlx::Error::Database(error)) = probe {
                if error.code().as_deref() == Some("55P03") {
                    break;
                }
                panic!("unexpected authority probe error: {error}");
            }
            assert!(
                !http.is_finished(),
                "confirmation must wait for the SQLite writer"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    http.abort();
    let mut verification = {
        let pool = fixture.pool.clone();
        let user = fixture.destination;
        tokio::spawn(async move {
            den_http::user::email_settings::set_admin_email_verified(&pool, user.get(), false)
                .await
                .unwrap();
        })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut verification)
            .await
            .is_err()
    );
    writer.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), verification)
        .await
        .unwrap()
        .unwrap();
    assert!(core::list_identities(&store)
        .await
        .unwrap()
        .iter()
        .any(|identity| identity.entity_id == fixture.alice
            && identity.active_user_id == Some(fixture.destination)));
    let recovered_history = core::receipt_history(&store, 100).await.unwrap();
    assert_eq!(recovered_history.receipts.len(), 100);
    assert_eq!(recovered_history.unreadable_entries, 0);
    let recovered = recovered_history.receipts.first().unwrap();
    assert_eq!(recovered.entity_id, fixture.alice);
    assert_eq!(recovered.recorded_bear_id, fixture.bear_id.to_string());
    let (status, page, _) = request(&fixture.app, &fixture.cookie, "GET", LIST, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let history = page.split_once("id=\"identity-receipts\"").unwrap().1;
    assert!(history.contains(&recovered.receipt_id.to_string()));
    assert!(history.contains(&recovered.recorded_bear_id));
    assert!(
        history.find(&recovered.receipt_id.to_string()).unwrap()
            < history.find("source-bear").unwrap()
    );
    assert!(history.contains("Imported receipts are informational historical claims"));
    assert!(history.contains("not authentication or permission grants"));
}
