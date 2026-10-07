#[path = "history_tests.rs"]
mod history_tests;
#[path = "inference_fixture.rs"]
mod inference_fixture;
#[path = "pending_state_tests.rs"]
mod pending_state_tests;
#[path = "publication_race_tests.rs"]
mod publication_race_tests;

use super::*;
use den_service::{
    bears::hats::{self, bindings},
    conversation::persistence,
};
use inference_fixture::InferenceFixture;

fn assert_access(session: &Value, state: &str, may_select_hat: bool) {
    assert_eq!(
        session["access"],
        json!({"state": state, "may_select_hat": may_select_hat}),
        "{session}"
    );
}

fn assert_authorization_error(response: &Value, method: &str, reason: &str) {
    assert_eq!(response["error"]["code"], -32001, "{response}");
    assert_eq!(
        response["error"]["message"],
        format!("BearWire {method} failed"),
        "{response}"
    );
    assert_eq!(
        response["error"]["data"]["error"],
        format!("Authorization Error: {reason}"),
        "{response}"
    );
    assert!(response.get("result").is_none(), "{response}");
}

async fn conversations(pool: &sqlx::PgPool, bear: Uuid) -> Vec<persistence::ConversationRecord> {
    persistence::list_conversations_for_bear(pool, bear, 100)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn pending_inspection_reconnect_and_invalid_selection_never_materialize(pool: sqlx::PgPool) {
    let user = create_test_user(&pool).await;
    let (bear, slug) = create_test_bear_without_hats(&pool).await;
    let token = create_token_for_bear(&pool, user, bear).await;
    let session_id = format!("pending-{}", Uuid::new_v4());
    let params = json!({"bear_slug": slug, "session_id": session_id});
    let state = test_state(pool.clone());
    let opened = rpc_value(state.clone(), &token, "session.open", params.clone()).await;
    assert_eq!(opened["result"]["ok"], true, "{opened}");
    assert_access(&opened["result"]["session"], "awaiting_hat", true);
    assert!(opened["result"]["session"]["history_conversation_id"].is_null());
    assert!(opened["result"]["session"]["resolved_conversation_id"].is_null());
    let pending_id = opened["result"]["session"]["conversation_id"]
        .as_str()
        .unwrap();
    assert!(pending_id.starts_with("new-"));
    let persisted = client_sessions::find_for_user_bear_session_id(&pool, user, bear, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.user_id, user);
    assert_eq!(persisted.conversation_id, pending_id);
    assert!(conversations(&pool, bear).await.is_empty());

    for method in ["session.state", "hats.list", "session.model.get"] {
        let inspected = rpc_value(state.clone(), &token, method, params.clone()).await;
        assert!(inspected.get("error").is_none(), "{method}: {inspected}");
        assert!(conversations(&pool, bear).await.is_empty());
        if method == "session.state" {
            assert_access(&inspected["result"]["session"], "awaiting_hat", true);
            assert!(inspected["result"]["session"]["history_conversation_id"].is_null());
        } else if method == "hats.list" {
            assert!(inspected["result"]["selected_hat_id"].is_null());
        } else {
            assert!(inspected["result"]["conversation_id"].is_null());
        }
    }
    let list = rpc_value(
        state.clone(),
        &token,
        "session.state",
        json!({"bear_slug": slug}),
    )
    .await;
    assert_access(&list["result"]["sessions"][0], "awaiting_hat", true);
    for method in ["session.model.set", "session.compact", "run.start"] {
        let denied = rpc_value(state.clone(), &token, method,
            json!({"bear_slug": slug, "session_id": session_id, "selection_mode": "auto", "prompt": "rejected prompt"})).await;
        assert!(denied.get("error").is_some(), "{method}: {denied}");
        assert!(conversations(&pool, bear).await.is_empty());
    }
    let (foreign_bear, _) = create_test_bear_without_hats(&pool).await;
    let foreign = hats::create_hat(
        &pool,
        BearId::new(foreign_bear),
        UserId::new(user),
        "Foreign",
        "Foreign identity",
    )
    .await
    .unwrap();
    for hat_id in [
        "invalid".to_string(),
        Uuid::new_v4().to_string(),
        foreign.id.to_string(),
    ] {
        let denied = rpc_value(
            state.clone(),
            &token,
            "session.hat.select",
            json!({"bear_slug": slug, "session_id": session_id, "hat_id": hat_id}),
        )
        .await;
        assert!(denied.get("error").is_some(), "{denied}");
        assert!(conversations(&pool, bear).await.is_empty());
    }
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "Editor",
        "Editor identity",
    )
    .await
    .unwrap();
    hats::set_ide_default_hat(&pool, BearId::new(bear), hat.id)
        .await
        .unwrap();
    // Even a newly configured default cannot promote a reconnecting pending session.
    let reopened = rpc_value(
        test_state(pool.clone()),
        &token,
        "session.open",
        params.clone(),
    )
    .await;
    assert_access(&reopened["result"]["session"], "awaiting_hat", true);
    assert_eq!(reopened["result"]["session"]["conversation_id"], pending_id);
    assert!(conversations(&pool, bear).await.is_empty());
    let chosen = rpc_value(
        state.clone(),
        &token,
        "session.hat.select",
        json!({"bear_slug": slug, "session_id": session_id, "hat_id": hat.id}),
    )
    .await;
    assert_eq!(chosen["result"]["ok"], true, "{chosen}");
    assert_access(&chosen["result"]["session"], "executable", true);
    let canonical = conversations(&pool, bear).await;
    assert_eq!(canonical.len(), 1);
    let external = chosen["result"]["conversation_id"].as_str().unwrap();
    assert_eq!(
        canonical[0].external_conversation_id.as_deref(),
        Some(external)
    );
    assert_eq!(
        chosen["result"]["session"]["history_conversation_id"],
        external
    );
    assert_eq!(
        bindings::conversation_hat(&pool, BearId::new(bear), canonical[0].id)
            .await
            .unwrap(),
        Some(hat.id)
    );
    let viewer = den_service::conversation::viewer::ConversationViewer::resolve(
        &pool,
        BearId::new(bear),
        UserId::new(user),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(viewer
        .may_read_own_source(&pool, canonical[0].id)
        .await
        .unwrap());
    let reconnect = rpc_value(test_state(pool.clone()), &token, "session.open", params).await;
    assert_eq!(
        reconnect["result"]["session"]["history_conversation_id"],
        external
    );
    assert_eq!(conversations(&pool, bear).await.len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn explicit_selection_after_rejected_prompt_recovers_one_canonical_source(
    pool: sqlx::PgPool,
) {
    let user = create_test_user(&pool).await;
    let (bear, slug) = create_test_bear_without_hats(&pool).await;
    let token = create_token_for_bear(&pool, user, bear).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "Editor",
        "Editor identity",
    )
    .await
    .unwrap();
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".into();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".into();
    seed_test_bifrost_virtual_key(&pool, bear, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("recovery-{}", Uuid::new_v4());
    let open = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({"bear_slug": slug, "session_id": session_id}),
    )
    .await;
    assert_access(&open["result"]["session"], "awaiting_hat", true);
    let rejected = rpc_value(
        state.clone(),
        &token,
        "run.start",
        json!({"bear_slug": slug, "session_id": session_id, "prompt": "must not persist"}),
    )
    .await;
    assert!(rejected.get("error").is_some(), "{rejected}");
    assert!(conversations(&pool, bear).await.is_empty());
    let selection = json!({"bear_slug": slug, "session_id": session_id, "hat_id": hat.id});
    let (first, second) = tokio::join!(
        rpc_value(
            state.clone(),
            &token,
            "session.hat.select",
            selection.clone()
        ),
        rpc_value(state.clone(), &token, "session.hat.select", selection),
    );
    assert!(
        first["result"]["ok"] == true || second["result"]["ok"] == true,
        "{first}; {second}"
    );
    let records = conversations(&pool, bear).await;
    assert_eq!(records.len(), 1);
    let external = records[0].external_conversation_id.as_deref().unwrap();
    let started = rpc_value(
        state.clone(),
        &token,
        "run.start",
        json!({"bear_slug": slug, "session_id": session_id, "prompt": "admitted prompt"}),
    )
    .await;
    assert_eq!(started["result"]["ok"], true, "{started}");
    wait_for_completed_run(&pool, started["result"]["run_id"].as_str().unwrap()).await;
    wait_for_user_message(&pool, bear, external, "admitted prompt").await;
    assert_eq!(
        wait_for_resolved_conversation_id(&pool, user, &slug, &session_id).await,
        external
    );
    assert_eq!(conversations(&pool, bear).await.len(), 1);
    let messages = persistence::list_messages_page(&pool, records[0].id, None, 100)
        .await
        .unwrap();
    assert!(!messages
        .iter()
        .any(|m| m.content_text.contains("must not persist")));
    let current = rpc_value(
        state,
        &token,
        "session.state",
        json!({"bear_slug": slug, "session_id": session_id}),
    )
    .await;
    assert_access(&current["result"]["session"], "executable", false);
}

#[sqlx::test(migrations = "../../migrations")]
async fn reconnect_cannot_substitute_a_canonical_history_or_work_source(pool: sqlx::PgPool) {
    let user = create_test_user(&pool).await;
    let (bear, slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user, bear).await;
    let state = test_state(pool.clone());
    let session_id = format!("source-{}", Uuid::new_v4());
    let opened = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({"bear_slug": slug, "session_id": session_id}),
    )
    .await;
    assert_access(&opened["result"]["session"], "executable", true);
    let original = opened["result"]["session"]["history_conversation_id"]
        .as_str()
        .unwrap();
    let replacement = format!("den-conv-{}", Uuid::new_v4().simple());
    ensure_conversation_for_external_id(&pool, bear, Some(user), &replacement, None, None)
        .await
        .unwrap();
    for requested in [replacement.as_str(), "new-replacement"] {
        let denied = rpc_value(
            state.clone(),
            &token,
            "session.open",
            json!({"bear_slug": slug, "session_id": session_id, "conversation_id": requested}),
        )
        .await;
        assert!(denied.get("error").is_some(), "{denied}");
    }
    let current = rpc_value(
        state.clone(),
        &token,
        "session.state",
        json!({"bear_slug": slug, "session_id": session_id}),
    )
    .await;
    assert_eq!(
        current["result"]["session"]["history_conversation_id"],
        original
    );
    assert_eq!(conversations(&pool, bear).await.len(), 2);
    let configured = rpc_value(
        state.clone(),
        &token,
        "session.model.set",
        json!({
            "bear_slug": slug, "session_id": session_id, "selection_mode": "auto",
        }),
    )
    .await;
    assert_eq!(
        configured["result"]["ok"], true,
        "live owner configuration: {configured}"
    );
    let work = create_checkoutable_work_run(&pool, user, bear).await;
    let work_session = format!("work-{}", Uuid::new_v4());
    let checkout = rpc_value(
        state.clone(),
        &token,
        "work.checkout",
        json!({
            "bear_slug": slug, "session_id": work_session, "work_order_id": work,
            "compatibility": {"protocol": 1, "capabilities": ["tool_attempt_token"]},
        }),
    )
    .await;
    assert_eq!(checkout["result"]["ok"], true, "{checkout}");
    let work_opened = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({"bear_slug": slug, "session_id": work_session}),
    )
    .await;
    assert_eq!(work_opened["result"]["ok"], true, "{work_opened}");
    let before = client_sessions::find_for_user_bear_session_id(&pool, user, bear, &work_session)
        .await
        .unwrap()
        .unwrap();
    let denied = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({
            "bear_slug": slug, "session_id": work_session, "conversation_id": replacement,
        }),
    )
    .await;
    assert!(denied.get("error").is_some(), "{denied}");
    let after = client_sessions::find_for_user_bear_session_id(&pool, user, bear, &work_session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.conversation_id, before.conversation_id);
    assert_eq!(
        after.resolved_conversation_id,
        before.resolved_conversation_id
    );
    let reopened = rpc_value(
        state,
        &token,
        "session.open",
        json!({"bear_slug": slug, "session_id": work_session}),
    )
    .await;
    assert_eq!(reopened["result"]["ok"], true, "{reopened}");
    assert_access(&reopened["result"]["session"], "executable", false);
    let source = persistence::get_conversation_for_external_id(
        &pool,
        bear,
        reopened["result"]["session"]["history_conversation_id"]
            .as_str()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        bindings::conversation_hat(&pool, BearId::new(bear), source.id)
            .await
            .unwrap(),
        None
    );
}
