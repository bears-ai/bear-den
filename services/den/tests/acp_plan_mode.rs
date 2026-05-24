//! Integration coverage for ACP pair plan mode. Requires `DATABASE_URL`.

use den::{
    core::{
        acp_plan_mode::{self, AcpPlanModeRequestedBy, EnterPlanModeParams, SubmitPlanModeParams},
        bears::{db as bears_db, db::BearParams, BearAgentRole},
    },
    startup::run_sqlx_migrations,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn apply_migrations(pool: &sqlx::PgPool) {
    run_sqlx_migrations(pool)
        .await
        .expect("sqlx migrations for ACP plan mode test");
}

async fn create_test_user(pool: &sqlx::PgPool) -> i32 {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("pm{}", &suffix[..18]);
    let email = format!("{username}@example.test");
    let (user_id,): (i32,) = sqlx::query_as(
        r#"
        INSERT INTO users (email, username, display_name, passhash)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(email)
    .bind(&username)
    .bind(format!("Plan Mode Test {username}"))
    .bind("unused-in-plan-mode-tests")
    .fetch_one(pool)
    .await
    .expect("insert test user");
    user_id
}

async fn create_test_bear(pool: &sqlx::PgPool) -> Uuid {
    let suffix = Uuid::new_v4().simple().to_string();
    bears_db::create_bear(
        pool,
        BearParams {
            slug: &format!("plan-mode-test-{}", &suffix[..12]),
            name: "Plan Mode Test Bear",
            description: "ACP plan mode integration test bear",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            letta_agent_type: None,
            letta_tool_ids: sqlx::types::Json(Vec::<String>::new()),
            context_profile: None,
        },
    )
    .await
    .expect("create test bear")
}

async fn insert_role_agent(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    role: BearAgentRole,
    agent_id: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO bear_agents (bear_id, role, letta_agent_id, provisioning_status, last_synced_at)
        VALUES ($1, $2, $3, 'ready', NOW())
        ON CONFLICT (bear_id, role)
        DO UPDATE SET letta_agent_id = EXCLUDED.letta_agent_id,
                      provisioning_status = 'ready',
                      last_synced_at = NOW(),
                      updated_at = NOW()
        "#,
    )
    .bind(bear_id)
    .bind(role.as_str())
    .bind(agent_id)
    .execute(pool)
    .await
    .expect("insert role agent");
}

#[tokio::test]
async fn plan_mode_lifecycle_records_artifact_and_approval() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL for integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect postgres");

    apply_migrations(&pool).await;
    let user_id = create_test_user(&pool).await;
    let bear_id = create_test_bear(&pool).await;
    bears_db::grant_membership(&pool, user_id, bear_id, Some(bears_db::BEAR_ROLE_ADMIN))
        .await
        .expect("grant bear membership");
    insert_role_agent(
        &pool,
        bear_id,
        BearAgentRole::Pair,
        "agent-pair-plan-mode-test",
    )
    .await;

    let entered = acp_plan_mode::enter_plan_mode(
        &pool,
        EnterPlanModeParams {
            user_id,
            bear_id,
            bear_slug: "plan-mode-test".to_string(),
            acp_session_id: "acp-plan-mode-session".to_string(),
            reason: "Need to inspect before editing".to_string(),
            requested_by: AcpPlanModeRequestedBy::Pair,
            previous_permission_mode: Some("default".to_string()),
        },
    )
    .await
    .expect("enter plan mode");
    assert_eq!(entered.state, "active");

    let submitted = acp_plan_mode::submit_plan_artifact(
        &pool,
        SubmitPlanModeParams {
            user_id,
            bear_id,
            acp_session_id: "acp-plan-mode-session".to_string(),
            plan_mode_id: Some(entered.id),
            title: "Implementation plan".to_string(),
            body: "1. Read files\n2. Edit code\n3. Test".to_string(),
            artifact_path: "pair/plans/mem_test.md".to_string(),
            approval_request_id: Some("approval-1".to_string()),
        },
    )
    .await
    .expect("submit plan artifact");
    assert_eq!(submitted.state, "submitted");
    assert_eq!(
        submitted.plan_artifact_path.as_deref(),
        Some("pair/plans/mem_test.md")
    );

    let approved = acp_plan_mode::approve_plan_mode(
        &pool,
        user_id,
        bear_id,
        "acp-plan-mode-session",
        entered.id,
    )
    .await
    .expect("approve plan mode");
    assert_eq!(approved.state, "approved");
    assert!(approved.closed_at.is_some());

    let active =
        acp_plan_mode::active_for_session(&pool, user_id, bear_id, "acp-plan-mode-session")
            .await
            .expect("query active plan mode");
    assert!(active.is_none());

    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM acp_plan_mode_events WHERE plan_mode_id = $1",
    )
    .bind(entered.id)
    .fetch_one(&pool)
    .await
    .expect("count plan mode events");
    assert!(event_count >= 4);
}
