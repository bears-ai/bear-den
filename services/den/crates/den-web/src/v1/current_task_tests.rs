use super::access_tests::{
    app_with_runtime, bound_conversation, conversation, login, request, seed,
};
use super::*;
use axum::response::IntoResponse;
use den_service::bears::db::revoke_membership;
use http_body_util::BodyExt;
use minijinja::Environment;
use sqlx::PgPool;

async fn app(pool: &PgPool) -> Router {
    app_with_runtime(
        pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await
}

async fn choices(app: &Router, cookie: &str, bear: Uuid, id: &str) -> (StatusCode, Value) {
    request(
        app,
        cookie,
        "GET",
        &format!("/v1/chat/current-task?bear_id={bear}&conversation_id={id}"),
        Value::Null,
    )
    .await
}

async fn create(app: &Router, cookie: &str, bear: Uuid, id: &str, title: &str) -> Uuid {
    let (status, body) = request(
        app,
        cookie,
        "POST",
        "/v1/chat/current-task",
        json!({"bear_id": bear, "conversation_id": id, "title": title}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Uuid::parse_str(body["task"]["id"].as_str().unwrap()).unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn chat_task_choices_are_human_authorized_and_selection_remains_canonical(pool: PgPool) {
    let (bear, [owner, other, _]) = seed(&pool).await;
    let id = "conv-task-picker-owner";
    bound_conversation(&pool, bear, owner, id).await;
    let app = app(&pool).await;
    let cookie = login(&app, owner).await;
    let own_task = create(&app, &cookie, bear, id, "Readable <task>").await;
    let (status, initial) = choices(&app, &cookie, bear, id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["tasks"][0]["id"], json!(own_task));
    assert_eq!(initial["tasks"][0]["title"], "Readable <task>");
    assert_eq!(initial["tasks"][0]["status"], "pending");
    assert!(initial["tasks"][0].get("body").is_none());
    let session_id = initial["session_id"].as_str().unwrap();
    let session = client_sessions::find_for_user_bear_session_id(&pool, owner, bear, session_id)
        .await
        .unwrap()
        .unwrap();

    // A trusted historical attachment must not turn another human's private task
    // into a browser choice merely because it points at this session.
    let foreign = PgDocketService::from_pool(&pool)
        .create_task(DocketTaskCreate {
            bear_id: bear,
            job_id: None,
            session_anchor_id: Some(session.id),
            parent_task_id: None,
            sibling_order: 1,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Run,
            title: "Foreign private title".into(),
            body: "Foreign private body".into(),
            completion_criteria: vec!["Done".into()],
            difficulty: Some(DocketTaskDifficulty::Trivial),
            effort_hint: Some(DocketEffortHint::Low),
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".into(),
            created_by_user_id: Some(other),
            created_by_agent_id: None,
            created_in_run_id: None,
        })
        .await
        .unwrap();
    let (_, listed) = choices(&app, &cookie, bear, id).await;
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains("Foreign private"));
    let body = json!({"bear_id": bear, "conversation_id": id, "task_id": own_task});
    let (status, preview) = request(
        &app,
        &cookie,
        "POST",
        "/v1/chat/current-task/selection-request",
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["confirmation_required"], true);
    assert_eq!(preview["title"], "Readable <task>");
    let (_, after_preview) = choices(&app, &cookie, bear, id).await;
    assert!(after_preview["current_task_id"].is_null());
    let (status, _) = request(&app, &cookie, "POST", "/v1/chat/current-task/select", body).await;
    assert_eq!(status, StatusCode::OK);
    let (_, selected) = choices(&app, &cookie, bear, id).await;
    assert_eq!(selected["current_task_id"], json!(own_task));
    let canonical = conversation_persistence::get_conversation_for_external_id(&pool, bear, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(canonical.current_title.as_deref(), Some("Readable <task>"));
    for suffix in ["selection-request", "select"] {
        let (status, _) = request(
            &app,
            &cookie,
            "POST",
            &format!("/v1/chat/current-task/{suffix}"),
            json!({"bear_id": bear, "conversation_id": id, "task_id": foreign.id}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _) = request(
        &app,
        &cookie,
        "POST",
        "/v1/chat/current-task/clear",
        json!({"bear_id": bear, "conversation_id": id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(choices(&app, &cookie, bear, id).await.1["current_task_id"].is_null());
}

#[sqlx::test(migrations = "../../migrations")]
async fn chat_task_controls_reject_pending_legacy_and_admin_history_without_creating_sessions(
    pool: PgPool,
) {
    let (bear, [owner, other, admin]) = seed(&pool).await;
    let owned = "conv-task-history-owner";
    bound_conversation(&pool, bear, owner, owned).await;
    let legacy = "conv-task-history-legacy";
    conversation(&pool, bear, Some(owner), legacy).await;
    let app = app(&pool).await;
    let owner_cookie = login(&app, owner).await;
    let admin_cookie = login(&app, admin).await;
    let other_cookie = login(&app, other).await;
    let (status, _) = request(
        &app,
        &admin_cookie,
        "GET",
        &format!("/v1/chat/history?bear_id={bear}&conversation_id={owned}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for (user, cookie, id, expected) in [
        (admin, &admin_cookie, owned, StatusCode::FORBIDDEN),
        (other, &other_cookie, owned, StatusCode::FORBIDDEN),
        (owner, &owner_cookie, legacy, StatusCode::FORBIDDEN),
        (
            owner,
            &owner_cookie,
            "new-task-pending",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, error) = choices(&app, cookie, bear, id).await;
        assert_eq!(status, expected);
        assert!(error["message"].is_string());
        for path in ["", "/selection-request", "/select", "/clear"] {
            let (status, _) = request(&app, cookie, "POST", &format!("/v1/chat/current-task{path}"),
                json!({"bear_id": bear, "conversation_id": id, "title": "Must not create", "task_id": Uuid::new_v4()})).await;
            assert_eq!(status, expected, "{id}: {path}");
        }
        assert!(client_sessions::find_for_user_bear_session_id(
            &pool,
            user,
            bear,
            &browser_client_session_id(user, bear, id)
        )
        .await
        .unwrap()
        .is_none());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn chat_task_default_is_canonical_and_revocation_after_preview_cannot_select(pool: PgPool) {
    let (bear, [owner, _, _]) = seed(&pool).await;
    let canonical_id = format!("conv-web-default-{owner}");
    bound_conversation(&pool, bear, owner, &canonical_id).await;
    let other_id = "conv-task-other-chat";
    bound_conversation(&pool, bear, owner, other_id).await;
    let app = app(&pool).await;
    let cookie = login(&app, owner).await;
    let task_id = create(&app, &cookie, bear, "default", "Default-only task").await;
    let (_, listed) = choices(&app, &cookie, bear, "default").await;
    assert_eq!(
        listed["session_id"],
        browser_client_session_id(owner, bear, &canonical_id)
    );
    assert!(choices(&app, &cookie, bear, other_id).await.1["tasks"]
        .as_array()
        .unwrap()
        .is_empty());
    let selection = json!({"bear_id": bear, "conversation_id": "default", "task_id": task_id});
    assert_eq!(
        request(
            &app,
            &cookie,
            "POST",
            "/v1/chat/current-task/selection-request",
            selection.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            &cookie,
            "POST",
            "/v1/chat/current-task/select",
            json!({"bear_id": bear, "conversation_id": other_id, "task_id": task_id})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    revoke_membership(&pool, owner, bear).await.unwrap();
    assert_eq!(
        request(
            &app,
            &cookie,
            "POST",
            "/v1/chat/current-task/select",
            selection
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let session = client_sessions::find_for_user_bear_session_id(
        &pool,
        owner,
        bear,
        &browser_client_session_id(owner, bear, &canonical_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(session.current_task_id.is_none());
}

#[tokio::test]
async fn chat_task_errors_are_json_not_error_pages() {
    let response = current_task_choices::ChatTaskError::from(CustomError::Authorization(
        "task access revoked".into(),
    ))
    .into_response();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("task access revoked"));
}

#[test]
fn chat_task_picker_renders_through_the_management_shell_and_fixture() {
    let mut env = Environment::new();
    env.set_loader(minijinja::path_loader(format!(
        "{}/src/templates",
        env!("CARGO_MANIFEST_DIR")
    )));
    for name in [
        "hexadecimal",
        "urlencode",
        "markdown",
        "timeago",
        "humanize_time",
        "is_future",
    ] {
        env.add_filter(name, |value: minijinja::Value| value);
    }
    for name in ["bear_chat.html", "design/chat.html"] {
        let html = env.get_template(name).unwrap().render(minijinja::context! {
            bear => json!({"name": "Chat <Bear>", "slug": "chat-test"}),
            bear_name => "Chat <Bear>", bear_slug => "chat-test", bear_id => Uuid::nil().to_string(),
            bear_nav_active => "chat", can_manage_bear => false,
        }).unwrap();
        assert!(html.contains("<html"));
        assert!(html.contains("/assets/js/task-selector.js"));
        assert!(html.contains("id=\"den-task-picker\""));
        assert!(html.contains("Search tasks"));
        assert!(html.contains("Confirm current task"));
        assert!(html.contains("Retry") || html.contains("Refresh tasks"));
        assert!(!html.contains("Task UUID"));
    }
}
