use super::*;
use den_core::{
    tools::{
        arguments::DenToolChannelContext,
        constants::{DEN_RUN_WRITE_RESULT, DEN_WEB_FETCH},
        context::DenToolInvocationContext,
    },
    ArmatureAvailability, BearProfile, Governance, TurnExecutionOrigin,
};
use uuid::Uuid;

fn context(profile: BearProfile) -> DenToolInvocationContext {
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

#[test]
fn direct_invoker_cannot_widen_a_pair_origin_with_a_curate_or_work_profile() {
    let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let pair = EffectivePolicy::compile_for_origin(origin, Governance::Interactive);
    assert!(require_origin_policy_and_descriptor(
        &context(BearProfile::Pair),
        &pair,
        origin,
        DEN_WEB_FETCH
    )
    .is_ok());
    assert!(require_origin_policy_and_descriptor(
        &context(BearProfile::Pair),
        &pair,
        origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_err());
    for forged in [
        BearProfile::Curate,
        BearProfile::Work,
        BearProfile::Watch,
        BearProfile::Chat,
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
    let mut work_context = context(BearProfile::Work);
    work_context.work_run_id = Some(Uuid::new_v4());
    assert!(require_origin_policy_and_descriptor(
        &work_context,
        &work,
        work_origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_ok());
    let mut forged_work = work_context;
    forged_work.profile = Some(BearProfile::Pair);
    assert!(require_origin_policy_and_descriptor(
        &forged_work,
        &work,
        work_origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_err());
    assert!(
        require_origin_policy_and_descriptor(
            &context(BearProfile::Pair),
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
        &context(BearProfile::Chat),
        &chat,
        chat_origin,
        DEN_WEB_FETCH
    )
    .is_err());
}

#[test]
fn direct_invoker_requires_work_run_binding_only_for_work_origin() {
    let work_origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let work = EffectivePolicy::compile_for_origin(work_origin, Governance::Interactive);
    assert!(matches!(
        require_origin_policy_and_descriptor(
            &context(BearProfile::Work),
            &work,
            work_origin,
            DEN_RUN_WRITE_RESULT,
        ),
        Err(DenError::Authorization(_))
    ));

    let pair_origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let pair = EffectivePolicy::compile_for_origin(pair_origin, Governance::Interactive);
    let mut forged = context(BearProfile::Pair);
    forged.work_run_id = Some(Uuid::new_v4());
    assert!(matches!(
        require_origin_policy_and_descriptor(&forged, &pair, pair_origin, DEN_WEB_FETCH),
        Err(DenError::Authorization(_))
    ));
}

#[sqlx::test]
async fn work_tool_rechecks_the_live_run_at_effect_time(pool: PgPool) -> Result<(), DenError> {
    let work_origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let mut work = context(BearProfile::Work);
    work.work_run_id = Some(Uuid::new_v4());
    assert!(matches!(
        require_live_work_tool_source(&pool, &work, work_origin).await,
        Err(DenError::Authorization(_))
    ));
    let pair_origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    assert!(
        require_live_work_tool_source(&pool, &context(BearProfile::Pair), pair_origin)
            .await
            .is_ok()
    );
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
    let mut pair = context(BearProfile::Pair);
    pair.bear_id = bear_id;
    pair.user_id = user_id;
    let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    assert!(matches!(
        require_current_tool_actor(&pool, &pair, origin).await,
        Err(DenError::Authorization(_))
    ));
    grant_membership(&pool, user_id, bear_id, Some("member")).await?;
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
    let mut call = context(BearProfile::Pair);
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
        Err(DenError::Authorization(_))
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
    assert!(profile_binding_id(&pool, bear_id, BearProfile::Pair)
        .await?
        .is_none());
    let mut call = context(BearProfile::Pair);
    call.bear_id = bear_id;
    call.user_id = user_id;
    call.binding_id = format!("den-native:{bear_id}:pair");
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
    )
    .await?;
    assert_eq!(self_view["bear"]["bear_id"], bear_id.to_string());
    assert!(matches!(
        den_core::tools::identity::context_role(&ctx, &call).await,
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
