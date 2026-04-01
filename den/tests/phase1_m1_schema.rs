//! Verifies Phase 1 migrations: bear registry, provisioning columns, membership, user columns.
//! Requires `DATABASE_URL` and `sqlx migrate run` applied (all `migrations/*.up.sql`).

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

#[tokio::test]
async fn m1b_bears_has_system_prompt_column() {
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
          AND table_name = 'bears'
          AND column_name = 'system_prompt'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");

    assert_eq!(n, 1, "bears missing system_prompt");
}

#[tokio::test]
async fn m1b_letta_agent_id_nullable() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    let nullable: String = sqlx::query_scalar(
        r#"
        SELECT is_nullable
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'bears'
          AND column_name = 'letta_agent_id'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");

    assert_eq!(
        nullable, "YES",
        "letta_agent_id should be nullable until Letta provisions the agent"
    );
}

#[tokio::test]
async fn m1c_bear_templates_table_exists() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM bear_templates")
        .fetch_one(&pool)
        .await
        .expect("bear_templates table queryable");

    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'bears'
          AND column_name = 'source_template_id'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");

    assert_eq!(n, 1, "bears missing source_template_id");
}
