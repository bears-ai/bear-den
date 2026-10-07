use super::{publish_open_metadata, publish_run_metadata};
use crate::methods::run::source_preflight::{admit, helpers::Fixture};
use den_http::errors::CustomError;
use den_service::{
    bears::hats::{self, bindings, turn_binding::NativeTurnSource},
    client_sessions::{self, UpsertClientSession},
    conversation::{persistence, viewer::ConversationViewer},
};
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
async fn stale_pending_alias_cannot_overwrite_latest_canonical_source(pool: sqlx::PgPool) {
    let fixture = Fixture::new(&pool).await;
    let hat = hats::ide_default_hat(&pool, fixture.bear)
        .await
        .unwrap()
        .unwrap();
    let mut sources = Vec::new();
    for _ in 0..2 {
        let source = persistence::ensure_conversation_for_external_id(
            &pool,
            fixture.bear.as_uuid(),
            Some(fixture.user.get()),
            &format!("den-conv-{}", Uuid::new_v4().simple()),
            None,
            None,
        )
        .await
        .unwrap();
        bindings::bind_conversation_hat(&pool, fixture.bear, source.id, hat)
            .await
            .unwrap();
        sources.push(source);
    }
    let old = &sources[0];
    let winner = &sources[1];
    let pending = format!("new-cas-{}", Uuid::new_v4());
    let session_id = format!("cas-{}", Uuid::new_v4());
    client_sessions::upsert_session(
        &pool,
        UpsertClientSession {
            user_id: fixture.user.get(),
            bear_id: fixture.bear.as_uuid(),
            bear_slug: fixture.slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id: "original-runtime".into(),
            conversation_id: pending.clone(),
            resolved_conversation_id: old.external_conversation_id.clone(),
            client: "original-client".into(),
            cwd: Some("/original".into()),
            current_mode: None,
        },
    )
    .await
    .unwrap();
    // Simulate a newer canonical publication without changing the stored alias P.
    client_sessions::mark_resolved(
        &pool,
        fixture.user.get(),
        fixture.bear.as_uuid(),
        &session_id,
        winner.external_conversation_id.as_deref().unwrap(),
    )
    .await
    .unwrap();
    let before = client_sessions::find_for_user_bear_session_id(
        &pool,
        fixture.user.get(),
        fixture.bear.as_uuid(),
        &session_id,
    )
    .await
    .unwrap()
    .unwrap();
    let stale = || UpsertClientSession {
        user_id: fixture.user.get(),
        bear_id: fixture.bear.as_uuid(),
        bear_slug: fixture.slug.clone(),
        client_session_id: session_id.clone(),
        runtime_session_id: "stale-runtime".into(),
        conversation_id: pending.clone(),
        resolved_conversation_id: old.external_conversation_id.clone(),
        client: "stale-client".into(),
        cwd: Some("/stale".into()),
        current_mode: None,
    };
    for result in [
        publish_run_metadata(&pool, stale()).await,
        publish_open_metadata(&pool, stale()).await,
    ] {
        assert!(matches!(result,
            Err(CustomError::Authorization(message)) if message == "admitted canonical session conversation changed"));
    }
    let after = client_sessions::find_for_user_bear_session_id(
        &pool,
        fixture.user.get(),
        fixture.bear.as_uuid(),
        &session_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(after.conversation_id, before.conversation_id);
    assert_eq!(
        after.resolved_conversation_id,
        before.resolved_conversation_id
    );
    assert_eq!(after.runtime_session_id, before.runtime_session_id);
    assert_eq!(after.client, before.client);
    assert_eq!(after.cwd, before.cwd);
    assert_eq!(after.updated_at, before.updated_at);
    let viewer = ConversationViewer::resolve(&pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    let next = admit(
        &pool,
        &viewer,
        fixture.bear,
        fixture.user,
        &session_id,
        &pending,
        None,
    )
    .await
    .unwrap();
    assert_eq!(next.conversation.id, winner.id);
    assert_eq!(next.turn_source, NativeTurnSource::Conversation(winner.id));
}
