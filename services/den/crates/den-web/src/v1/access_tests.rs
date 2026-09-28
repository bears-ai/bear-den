use super::*;
use crate::web_chat_runtime::WebChatRuntime;
use axum::{http::Request, routing::get};
use axum_login::AuthnBackend;
use den_service::bears::db::{
    create_bear, grant_membership, revoke_membership, BearParams, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER,
};
use http_body_util::BodyExt;
use minijinja::Environment;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

use crate::{auth_backend::Backend, config::Config};

pub(super) async fn seed(pool: &PgPool) -> (Uuid, [i32; 3]) {
    let bear = create_bear(
        pool,
        BearParams {
            slug: "web-access-test",
            name: "Access test",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let mut users = [0; 3];
    for (index, name) in ["accessone", "accesstwo", "accessadmin"]
        .into_iter()
        .enumerate()
    {
        users[index] = sqlx::query_scalar!(
            "INSERT INTO users (username, email) VALUES ($1, $1) RETURNING id",
            name
        )
        .fetch_one(pool)
        .await
        .unwrap();
        grant_membership(
            pool,
            users[index],
            bear,
            Some(if index == 2 {
                BEAR_ROLE_ADMIN
            } else {
                BEAR_ROLE_MEMBER
            }),
        )
        .await
        .unwrap();
    }
    (bear, users)
}

async fn test_login(Path(user_id): Path<i32>, mut auth: AuthSession) -> StatusCode {
    let user = auth.backend.get_user(&user_id).await.unwrap().unwrap();
    auth.login(&user).await.unwrap();
    StatusCode::OK
}

async fn app(pool: &PgPool) -> Router {
    app_with_runtime(
        pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await
}

pub(super) async fn app_with_runtime(pool: &PgPool, runtime: Arc<dyn WebChatRuntime>) -> Router {
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.unwrap();
    Router::new()
        .nest("/v1", router())
        .route("/test-login/{user_id}", get(test_login))
        .with_state({
            let mut config = Config::test_stub();
            config.llm_api_url = "http://127.0.0.1:1".to_string();
            AppState::test_with_template_env_and_chat_runtime(
                pool.clone(),
                Environment::new(),
                Arc::new(config),
                runtime,
            )
        })
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                Backend::new(pool.clone()),
                axum_login::tower_sessions::SessionManagerLayer::new(store),
            )
            .build(),
        )
}

pub(super) async fn login(app: &Router, user: i32) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/test-login/{user}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

pub(super) async fn request(
    app: &Router,
    cookie: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

pub(super) async fn conversation(pool: &PgPool, bear: Uuid, owner: Option<i32>, id: &str) -> Uuid {
    conversation_persistence::ensure_conversation_for_external_id(pool, bear, owner, id, None, None)
        .await
        .unwrap()
        .id
}

#[sqlx::test(migrations = "../../migrations")]
async fn authenticated_routes_enforce_canonical_ownership(pool: PgPool) {
    let (bear, [one, two, admin]) = seed(&pool).await;
    let own = conversation(&pool, bear, Some(one), "conv-owned-one").await;
    let other = conversation(&pool, bear, Some(two), "conv-owned-two").await;
    conversation(&pool, bear, None, "conv-unowned").await;
    archived_conversations::set_archived(&pool, bear, "conv-owned-two", Some(two), "test", true)
        .await
        .unwrap();
    conversation_persistence::append_message(
        &pool,
        other,
        &den_service::conversation::message_types::ConversationMessageWrite::user_turn(
            "private",
            json!({"type":"user_input", "text":"private"}),
            None,
        ),
    )
    .await
    .unwrap();
    let app = app(&pool).await;
    let a = login(&app, one).await;
    let b = login(&app, two).await;
    let root = format!("/v1/chat/conversations?bear_id={bear}");
    let (status, list) = request(&app, &a, "GET", &root, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let ids = list["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"conv-owned-one"));
    assert!(!ids.contains(&"conv-owned-two"));
    assert!(!ids.contains(&"conv-unowned"));
    assert!(ids.contains(&"default"));
    let (_, admin_list) = request(&app, &login(&app, admin).await, "GET", &root, Value::Null).await;
    assert!(admin_list["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == "conv-unowned"));

    for id in ["conv-owned-two", "conv-unowned"] {
        let history = format!("/v1/chat/history?bear_id={bear}&conversation_id={id}");
        let artifacts = format!("/v1/chat/artifacts?bear_id={bear}&conversation_id={id}");
        let patch = format!("/v1/chat/conversations/{id}");
        assert_eq!(
            request(&app, &a, "GET", &history, Value::Null).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, &a, "GET", &artifacts, Value::Null).await.0,
            StatusCode::FORBIDDEN
        );
        for update in [
            json!({"title":"stolen"}),
            json!({"archived":true}),
            json!({"deleted":true}),
        ] {
            assert_eq!(request(&app, &a, "PATCH", &patch, json!({"bear_id":bear, "title":update["title"], "archived":update["archived"], "deleted":update["deleted"]})).await.0, StatusCode::FORBIDDEN);
        }
        assert_eq!(
            request(
                &app,
                &a,
                "POST",
                "/v1/chat/send",
                json!({"bear_id":bear,"conversation_id":id,"message":"hello"})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        for (path, payload) in [
            (
                "/v1/chat/current-task",
                json!({"bear_id":bear,"conversation_id":id,"title":"task"}),
            ),
            (
                "/v1/chat/current-task/selection-request",
                json!({"bear_id":bear,"conversation_id":id,"task_id":Uuid::new_v4()}),
            ),
            (
                "/v1/chat/current-task/select",
                json!({"bear_id":bear,"conversation_id":id,"task_id":Uuid::new_v4()}),
            ),
            (
                "/v1/chat/current-task/clear",
                json!({"bear_id":bear,"conversation_id":id}),
            ),
        ] {
            assert_eq!(
                request(&app, &a, "POST", path, payload).await.0,
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            request(
                &app,
                &a,
                "GET",
                &format!("/v1/chat/model?bear_id={bear}&conversation_id={id}"),
                Value::Null
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                &a,
                "PATCH",
                "/v1/chat/model",
                json!({"bear_id":bear,"conversation_id":id,"selection_mode":"auto"})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                &a,
                "GET",
                &format!("/v1/chat/current-task?bear_id={bear}&conversation_id={id}"),
                Value::Null
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        request(
            &app,
            &b,
            "GET",
            &format!("/v1/chat/artifacts?bear_id={bear}&conversation_id=conv-owned-two"),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            &b,
            "GET",
            &format!("/v1/chat/history?bear_id={bear}&conversation_id=conv-owned-two"),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            &a,
            "PATCH",
            "/v1/chat/conversations/conv-owned-one",
            json!({"bear_id":bear,"title":"mine"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let state = AppState::test_with_template_env(
        pool.clone(),
        Environment::new(),
        Arc::new(Config::test_stub()),
    );
    let bear_row = bears_db::get_bear(&pool, bear).await.unwrap().unwrap();
    assert!(
        browser_client_session(&state, one, &bear_row, "conv-owned-two")
            .await
            .is_err()
    );
    assert!(maybe_handle_direct_set_conversation_title(
        &state,
        ConversationTitleRequest {
            user_id: one,
            bear: &bear_row,
            conv_id: "conv-owned-two",
            message: "rename conversation to stolen",
            request_id: Uuid::new_v4(),
        }
    )
    .await
    .is_err());
    assert!(maybe_handle_direct_set_conversation_title(
        &state,
        ConversationTitleRequest {
            user_id: one,
            bear: &bear_row,
            conv_id: "conv-owned-one",
            message: "rename conversation to direct owner title",
            request_id: Uuid::new_v4(),
        }
    )
    .await
    .unwrap()
    .is_some());
    let owned =
        conversation_persistence::get_conversation_for_external_id(&pool, bear, "conv-owned-one")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(owned.id, own);
    assert_eq!(owned.current_title.as_deref(), Some("direct owner title"));
    assert_eq!(
        conversation_persistence::get_conversation_for_external_id(&pool, bear, "conv-owned-two")
            .await
            .unwrap()
            .unwrap()
            .current_title,
        None
    );
    revoke_membership(&pool, one, bear).await.unwrap();
    assert_eq!(
        request(&app, &a, "GET", &root, Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &a,
            "POST",
            "/v1/chat/send",
            json!({"bear_id":bear,"conversation_id":"conv-owned-one","message":"hello"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &a,
            "PATCH",
            "/v1/chat/conversations/conv-owned-one",
            json!({"bear_id":bear,"title":"after revoke"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &a,
            "GET",
            &format!("/v1/chat/history?bear_id={bear}&conversation_id=conv-owned-one"),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn ownerless_legacy_default_is_never_inherited(pool: PgPool) {
    let (bear, [one, two, admin]) = seed(&pool).await;
    let ownerless = conversation(&pool, bear, None, "default").await;
    let (viewer, one_id) = checked_chat_id(&pool, bear, one, "default").await.unwrap();
    let (_, two_id) = checked_chat_id(&pool, bear, two, "default").await.unwrap();
    assert_ne!(one_id, "default");
    assert_ne!(one_id, two_id);
    assert!(!viewer.may_access_external(&pool, "default").await.unwrap());
    assert!(conversation_viewer(&pool, bear, admin)
        .await
        .unwrap()
        .may_access_id(&pool, ownerless)
        .await
        .unwrap());
    ensure_chat_conversation(&pool, bear, one, &viewer, &one_id)
        .await
        .unwrap();
    let app = app(&pool).await;
    let cookie = login(&app, one).await;
    let (_, list) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/conversations?bear_id={bear}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        list["conversations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["id"] == "default")
            .count(),
        1
    );
    let (_, history) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/history?bear_id={bear}&conversation_id=default"),
        Value::Null,
    )
    .await;
    assert!(history["messages"].as_array().unwrap().is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn default_and_conflict_are_isolated_and_list_filters_before_limit(pool: PgPool) {
    let (bear, [one, two, admin]) = seed(&pool).await;
    let legacy = conversation(&pool, bear, Some(one), "default").await;
    conversation_persistence::append_message(
        &pool,
        legacy,
        &den_service::conversation::message_types::ConversationMessageWrite::user_turn(
            "first user's private default",
            json!({"type":"user_input", "text":"first user's private default"}),
            None,
        ),
    )
    .await
    .unwrap();
    let guessed_default = format!("conv-web-default-{two}");
    conversation(&pool, bear, Some(one), &guessed_default).await;
    let (first, id_one) = checked_chat_id(&pool, bear, one, "default").await.unwrap();
    let (second, id_two) = checked_chat_id(&pool, bear, two, "default").await.unwrap();
    assert_eq!(id_one, "default");
    assert_ne!(id_two, id_one);
    assert_ne!(id_two, guessed_default);
    let default_two = ensure_chat_conversation(&pool, bear, two, &second, &id_two)
        .await
        .unwrap();
    assert_eq!(
        default_two.external_conversation_id.as_deref(),
        Some(id_two.as_str())
    );

    conversation_persistence::delete_conversation_for_external_id(&pool, bear, &guessed_default)
        .await
        .unwrap();
    assert_eq!(
        checked_chat_id(&pool, bear, two, "default")
            .await
            .unwrap()
            .1,
        id_two
    );
    assert!(checked_chat_id(&pool, bear, one, &id_two).await.is_err());
    assert!(ensure_chat_conversation(&pool, bear, one, &first, &id_two)
        .await
        .is_err());
    // Simulate another member winning the insert after the first preflight.
    let (_, pending) = checked_chat_id(&pool, bear, two, "conv-guessed")
        .await
        .unwrap();
    let stolen = conversation(&pool, bear, Some(one), &pending).await;
    assert!(
        ensure_chat_conversation(&pool, bear, two, &second, "conv-guessed")
            .await
            .is_err()
    );
    assert_eq!(
        conversation_persistence::get_conversation_for_external_id(&pool, bear, "conv-guessed")
            .await
            .unwrap()
            .unwrap()
            .id,
        stolen
    );

    let ownerless_default = conversation(&pool, bear, None, "conv-web-default-999").await;
    assert!(checked_chat_id(&pool, bear, one, "conv-web-default-999")
        .await
        .is_err());
    assert!(conversation_viewer(&pool, bear, admin)
        .await
        .unwrap()
        .may_access_id(&pool, ownerless_default)
        .await
        .unwrap());
    let own = conversation(&pool, bear, Some(two), "conv-old-own").await;
    sqlx::query!(
        "UPDATE conversations SET updated_at = NOW() - INTERVAL '2 hours' WHERE id = $1",
        own
    )
    .execute(&pool)
    .await
    .unwrap();
    for n in 0..105 {
        conversation(&pool, bear, Some(one), &format!("conv-foreign-{n}")).await;
    }
    let app = app(&pool).await;
    let cookie = login(&app, two).await;
    let (status, list) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/conversations?bear_id={bear}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids = list["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"conv-old-own"));
    assert!(ids.contains(&"default"));
    assert!(!ids.iter().any(|id| id.starts_with("conv-foreign-")));
    let (_, history) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/history?bear_id={bear}&conversation_id=default"),
        Value::Null,
    )
    .await;
    assert_eq!(history["messages"].as_array().unwrap().len(), 0);
    let (_, legacy_history) = request(
        &app,
        &login(&app, one).await,
        "GET",
        &format!("/v1/chat/history?bear_id={bear}&conversation_id=default"),
        Value::Null,
    )
    .await;
    assert_eq!(
        legacy_history["messages"][0]["text"],
        "first user's private default"
    );
    assert!(
        ConversationViewer::resolve(&pool, BearId::new(bear), UserId::new(admin))
            .await
            .unwrap()
            .is_some()
    );
}
