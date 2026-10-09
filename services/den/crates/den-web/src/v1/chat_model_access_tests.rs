use super::access_tests::{
    app_with_runtime, bound_conversation, conversation, login, request, seed,
};
use super::*;
use axum::http::Request;
use den_core::ThinkingEffort;
use den_service::bears::model_configurations;
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

pub(super) async fn raw_request(
    app: &Router,
    cookie: &str,
    method: &str,
    uri: &str,
    body: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

pub(super) async fn assert_json_error(response: Response, expected: StatusCode) -> Value {
    assert_eq!(response.status(), expected);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    Uuid::parse_str(&request_id).unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["request_id"], request_id);
    assert!(body["error"].is_string());
    assert!(body["message"].is_string());
    assert!(body["code"].is_string());
    assert!(!String::from_utf8_lossy(&bytes).contains("<!doctype"));
    body
}

#[sqlx::test(migrations = "../../migrations")]
async fn first_default_model_get_is_read_only_and_previews_configured_bear_model(pool: PgPool) {
    let (bear, [owner, _, admin]) = seed(&pool).await;
    let bear_id = BearId::new(bear);
    let hat = hats::create_hat(
        &pool,
        bear_id,
        UserId::new(admin),
        "Chat",
        "Chat responsibility",
    )
    .await
    .unwrap();
    let configured = model_configurations::create(
        &pool,
        bear_id,
        "Deep",
        "openai/gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    let hat_model =
        model_configurations::create(&pool, bear_id, "Quick", "openai/gpt-5-mini", None)
            .await
            .unwrap();
    model_configurations::set_default(&pool, bear_id, Some(configured.id))
        .await
        .unwrap();
    model_configurations::set_hat_override(&pool, bear_id, hat.id, Some(hat_model.id))
        .await
        .unwrap();
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    let cookie = login(&app, owner).await;
    for suffix in [
        "",
        "&conversation_id=",
        "&conversation_id=default",
        "&conversation_id=new-preview",
        "&conversation_id=conv-missing",
        "&conversation_id=den-conv-missing",
    ] {
        let response = raw_request(
            &app,
            &cookie,
            "GET",
            &format!("/v1/chat/model?bear_id={bear}{suffix}"),
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["effective_model"], "openai/gpt-5");
        assert_eq!(body["source"], "bear_default");
        assert_eq!(body["configuration_id"], json!(configured.id));
        assert_eq!(body["configuration_name"], "Deep");
        assert_eq!(body["thinking_effort"], "high");
        assert!(body["selected_model"].is_null());
        assert!(body["requested_model"].is_null());
    }
    let viewer = conversation_viewer(&pool, bear, owner).await.unwrap();
    assert!(viewer.list_visible(&pool, 100).await.unwrap().is_empty());
    model_configurations::set_default(&pool, bear_id, None)
        .await
        .unwrap();
    let (_, body) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/model?bear_id={bear}"),
        Value::Null,
    )
    .await;
    assert_eq!(body["source"], "deployment_default");
    assert_eq!(body["effective_model"], "openai/gpt-4.1");
    assert!(body["configuration_id"].is_null());
    assert!(viewer.list_visible(&pool, 100).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn unbound_history_model_inspection_never_implies_hat_or_execution_authority(pool: PgPool) {
    let (bear, [owner, other, admin]) = seed(&pool).await;
    let canonical = conversation(&pool, bear, Some(owner), "conv-unbound-history").await;
    let legacy_default = conversation(&pool, bear, Some(owner), "default").await;
    for id in [canonical, legacy_default] {
        conversation_persistence::set_conversation_model_state(
            &pool,
            id,
            "explicit",
            Some("openai/gpt-5-mini"),
            Some("openai/gpt-5-mini"),
            None,
        )
        .await
        .unwrap();
    }
    let configured = model_configurations::create(
        &pool,
        BearId::new(bear),
        "Deep",
        "openai/gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    model_configurations::set_default(&pool, BearId::new(bear), Some(configured.id))
        .await
        .unwrap();
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    for human in [owner, admin] {
        let cookie = login(&app, human).await;
        let (status, body) = request(
            &app,
            &cookie,
            "GET",
            &format!("/v1/chat/model?bear_id={bear}&conversation_id=conv-unbound-history"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["source"], "bear_default");
        assert_eq!(body["selection_mode"], "auto");
        assert!(body["selected_model"].is_null());
        assert_eq!(body["configuration_id"], json!(configured.id));
    }
    let cookie = login(&app, owner).await;
    let (status, body) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/model?bear_id={bear}&conversation_id=default"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["source"], "bear_default");
    let other_cookie = login(&app, other).await;
    assert_json_error(
        raw_request(
            &app,
            &other_cookie,
            "GET",
            &format!("/v1/chat/model?bear_id={bear}&conversation_id=conv-unbound-history"),
            "",
        )
        .await,
        StatusCode::FORBIDDEN,
    )
    .await;
    for id in [canonical, legacy_default] {
        assert!(
            hats::bindings::conversation_hat(&pool, BearId::new(bear), id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            den_service::model_selection::conversation_model_pin(&pool, id)
                .await
                .unwrap()
                .as_deref(),
            Some("openai/gpt-5-mini")
        );
        assert!(matches!(
            den_service::model_selection::resolve_conversation_primary_model(
                &pool,
                BearId::new(bear),
                id,
                "openai/gpt-4.1"
            )
            .await,
            Err(DenError::Authorization(_))
        ));
    }
    assert!(matches!(
        den_service::conversation::viewer::require_ordinary_tool_source(
            &pool,
            BearId::new(bear),
            UserId::new(owner),
            "conv-unbound-history"
        )
        .await,
        Err(DenError::Authorization(_))
    ));
    model_configurations::set_default(&pool, BearId::new(bear), None)
        .await
        .unwrap();
    let (_, body) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/model?bear_id={bear}&conversation_id=conv-unbound-history"),
        Value::Null,
    )
    .await;
    assert_eq!(body["source"], "deployment_default");
    assert_eq!(body["effective_model"], "openai/gpt-4.1");
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_patch_requires_existing_live_owned_bound_unarchived_source(pool: PgPool) {
    let (bear, [owner, other, admin]) = seed(&pool).await;
    let bound = bound_conversation(&pool, bear, owner, "conv-bound-owner").await;
    let unbound = conversation(&pool, bear, Some(owner), "conv-unbound-owner").await;
    let ownerless = conversation(&pool, bear, None, "conv-ownerless").await;
    let archived = bound_conversation(&pool, bear, owner, "conv-archived-owner").await;
    let inactive = bound_conversation(&pool, bear, owner, "conv-inactive-owner").await;
    archived_conversations::set_archived(
        &pool,
        bear,
        "conv-archived-owner",
        Some(owner),
        "test",
        true,
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
    for id in [bound, unbound, ownerless, archived, inactive] {
        conversation_persistence::set_conversation_model_state(
            &pool,
            id,
            "explicit",
            Some("openai/gpt-5"),
            Some("openai/gpt-5"),
            None,
        )
        .await
        .unwrap();
    }
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    let owner_cookie = login(&app, owner).await;
    let other_cookie = login(&app, other).await;
    let admin_cookie = login(&app, admin).await;
    for (cookie, external) in [
        (&owner_cookie, "default"),
        (&owner_cookie, "new-pending"),
        (&owner_cookie, "conv-missing"),
        (&owner_cookie, "conv-unbound-owner"),
        (&owner_cookie, "conv-ownerless"),
        (&owner_cookie, "conv-archived-owner"),
        (&owner_cookie, "conv-inactive-owner"),
        (&other_cookie, "conv-bound-owner"),
        (&admin_cookie, "conv-bound-owner"),
        (&admin_cookie, "conv-ownerless"),
    ] {
        for selection_mode in ["auto", "explicit"] {
            let body = json!({"bear_id":bear, "conversation_id":external, "selection_mode":selection_mode, "model":"openai/gpt-4.1"}).to_string();
            assert_json_error(
                raw_request(&app, cookie, "PATCH", "/v1/chat/model", &body).await,
                StatusCode::FORBIDDEN,
            )
            .await;
        }
    }
    let body = json!({"bear_id":bear, "selection_mode":"auto"}).to_string();
    assert_json_error(
        raw_request(&app, &owner_cookie, "PATCH", "/v1/chat/model", &body).await,
        StatusCode::FORBIDDEN,
    )
    .await;
    for external in [
        format!("conv-web-default-{owner}"),
        "new-pending".into(),
        "conv-missing".into(),
    ] {
        assert!(
            conversation_persistence::get_conversation_for_external_id(&pool, bear, &external)
                .await
                .unwrap()
                .is_none()
        );
    }
    for id in [bound, unbound, ownerless, archived, inactive] {
        assert_eq!(
            den_service::model_selection::conversation_model_pin(&pool, id)
                .await
                .unwrap()
                .as_deref(),
            Some("openai/gpt-5")
        );
    }
    assert!(
        hats::bindings::conversation_hat(&pool, BearId::new(bear), unbound)
            .await
            .unwrap()
            .is_none()
    );
    // Admins may inspect someone else's pin, but may not change it.
    let (status, inspected) = request(
        &app,
        &admin_cookie,
        "GET",
        &format!("/v1/chat/model?bear_id={bear}&conversation_id=conv-bound-owner"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inspected["source"], "conversation_explicit");
    assert_eq!(inspected["selected_model"], "openai/gpt-5");
    for mode in ["explicit", "auto"] {
        let (status, body) = request(&app, &owner_cookie, "PATCH", "/v1/chat/model", json!({"bear_id":bear, "conversation_id":"conv-bound-owner", "selection_mode":mode, "model":"openai/gpt-4.1"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["selection_mode"], mode);
    }
    assert!(
        den_service::model_selection::conversation_model_pin(&pool, bound)
            .await
            .unwrap()
            .is_none()
    );
    den_service::bears::db::revoke_membership(&pool, owner, bear)
        .await
        .unwrap();
    let body = json!({"bear_id":bear, "conversation_id":"conv-bound-owner", "selection_mode":"explicit", "model":"openai/gpt-5"}).to_string();
    assert_json_error(
        raw_request(&app, &owner_cookie, "PATCH", "/v1/chat/model", &body).await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert!(
        den_service::model_selection::conversation_model_pin(&pool, bound)
            .await
            .unwrap()
            .is_none()
    );
}
