//! Integration coverage for Den-owned Bear work plans. Requires `DATABASE_URL`.

use den::{
    config::Config,
    core::{
        bears::{db as bears_db, BearAgentRole},
        den_tools::{
            self, DenToolChannelContext, DenToolInvocationContext, DEN_WORK_PLAN_LIST,
            DEN_WORK_PLAN_UPDATE,
        },
        work_plans::{
            self, WorkPlanItem, WorkPlanItemStatus, WorkPlanListFilter, WorkPlanStatus,
            WorkPlanUpdate, WorkPlanUpsert, WorkPlanVisibility,
        },
    },
    startup::run_sqlx_migrations,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn apply_migrations(pool: &sqlx::PgPool) {
    run_sqlx_migrations(pool)
        .await
        .expect("sqlx migrations for integration test");
}

async fn create_test_user(pool: &sqlx::PgPool) -> i32 {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("u{}", &suffix[..20]);
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
    .bind(format!("Work Plan Test {username}"))
    .bind("unused-in-work-plan-tests")
    .fetch_one(pool)
    .await
    .expect("insert test user");
    user_id
}

async fn create_test_bear(pool: &sqlx::PgPool) -> Uuid {
    let suffix = Uuid::new_v4().simple().to_string();
    bears_db::create_bear(
        pool,
        &format!("work-plan-test-{}", &suffix[..12]),
        "Work Plan Test Bear",
        "Work plan integration test bear",
        "",
        None,
        None,
        None,
        sqlx::types::Json(Vec::<String>::new()),
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

fn den_context(bear_id: Uuid, user_id: i32, role_agent_id: &str) -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id,
        bear_slug: "work-plan-test".to_string(),
        role_agent_id: role_agent_id.to_string(),
        agent_role: Some(BearAgentRole::Pair),
        user_id,
        username: Some("work-plan-user".to_string()),
        membership_role: Some(bears_db::BEAR_ROLE_ADMIN.to_string()),
        conversation_id: "conv-den-tool-work-plan".to_string(),
        session_id: "session-den-tool-work-plan".to_string(),
        acp_session_id: Some("session-den-tool-work-plan".to_string()),
        conversation_selection: Some("conv-den-tool-work-plan".to_string()),
        runtime_target: Some("conv-den-tool-work-plan".to_string()),
        workspace_roots: Vec::new(),
        session_policy: None,
        activity: None,
        request_id: Some(Uuid::new_v4().to_string()),
        channel: DenToolChannelContext {
            family: Some("acp".to_string()),
            client: Some("zed".to_string()),
            protocol: Some("acp".to_string()),
        },
    }
}

fn item(id: &str, status: WorkPlanItemStatus) -> WorkPlanItem {
    WorkPlanItem {
        id: id.to_string(),
        title: format!("Item {id}"),
        summary: None,
        status,
        blocked_reason: None,
        source_refs: Vec::new(),
    }
}

fn update(title: &str, visibility: WorkPlanVisibility) -> WorkPlanUpdate {
    WorkPlanUpdate {
        title: title.to_string(),
        summary: "Current plan".to_string(),
        visibility,
        status: WorkPlanStatus::Active,
        items: vec![item("one", WorkPlanItemStatus::InProgress)],
        workspace_context: json!({ "redacted": true }),
    }
}

#[tokio::test]
async fn work_plan_crud_writes_events_and_enforces_visibility() {
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

    let created = work_plans::create_or_update_work_plan(
        &pool,
        WorkPlanUpsert {
            bear_id,
            owner_role: BearAgentRole::Pair,
            owner_agent_id: Some("agent-pair-work-plan-test".to_string()),
            created_by_user_id: Some(user_id),
            source_conversation_id: Some("conv-work-plan-test".to_string()),
            source_acp_session_id: Some("session-work-plan-test".to_string()),
            source_channel: json!({ "protocol": "acp" }),
            plan_id: None,
            expected_version: None,
            update: update("Private pair plan", WorkPlanVisibility::PrivateToRole),
        },
    )
    .await
    .expect("create work plan");
    assert_eq!(created.version, 1);

    let pair_plans = work_plans::list_visible_work_plans(
        &pool,
        bear_id,
        BearAgentRole::Pair,
        user_id,
        WorkPlanListFilter::default(),
    )
    .await
    .expect("list pair-visible plans");
    assert_eq!(pair_plans.len(), 1);
    assert_eq!(pair_plans[0].id, created.id);
    assert!(pair_plans[0].current_item.is_some());

    let talk_plans = work_plans::list_visible_work_plans(
        &pool,
        bear_id,
        BearAgentRole::Talk,
        user_id,
        WorkPlanListFilter::default(),
    )
    .await
    .expect("list talk-visible plans");
    assert!(talk_plans.is_empty());

    let updated = work_plans::create_or_update_work_plan(
        &pool,
        WorkPlanUpsert {
            bear_id,
            owner_role: BearAgentRole::Pair,
            owner_agent_id: Some("agent-pair-work-plan-test".to_string()),
            created_by_user_id: Some(user_id),
            source_conversation_id: Some("conv-work-plan-test".to_string()),
            source_acp_session_id: Some("session-work-plan-test".to_string()),
            source_channel: json!({ "protocol": "acp" }),
            plan_id: Some(created.id),
            expected_version: Some(created.version),
            update: update("Visible pair plan", WorkPlanVisibility::BearVisible),
        },
    )
    .await
    .expect("update work plan");
    assert_eq!(updated.version, 2);

    let talk_plans = work_plans::list_visible_work_plans(
        &pool,
        bear_id,
        BearAgentRole::Talk,
        user_id,
        WorkPlanListFilter::default(),
    )
    .await
    .expect("list talk-visible plans after visibility update");
    assert_eq!(talk_plans.len(), 1);
    assert_eq!(talk_plans[0].title, "Visible pair plan");

    let event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM bear_work_plan_events WHERE plan_id = $1")
            .bind(created.id)
            .fetch_one(&pool)
            .await
            .expect("count work plan events");
    assert_eq!(event_count, 2);
}

#[tokio::test]
async fn work_plan_den_tools_update_and_list_current_role_plans() {
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
        "agent-pair-den-tool-plan",
    )
    .await;

    let config = Config::load();
    let update_result = den_tools::invoke_den_tool(
        &pool,
        &config,
        DEN_WORK_PLAN_UPDATE,
        json!({
            "title": "Pair implementation plan",
            "summary": "Track the active coding plan",
            "visibility": "bear_visible",
            "status": "active",
            "items": [{
                "id": "implement",
                "title": "Implement CRUD",
                "status": "in_progress"
            }],
            "workspace_context": { "redacted": true }
        }),
        den_context(bear_id, user_id, "agent-pair-den-tool-plan"),
    )
    .await
    .expect("update work plan through Den tool");
    assert_eq!(update_result["plan"]["title"], "Pair implementation plan");
    assert_eq!(update_result["plan"]["version"], 1);

    let list_result = den_tools::invoke_den_tool(
        &pool,
        &config,
        DEN_WORK_PLAN_LIST,
        json!({}),
        den_context(bear_id, user_id, "agent-pair-den-tool-plan"),
    )
    .await
    .expect("list work plans through Den tool");
    let plans = list_result["plans"].as_array().expect("plans array");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0]["title"], "Pair implementation plan");
    assert!(plans[0].get("workspace_context").is_none());
}
