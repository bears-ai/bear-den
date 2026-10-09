use super::access_tests::{
    app_with_runtime, bound_conversation, conversation, login, request, seed,
};
use super::chat_model_access_tests::{assert_json_error, raw_request};
use super::*;
use crate::web_chat_runtime::{WebChatRuntime, WebChatRuntimeRequest, WebChatRuntimeStream};
use sqlx::PgPool;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

async fn list(
    app: &Router,
    cookie: &str,
    bear: Uuid,
    selection: Option<&str>,
) -> (StatusCode, Value) {
    let mut uri = format!("/v1/chat/conversations?bear_id={bear}");
    if let Some(selection) = selection {
        uri.push_str(&format!("&conversation_id={selection}"));
    }
    request(app, cookie, "GET", &uri, Value::Null).await
}

#[derive(Default)]
struct UnexpectedRuntime(AtomicUsize);

impl WebChatRuntime for UnexpectedRuntime {
    fn stream_chat(
        &self,
        _state: &AppState,
        _request: WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<'static, Result<WebChatRuntimeStream, CustomError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(CustomError::System(
                "read-only source reached runtime".into(),
            ))
        })
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn canonical_default_links_resolve_to_the_same_displayed_owned_conversation(pool: PgPool) {
    let (bear, [owner, other, _]) = seed(&pool).await;
    let occupied = format!("conv-web-default-{owner}");
    conversation(&pool, bear, Some(other), &occupied).await;
    let external = format!("conv-web-default-{owner}-1");
    let canonical = bound_conversation(&pool, bear, owner, &external).await;
    conversation_persistence::append_message(
        &pool,
        canonical,
        &den_service::conversation::message_types::ConversationMessageWrite::user_turn(
            "Canonical default history",
            json!({"text":"Canonical default history"}),
            None,
        ),
    )
    .await
    .unwrap();
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    let cookie = login(&app, owner).await;
    let (status, inventory) = list(&app, &cookie, bear, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(inventory["selected_conversation_id"].is_null());
    for selection in [&external, "default"] {
        let (status, response) = list(&app, &cookie, bear, Some(selection)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["selected_conversation_id"], "default");
        let rows = response["conversations"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "default");
        assert_eq!(rows[0]["can_send"], true);
        assert_eq!(rows[0]["own_notes_available"], true);
        assert!(rows[0]["hat_id"].is_string());
        let (_, resolved) = checked_chat_id(&pool, bear, owner, selection)
            .await
            .unwrap();
        assert_eq!(resolved, external);
        for history_id in [
            selection,
            response["selected_conversation_id"].as_str().unwrap(),
        ] {
            let (status, history) = request(
                &app,
                &cookie,
                "GET",
                &format!("/v1/chat/history?bear_id={bear}&conversation_id={history_id}"),
                Value::Null,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(history["messages"].as_array().unwrap().len(), 1);
            assert_eq!(history["messages"][0]["text"], "Canonical default history");
        }
    }
    assert_eq!(
        conversation_persistence::get_conversation_for_external_id(&pool, bear, &external)
            .await
            .unwrap()
            .unwrap()
            .id,
        canonical
    );
    assert_eq!(
        conversation_persistence::list_conversations_for_bear(&pool, bear, 200)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn explicit_legacy_default_selection_keeps_each_humans_existing_scope(pool: PgPool) {
    let (bear, [one, two, admin]) = seed(&pool).await;
    let legacy = bound_conversation(&pool, bear, one, "default").await;
    let scoped = format!("conv-web-default-{two}");
    let second = bound_conversation(&pool, bear, two, &scoped).await;
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    for (human, expected) in [(one, legacy), (two, second)] {
        let cookie = login(&app, human).await;
        let (status, response) = list(&app, &cookie, bear, Some("default")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["selected_conversation_id"], "default");
        let rows = response["conversations"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "default");
        assert_eq!(rows[0]["can_send"], true);
        let (_, external) = checked_chat_id(&pool, bear, human, "default")
            .await
            .unwrap();
        assert_eq!(
            conversation_persistence::get_conversation_for_external_id(&pool, bear, &external)
                .await
                .unwrap()
                .unwrap()
                .id,
            expected
        );
    }
    let cookie = login(&app, admin).await;
    let body = assert_json_error(
        raw_request(
            &app,
            &cookie,
            "GET",
            &format!("/v1/chat/conversations?bear_id={bear}&conversation_id=default"),
            "",
        )
        .await,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(body["code"], "conversation_unavailable");
    assert!(conversation_persistence::get_conversation_for_external_id(
        &pool,
        bear,
        &format!("conv-web-default-{admin}")
    )
    .await
    .unwrap()
    .is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn exact_owned_history_selection_includes_a_row_older_than_the_latest_hundred(pool: PgPool) {
    let (bear, [owner, _, _]) = seed(&pool).await;
    let oldest = bound_conversation(&pool, bear, owner, "conv-oldest-owned").await;
    conversation_persistence::append_message(
        &pool,
        oldest,
        &den_service::conversation::message_types::ConversationMessageWrite::user_turn(
            "Older authorized history",
            json!({"text":"Older authorized history"}),
            None,
        ),
    )
    .await
    .unwrap();
    for index in 0..100 {
        conversation(&pool, bear, Some(owner), &format!("conv-recent-{index:03}")).await;
    }
    assert!(!conversation_viewer(&pool, bear, owner)
        .await
        .unwrap()
        .list_visible(&pool, 100)
        .await
        .unwrap()
        .iter()
        .any(|row| row.id == oldest));
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    let cookie = login(&app, owner).await;
    let (status, recent) = list(&app, &cookie, bear, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recent["conversations"].as_array().unwrap().len(), 100);
    assert!(recent["selected_conversation_id"].is_null());
    assert!(!recent["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "conv-oldest-owned"));
    let (status, selected) = list(&app, &cookie, bear, Some("conv-oldest-owned")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(selected["selected_conversation_id"], "conv-oldest-owned");
    let rows = selected["conversations"].as_array().unwrap();
    assert_eq!(rows.len(), 101);
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| row["id"] == "conv-oldest-owned")
        .collect();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["can_send"], true);
    let (status, history) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/history?bear_id={bear}&conversation_id=conv-oldest-owned"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["messages"][0]["text"], "Older authorized history");
    let (status, selected_recent) = list(&app, &cookie, bear, Some("conv-recent-099")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        selected_recent["selected_conversation_id"],
        "conv-recent-099"
    );
    assert_eq!(
        selected_recent["conversations"].as_array().unwrap().len(),
        100
    );
    assert_eq!(
        conversation_persistence::list_conversations_for_bear(&pool, bear, 200)
            .await
            .unwrap()
            .len(),
        101
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn unavailable_selections_do_not_substitute_or_disclose_history_or_unarchive_it(
    pool: PgPool,
) {
    let (bear, [owner, other, _]) = seed(&pool).await;
    bound_conversation(&pool, bear, owner, "conv-available-owner").await;
    bound_conversation(&pool, bear, other, "conv-private-other").await;
    bound_conversation(&pool, bear, owner, "conv-archive-hidden").await;
    conversation(&pool, bear, Some(owner), "new-hidden-pending").await;
    conversation_persistence::set_conversation_title_and_sync_client_sessions(
        &pool,
        bear,
        "conv-private-other",
        "PRIVATE-HISTORY-CANARY",
    )
    .await
    .unwrap();
    archived_conversations::set_archived(
        &pool,
        bear,
        "conv-archive-hidden",
        Some(owner),
        "test",
        true,
    )
    .await
    .unwrap();
    let runtime = Arc::new(UnexpectedRuntime::default());
    let app = app_with_runtime(&pool, runtime.clone()).await;
    let cookie = login(&app, owner).await;
    for selection in [
        "conv-private-other",
        "conv-missing-history",
        "new-hidden-pending",
        "conv-archive-hidden",
        "default",
    ] {
        let body = assert_json_error(
            raw_request(
                &app,
                &cookie,
                "GET",
                &format!("/v1/chat/conversations?bear_id={bear}&conversation_id={selection}"),
                "",
            )
            .await,
            StatusCode::NOT_FOUND,
        )
        .await;
        assert_eq!(body["code"], "conversation_unavailable");
        assert!(body.get("conversations").is_none());
        assert!(body.get("selected_conversation_id").is_none());
        assert!(!body.to_string().contains(selection));
        assert!(!body.to_string().contains("PRIVATE-HISTORY-CANARY"));
    }
    let (status, inventory) = list(&app, &cookie, bear, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(inventory["selected_conversation_id"].is_null());
    let rows = inventory["conversations"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "conv-available-owner");
    assert_eq!(rows[0]["can_send"], true);
    assert_eq!(
        conversation_persistence::list_conversations_for_bear(&pool, bear, 200)
            .await
            .unwrap()
            .len(),
        4
    );
    assert!(archived_conversations::list_for_bear(&pool, bear)
        .await
        .unwrap()
        .contains("conv-archive-hidden"));
    for (method, route, body) in [
        (
            "PATCH",
            "/v1/chat/model",
            json!({"bear_id":bear, "conversation_id":"conv-archive-hidden", "selection_mode":"auto"}),
        ),
        (
            "POST",
            "/v1/chat/send",
            json!({"bear_id":bear, "conversation_id":"conv-archive-hidden", "message":"must remain archived"}),
        ),
    ] {
        let body = assert_json_error(
            raw_request(&app, &cookie, method, route, &body.to_string()).await,
            StatusCode::FORBIDDEN,
        )
        .await;
        assert_eq!(body["code"], "conversation_archived");
    }
    assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn inactive_owned_history_stays_visible_but_readiness_does_not_grant_send_or_model_writes(
    pool: PgPool,
) {
    let (bear, [owner, _, admin]) = seed(&pool).await;
    let inactive = bound_conversation(&pool, bear, owner, "conv-inactive-history").await;
    let active = bound_conversation(&pool, bear, owner, "conv-active-history").await;
    conversation(&pool, bear, Some(owner), "conv-unbound-history").await;
    conversation_persistence::append_message(
        &pool,
        inactive,
        &den_service::conversation::message_types::ConversationMessageWrite::user_turn(
            "Retained inactive history",
            json!({"text":"Retained inactive history"}),
            None,
        ),
    )
    .await
    .unwrap();
    conversation_persistence::set_conversation_model_state(
        &pool,
        inactive,
        "explicit",
        Some("openai/gpt-5"),
        Some("openai/gpt-5"),
        None,
    )
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE conversations SET status = 'archived' WHERE id = $1",
        inactive
    )
    .execute(&pool)
    .await
    .unwrap();
    let runtime = Arc::new(UnexpectedRuntime::default());
    let app = app_with_runtime(&pool, runtime.clone()).await;
    let cookie = login(&app, owner).await;
    let (status, response) = list(&app, &cookie, bear, Some("conv-inactive-history")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["selected_conversation_id"],
        "conv-inactive-history"
    );
    let rows = response["conversations"].as_array().unwrap();
    let inactive_row = rows
        .iter()
        .find(|row| row["id"] == "conv-inactive-history")
        .unwrap();
    assert!(inactive_row["hat_id"].is_string());
    assert_eq!(inactive_row["can_send"], false);
    assert_eq!(inactive_row["own_notes_available"], false);
    assert_eq!(
        rows.iter()
            .find(|row| row["id"] == "conv-active-history")
            .unwrap()["can_send"],
        true
    );
    assert_eq!(
        rows.iter()
            .find(|row| row["id"] == "conv-unbound-history")
            .unwrap()["can_send"],
        false
    );
    let admin_cookie = login(&app, admin).await;
    let (_, inspected) = list(&app, &admin_cookie, bear, Some("conv-active-history")).await;
    let inspected = inspected["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "conv-active-history")
        .unwrap();
    assert_eq!(inspected["can_send"], false);
    assert_eq!(inspected["own_notes_available"], false);
    let (status, history) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/history?bear_id={bear}&conversation_id=conv-inactive-history"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["messages"].as_array().unwrap().len(), 1);
    assert_eq!(history["messages"][0]["text"], "Retained inactive history");
    for (method, route, body) in [
        (
            "PATCH",
            "/v1/chat/model",
            json!({"bear_id":bear, "conversation_id":"conv-inactive-history", "selection_mode":"auto"}),
        ),
        (
            "POST",
            "/v1/chat/send",
            json!({"bear_id":bear, "conversation_id":"conv-inactive-history", "message":"must not persist"}),
        ),
    ] {
        let body = assert_json_error(
            raw_request(&app, &cookie, method, route, &body.to_string()).await,
            StatusCode::FORBIDDEN,
        )
        .await;
        assert_eq!(body["code"], "conversation_read_only");
    }
    assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        den_service::model_selection::conversation_model_pin(&pool, inactive)
            .await
            .unwrap()
            .as_deref(),
        Some("openai/gpt-5")
    );
    assert_eq!(
        conversation_persistence::list_messages_page(&pool, inactive, None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        hats::bindings::conversation_hat(&pool, BearId::new(bear), active)
            .await
            .unwrap()
            .is_some()
    );
    bears_db::revoke_membership(&pool, owner, bear)
        .await
        .unwrap();
    assert_json_error(
        raw_request(
            &app,
            &cookie,
            "GET",
            &format!("/v1/chat/conversations?bear_id={bear}&conversation_id=conv-active-history"),
            "",
        )
        .await,
        StatusCode::FORBIDDEN,
    )
    .await;
    for (method, route, body) in [
        (
            "PATCH",
            "/v1/chat/model",
            json!({"bear_id":bear, "conversation_id":"conv-active-history", "selection_mode":"auto"}),
        ),
        (
            "POST",
            "/v1/chat/send",
            json!({"bear_id":bear, "conversation_id":"conv-active-history", "message":"stale readiness must not grant access"}),
        ),
    ] {
        let body = assert_json_error(
            raw_request(&app, &cookie, method, route, &body.to_string()).await,
            StatusCode::FORBIDDEN,
        )
        .await;
        assert_eq!(body["code"], "access_unavailable");
    }
    assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn readiness_propagates_database_failures_instead_of_advertising_success_or_read_only() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@localhost/unused")
        .unwrap();
    pool.close().await;
    assert!(matches!(
        chat_conversations::ordinary_source_can_send(
            &pool,
            BearId::new(Uuid::nil()),
            UserId::new(1),
            "conv-unavailable"
        )
        .await,
        Err(DenError::DatabaseUnavailable(_))
    ));
}
