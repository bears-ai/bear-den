use super::*;
use den_core::{
    tools::{
        arguments::DenToolChannelContext,
        constants::{DEN_RUN_WRITE_RESULT, DEN_WEB_FETCH},
        context::DenToolInvocationContext,
    },
    ArmatureAvailability, Governance, RuntimeContextLabel, TurnExecutionOrigin,
};
use uuid::Uuid;

fn context(profile: RuntimeContextLabel) -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: Uuid::nil(),
        bear_slug: "test".into(),
        binding_id: "test".into(),
        profile: Some(profile),
        user_id: 1,
        username: None,
        membership_role: None,
        conversation_id: "conv".into(),
        session_id: "session".into(),
        work_run_id: None,
        client_session_id: None,
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: Vec::new(),
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: None,
        channel: DenToolChannelContext::default(),
    }
}

#[tokio::test]
async fn internal_dispatch_denies_before_argument_preflight_or_storage() {
    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let config = crate::config::Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let ctx = DenToolContext::new(&pool, &config, &stores);
    for (origin, profile) in [
        (
            TurnExecutionOrigin::InternalCuration,
            RuntimeContextLabel::Curation,
        ),
        (
            TurnExecutionOrigin::InboundObservation,
            RuntimeContextLabel::Observation,
        ),
    ] {
        let arguments = serde_json::json!({
            "kind": "note", "title": "Plan concepts",
            "body": "High-level understanding of the architecture: how plan artifacts differ from live progress tracking and why the distinction matters for durable memory."
        });
        let tool = den_core::tools::constants::DEN_MEMORY_WRITE_ENTRY;
        let core = den_core::tools::dispatch::invoke_den_tool_for_origin(
            &ctx,
            tool,
            arguments.clone(),
            context(profile),
            origin,
            Governance::Interactive,
        )
        .await;
        assert!(matches!(core, Err(DenError::Authorization(_))));
        let session = invoke_den_tool_for_origin(
            &pool,
            &config,
            &stores,
            tool,
            arguments,
            context(profile),
            origin,
            Governance::Interactive,
        )
        .await;
        assert!(matches!(session, Err(CustomError::Authorization(_))));
    }
}

