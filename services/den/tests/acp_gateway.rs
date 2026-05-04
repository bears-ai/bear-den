//! Integration coverage for the ACP API gateway. Requires `DATABASE_URL`.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use den::{
    api,
    config::Config,
    core::{
        acp_tokens,
        bears::{db as bears_db, BearAgentRole},
    },
    startup::run_sqlx_migrations,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

#[derive(Clone)]
struct TestLettaState {
    captured: Arc<Mutex<Option<Value>>>,
}

struct TestApp {
    app: axum::Router,
    pool: sqlx::PgPool,
    captured_letta_body: Arc<Mutex<Option<Value>>>,
}

struct TestUserBear {
    user_id: i32,
    bear_id: Uuid,
    bear_slug: String,
    raw_token: String,
}

async fn apply_app_migrations(pool: &sqlx::PgPool) {
    run_sqlx_migrations(pool)
        .await
        .expect("sqlx migrations for ACP integration test");
}

async fn start_fake_letta() -> (String, Arc<Mutex<Option<Value>>>) {
    let captured = Arc::new(Mutex::new(None));
    let state = TestLettaState {
        captured: captured.clone(),
    };
    let app = Router::new()
        .route(
            "/v1/conversations/{conversation_id}/messages",
            post(fake_letta_conversation_messages),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Letta");
    let addr: SocketAddr = listener.local_addr().expect("fake Letta local addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("fake Letta server");
    });
    (format!("http://{addr}"), captured)
}

async fn fake_letta_conversation_messages(
    State(state): State<TestLettaState>,
    Path(conversation_id): Path<String>,
    Json(mut body): Json<Value>,
) -> Response {
    body["conversation_id"] = json!(conversation_id);
    *state.captured.lock().await = Some(body);
    (
        [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
        concat!(
            "data: {\"type\":\"conversation_resolved\",\"conversation_id\":\"conv-fake-resolved123\"}\n\n",
            "data: {\"type\":\"assistant_delta\",\"text\":\"hello from fake Letta\"}\n\n",
            "data: {\"type\":\"reasoning_delta\",\"text\":\"thinking\"}\n\n",
            "data: {\"type\":\"done\",\"outcome\":\"ok\"}\n\n"
        ),
    )
        .into_response()
}

async fn test_app() -> TestApp {
    dotenvy::dotenv().ok();
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL for ACP integration test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&database_url)
        .await
        .expect("connect postgres");
    apply_app_migrations(&pool).await;

    let (letta_base_url, captured_letta_body) = start_fake_letta().await;
    let mut config = Config::load();
    config.database_url = database_url;
    config.run_api = true;
    config.acp_gateway_enabled = true;
    config.letta_base_url = letta_base_url;
    config.codepool_base_url = String::new();
    config.api_server_url = "http://localhost:3001".to_string();
    config.web_server_url = "http://localhost:3000".to_string();

    let store = PostgresStore::new(pool.clone());
    store
        .migrate()
        .await
        .expect("tower-sessions postgres migrate");
    let app = api::create_api_app(pool.clone(), store, Arc::new(config))
        .await
        .expect("build api router");

    TestApp {
        app,
        pool,
        captured_letta_body,
    }
}

async fn create_test_user_bear(pool: &sqlx::PgPool, membership: bool) -> TestUserBear {
    create_test_user_bear_with_pair(pool, membership, true).await
}

