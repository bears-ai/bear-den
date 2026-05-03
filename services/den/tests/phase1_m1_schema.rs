//! Verifies Phase 1 migrations: bear registry, provisioning columns, membership, user columns.
//! Requires `DATABASE_URL` (empty database is fine — migrations run here like production).

use den::startup::run_sqlx_migrations;
use sqlx::postgres::PgPoolOptions;

async fn apply_migrations(pool: &sqlx::PgPool) {
    run_sqlx_migrations(pool)
        .await
        .expect("sqlx migrations for integration test");
}

#[tokio::test]
async fn m1_bears_and_membership_tables_exist() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    apply_migrations(&pool).await;

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

    apply_migrations(&pool).await;

    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'users'
          AND column_name = 'is_admin'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");

    assert_eq!(n, 1, "users missing is_admin");

    let webui_cols: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'users'
          AND column_name = 'webui_account_id'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");

    assert_eq!(
        webui_cols, 0,
        "users.webui_account_id should be removed (see 20260418130000 migration)"
    );
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

    apply_migrations(&pool).await;

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

    apply_migrations(&pool).await;

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
async fn m1c_bears_letta_sync_columns_exist() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    apply_migrations(&pool).await;

    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'bears'
          AND column_name IN ('letta_agent_type', 'letta_tool_ids')
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");

    assert_eq!(n, 2, "bears missing letta_agent_type or letta_tool_ids");
}

#[tokio::test]
async fn multi_agent_tables_columns_and_role_constraints_exist() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    apply_migrations(&pool).await;

    for table in [
        "bear_agents",
        "bear_skills_manifest",
        "bear_skill_proposals",
        "bear_subscriptions",
    ] {
        let n: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)::bigint
            FROM information_schema.tables
            WHERE table_schema = 'public'
              AND table_name = $1
            "#,
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("information_schema query");
        assert_eq!(n, 1, "missing table {table}");
    }

    let bear_cols: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'bears'
          AND column_name IN ('memfs_repo_path', 'provisioning_version')
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");
    assert_eq!(bear_cols, 2, "bears missing multi-agent columns");

    let role_check: String = sqlx::query_scalar(
        r#"
        SELECT pg_get_constraintdef(c.oid)
        FROM pg_constraint c
        INNER JOIN pg_class t ON t.oid = c.conrelid
        WHERE t.relname = 'bear_agents'
          AND c.contype = 'c'
          AND pg_get_constraintdef(c.oid) LIKE '%watch%'
        LIMIT 1
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("bear_agents role check");
    assert!(role_check.contains("talk"));
    assert!(role_check.contains("watch"));
}

#[tokio::test]
async fn m1d_bears_runtime_plan_column_exists() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    apply_migrations(&pool).await;

    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'bears'
          AND column_name = 'runtime_plan'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query");

    assert_eq!(n, 1, "bears missing runtime_plan");
}
