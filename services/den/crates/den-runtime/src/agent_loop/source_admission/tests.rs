use super::*;
use crate::{
    agent_loop::{
        agent_loop_session_key, resolve_agent_loop_control, run_agent_step_stream,
        AgentLoopControlResolutionInput, AgentLoopSessionStore, AgentStepOverflowContext,
        FreeformPolicy, ObjectiveOrientation, StrategyProfile, ToolCallBudgetLimits,
        TurnBudgetPolicy,
    },
    llm::{ChatMessage, LlmClient},
};
use den_core::{config::Config, ids::HatId, ArmatureAvailability};
use den_service::{
    bears::{db, hats},
    client_sessions::{self, UpsertClientSession},
    conversation::persistence,
};
use futures::StreamExt;
use std::sync::Arc;

pub(crate) fn test_session(
    bear_id: Uuid,
    user_id: i32,
    conversation: &str,
    client: &str,
    run: &str,
) -> AgentLoopSession {
    AgentLoopSession {
        session_key: agent_loop_session_key(conversation, client, run),
        bear_id,
        bear_slug: "source-admission-bear".into(),
        user_id: Some(user_id),
        conversation_id: conversation.into(),
        client_session_id: client.into(),
        work_run_id: None,
        checkpoint_audit_context: None,
        workspace_roots: vec![],
        session_capabilities: vec![],
        recently_discovered_capabilities: vec![],
        request_id: Some("original-request".into()),
        run_id: Some(run.into()),
        technical_budget_recovery_start_payload: None,
        messages: vec![],
        tools: vec![],
        budget_components: Default::default(),
        model: "openai/gpt-4.1".into(),
        model_request_profile: den_core::ModelRequestProfile {
            approved_model_ref: "openai/gpt-4.1".into(),
            ..Default::default()
        },
        model_context_window: None,
        model_max_output_tokens: None,
        model_token_calibration: None,
        bifrost_virtual_key: None,
        api_style: None,
        step: 0,
        turn_budget: TurnBudgetPolicy {
            max_wall_clock_ms: 60_000,
            emergency_hard_steps: 8,
            tool_call_limits: ToolCallBudgetLimits {
                total: 8,
                read: 8,
                search: 8,
                fetch: 8,
                execute: 8,
                write: 8,
                destructive: 8,
                other: 8,
            },
            max_consecutive_tool_failures: 2,
            max_same_tool_signature_repeats: 2,
            post_mutation_verification_window: None,
        },
        turn_budget_state: Default::default(),
        agent_loop_control: resolve_agent_loop_control(AgentLoopControlResolutionInput {
            model_handle: Some("openai/test"),
            model_default: None,
            bear_override: None,
            task_escalation: None,
            origin: den_core::TurnExecutionOrigin::ArmatureConversation(
                den_core::ArmatureAvailability::Connected,
            ),
            governance: den_core::Governance::Interactive,
            objective_orientation: None,
            pre_risk: false,
        })
        .expect("ordinary test origin"),
        governance: den_core::Governance::Interactive,
        objective_orientation: ObjectiveOrientation::Freeform {
            policy: FreeformPolicy::closed(),
        },
        checkpoint_state: Default::default(),
        pending_checkpoint_request: None,
        pending_checkpoint_task_action: None,
        pending_checkpoint_recovery_attempts: 0,
        strategy: StrategyProfile::plain_react(),
        stream_tokens: false,
        key_memory_projection_cache_key: None,
        latest_context_budget: None,
        latest_projected_memory: None,
        latest_recalled_memory: None,
        cached_activity_plan_projection: None,
        origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        profile: RuntimeContextLabel::ArmatureConversation,
        overflow_retry_attempted: false,
        overflow_compaction_recovered: false,
    }
}

