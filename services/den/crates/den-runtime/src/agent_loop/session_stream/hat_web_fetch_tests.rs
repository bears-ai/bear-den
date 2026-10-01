use std::sync::Arc;

use super::{tests::test_session, *};
use den_core::ids::{BearId, UserId};
use den_service::{
    bears::{
        db,
        hats::{
            self,
            access::{HatAccessGrant, HttpsHost, ToolActionKey},
        },
    },
    conversation::persistence,
};
use futures::StreamExt;
use sqlx::PgPool;

fn fetch_stream(pool: &PgPool, bear: Uuid, user: i32, conversation: &str) -> SessionTrackingStream {
    let mut session = test_session("hat-web-native:client", bear);
    session.conversation_id = conversation.into();
    session.user_id = Some(user);
    session.client_session_id = "hat-web-client".into();
    let store = AgentLoopSessionStore::default();
    store.insert(session.clone());
    let inner = futures::stream::iter([Ok(RuntimeStreamEvent::Semantic(
        RuntimeSemanticEvent::ToolCallRequested {
            tool_call_id: "hat-web-call".into(),
            tool_name: "web_fetch".into(),
            title: None,
            kind: Some("function".into()),
            arguments: serde_json::json!({"url": "https://example.com/docs"}),
            approval_request_id: None,
            approval_required: true,
            approval_reason: None,
            run_id: None,
        },
    ))]);
    SessionTrackingStream::new(
        Box::pin(inner),
        &session,
        store,
        pool.clone(),
        bear,
        "hat-web-native".into(),
        Some(user),
        conversation.into(),
        "hat-web-client".into(),
        Some(Uuid::new_v4().to_string()),
        Arc::new(Config::test_stub()),
        BearProfile::Pair,
        NativeToolDispatchMode::DeferToClient,
    )
}

#[sqlx::test(migrations = "../../migrations")]
async fn hat_grant_routes_one_native_fetch_without_pause_and_revocation_restores_once(
    pool: PgPool,
) {
    let bear = db::create_bear(
        &pool,
        db::BearParams {
            slug: "nativehatfetch",
            name: "Native hat fetch",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let admin = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('dispatch-hat@example.test', 'dispatchhat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    db::grant_membership(&pool, admin, bear, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Research",
        "Fetch docs",
    )
    .await
    .unwrap();
    let other_hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Other",
        "Other role",
    )
    .await
    .unwrap();
    let own = persistence::ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(admin),
        "hat-web-own",
        None,
        None,
    )
    .await
    .unwrap();
    let other = persistence::ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(admin),
        "hat-web-other",
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear), own.id, hat.id)
        .await
        .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear), other.id, other_hat.id)
        .await
        .unwrap();
    let tool = HatAccessGrant::ToolForHat(ToolActionKey::from_provider_name("web_fetch").unwrap());
    let host = HatAccessGrant::HttpsHost(HttpsHost::parse("example.com").unwrap());
    hats::access::grant(
        &pool,
        BearId::new(bear),
        hat.id,
        UserId::new(admin),
        &tool,
        true,
    )
    .await
    .unwrap();
    let host_id = hats::access::grant(
        &pool,
        BearId::new(bear),
        hat.id,
        UserId::new(admin),
        &host,
        true,
    )
    .await
    .unwrap();
    let mut allowed = fetch_stream(&pool, bear, admin, "hat-web-own");
    let event = allowed.next().await.unwrap().unwrap();
    assert!(matches!(
        event,
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested {
            approval_required: false,
            approval_request_id: None,
            ..
        })
    ));
    assert!(allowed.pending_server_tool.is_some());
    assert!(allowed.pending_approval.is_none());

    let mut other = fetch_stream(&pool, bear, admin, "hat-web-other");
    let event = other.next().await.unwrap().unwrap();
    assert!(matches!(
        event,
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested {
            approval_required: true,
            approval_request_id: Some(_),
            ..
        })
    ));
    assert!(other.pending_server_tool.is_none());
    assert!(other.pending_pause_after_tool.is_some());

    hats::access::revoke(
        &pool,
        BearId::new(bear),
        hat.id,
        UserId::new(admin),
        host_id,
    )
    .await
    .unwrap();
    let mut revoked = fetch_stream(&pool, bear, admin, "hat-web-own");
    let event = revoked.next().await.unwrap().unwrap();
    assert!(matches!(
        event,
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested {
            approval_required: true,
            approval_request_id: Some(_),
            ..
        })
    ));
    assert!(revoked.pending_pause_after_tool.is_some());
    assert!(revoked.pending_server_tool.is_none());
}