#[test]
fn direct_invoker_cannot_widen_a_pair_origin_with_a_curate_or_work_profile() {
    let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let pair = EffectivePolicy::compile_for_origin(origin, Governance::Interactive);
    assert!(require_origin_policy_and_descriptor(
        &context(RuntimeContextLabel::ArmatureConversation),
        &pair,
        origin,
        DEN_WEB_FETCH
    )
    .is_ok());
    assert!(require_origin_policy_and_descriptor(
        &context(RuntimeContextLabel::ArmatureConversation),
        &pair,
        origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_err());
    for forged in [
        RuntimeContextLabel::Curation,
        RuntimeContextLabel::JobRun,
        RuntimeContextLabel::Observation,
        RuntimeContextLabel::ChannelConversation,
    ] {
        assert!(
            matches!(
                require_origin_policy_and_descriptor(
                    &context(forged),
                    &pair,
                    origin,
                    DEN_WEB_FETCH
                ),
                Err(DenError::Authorization(_))
            ),
            "{forged:?}"
        );
    }
    let work_origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let work = EffectivePolicy::compile_for_origin(work_origin, Governance::Interactive);
    let mut work_context = context(RuntimeContextLabel::JobRun);
    work_context.work_run_id = Some(Uuid::new_v4());
    assert!(require_origin_policy_and_descriptor(
        &work_context,
        &work,
        work_origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_ok());
    let mut forged_work = work_context;
    forged_work.profile = Some(RuntimeContextLabel::ArmatureConversation);
    assert!(require_origin_policy_and_descriptor(
        &forged_work,
        &work,
        work_origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_err());
    assert!(
        require_origin_policy_and_descriptor(
            &context(RuntimeContextLabel::ArmatureConversation),
            &work,
            origin,
            DEN_WEB_FETCH
        )
        .is_err(),
        "a Work policy cannot claim an interactive origin"
    );
    let chat_origin = TurnExecutionOrigin::ChannelConversation;
    let chat = EffectivePolicy::compile_for_origin(chat_origin, Governance::Interactive);
    assert!(require_origin_policy_and_descriptor(
        &context(RuntimeContextLabel::ChannelConversation),
        &chat,
        chat_origin,
        DEN_WEB_FETCH
    )
    .is_err());
}

#[tokio::test]
async fn internal_tool_origins_are_denied_before_database_or_descriptor_lookup() {
    use std::sync::Arc;

    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let config = Arc::new(crate::config::Config::test_stub());
    let stores = den_memory::MemoryStoreManager::new(&config);
    let state = DenState::new(
        pool.clone(),
        config.clone(),
        Arc::new(den_service::bifrost::BifrostClient::new(&config)),
        stores.clone(),
    );
    let invoker = DenRuntimeToolInvoker::new(state);
    let ctx = DenToolContext::new(&pool, &config, &stores);
    for (profile, origin) in [
        (
            RuntimeContextLabel::Curation,
            TurnExecutionOrigin::InternalCuration,
        ),
        (
            RuntimeContextLabel::Observation,
            TurnExecutionOrigin::InboundObservation,
        ),
    ] {
        for tool_name in [DEN_WEB_FETCH, "unknown_den_tool"] {
            let call = context(profile);
            let policy =
                EffectivePolicy::compile_for_origin(origin, Governance::AutonomousContinuation);
            assert!(matches!(
                invoker
                    .invoke(RuntimeToolInvocation {
                        tool_name: tool_name.into(),
                        arguments: serde_json::json!({}),
                        context: call.clone(),
                        origin,
                        effective_policy: policy,
                        origin_run_id: None,
                        tool_call_id: den_runtime::turn_ids::ToolCallId::new("internal-denied")
                            .unwrap(),
                    })
                    .await,
                Err(DenError::Authorization(_))
            ));
            assert!(matches!(
                den_core::tools::dispatch::authorize_den_tool_for_origin(
                    &ctx, tool_name, &call, origin
                )
                .await,
                Err(DenError::Authorization(_))
            ));
        }
    }
}

#[test]
fn direct_invoker_requires_work_run_binding_only_for_work_origin() {
    let work_origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let work = EffectivePolicy::compile_for_origin(work_origin, Governance::Interactive);
    assert!(matches!(
        require_origin_policy_and_descriptor(
            &context(RuntimeContextLabel::JobRun),
            &work,
            work_origin,
            DEN_RUN_WRITE_RESULT,
        ),
        Err(DenError::Authorization(_))
    ));

    let pair_origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let pair = EffectivePolicy::compile_for_origin(pair_origin, Governance::Interactive);
    let mut forged = context(RuntimeContextLabel::ArmatureConversation);
    forged.work_run_id = Some(Uuid::new_v4());
    assert!(matches!(
        require_origin_policy_and_descriptor(&forged, &pair, pair_origin, DEN_WEB_FETCH),
        Err(DenError::Authorization(_))
    ));
}

#[sqlx::test]
async fn work_tool_rechecks_the_live_run_at_effect_time(pool: PgPool) -> Result<(), DenError> {
    let work_origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let mut work = context(RuntimeContextLabel::JobRun);
    work.work_run_id = Some(Uuid::new_v4());
    assert!(matches!(
        require_live_work_tool_source(&pool, &work, work_origin).await,
        Err(DenError::Authorization(_))
    ));
    let pair_origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    assert!(require_live_work_tool_source(
        &pool,
        &context(RuntimeContextLabel::ArmatureConversation),
        pair_origin
    )
    .await
    .is_ok());
    Ok(())
}

#[sqlx::test]
async fn ordinary_tool_actor_loses_access_immediately_after_membership_revocation(
    pool: PgPool,
) -> Result<(), DenError> {
    use den_service::bears::db::{create_bear, grant_membership, revoke_membership, BearParams};

    let bear_id = create_bear(
        &pool,
        BearParams {
            slug: "invoker-member-test",
            name: "Invoker membership test",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;
    let user_id = crate::core::user::db::create_user(
        &pool,
        "invoker-member@example.test",
        "invokermember",
        "Invoker test",
        "test-hash",
    )
    .await?;
    let mut pair = context(RuntimeContextLabel::ArmatureConversation);
    pair.bear_id = bear_id;
    pair.user_id = user_id;
    let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    assert!(matches!(
        require_current_tool_actor(&pool, &pair, origin).await,
        Err(DenError::Authorization(_))
    ));
    grant_membership(&pool, user_id, bear_id, Some("member")).await?;
    crate::core::tools::tests::source_fixture::admit_tool_source(&pool, &mut pair).await?;
    require_current_tool_actor(&pool, &pair, origin).await?;
    revoke_membership(&pool, user_id, bear_id).await?;
    assert!(matches!(
        require_current_tool_actor(&pool, &pair, origin).await,
        Err(DenError::Authorization(_))
    ));
    Ok(())
}

#[sqlx::test]
async fn hat_bound_tool_source_requires_current_conversation_owner(
    pool: PgPool,
) -> Result<(), DenError> {
    use den_core::ids::{BearId, UserId};
    use den_service::{
        bears::{
            db::{create_bear, grant_membership, BearParams},
            hats::{bindings::bind_conversation_hat, create_hat},
        },
        conversation::persistence::ensure_conversation_for_external_id,
    };

    let bear_id = create_bear(
        &pool,
        BearParams {
            slug: "invoker-source-test",
            name: "Invoker source test",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;
    let owner = crate::core::user::db::create_user(
        &pool,
        "sourceowner@example.test",
        "sourceowner",
        "Source owner",
        "test-hash",
    )
    .await?;
    let other = crate::core::user::db::create_user(
        &pool,
        "sourceother@example.test",
        "sourceother",
        "Other member",
        "test-hash",
    )
    .await?;
    grant_membership(&pool, owner, bear_id, Some("admin")).await?;
    grant_membership(&pool, other, bear_id, Some("member")).await?;
    let hat = create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(owner),
        "Review",
        "Review sources",
    )
    .await?;
    let conversation = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(owner),
        "source-owner-conv",
        Some("source-owner-session"),
        None,
    )
    .await?;
    bind_conversation_hat(&pool, BearId::new(bear_id), conversation.id, hat.id).await?;

    let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let mut call = context(RuntimeContextLabel::ArmatureConversation);
    call.bear_id = bear_id;
    call.conversation_id = "source-owner-conv".into();
    call.user_id = owner;
    require_current_tool_actor(&pool, &call, origin).await?;
    call.user_id = other;
    assert!(matches!(
        require_current_tool_actor(&pool, &call, origin).await,
        Err(DenError::Authorization(_))
    ));
    call.conversation_id = "invented-conversation".into();
    assert!(matches!(
        require_current_tool_actor(&pool, &call, origin).await,
        Err(DenError::NotFound(_))
    ));
    Ok(())
}

#[sqlx::test]
async fn native_core_dispatcher_uses_origin_audience_at_effect_time(
    pool: PgPool,
) -> Result<(), DenError> {
    use den_core::tools::{constants::DEN_BEAR_GET_SELF, dispatch};
    use den_service::bears::db::{create_bear, grant_membership, profile_binding_id, BearParams};

    let bear_id = create_bear(
        &pool,
        BearParams {
            slug: "typed-dispatch-test",
            name: "Typed dispatcher test",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;
    let user_id = crate::core::user::db::create_user(
        &pool,
        "typeddispatch@example.test",
        "typeddispatch",
        "Typed dispatcher test",
        "test-hash",
    )
    .await?;
    grant_membership(&pool, user_id, bear_id, Some("member")).await?;
    assert!(
        profile_binding_id(&pool, bear_id, RuntimeContextLabel::ArmatureConversation)
            .await?
            .is_none()
    );
    let mut call = context(RuntimeContextLabel::ArmatureConversation);
    call.bear_id = bear_id;
    call.user_id = user_id;
    let hat = den_service::bears::hats::create_hat(
        &pool,
        bear_id.into(),
        user_id.into(),
        "Direct dispatch",
        "Authorize direct tools",
    )
    .await?;
    let conversation = den_service::conversation::persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &call.conversation_id,
        Some(&call.session_id),
        None,
    )
    .await?;
    den_service::bears::hats::bindings::bind_conversation_hat(
        &pool,
        bear_id.into(),
        conversation.id,
        hat.id,
    )
    .await?;
    den_service::client_sessions::upsert_session(
        &pool,
        den_service::client_sessions::UpsertClientSession {
            user_id,
            bear_id,
            bear_slug: call.bear_slug.clone(),
            client_session_id: call.session_id.clone(),
            runtime_session_id: "native-direct".into(),
            conversation_id: call.conversation_id.clone(),
            resolved_conversation_id: None,
            client: "bear-armature".into(),
            cwd: None,
            current_mode: None,
        },
    )
    .await?;
    call.binding_id =
        den_service::bears::hats::turn_binding::NativeTurnSource::Conversation(conversation.id)
            .binding_id(bear_id.into());
    let config = crate::config::Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let ctx = DenToolContext::new(&pool, &config, &stores);
    let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let self_view = dispatch::invoke_den_tool_for_origin(
        &ctx,
        DEN_BEAR_GET_SELF,
        serde_json::json!({}),
        call.clone(),
        origin,
        den_core::Governance::Interactive,
    )
    .await?;
    assert_eq!(self_view["bear"]["bear_id"], bear_id.to_string());
    for wrong_binding in [
        format!("den-native:{bear_id}:pair"),
        den_service::bears::hats::turn_binding::NativeTurnSource::Conversation(Uuid::new_v4())
            .binding_id(bear_id.into()),
        den_service::bears::hats::turn_binding::NativeTurnSource::WorkRun(conversation.id)
            .binding_id(bear_id.into()),
    ] {
        let mut forged = call.clone();
        forged.binding_id = wrong_binding;
        assert!(matches!(
            dispatch::authorize_den_tool_for_origin(&ctx, DEN_BEAR_GET_SELF, &forged, origin).await,
            Err(DenError::Authorization(_)),
        ));
    }
    let mut missing = call.clone();
    missing.conversation_id = "missing-direct-source".into();
    assert!(matches!(
        dispatch::authorize_den_tool_for_origin(&ctx, DEN_BEAR_GET_SELF, &missing, origin).await,
        Err(DenError::Authorization(_)),
    ));
    den_service::bears::db::ensure_bear_profile_binding_rows(&pool, bear_id).await?;
    for (profile, internal_origin) in [
        (
            RuntimeContextLabel::Curation,
            TurnExecutionOrigin::InternalCuration,
        ),
        (
            RuntimeContextLabel::Observation,
            TurnExecutionOrigin::InboundObservation,
        ),
    ] {
        let mut internal_call = call.clone();
        internal_call.profile = Some(profile);
        internal_call.binding_id = profile_binding_id(&pool, bear_id, profile).await?.unwrap();
        assert!(matches!(
            dispatch::authorize_den_tool_for_origin(
                &ctx,
                DEN_BEAR_GET_SELF,
                &internal_call,
                internal_origin
            )
            .await,
            Err(DenError::Authorization(_)),
        ));
    }
    let mut other_source = call.clone();
    other_source.conversation_id = "another-owned-source".into();
    other_source.session_id = "another-owned-client".into();
    other_source.client_session_id = Some(other_source.session_id.clone());
    crate::core::tools::tests::source_fixture::admit_tool_source(&pool, &mut other_source).await?;
    dispatch::authorize_den_tool_for_origin(&ctx, DEN_BEAR_GET_SELF, &other_source, origin).await?;
    other_source.binding_id = call.binding_id.clone();
    assert!(matches!(
        dispatch::authorize_den_tool_for_origin(&ctx, DEN_BEAR_GET_SELF, &other_source, origin)
            .await,
        Err(DenError::Authorization(_)),
    ));
    let mut forged_client = call.clone();
    forged_client.client_session_id = Some("another-owned-client".into());
    assert!(matches!(
        dispatch::authorize_den_tool_for_origin(&ctx, DEN_BEAR_GET_SELF, &forged_client, origin)
            .await,
        Err(DenError::Authorization(_)),
    ));
    assert!(matches!(
        dispatch::authorize_den_tool_for_origin(&ctx, DEN_RUN_WRITE_RESULT, &call, origin).await,
        Err(DenError::Authorization(_))
    ));
    assert!(matches!(
        dispatch::authorize_den_tool_for_origin(
            &ctx,
            DEN_WEB_FETCH,
            &call,
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        )
        .await,
        Err(DenError::Authorization(_))
    ));
    Ok(())
}
