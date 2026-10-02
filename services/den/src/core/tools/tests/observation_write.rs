use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::Config,
    core::{
        tools::{
            arguments::DenToolChannelContext,
            constants::DEN_OBSERVATION_WRITE,
            session::{invoke_den_tool_for_origin, DenToolInvocationContext},
        },
        user::db::create_user,
    },
};
use den_core::{Governance, TurnExecutionOrigin};
use den_memory::MemoryStoreManager;
use den_service::bears::{db, db::grant_membership, db::BearParams, BearProfile};

async fn seed_watch_agent(
    pool: &PgPool,
    bear_id: Uuid,
    agent_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query(
        r"
        INSERT INTO bear_profile_bindings (bear_id, profile, binding_id)
        VALUES ($1, 'watch', $2)
        ON CONFLICT (bear_id, profile)
        DO NOTHING
        ",
    )
    .bind(bear_id)
    .bind(agent_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[sqlx::test]
async fn observation_write_persists_and_enqueues_memory_curate(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "test-observation-write-bear",
            name: "Test Observation Write Bear",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;

    let suffix = Uuid::new_v4().simple().to_string();
    let user_id = create_user(
        &pool,
        &format!("obs-{}@ex.com", &suffix[..8]),
        &format!("obs{}", &suffix[..12]),
        "Observation Tester",
        "test-hash",
    )
    .await?;
    grant_membership(&pool, user_id, bear_id, Some("admin")).await?;

    let agent_id = format!("watch-agent-{}", Uuid::new_v4());
    seed_watch_agent(&pool, bear_id, &agent_id).await?;

    let context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-observation-write-bear".to_string(),
        binding_id: agent_id.clone(),
        profile: Some(BearProfile::Watch),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("owner".to_string()),
        conversation_id: "conv-watch-observation-test".to_string(),
        session_id: "watch-session".to_string(),
        work_run_id: None,
        client_session_id: None,
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: vec![],
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: Some(Uuid::new_v4().to_string()),
        channel: DenToolChannelContext::default(),
    };

    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);

    let payload = invoke_den_tool_for_origin(
        &pool,
        &config,
        &stores,
        DEN_OBSERVATION_WRITE,
        json!({
            "observation_id": "deploy-failure-001",
            "summary": "Deployment pipeline failed on main.",
            "salience": "high"
        }),
        context,
        TurnExecutionOrigin::InboundObservation,
        Governance::AutonomousContinuation,
    )
    .await?;

    assert_eq!(payload["observation_id"], "deploy-failure-001");
    assert_eq!(payload["status"], "review_queued");
    assert!(payload["proposal_id"].is_string());

    let queued = sqlx::query_scalar::<_, i64>(
        r"
        SELECT COUNT(*)::bigint
        FROM bear_reflection_runs
        WHERE bear_id = $1
          AND lane = 'memory_curate'
          AND trigger = 'watch_observation'
        ",
    )
    .bind(bear_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(queued, 1);

    let replay_context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-observation-write-bear".to_string(),
        binding_id: agent_id,
        profile: Some(BearProfile::Watch),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("owner".to_string()),
        conversation_id: "conv-watch-observation-test".to_string(),
        session_id: "watch-session".to_string(),
        work_run_id: None,
        client_session_id: None,
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: vec![],
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: Some(Uuid::new_v4().to_string()),
        channel: DenToolChannelContext::default(),
    };
    let replay = invoke_den_tool_for_origin(
        &pool,
        &config,
        &stores,
        DEN_OBSERVATION_WRITE,
        json!({
            "observation_id": "deploy-failure-001",
            "summary": "Deployment pipeline failed on main.",
            "salience": "high"
        }),
        replay_context,
        TurnExecutionOrigin::InboundObservation,
        Governance::AutonomousContinuation,
    )
    .await?;
    assert_eq!(replay["idempotent_replay"], true);

    Ok(())
}