pub(crate) async fn admit_existing_session(
    pool: &PgPool,
    session: &AgentLoopSession,
) -> (Uuid, HatId) {
    let user = session.user_id.unwrap();
    db::grant_membership(pool, user, session.bear_id, Some(db::BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let conversation = persistence::ensure_conversation_for_external_id(
        pool,
        session.bear_id,
        Some(user),
        &session.conversation_id,
        None,
        None,
    )
    .await
    .unwrap();
    let hat = hats::create_hat(
        pool,
        session.bear_id.into(),
        user.into(),
        "Live hat",
        "Keep the live source context",
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(pool, session.bear_id.into(), conversation.id, hat.id)
        .await
        .unwrap();
    bind_client(pool, session, &session.conversation_id).await;
    (conversation.id, hat.id)
}

pub(crate) async fn bind_client(pool: &PgPool, session: &AgentLoopSession, target: &str) {
    client_sessions::upsert_session(
        pool,
        UpsertClientSession {
            user_id: session.user_id.unwrap(),
            bear_id: session.bear_id,
            bear_slug: session.bear_slug.clone(),
            client_session_id: session.client_session_id.clone(),
            runtime_session_id: "source-runtime".into(),
            conversation_id: target.into(),
            resolved_conversation_id: None,
            client: "bear-armature".into(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .unwrap();
}

pub(crate) async fn fixture(pool: &PgPool) -> (AgentLoopSession, Uuid, HatId) {
    let bear = db::create_bear(
        pool,
        db::BearParams {
            slug: "source-admission-bear",
            name: "Source admission Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('turnhat@example.test', 'turnhat') RETURNING id"
    ).fetch_one(pool).await.unwrap();
    let session = test_session(
        bear,
        user,
        &format!("den-conv-{}", Uuid::new_v4().simple()),
        &format!("source-client-{}", Uuid::new_v4().simple()),
        "source-run",
    );
    let (conversation, hat) = admit_existing_session(pool, &session).await;
    (session, conversation, hat)
}

pub(crate) fn step_context(pool: &PgPool, session: &AgentLoopSession) -> AgentStepOverflowContext {
    let store = AgentLoopSessionStore::default();
    store.insert(session.clone());
    AgentStepOverflowContext {
        pool: pool.clone(),
        config: Arc::new(Config::test_stub()),
        profile: session.profile,
        session_store: store,
    }
}

pub(crate) async fn work_fixture(pool: &PgPool) -> (AgentLoopSession, HatId) {
    let (mut session, _, hat) = fixture(pool).await;
    let surface = sqlx::query_scalar!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, $2, 'git_workspace', $3, now(), now()) RETURNING id",
        Uuid::new_v4(),
        "source-work-surface",
        session.user_id.unwrap(),
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface,
        session.bear_id,
    )
    .execute(pool)
    .await
    .unwrap();
    hats::allow_surface(pool, session.bear_id.into(), hat, surface)
        .await
        .unwrap();
    // Only the fixture simulates the existing Work review gate; no runtime bypass.
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        hat.as_uuid()
    )
    .execute(pool)
    .await
    .unwrap();
    let job = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, created_by_user_id, created_by_role, goal)
         VALUES ($1, $2, 'ui', 'Review the repository') RETURNING id",
        session.bear_id,
        session.user_id.unwrap(),
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id, work_surface_id) VALUES ($1, $2)",
        job,
        surface,
    )
    .execute(pool)
    .await
    .unwrap();
    hats::bindings::bind_job_hat(pool, session.bear_id.into(), job, hat)
        .await
        .unwrap();
    let job_run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs (job_id) VALUES ($1) RETURNING id",
        job
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let work =
        sqlx::query_scalar!(
        "INSERT INTO bear_work_runs (bear_id, job_id, job_run_id) VALUES ($1, $2, $3) RETURNING id",
        session.bear_id, job, job_run,
    )
        .fetch_one(pool)
        .await
        .unwrap();
    let claimed =
        work_runs::claim_next_work_run(pool, "source-runner", std::time::Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(claimed.id, work);
    work_runs::bind_work_run_session(pool, work, session.bear_id, &session.client_session_id)
        .await
        .unwrap();
    session.origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected);
    session.profile = RuntimeContextLabel::JobRun;
    session.work_run_id = Some(work);
    (session, hat)
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_next_step_rechecks_exact_run_and_hat_eligibility(pool: PgPool) {
    let (session, hat) = work_fixture(&pool).await;
    let llm = LlmClient::new(&Config::test_stub());
    assert!(
        run_agent_step_stream(&llm, &session, Some(step_context(&pool, &session)))
            .await
            .is_ok()
    );
    let mut unattended = session.clone();
    unattended.user_id = None;
    assert!(require_ordinary_session_source(&pool, (&unattended).into())
        .await
        .is_ok());
    let actor = session.user_id.unwrap();
    let mut foreign_actor = session.clone();
    foreign_actor.user_id = Some(actor + 1);
    assert!(matches!(
        require_ordinary_session_source(&pool, (&foreign_actor).into()).await,
        Err(DenError::Authorization(_)),
    ));
    db::revoke_membership(&pool, actor, session.bear_id)
        .await
        .unwrap();
    for source in [&session, &unattended] {
        assert!(matches!(
            require_ordinary_session_source(&pool, source.into()).await,
            Err(DenError::Authorization(_)),
        ));
    }
    db::grant_membership(&pool, actor, session.bear_id, Some(db::BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let mut replay = session.clone();
    replay.work_run_id = Some(Uuid::new_v4());
    assert!(matches!(
        run_agent_step_stream(&llm, &replay, Some(step_context(&pool, &replay))).await,
        Err(DenError::Authorization(_))
    ));
    hats::manage::disable_work(&pool, session.bear_id.into(), hat)
        .await
        .unwrap();
    assert!(matches!(
        run_agent_step_stream(&llm, &session, Some(step_context(&pool, &session))).await,
        Err(DenError::Authorization(_))
    ));
}

#[tokio::test]
async fn standalone_budget_stop_needs_no_source_but_inference_does() {
    let config = Config::test_stub();
    let llm = LlmClient::new(&config);
    let mut session = test_session(Uuid::new_v4(), 1, "synthetic", "synthetic", "synthetic");
    assert!(matches!(
        run_agent_step_stream(&llm, &session, None).await,
        Err(DenError::Authorization(_))
    ));
    // A registered model's catalog window takes precedence over this fallback.
    // This synthetic source-free stop must neither resolve a model nor infer.
    session.model = "budget-fixture/unregistered".into();
    session
        .model_request_profile
        .approved_model_ref
        .clone_from(&session.model);
    session.model_context_window = Some(1);
    session.messages.push(ChatMessage {
        role: "user".into(),
        content: Some("large context".repeat(1000)),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    });
    assert!(matches!(
        run_agent_step_stream(&llm, &session, None).await,
        Err(DenError::ValidationError(_))
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_ordinary_next_step_requires_current_membership_and_owned_hat(pool: PgPool) {
    let (session, canonical, _) = fixture(&pool).await;
    let llm = LlmClient::new(&Config::test_stub());
    for origin in [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::BrowserTaskSession,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
    ] {
        let mut next = session.clone();
        next.origin = origin;
        next.profile =
            den_core::EffectivePolicy::compile_for_origin(origin, next.governance).context_label;
        // Constructing a stream is still lazy: no upstream request is made here.
        assert!(
            run_agent_step_stream(&llm, &next, Some(step_context(&pool, &next)))
                .await
                .is_ok()
        );
        let mut foreign_owner = next.clone();
        foreign_owner.user_id = Some(session.user_id.unwrap() + 1);
        assert!(run_agent_step_stream(
            &llm,
            &foreign_owner,
            Some(step_context(&pool, &foreign_owner))
        )
        .await
        .is_err());
    }
    db::revoke_membership(&pool, session.user_id.unwrap(), session.bear_id)
        .await
        .unwrap();
    for origin in [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::BrowserTaskSession,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
    ] {
        let mut next = session.clone();
        next.origin = origin;
        next.profile =
            den_core::EffectivePolicy::compile_for_origin(origin, next.governance).context_label;
        let context = step_context(&pool, &next);
        assert!(run_agent_step_stream(&llm, &next, Some(context.clone()))
            .await
            .is_err());
        assert!(context
            .session_store
            .get(&next.session_key)
            .unwrap()
            .latest_context_budget
            .is_none());
    }
    assert!(persistence::list_messages_page(&pool, canonical, None, 100)
        .await
        .unwrap()
        .is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn lazy_inference_rechecks_after_construction_and_internal_origins_never_infer(pool: PgPool) {
    let (session, canonical, _) = fixture(&pool).await;
    let llm = LlmClient::new(&Config::test_stub());
    let mut stream = run_agent_step_stream(&llm, &session, Some(step_context(&pool, &session)))
        .await
        .unwrap();
    client_sessions::mark_closed(
        &pool,
        client_sessions::find_for_user_bear_session_id(
            &pool,
            session.user_id.unwrap(),
            session.bear_id,
            &session.client_session_id,
        )
        .await
        .unwrap()
        .unwrap()
        .id,
    )
    .await
    .unwrap();
    let mut denied = false;
    while let Some(event) = stream.next().await {
        if let Err(error) = event {
            assert!(matches!(error, DenError::Authorization(_)));
            denied = true;
        }
    }
    assert!(denied);
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        let mut internal = session.clone();
        internal.origin = origin;
        internal.profile =
            den_core::EffectivePolicy::compile_for_origin(origin, internal.governance)
                .context_label;
        assert!(matches!(
            run_agent_step_stream(&llm, &internal, Some(step_context(&pool, &internal))).await,
            Err(DenError::Authorization(_))
        ));
    }
    assert!(persistence::list_messages_page(&pool, canonical, None, 100)
        .await
        .unwrap()
        .is_empty());
}
