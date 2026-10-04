use super::*;
use den_core::ids::{BearId, UserId};
use den_service::{
    bears::{
        db::{self, BearParams},
        hats, managed_blocks,
    },
    conversation::persistence,
};
use sqlx::{types::Json, PgPool};

fn context<'a>(
    pool: &'a PgPool,
    config: &'a Config,
    stores: &'a MemoryStoreManager,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
    conversation_id: &'a str,
    session_id: Option<&'a str>,
) -> AssembleTurnContext<'a> {
    AssembleTurnContext {
        pool,
        config,
        stores,
        bear_id,
        origin: match profile {
            RuntimeContextLabel::ChannelConversation => {
                den_core::TurnExecutionOrigin::ChannelConversation
            }
            RuntimeContextLabel::ArmatureConversation => {
                den_core::TurnExecutionOrigin::ArmatureConversation(
                    den_core::ArmatureAvailability::Connected,
                )
            }
            RuntimeContextLabel::JobRun => den_core::TurnExecutionOrigin::AuthorizedWorkRun(
                den_core::ArmatureAvailability::Connected,
            ),
            RuntimeContextLabel::Curation => den_core::TurnExecutionOrigin::InternalCuration,
            RuntimeContextLabel::Observation => den_core::TurnExecutionOrigin::InboundObservation,
        },
        governance: den_core::Governance::Interactive,
        conversation_id,
        turn_runtime_context: None,
        human_message: None,
        tool_messages: &[],
        session_id,
        workspace_roots: None,
        runtime_target: None,
        conversation_selection: None,
        user_id: None,
        client_context: None,
        include_prompt_memory: false,
        key_memory_cache: None,
        native_runtime: true,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_turn_uses_verified_hat_identity_without_cross_hat_or_stance_identity(pool: PgPool) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('turnhat@example.test', 'turnhat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear_id = db::create_bear(&pool, BearParams {
        slug: "turnhatbear", name: "Lumen", description: "", system_prompt: "legacy prompt",
        default_model: None, tools_enabled: None,
        context_profile: Some(Json(json!({
            "composition_version": 1,
            "role_contracts": {
                "chat": "OLD CHAT IDENTITY", "pair": "OLD PAIR IDENTITY",
                "curate": "{{ invalid_unused_template", "work": "OLD WORK IDENTITY", "watch": "{{ current_date }}"
            },
            "user_steering": "Shared Bear voice", "bear_context": "Common charter"
        }))),
    }).await.unwrap();
    let bear = db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let _compiled = managed_blocks::compile_and_store_managed_config_for_bear(&pool, &bear)
        .await
        .unwrap();
    let security = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Security",
        "Review secrets",
    )
    .await
    .unwrap();
    let support = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Support",
        "Help customers",
    )
    .await
    .unwrap();
    hats::manage::set_short_summary(
        &pool,
        BearId::new(bear_id),
        security.id,
        Some("Reviews security posture"),
        true,
    )
    .await
    .unwrap();
    hats::manage::set_short_summary(
        &pool,
        BearId::new(bear_id),
        support.id,
        Some("Assists customers"),
        true,
    )
    .await
    .unwrap();
    let conversation_a = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user),
        "conv-turn-security",
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(
        &pool,
        BearId::new(bear_id),
        conversation_a.id,
        security.id,
    )
    .await
    .unwrap();
    let conversation_b = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user),
        "conv-turn-support",
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(
        &pool,
        BearId::new(bear_id),
        conversation_b.id,
        support.id,
    )
    .await
    .unwrap();
    let _unbound = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user),
        "conv-turn-unbound",
        None,
        None,
    )
    .await
    .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("turn-hat-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let mut first_context = context(
        &pool,
        &config,
        &stores,
        bear_id,
        RuntimeContextLabel::ArmatureConversation,
        "conv-turn-security",
        Some("identity-runtime-session"),
    );
    first_context.turn_runtime_context = Some("OPAQUE LEGACY PROMPT MEMORY");
    let first = assemble_native_turn_for_bear(first_context, &bear)
        .await
        .unwrap();
    let same_hat_chat = assemble_native_turn_for_bear(
        context(
            &pool,
            &config,
            &stores,
            bear_id,
            RuntimeContextLabel::ChannelConversation,
            "conv-turn-security",
            None,
        ),
        &bear,
    )
    .await
    .unwrap();
    let second = assemble_native_turn_for_bear(
        context(
            &pool,
            &config,
            &stores,
            bear_id,
            RuntimeContextLabel::ChannelConversation,
            "conv-turn-support",
            None,
        ),
        &bear,
    )
    .await
    .unwrap();
    let first_system = first
        .messages
        .iter()
        .find(|message| message.role == "system")
        .and_then(|message| message.content.as_deref())
        .unwrap();
    let second_system = second
        .messages
        .iter()
        .find(|message| message.role == "system")
        .and_then(|message| message.content.as_deref())
        .unwrap();
    assert!(first_system.contains("Lumen, wearing the Security hat"));
    assert!(first_system.contains("A hat is the Bear's metaphor for a role or responsibility"));
    assert!(first_system.contains("Support: Assists customers"));
    assert!(first_system.contains("Review secrets"));
    assert!(first_system.contains("Shared Bear voice"));
    assert!(first_system.contains("Interactive collaboration mode"));
    assert!(!first_system.contains("OLD PAIR IDENTITY"));
    assert!(!first_system.contains("OPAQUE LEGACY PROMPT MEMORY"));
    assert!(!first_system.contains("Help customers"));
    let same_chat_system = same_hat_chat
        .messages
        .iter()
        .find(|message| message.role == "system")
        .and_then(|message| message.content.as_deref())
        .unwrap();
    assert!(same_chat_system.contains("Lumen, wearing the Security hat"));
    assert!(same_chat_system.contains("Review secrets"));
    assert!(same_chat_system.contains("Conversation mode"));
    assert!(!same_chat_system.contains("Help customers"));
    assert!(second_system.contains("Security: Reviews security posture"));
    assert!(second_system.contains("Help customers"));
    assert!(second_system.contains("Conversation mode"));
    assert!(!second_system.contains("Review secrets"));
    assert!(!second_system.contains("OLD CHAT IDENTITY"));
    assert!(matches!(
        assemble_native_turn_for_bear(
            context(
                &pool,
                &config,
                &stores,
                bear_id,
                RuntimeContextLabel::ArmatureConversation,
                "conv-turn-unbound",
                None
            ),
            &bear,
        )
        .await,
        Err(DenError::Authorization(_))
    ));
    assert!(matches!(
        assemble_native_turn_for_bear(
            context(
                &pool,
                &config,
                &stores,
                bear_id,
                RuntimeContextLabel::ArmatureConversation,
                "conv-turn-missing",
                None
            ),
            &bear,
        )
        .await,
        Err(DenError::Authorization(_))
    ));
    assert!(matches!(
        assemble_native_turn_for_bear(
            context(
                &pool,
                &config,
                &stores,
                bear_id,
                RuntimeContextLabel::JobRun,
                "den-conv-work-without-job",
                Some("missing-work-session"),
            ),
            &bear,
        )
        .await,
        Err(DenError::Authorization(_))
    ));
    let surface = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, 'hat-identity-work', 'git_workspace', $2, now(), now())",
        surface,
        user,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface,
        bear_id,
    )
    .execute(&pool)
    .await
    .unwrap();
    hats::allow_surface(&pool, BearId::new(bear_id), security.id, surface)
        .await
        .unwrap();
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        security.id.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    let job = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, hat_id, created_by_user_id, created_by_role, goal)
         VALUES ($1, $2, $3, 'ui', 'Review code') RETURNING id",
        bear_id,
        security.id.as_uuid(),
        user,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id, work_surface_id) VALUES ($1, $2)",
        job,
        surface,
    )
    .execute(&pool)
    .await
    .unwrap();
    let job_run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs (job_id) VALUES ($1) RETURNING id",
        job,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO bear_work_runs (bear_id, job_id, job_run_id, state, bearwire_session_id)
         VALUES ($1, $2, $3, 'claimed', 'hat-work-session')",
        bear_id,
        job,
        job_run,
    )
    .execute(&pool)
    .await
    .unwrap();
    let work = assemble_native_turn_for_bear(
        context(
            &pool,
            &config,
            &stores,
            bear_id,
            RuntimeContextLabel::JobRun,
            "den-conv-work-identity",
            Some("hat-work-session"),
        ),
        &bear,
    )
    .await
    .unwrap();
    let work_system = work
        .messages
        .iter()
        .find(|message| message.role == "system")
        .and_then(|message| message.content.as_deref())
        .unwrap();
    assert!(work_system.contains("Lumen, wearing the Security hat"));
    assert!(work_system.contains("Authorized Work mode"));
    assert!(work_system.contains("Support: Assists customers"));
    assert!(work_system.contains("Review secrets"));
    assert!(!work_system.contains("Help customers"));
    assert!(!work_system.contains("OLD WORK IDENTITY"));
    for profile in [
        RuntimeContextLabel::ArmatureConversation,
        RuntimeContextLabel::ChannelConversation,
    ] {
        assert!(
            matches!(
                assemble_native_turn_for_bear(
                    context(
                        &pool,
                        &config,
                        &stores,
                        bear_id,
                        profile,
                        "conv-turn-security",
                        Some("hat-work-session"),
                    ),
                    &bear,
                )
                .await,
                Err(DenError::Authorization(_))
            ),
            "{profile:?} cannot read a Work-bound session as an interactive conversation"
        );
    }
    hats::manage::update_hat(
        &pool,
        BearId::new(bear_id),
        security.id,
        "Security",
        "Review updated security risks",
        "Inspect newly scoped secrets",
        true,
    )
    .await
    .unwrap();
    let refreshed = assemble_native_turn_for_bear(
        context(
            &pool,
            &config,
            &stores,
            bear_id,
            RuntimeContextLabel::ArmatureConversation,
            "conv-turn-security",
            None,
        ),
        &bear,
    )
    .await
    .unwrap();
    let refreshed_system = refreshed
        .messages
        .iter()
        .find(|message| message.role == "system")
        .and_then(|message| message.content.as_deref())
        .unwrap();
    assert!(refreshed_system.contains("Inspect newly scoped secrets"));
    assert!(refreshed_system.contains("Shared Bear voice"));
    assert!(refreshed_system.contains("Interactive collaboration mode"));
    assert!(!refreshed_system.contains("OLD PAIR IDENTITY"));
    assert!(!refreshed_system.contains("OPAQUE LEGACY PROMPT MEMORY"));
    assert!(!refreshed_system.contains("Review secrets"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn zero_hat_ordinary_sources_fail_before_assembly_even_without_prompt_memory(pool: PgPool) {
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "zero-hat-assembly",
            name: "No hat",
            description: "",
            system_prompt: "old identity",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let bear = db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        None,
        "conv-unbound-no-hats",
        None,
        None,
    )
    .await
    .unwrap();
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    for profile in [
        RuntimeContextLabel::ArmatureConversation,
        RuntimeContextLabel::ChannelConversation,
        RuntimeContextLabel::JobRun,
        RuntimeContextLabel::Curation,
        RuntimeContextLabel::Observation,
    ] {
        for external in ["conv-unbound-no-hats", "conv-missing-no-hats"] {
            let ctx = context(&pool, &config, &stores, bear_id, profile, external, None);
            assert!(
                matches!(
                    assemble_native_turn_for_bear(ctx, &bear).await,
                    Err(DenError::Authorization(_))
                ),
                "{profile:?}: {external}"
            );
        }
    }
    assert!(hats::list_hats(&pool, BearId::new(bear_id))
        .await
        .unwrap()
        .is_empty());
}