async fn create_test_user_bear_with_pair(
    pool: &sqlx::PgPool,
    membership: bool,
    provision_pair: bool,
) -> TestUserBear {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("u{}", &suffix[..20]);
    let email = format!("{username}@example.test");
    let bear_slug = format!("acp-test-{}", &suffix[..12]);

    let (user_id,): (i32,) = sqlx::query_as(
        r#"
        INSERT INTO users (email, username, display_name, passhash)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(&email)
    .bind(&username)
    .bind(format!("ACP Test {username}"))
    .bind("unused-in-acp-tests")
    .fetch_one(pool)
    .await
    .expect("insert test user");

    let bear_id = bears_db::create_bear(
        pool,
        &bear_slug,
        "ACP Test Bear",
        "ACP integration test bear",
        "",
        None,
        None,
        None,
        sqlx::types::Json(Vec::<String>::new()),
    )
    .await
    .expect("create test bear");
    bears_db::set_letta_agent_id(pool, bear_id, "agent-acp-talk-test")
        .await
        .expect("set legacy talk agent id");
    if provision_pair {
        sqlx::query(
            r#"
            INSERT INTO bear_agents (bear_id, role, letta_agent_id, provisioning_status, last_synced_at)
            VALUES ($1, $2, $3, 'ready', NOW())
            ON CONFLICT (bear_id, role)
            DO UPDATE SET letta_agent_id = EXCLUDED.letta_agent_id,
                          provisioning_status = 'ready',
                          last_synced_at = NOW(),
                          updated_at = NOW()
            "#,
        )
        .bind(bear_id)
        .bind(BearAgentRole::Pair.as_str())
        .bind("agent-acp-pair-test")
        .execute(pool)
        .await
        .expect("set pair agent id");
    }
    if membership {
        bears_db::grant_membership(pool, user_id, bear_id, Some(bears_db::BEAR_ROLE_ADMIN))
            .await
            .expect("grant test membership");
    }
    let created = acp_tokens::create_for_bear(pool, user_id, bear_id, "ACP test token")
        .await
        .expect("create ACP token");

    TestUserBear {
        user_id,
        bear_id,
        bear_slug,
        raw_token: created.raw_token,
    }
}

async fn get_acp_sessions(
    app: axum::Router,
    slug: &str,
    token: Option<&str>,
    query: Option<&str>,
) -> axum::response::Response {
    let uri = match query {
        Some(q) => format!("/acp/bears/{slug}/sessions?{q}"),
        None => format!("/acp/bears/{slug}/sessions"),
    };
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::ACCEPT, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("ACP sessions list response")
}

async fn get_acp_session(
    app: axum::Router,
    slug: &str,
    session_id: &str,
    token: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("GET")
        .uri(format!("/acp/bears/{slug}/sessions/{session_id}"))
        .header(header::ACCEPT, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("ACP session get response")
}

async fn post_prompt(
    app: axum::Router,
    slug: &str,
    session_id: &str,
    token: Option<&str>,
    mut body: Value,
) -> axum::response::Response {
    if body.get("client_context").is_none() {
        body["client_context"] = json!({ "cwd": "/tmp/acp-workspace" });
    }
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/acp/bears/{slug}/sessions/{session_id}/prompt"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("ACP prompt response")
}

#[tokio::test]
async fn acp_prompt_requires_bearer_auth() {
    let fixture = test_app().await;
    let res = post_prompt(
        fixture.app,
        "missing-auth-bear",
        "session-1",
        None,
        json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&body).expect("JSON error body");
    assert_eq!(value["error_code"], "authentication");
}

#[tokio::test]
async fn acp_token_auth_failures_are_rejected() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let invalid = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-invalid",
        Some("bears_acp_invalid"),
        json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);

    let revoked_token = acp_tokens::create_for_bear(
        &fixture.pool,
        user_bear.user_id,
        user_bear.bear_id,
        "revoked ACP test token",
    )
    .await
    .expect("create revoked token");
    acp_tokens::revoke_for_user(&fixture.pool, user_bear.user_id, revoked_token.id)
        .await
        .expect("revoke token");
    let revoked = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-revoked",
        Some(&revoked_token.raw_token),
        json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    let expired_token = acp_tokens::create_for_bear(
        &fixture.pool,
        user_bear.user_id,
        user_bear.bear_id,
        "expired ACP test token",
    )
    .await
    .expect("create expired token");
    sqlx::query("UPDATE acp_tokens SET expires_at = NOW() - interval '1 hour' WHERE id = $1")
        .bind(expired_token.id)
        .execute(&fixture.pool)
        .await
        .expect("expire token");
    let expired = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-expired",
        Some(&expired_token.raw_token),
        json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);

    let no_membership = create_test_user_bear(&fixture.pool, false).await;
    let no_membership_response = post_prompt(
        fixture.app,
        &no_membership.bear_slug,
        "session-no-membership",
        Some(&no_membership.raw_token),
        json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(no_membership_response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn acp_prompt_rejects_empty_messages_before_runtime_call() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let res = post_prompt(
        fixture.app,
        &user_bear.bear_slug,
        "session-empty",
        Some(&user_bear.raw_token),
        json!({ "message": "   " }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&body).expect("JSON error body");
    assert_eq!(value["error_code"], "validation");
    assert!(fixture.captured_letta_body.lock().await.is_none());
}

#[tokio::test]
async fn acp_prompt_treats_legacy_default_conversation_id_as_omitted() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let res = post_prompt(
        fixture.app,
        &user_bear.bear_slug,
        "session-default-legacy",
        Some(&user_bear.raw_token),
        json!({
            "message": "hello bear",
            "conversation_id": "default",
            "client": "zed"
        }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    let captured = fixture
        .captured_letta_body
        .lock()
        .await
        .clone()
        .expect("Letta request captured");
    assert_eq!(
        captured["conversation_id"],
        "new-acp-zed-session-default-legacy"
    );
}

#[tokio::test]
async fn acp_prompt_reuses_resolved_conversation_after_legacy_default_id() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let first = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-reuse-resolved",
        Some(&user_bear.raw_token),
        json!({
            "message": "first",
            "conversation_id": "default",
            "client": "zed"
        }),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let _ = first.into_body().collect().await.unwrap().to_bytes();

    let second = post_prompt(
        fixture.app,
        &user_bear.bear_slug,
        "session-reuse-resolved",
        Some(&user_bear.raw_token),
        json!({
            "message": "second",
            "conversation_id": "default",
            "client": "zed"
        }),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);

    let captured = fixture
        .captured_letta_body
        .lock()
        .await
        .clone()
        .expect("Letta request captured");
    assert_eq!(captured["conversation_id"], "new-acp-zed-session-reuse-resolved");
}

#[tokio::test]
async fn acp_prompt_missing_pair_returns_operator_actionable_error_without_legacy_fallback() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear_with_pair(&fixture.pool, true, false).await;

    let res = post_prompt(
        fixture.app,
        &user_bear.bear_slug,
        "session-missing-pair",
        Some(&user_bear.raw_token),
        json!({
            "message": "hello pair?",
            "client": "zed"
        }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&body).expect("JSON error body");
    assert_eq!(value["error_code"], "validation");
    let message = value["error"].as_str().expect("error message");
    assert!(message.contains("pair"), "message should name missing pair role: {message}");
    assert!(
        message.contains("Provision missing role agents"),
        "message should tell operator how to remediate: {message}"
    );
    assert!(
        fixture.captured_letta_body.lock().await.is_none(),
        "ACP must not fall back to legacy talk id or call Letta when pair is missing"
    );
}

#[tokio::test]
async fn acp_prompt_streams_to_pair_agent_and_maps_sse() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let res = post_prompt(
        fixture.app,
        &user_bear.bear_slug,
        "session-success",
        Some(&user_bear.raw_token),
        json!({
            "message": "hello bear",
            "client": "zed"
        }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream; charset=utf-8")
    );
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("SSE body is UTF-8");
    assert!(text.contains("\"type\":\"agent_message_chunk\""));
    assert!(text.contains("hello from fake Letta"));
    assert!(text.contains("\"type\":\"status\""));
    assert!(text.contains("thinking"));
    assert!(text.contains("\"type\":\"done\""));

    let captured = fixture
        .captured_letta_body
        .lock()
        .await
        .clone()
        .expect("Letta request captured");
    assert!(captured.get("session_id").is_none());
    assert_eq!(captured["conversation_id"], "new-acp-zed-session-success");
    assert_eq!(captured["agent_id"], "agent-acp-pair-test");
    assert_eq!(captured["messages"][0]["content"], "hello bear");
    assert_ne!(captured["agent_id"], "agent-acp-talk-test");
}

#[tokio::test]
async fn acp_sessions_list_requires_auth() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let res = get_acp_sessions(fixture.app.clone(), &user_bear.bear_slug, None, None).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let ok = get_acp_sessions(
        fixture.app,
        &user_bear.bear_slug,
        Some(&user_bear.raw_token),
        None,
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
}

#[tokio::test]
async fn acp_sessions_list_returns_row_after_prompt() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    let session_id = "session-bind-list";

    let prompt = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        session_id,
        Some(&user_bear.raw_token),
        json!({ "message": "yo", "client": "zed" }),
    )
    .await;
    assert_eq!(prompt.status(), StatusCode::OK);
    let _ = prompt.into_body().collect().await.unwrap();

    let list = get_acp_sessions(
        fixture.app.clone(),
        &user_bear.bear_slug,
        Some(&user_bear.raw_token),
        None,
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let body = list.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&body).expect("sessions JSON");
    let sessions = value["sessions"].as_array().expect("sessions array");
    assert!(
        sessions
            .iter()
            .any(|row| row["acp_session_id"] == session_id),
        "expected acp_session row for {session_id}: {sessions:?}"
    );

    let one = get_acp_session(
        fixture.app,
        &user_bear.bear_slug,
        session_id,
        Some(&user_bear.raw_token),
    )
    .await;
    assert_eq!(one.status(), StatusCode::OK);
    let one_body = one.into_body().collect().await.unwrap().to_bytes();
    let row: Value = serde_json::from_slice(&one_body).expect("session JSON");
    assert_eq!(row["acp_session_id"], session_id);
    assert!(row["runtime_session_id"].as_str().is_some());
    assert!(row.get("codepool_session_id").is_none());
}

#[tokio::test]
async fn acp_session_get_unknown_returns_404() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let res = get_acp_session(
        fixture.app,
        &user_bear.bear_slug,
        "no-such-session",
        Some(&user_bear.raw_token),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
