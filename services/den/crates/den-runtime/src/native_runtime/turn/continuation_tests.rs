use super::*;
use den_core::ArmatureAvailability;
use den_service::{
    bears::db::{self, BearParams},
    client_sessions::{self, UpsertClientSession},
    conversation::persistence::ensure_conversation_for_external_id,
};

fn binding(id: String) -> RoleRuntimeBinding {
    RoleRuntimeBinding {
        binding_id: id,
        compatibility_backend: Some("native".into()),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn approved_tool_continuation_requires_the_same_owned_live_conversation(pool: PgPool) {
    let bear = db::create_bear(
        &pool,
        BearParams {
            slug: "continuationsourcebear",
            name: "Continuation source Bear",
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
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    db::grant_membership(&pool, user, bear, Some(db::BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let external = format!("den-conv-{}", Uuid::new_v4().simple());
    let other_external = format!("den-conv-{}", Uuid::new_v4().simple());
    let conversation =
        ensure_conversation_for_external_id(&pool, bear, Some(user), &external, None, None)
            .await
            .unwrap();
    ensure_conversation_for_external_id(&pool, bear, Some(user), &other_external, None, None)
        .await
        .unwrap();
    let client_id = format!("client-{}", Uuid::new_v4().simple());
    let upsert = |target: &str| UpsertClientSession {
        user_id: user,
        bear_id: bear,
        bear_slug: "continuationsourcebear".into(),
        client_session_id: client_id.clone(),
        runtime_session_id: "runtime-test".into(),
        conversation_id: target.into(),
        resolved_conversation_id: None,
        client: "bear-armature".into(),
        cwd: None,
        current_mode: None,
    };
    client_sessions::upsert_session(&pool, upsert(&external))
        .await
        .unwrap();
    assert!(db::profile_binding_id(&pool, bear, BearProfile::Pair)
        .await
        .unwrap()
        .is_none());
    let origin =
        den_core::TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let source = || ContinuationSource {
        bear_id: bear,
        user_id: Some(user),
        origin,
        profile: BearProfile::Pair,
        conversation_id: &external,
        client_session_id: &client_id,
        work_run_id: None,
    };
    let expected =
        binding(NativeTurnSource::Conversation(conversation.id).binding_id(BearId::new(bear)));
    require_continuation_binding(&pool, source(), &expected)
        .await
        .unwrap();
    for wrong in [
        NativeTurnSource::Conversation(Uuid::new_v4()).binding_id(BearId::new(bear)),
        NativeTurnSource::WorkRun(conversation.id).binding_id(BearId::new(bear)),
        NativeTurnSource::Conversation(conversation.id).binding_id(BearId::new(Uuid::new_v4())),
    ] {
        assert!(matches!(
            require_continuation_binding(&pool, source(), &binding(wrong)).await,
            Err(DenError::Authorization(_))
        ));
    }
    assert!(require_continuation_binding(
        &pool,
        ContinuationSource {
            user_id: Some(user + 1),
            ..source()
        },
        &expected,
    )
    .await
    .is_err());
    assert!(require_continuation_binding(
        &pool,
        ContinuationSource {
            bear_id: Uuid::new_v4(),
            ..source()
        },
        &expected,
    )
    .await
    .is_err());
    assert!(require_continuation_binding(
        &pool,
        ContinuationSource {
            profile: BearProfile::Work,
            ..source()
        },
        &expected,
    )
    .await
    .is_err());
    client_sessions::upsert_session(&pool, upsert(&other_external))
        .await
        .unwrap();
    assert!(require_continuation_binding(&pool, source(), &expected)
        .await
        .is_err());
    client_sessions::upsert_session(&pool, upsert(&external))
        .await
        .unwrap();
    let row = client_sessions::find_for_user_bear_session_id(&pool, user, bear, &client_id)
        .await
        .unwrap()
        .unwrap();
    client_sessions::mark_closed(&pool, row.id).await.unwrap();
    assert!(require_continuation_binding(&pool, source(), &expected)
        .await
        .is_err());
    db::revoke_membership(&pool, user, bear).await.unwrap();
    assert!(require_continuation_binding(&pool, source(), &expected)
        .await
        .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_continuation_needs_exact_live_run_and_internal_lanes_keep_registration(pool: PgPool) {
    let bear = db::create_bear(
        &pool,
        BearParams {
            slug: "continuationinternalbear",
            name: "Continuation internal Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let work_id = Uuid::new_v4();
    let work = ContinuationSource {
        bear_id: bear,
        user_id: None,
        origin: den_core::TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        profile: BearProfile::Work,
        conversation_id: "unused",
        client_session_id: "unbound-work-session",
        work_run_id: Some(work_id),
    };
    assert!(require_continuation_binding(
        &pool,
        work,
        &binding(NativeTurnSource::WorkRun(work_id).binding_id(BearId::new(bear))),
    )
    .await
    .is_err());
    assert!(require_continuation_binding(
        &pool,
        ContinuationSource {
            work_run_id: None,
            ..work
        },
        &binding(NativeTurnSource::WorkRun(work_id).binding_id(BearId::new(bear))),
    )
    .await
    .is_err());
    let internal = ContinuationSource {
        bear_id: bear,
        user_id: None,
        origin: den_core::TurnExecutionOrigin::InternalCuration,
        profile: BearProfile::Curate,
        conversation_id: "unused",
        client_session_id: "unused",
        work_run_id: None,
    };
    let guessed = binding(format!("den-native:{bear}:pair"));
    assert!(require_continuation_binding(&pool, internal, &guessed)
        .await
        .is_err());
    db::ensure_bear_profile_binding_rows(&pool, bear)
        .await
        .unwrap();
    let registered = db::profile_binding_id(&pool, bear, BearProfile::Curate)
        .await
        .unwrap()
        .unwrap();
    require_continuation_binding(&pool, internal, &binding(registered))
        .await
        .unwrap();
    let watch = ContinuationSource {
        origin: den_core::TurnExecutionOrigin::InboundObservation,
        profile: BearProfile::Watch,
        ..internal
    };
    let registered = db::profile_binding_id(&pool, bear, BearProfile::Watch)
        .await
        .unwrap()
        .unwrap();
    require_continuation_binding(&pool, watch, &binding(registered))
        .await
        .unwrap();
    assert!(require_continuation_binding(&pool, watch, &guessed)
        .await
        .is_err());
}
