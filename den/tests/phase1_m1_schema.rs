//! Verifies Phase 1 M1 migrations: `bears`, `user_bear`, `audit_chat`, user columns.
//! Requires `DATABASE_URL` and `sqlx migrate run` applied.

use sqlx::postgres::PgPoolOptions;

#[tokio::test]
async fn m1_bears_and_membership_tables_exist() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM bears")
        .fetch_one(&pool)
        .await
        .expect("bears table queryable");

    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM user_bear")
        .fetch_one(&pool)
        .await
        .expect("user_bear table queryable");

    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_chat")
        .fetch_one(&pool)
        .await
        .expect("audit_chat table queryable");
}

#[tokio::test]
async fn m1_users_extended_columns_exist() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'users'
          AND column_name IN ('webui_account_id', 'is_admin')
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");

    assert_eq!(n, 2, "users missing webui_account_id or is_admin");
}
