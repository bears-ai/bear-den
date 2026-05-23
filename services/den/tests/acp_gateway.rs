//! Integration coverage for the ACP API gateway. Requires `DATABASE_URL`.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use den::{
    api,
    config::Config,
    core::{
        acp_sessions, acp_tokens,
        bears::{db as bears_db, BearAgentRole},
        work_plans::{
            self, WorkPlanItem, WorkPlanItemStatus, WorkPlanStatus, WorkPlanUpdate, WorkPlanUpsert,
            WorkPlanVisibility,
        },
    },
    startup::run_sqlx_migrations,
};
use http_body_util::BodyExt;
use regex::Regex;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

#[derive(Clone)]
struct TestLettaState {
    captured: Arc<Mutex<Option<Value>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    cancel_requests: Arc<Mutex<Vec<Value>>>,
    script: Arc<Mutex<Vec<String>>>,
}

struct TestApp {
    app: axum::Router,
    pool: sqlx::PgPool,
    captured_letta_body: Arc<Mutex<Option<Value>>>,
    letta_requests: Arc<Mutex<Vec<Value>>>,
    letta_cancel_requests: Arc<Mutex<Vec<Value>>>,
    letta_script: Arc<Mutex<Vec<String>>>,
    _db_permit: SemaphorePermit<'static>,
}

struct TestUserBear {
    user_id: i32,
    bear_id: Uuid,
    bear_slug: String,
    pair_agent_id: String,
    raw_token: String,
}

static DB_TEST_PERMITS: Semaphore = Semaphore::const_new(4);
static MIGRATION_LOCK: Mutex<()> = Mutex::const_new(());

async fn apply_app_migrations(pool: &sqlx::PgPool) {
    run_sqlx_migrations(pool)
        .await
        .expect("sqlx migrations for ACP integration test");
}

async fn start_fake_web_server() -> String {
    async fn fake_page() -> impl IntoResponse {
        (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<html><body><h1>Fake docs</h1><p>web fetch fixture body</p></body></html>",
        )
    }

    let app = Router::new().route("/docs", axum::routing::get(fake_page));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake web server");
    let addr: SocketAddr = listener.local_addr().expect("fake web local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("fake web server");
    });
    format!("http://{addr}")
}

async fn start_fake_letta() -> (
    String,
    Arc<Mutex<Option<Value>>>,
    Arc<Mutex<Vec<Value>>>,
    Arc<Mutex<Vec<Value>>>,
    Arc<Mutex<Vec<String>>>,
) {
    let captured = Arc::new(Mutex::new(None));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let cancel_requests = Arc::new(Mutex::new(Vec::new()));
    let script = Arc::new(Mutex::new(Vec::new()));
    let state = TestLettaState {
        captured: captured.clone(),
        requests: requests.clone(),
        cancel_requests: cancel_requests.clone(),
        script: script.clone(),
    };
    let app = Router::new()
        .route("/v1/conversations/", post(fake_letta_create_conversation))
        .route(
            "/v1/conversations/{conversation_id}/messages",
            post(fake_letta_conversation_messages),
        )
        .route(
            "/v1/agents/{agent_id}/messages/cancel",
            post(fake_letta_cancel_agent_runs),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Letta");
    let addr: SocketAddr = listener.local_addr().expect("fake Letta local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("fake Letta server");
    });
    (
        format!("http://{addr}"),
        captured,
        requests,
        cancel_requests,
        script,
    )
}

async fn fake_letta_create_conversation(
    Query(_params): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    Json(json!({ "id": "conv-fake-resolved123" }))
}

async fn fake_letta_conversation_messages(
    State(state): State<TestLettaState>,
    Path(conversation_id): Path<String>,
    Json(mut body): Json<Value>,
) -> Response {
    body["conversation_id"] = json!(conversation_id);
    state.requests.lock().await.push(body.clone());
    *state.captured.lock().await = Some(body.clone());
    let scripted = {
        let mut script = state.script.lock().await;
        if script.is_empty() {
            None
        } else {
            Some(script.remove(0))
        }
    };
    let response = scripted.unwrap_or_else(|| {
        concat!(
            "data: {\"message_type\":\"conversation_resolved\",\"conversation_id\":\"conv-fake-resolved123\"}\n\n",
            "data: {\"message_type\":\"assistant_message\",\"content\":\"hello from fake Letta\"}\n\n",
            "data: {\"message_type\":\"reasoning_message\",\"reasoning\":\"thinking\"}\n\n",
            "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
        )
        .to_string()
    });
    (
        [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
        response,
    )
        .into_response()
}

async fn fake_letta_cancel_agent_runs(
    State(state): State<TestLettaState>,
    Path(agent_id): Path<String>,
    Json(mut body): Json<Value>,
) -> Json<Value> {
    body["agent_id"] = json!(agent_id);
    state.cancel_requests.lock().await.push(body);
    Json(json!({ "cancelled": true }))
}

async fn test_app() -> TestApp {
    let db_permit = DB_TEST_PERMITS
        .acquire()
        .await
        .expect("DB test semaphore should not close");
    dotenvy::dotenv().ok();
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL for ACP integration test");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(15))
        .connect(&database_url)
        .await
        .expect("connect postgres");
    {
        let _migration_guard = MIGRATION_LOCK.lock().await;
        apply_app_migrations(&pool).await;
    }

    let (letta_base_url, captured_letta_body, letta_requests, letta_cancel_requests, letta_script) =
        start_fake_letta().await;
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
        letta_requests,
        letta_cancel_requests,
        letta_script,
        _db_permit: db_permit,
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
    let pair_agent_id = format!("agent-acp-{}", &suffix[..8]);

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
        .bind(&pair_agent_id)
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
        pair_agent_id,
        raw_token: created.raw_token,
    }
}

async fn create_acp_session_work_plan(
    pool: &sqlx::PgPool,
    user_bear: &TestUserBear,
    session_id: &str,
) {
    work_plans::create_or_update_work_plan(
        pool,
        WorkPlanUpsert {
            bear_id: user_bear.bear_id,
            owner_role: BearAgentRole::Pair,
            owner_agent_id: Some(user_bear.pair_agent_id.clone()),
            created_by_user_id: Some(user_bear.user_id),
            source_conversation_id: Some("conv-acp-workboard-context".to_string()),
            source_acp_session_id: Some(session_id.to_string()),
            source_channel: json!({ "protocol": "acp" }),
            plan_id: None,
            expected_version: None,
            update: WorkPlanUpdate {
                title: "ACP context plan".to_string(),
                summary: "Visible in pair prompt".to_string(),
                visibility: WorkPlanVisibility::PrivateToRole,
                status: WorkPlanStatus::Active,
                items: vec![WorkPlanItem {
                    id: "current".to_string(),
                    title: "Inject ACP workboard".to_string(),
                    summary: None,
                    status: WorkPlanItemStatus::InProgress,
                    blocked_reason: None,
                    source_refs: Vec::new(),
                }],
                workspace_context: json!({ "private_path": "/tmp/secret" }),
            },
        },
    )
    .await
    .expect("create ACP session work plan");
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
    if body.get("adapter_contract").is_none() {
        body["adapter_contract"] = json!({ "name": "bears.acp.adapter", "version": 1 });
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

async fn post_permission_result(
    app: axum::Router,
    slug: &str,
    session_id: &str,
    permission_id: &str,
    token: Option<&str>,
    mut body: Value,
) -> axum::response::Response {
    if body.get("adapter_contract").is_none() {
        body["adapter_contract"] = json!({ "name": "bears.acp.adapter", "version": 1 });
    }
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/acp/bears/{slug}/sessions/{session_id}/permissions/{permission_id}"
        ))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("ACP permission result response")
}

async fn post_adapter_environment(
    app: axum::Router,
    slug: &str,
    session_id: &str,
    token: Option<&str>,
    mut body: Value,
) -> axum::response::Response {
    if body.get("adapter_contract").is_none() {
        body["adapter_contract"] = json!({ "name": "bears.acp.adapter", "version": 1 });
    }
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/acp/bears/{slug}/sessions/{session_id}/adapter-environment"
        ))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("ACP adapter environment response")
}

async fn post_tool_result(
    app: axum::Router,
    slug: &str,
    session_id: &str,
    tool_call_id: &str,
    token: Option<&str>,
    mut body: Value,
) -> axum::response::Response {
    if body.get("adapter_contract").is_none() {
        body["adapter_contract"] = json!({ "name": "bears.acp.adapter", "version": 1 });
    }
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/acp/bears/{slug}/sessions/{session_id}/tool-results/{tool_call_id}"
        ))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("ACP tool result response")
}

async fn post_cancel_session(
    app: axum::Router,
    slug: &str,
    session_id: &str,
    token: Option<&str>,
    mut body: Value,
) -> axum::response::Response {
    if body.get("adapter_contract").is_none() {
        body["adapter_contract"] = json!({ "name": "bears.acp.adapter", "version": 1 });
    }
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/acp/bears/{slug}/sessions/{session_id}/cancel"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("ACP cancel session response")
}

async fn get_acp_session_runtime(
    app: axum::Router,
    slug: &str,
    session_id: &str,
    token: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("GET")
        .uri(format!("/acp/bears/{slug}/sessions/{session_id}/runtime"));
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("ACP session runtime response")
}

fn letta_tool_request_sse_value(
    tool_name: &str,
    tool_call_id: &str,
    args: Value,
    run_id: Option<&str>,
) -> String {
    let mut event = json!({
        "id": format!("approval-{tool_call_id}"),
        "message_type": "approval_request_message",
        "tool_call": {
            "name": tool_name,
            "tool_call_id": tool_call_id,
            "arguments": args.to_string(),
        }
    });
    if let Some(run_id) = run_id {
        event["run_id"] = json!(run_id);
    }
    format!("data: {event}\n\n")
}

fn letta_tool_request_sse(tool_name: &str, tool_call_id: &str, args: Value) -> String {
    letta_tool_request_sse_value(tool_name, tool_call_id, args, None)
}

fn letta_tool_request_sse_with_run_id(
    tool_name: &str,
    tool_call_id: &str,
    args: Value,
    run_id: &str,
) -> String {
    letta_tool_request_sse_value(tool_name, tool_call_id, args, Some(run_id))
}

fn letta_malformed_tool_request_sse(tool_name: &str, tool_call_id: &str, args: Value) -> String {
    format!(
        "data: {}\n\n",
        json!({
            "message_type": "tool_call_message",
            "tool_call": {
                "name": tool_name,
                "tool_call_id": tool_call_id,
                "arguments": args.to_string(),
            }
        })
    )
}

fn letta_stop_sse() -> String {
    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n".to_string()
}

async fn response_text(response: axum::response::Response) -> String {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(body.to_vec()).expect("response body is UTF-8")
}

async fn read_response_until(response: &mut axum::response::Response, needle: &str) -> String {
    let mut text = String::new();
    while !text.contains(needle) {
        let frame = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            response.body_mut().frame(),
        )
        .await
        .expect("timed out waiting for streaming response frame")
        .expect("streaming response ended before expected text")
        .expect("streaming response frame");
        if let Some(data) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
    text
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
    assert_eq!(value["error_code"], "missing_authorization");
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
    assert_eq!(captured["conversation_id"], "conv-fake-resolved123");
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
    assert_eq!(captured["conversation_id"], "conv-fake-resolved123");
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
    assert!(
        message.contains("pair"),
        "message should name missing pair role: {message}"
    );
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
    create_acp_session_work_plan(&fixture.pool, &user_bear, "session-success").await;

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
    assert!(text.contains("\"type\":\"assistant_text_delta\""));
    assert!(text.contains("hello from fake Letta"));
    assert!(text.contains("\"type\":\"status_text\""));
    assert!(text.contains("thinking"));
    assert!(text.contains("\"type\":\"turn_complete\""));

    let captured = fixture
        .captured_letta_body
        .lock()
        .await
        .clone()
        .expect("Letta request captured");
    assert!(captured.get("session_id").is_none());
    assert_eq!(captured["conversation_id"], "conv-fake-resolved123");
    assert_eq!(captured["agent_id"], user_bear.pair_agent_id);
    let content = captured["messages"][0]["content"]
        .as_str()
        .expect("prompt content string");
    assert_eq!(content, "hello bear");
    assert!(!content.contains("<system-reminder>"));
    assert!(!content.contains("Den workboard context"));
    assert!(!content.contains("ACP context plan"));
    assert!(!content.contains("den.work_plan.update"));
    assert!(!content.contains("private_path"));
    assert!(!content.contains("multiple work surfaces"));
    assert!(!content.contains("A Workplace is the role-scoped memory surface"));
    assert!(
        !content.contains("Prefer work-surface-first retrieval for local-understanding questions")
    );
    assert!(captured.get("override_system").is_none());
    assert_ne!(captured["agent_id"], "agent-acp-talk-test");
}

#[tokio::test]
async fn acp_prompt_advertises_all_read_only_tool_descriptors() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let res = post_prompt(
        fixture.app,
        &user_bear.bear_slug,
        "session-tool-descriptors",
        Some(&user_bear.raw_token),
        json!({ "message": "what tools exist?", "client": "zed" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let _ = response_text(res).await;

    let captured = fixture
        .captured_letta_body
        .lock()
        .await
        .clone()
        .expect("Letta request captured");
    let client_tools = captured["client_tools"]
        .as_array()
        .expect("client_tools array");
    let names = client_tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"fs_read_text_file"));
    assert!(names.contains(&"fs_list_directory"));
    assert!(names.contains(&"fs_search_files"));
    assert!(names.contains(&"fs_replace_text"));
    assert!(names.contains(&"situation_get"));
    assert!(names.contains(&"web_search"));
    assert!(names.contains(&"memory_read"));
    assert!(!names.contains(&"den_situation_get"));
    assert!(!names.contains(&"den_web_search"));
    assert!(!names.contains(&"den_memory_read"));
    assert!(names
        .iter()
        .all(|name| !name.contains('.') && !name.contains('/')));
}

#[tokio::test]
async fn acp_read_text_file_tool_request_round_trips_result_to_letta() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "fs_read_text_file",
                "call-read-e2e",
                json!({ "path": "/tmp/acp-workspace/README.md", "line": 1, "limit": 10 })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"read complete\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let prompt_task = tokio::spawn(async move {
        post_prompt(
            app_for_prompt,
            &slug,
            "session-read-e2e",
            Some(&token),
            json!({ "message": "read a file", "client": "zed" }),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-read-e2e",
        "call-read-e2e",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-read-e2e",
            "tool_name": "fs_read_text_file",
            "approval_request_id": "approval-call-read-e2e",
            "status": "ok",
            "content": "# README\n",
            "structured_content": { "path": "/tmp/acp-workspace/README.md" },
            "diagnostic": { "source": "test" }
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_json: Value = serde_json::from_str(&response_text(result).await).unwrap();
    assert_eq!(result_json["accepted"], true, "{result_json}");

    let prompt = prompt_task.await.unwrap();
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = response_text(prompt).await;
    assert!(text.contains("\"type\":\"tool_request\""));
    assert!(text.contains("fs_read_text_file"));
    assert!(text.contains("Local tool fs_read_text_file completed"));
    assert!(text.contains("read complete"));

    let requests = fixture.letta_requests.lock().await.clone();
    assert_eq!(
        requests.len(),
        2,
        "expected original prompt and tool return"
    );
    assert_eq!(requests[1]["messages"][0]["type"], "approval");
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_call_id"],
        "call-read-e2e"
    );
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_return"],
        "# README\n"
    );
}

#[tokio::test]
async fn acp_cancel_session_signals_active_stream_and_cleans_pending_tool() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture
        .letta_script
        .lock()
        .await
        .push(letta_tool_request_sse_with_run_id(
            "fs_read_text_file",
            "call-cancel-e2e",
            json!({ "path": "/tmp/acp-workspace/README.md" }),
            "run-cancel-e2e",
        ));

    let mut prompt = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-cancel-e2e",
        Some(&user_bear.raw_token),
        json!({ "message": "read and then wait", "client": "zed" }),
    )
    .await;
    assert_eq!(prompt.status(), StatusCode::OK);
    let first = read_response_until(&mut prompt, "\"type\":\"tool_request\"").await;
    assert!(first.contains("call-cancel-e2e"), "{first}");

    let runtime = get_acp_session_runtime(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-cancel-e2e",
        Some(&user_bear.raw_token),
    )
    .await;
    assert_eq!(runtime.status(), StatusCode::OK);
    let runtime_json: Value = serde_json::from_str(&response_text(runtime).await).unwrap();
    assert_eq!(
        runtime_json["runtime"]["state"],
        json!("requires_action"),
        "{runtime_json}"
    );
    assert_eq!(
        runtime_json["runtime"]["active_turn"]["present"],
        json!(true),
        "{runtime_json}"
    );
    assert_eq!(
        runtime_json["runtime"]["active_turn"]["pending_adapter_tools"],
        json!(1),
        "{runtime_json}"
    );
    assert_eq!(
        runtime_json["runtime"]["active_turn"]["run_ids"],
        json!(["run-cancel-e2e"]),
        "{runtime_json}"
    );
    assert_eq!(
        runtime_json["runtime"]["source"],
        json!("acp_active_turn_registry"),
        "{runtime_json}"
    );
    assert_eq!(
        runtime_json["stream_turn"]["active"],
        json!(true),
        "{runtime_json}"
    );
    assert_eq!(
        runtime_json["stream_turn"]["turn"]["run_ids"],
        json!(["run-cancel-e2e"]),
        "{runtime_json}"
    );
    assert_eq!(
        runtime_json["context_budget"]["status"],
        json!("unavailable"),
        "{runtime_json}"
    );

    let cancel = post_cancel_session(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-cancel-e2e",
        Some(&user_bear.raw_token),
        json!({}),
    )
    .await;
    assert_eq!(cancel.status(), StatusCode::OK);
    let cancel_json: Value = serde_json::from_str(&response_text(cancel).await).unwrap();
    assert_eq!(cancel_json["ok"], true, "{cancel_json}");
    assert_eq!(cancel_json["cancelled"], true, "{cancel_json}");
    assert_eq!(
        cancel_json["stream_turn"]["acp_session_id"],
        json!("session-cancel-e2e"),
        "{cancel_json}"
    );
    assert_eq!(
        cancel_json["stream_turn"]["run_ids"],
        json!(["run-cancel-e2e"]),
        "{cancel_json}"
    );
    assert_eq!(
        cancel_json["active_turn"]["session_id"],
        json!("session-cancel-e2e"),
        "{cancel_json}"
    );
    assert_eq!(cancel_json["cancel_result"]["ok"], true, "{cancel_json}");
    assert_eq!(
        cancel_json["cancel_result"]["skipped"], false,
        "explicit ACP cancel should target known Letta run_ids: {cancel_json}"
    );
    assert_eq!(
        cancel_json["cancel_result"]["run_ids"],
        json!(["run-cancel-e2e"]),
        "{cancel_json}"
    );

    let late_result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-cancel-e2e",
        "call-cancel-e2e",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-cancel-e2e",
            "tool_name": "fs_read_text_file",
            "approval_request_id": "approval-call-cancel-e2e",
            "status": "cancelled",
            "content": "cancelled by adapter"
        }),
    )
    .await;
    assert_eq!(late_result.status(), StatusCode::OK);
    let late_result_json: Value = serde_json::from_str(&response_text(late_result).await).unwrap();
    assert_eq!(late_result_json["accepted"], false, "{late_result_json}");
    assert_eq!(
        late_result_json["reason"],
        json!("late_result_ignored"),
        "{late_result_json}"
    );

    let cancelled = read_response_until(&mut prompt, "\"status\":\"cancelled\"").await;
    assert!(
        cancelled.contains("\"type\":\"turn_result\""),
        "{cancelled}"
    );
    assert!(
        cancelled.contains("\"reason\":\"cancelled\""),
        "{cancelled}"
    );

    let cancel_requests = fixture.letta_cancel_requests.lock().await.clone();
    assert_eq!(
        cancel_requests.len(),
        1,
        "explicit ACP cancel with known run_ids should send exactly one targeted Letta cancel request; got {cancel_requests:?}"
    );
    assert_eq!(
        cancel_requests[0]["agent_id"],
        json!(user_bear.pair_agent_id),
        "{cancel_requests:?}"
    );
    assert_eq!(
        cancel_requests[0]["run_ids"],
        json!(["run-cancel-e2e"]),
        "{cancel_requests:?}"
    );
}

#[tokio::test]
async fn acp_tool_result_late_response_normalization_endpoint_cases() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    let missing = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-late-missing",
        "call-missing",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-missing",
            "tool_name": "fs_read_text_file",
            "status": "ok",
            "content": "late missing result"
        }),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::OK);
    let missing_json: Value = serde_json::from_str(&response_text(missing).await).unwrap();
    assert_eq!(missing_json["accepted"], false, "{missing_json}");
    assert_eq!(
        missing_json["reason"],
        json!("late_result_ignored"),
        "{missing_json}"
    );
    assert_eq!(
        missing_json["settlement"],
        json!("unknown"),
        "{missing_json}"
    );

    fixture
        .letta_script
        .lock()
        .await
        .push(letta_tool_request_sse(
            "fs_read_text_file",
            "call-late-already",
            json!({ "path": "/tmp/acp-workspace/already.md" }),
        ));
    let mut already_prompt = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-late-already",
        Some(&user_bear.raw_token),
        json!({ "message": "read then duplicate", "client": "zed" }),
    )
    .await;
    assert_eq!(already_prompt.status(), StatusCode::OK);
    let already_first = read_response_until(&mut already_prompt, "call-late-already").await;
    assert!(
        already_first.contains("\"type\":\"tool_request\""),
        "{already_first}"
    );
    let accepted = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-late-already",
        "call-late-already",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-late-already",
            "tool_name": "fs_read_text_file",
            "approval_request_id": "approval-call-late-already",
            "status": "ok",
            "content": "first result"
        }),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted_json: Value = serde_json::from_str(&response_text(accepted).await).unwrap();
    assert_eq!(accepted_json["accepted"], true, "{accepted_json}");
    let duplicate = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-late-already",
        "call-late-already",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-late-already",
            "tool_name": "fs_read_text_file",
            "approval_request_id": "approval-call-late-already",
            "status": "ok",
            "content": "duplicate result"
        }),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    let duplicate_json: Value = serde_json::from_str(&response_text(duplicate).await).unwrap();
    assert_eq!(duplicate_json["accepted"], false, "{duplicate_json}");
    assert_eq!(
        duplicate_json["reason"],
        json!("late_result_ignored"),
        "{duplicate_json}"
    );
    assert_eq!(
        duplicate_json["settlement"],
        json!("already_settled"),
        "{duplicate_json}"
    );

    for (session_id, call_id, first_status, expected_settlement) in [
        (
            "session-late-timeout",
            "call-late-timeout",
            "timeout",
            "timed_out",
        ),
        (
            "session-late-cancelled",
            "call-late-cancelled",
            "cancelled",
            "cancelled",
        ),
    ] {
        fixture
            .letta_script
            .lock()
            .await
            .push(letta_tool_request_sse(
                "fs_read_text_file",
                call_id,
                json!({ "path": format!("/tmp/acp-workspace/{call_id}.md") }),
            ));
        let mut prompt = post_prompt(
            fixture.app.clone(),
            &user_bear.bear_slug,
            session_id,
            Some(&user_bear.raw_token),
            json!({ "message": format!("read then {first_status}"), "client": "zed" }),
        )
        .await;
        assert_eq!(prompt.status(), StatusCode::OK);
        let first = read_response_until(&mut prompt, call_id).await;
        assert!(first.contains("\"type\":\"tool_request\""), "{first}");
        let first_result = post_tool_result(
            fixture.app.clone(),
            &user_bear.bear_slug,
            session_id,
            call_id,
            Some(&user_bear.raw_token),
            json!({
                "tool_call_id": call_id,
                "tool_name": "fs_read_text_file",
                "approval_request_id": format!("approval-{call_id}"),
                "status": first_status,
                "content": format!("first {first_status} result")
            }),
        )
        .await;
        assert_eq!(first_result.status(), StatusCode::OK);
        let first_result_json: Value =
            serde_json::from_str(&response_text(first_result).await).unwrap();
        assert_eq!(first_result_json["accepted"], true, "{first_result_json}");
        let settled_output =
            read_response_until(&mut prompt, "Local tool fs_read_text_file completed").await;
        assert!(
            settled_output.contains("Local tool fs_read_text_file completed"),
            "{settled_output}"
        );

        let late = post_tool_result(
            fixture.app.clone(),
            &user_bear.bear_slug,
            session_id,
            call_id,
            Some(&user_bear.raw_token),
            json!({
                "tool_call_id": call_id,
                "tool_name": "fs_read_text_file",
                "approval_request_id": format!("approval-{call_id}"),
                "status": "ok",
                "content": "late after settled"
            }),
        )
        .await;
        assert_eq!(late.status(), StatusCode::OK);
        let late_json: Value = serde_json::from_str(&response_text(late).await).unwrap();
        assert_eq!(late_json["accepted"], false, "{late_json}");
        assert_eq!(
            late_json["reason"],
            json!("late_result_ignored"),
            "{late_json}"
        );
        assert_eq!(
            late_json["settlement"],
            json!(expected_settlement),
            "{late_json}"
        );
    }
}

#[tokio::test]
async fn acp_list_directory_tool_request_round_trips_result_to_letta() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "fs_list_directory",
                "call-list-e2e",
                json!({ "path": "/tmp/acp-workspace", "limit": 2 })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"list complete\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let prompt_task = tokio::spawn(async move {
        post_prompt(
            app_for_prompt,
            &slug,
            "session-list-e2e",
            Some(&token),
            json!({ "message": "list files", "client": "zed" }),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-list-e2e",
        "call-list-e2e",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-list-e2e",
            "tool_name": "fs_list_directory",
            "approval_request_id": "approval-call-list-e2e",
            "status": "ok",
            "content": "file\t/tmp/acp-workspace/a.txt",
            "structured_content": {
                "entries": [{ "path": "/tmp/acp-workspace/a.txt", "kind": "file" }],
                "truncated": false
            },
            "diagnostic": { "source": "test" }
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_json: Value = serde_json::from_str(&response_text(result).await).unwrap();
    assert_eq!(result_json["accepted"], true, "{result_json}");

    let prompt = prompt_task.await.unwrap();
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = response_text(prompt).await;
    assert!(text.contains("fs_list_directory"));
    assert!(text.contains("\"max_entries\":1000"));
    assert!(text.contains("list complete"));

    let requests = fixture.letta_requests.lock().await.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_call_id"],
        "call-list-e2e"
    );
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["status"],
        "success"
    );
}

#[tokio::test]
async fn acp_search_files_tool_request_round_trips_result_to_letta() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "fs_search_files",
                "call-search-e2e",
                json!({ "path": "/tmp/acp-workspace", "query": "needle", "limit": 5 })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"search complete\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let prompt_task = tokio::spawn(async move {
        post_prompt(
            app_for_prompt,
            &slug,
            "session-search-e2e",
            Some(&token),
            json!({ "message": "search files", "client": "zed" }),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-search-e2e",
        "call-search-e2e",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-search-e2e",
            "tool_name": "fs_search_files",
            "approval_request_id": "approval-call-search-e2e",
            "status": "ok",
            "content": "/tmp/acp-workspace/a.txt:1: needle",
            "structured_content": {
                "matches": [{ "path": "/tmp/acp-workspace/a.txt", "line": 1, "preview": "needle" }],
                "truncated": false
            },
            "diagnostic": { "source": "test" }
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_json: Value = serde_json::from_str(&response_text(result).await).unwrap();
    assert_eq!(result_json["accepted"], true, "{result_json}");

    let prompt = prompt_task.await.unwrap();
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = response_text(prompt).await;
    assert!(text.contains("fs_search_files"));
    assert!(text.contains("\"max_results\":200"));
    assert!(text.contains("search complete"));

    let requests = fixture.letta_requests.lock().await.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_call_id"],
        "call-search-e2e"
    );
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_return"],
        "/tmp/acp-workspace/a.txt:1: needle"
    );
}

#[tokio::test]
async fn acp_replace_text_tool_request_round_trips_result_to_letta() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "fs_replace_text",
                "call-replace-e2e",
                json!({
                    "path": "/tmp/acp-workspace/a.txt",
                    "old_text": "before",
                    "new_text": "after"
                })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"replace complete\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let prompt_task = tokio::spawn(async move {
        post_prompt(
            app_for_prompt,
            &slug,
            "session-replace-e2e",
            Some(&token),
            json!({ "message": "replace text", "client": "zed" }),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-replace-e2e",
        "call-replace-e2e",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-replace-e2e",
            "tool_name": "fs_replace_text",
            "approval_request_id": "approval-call-replace-e2e",
            "status": "ok",
            "content": "Replaced 1 occurrence in /tmp/acp-workspace/a.txt",
            "structured_content": {
                "path": "/tmp/acp-workspace/a.txt",
                "replacements": 1
            },
            "diagnostic": { "source": "test" }
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_json: Value = serde_json::from_str(&response_text(result).await).unwrap();
    assert_eq!(result_json["accepted"], true);

    let prompt = prompt_task.await.unwrap();
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = response_text(prompt).await;
    assert!(text.contains("fs_replace_text"));
    assert!(text.contains("\"risk\":\"writes_workspace\""));
    assert!(text.contains("replace complete"));

    let requests = fixture.letta_requests.lock().await.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_call_id"],
        "call-replace-e2e"
    );
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["status"],
        "success"
    );
}

#[tokio::test]
async fn acp_create_text_file_tool_request_round_trips_result_to_letta() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "fs_create_text_file",
                "call-create-e2e",
                json!({
                    "path": "/tmp/acp-workspace/new.txt",
                    "content": "hello\n",
                    "create_parent_dirs": false
                })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"create complete\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let prompt_task = tokio::spawn(async move {
        post_prompt(
            app_for_prompt,
            &slug,
            "session-create-e2e",
            Some(&token),
            json!({ "message": "create file", "client": "zed" }),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-create-e2e",
        "call-create-e2e",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-create-e2e",
            "tool_name": "fs_create_text_file",
            "approval_request_id": "approval-call-create-e2e",
            "status": "ok",
            "content": "Created text file /tmp/acp-workspace/new.txt (6 bytes)",
            "structured_content": {
                "path": "/tmp/acp-workspace/new.txt",
                "created": true,
                "bytes": 6
            },
            "diagnostic": { "source": "test" }
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_json: Value = serde_json::from_str(&response_text(result).await).unwrap();
    assert_eq!(result_json["accepted"], true);

    let prompt = prompt_task.await.unwrap();
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = response_text(prompt).await;
    assert!(text.contains("fs_create_text_file"));
    assert!(text.contains("\"risk\":\"writes_workspace\""));
    assert!(text.contains("\"create_files\":true"));
    assert!(text.contains("create complete"));

    let requests = fixture.letta_requests.lock().await.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_call_id"],
        "call-create-e2e"
    );
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["status"],
        "success"
    );
}

#[tokio::test]
async fn acp_delete_path_tool_request_round_trips_result_to_letta() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "fs_delete_path",
                "call-delete-e2e",
                json!({
                    "path": "/tmp/acp-workspace/old.txt",
                    "expected_kind": "file"
                })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"delete complete\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let prompt_task = tokio::spawn(async move {
        post_prompt(
            app_for_prompt,
            &slug,
            "session-delete-e2e",
            Some(&token),
            json!({ "message": "delete file", "client": "zed" }),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-delete-e2e",
        "call-delete-e2e",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-delete-e2e",
            "tool_name": "fs_delete_path",
            "approval_request_id": "approval-call-delete-e2e",
            "status": "ok",
            "content": "Deleted file /tmp/acp-workspace/old.txt",
            "structured_content": {
                "path": "/tmp/acp-workspace/old.txt",
                "deleted": true,
                "kind": "file"
            },
            "diagnostic": { "source": "test" }
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_json: Value = serde_json::from_str(&response_text(result).await).unwrap();
    assert_eq!(result_json["accepted"], true);

    let prompt = prompt_task.await.unwrap();
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = response_text(prompt).await;
    assert!(text.contains("fs_delete_path"));
    assert!(text.contains("\"risk\":\"deletes_workspace\""));
    assert!(text.contains("\"max_entries\":100"));
    assert!(text.contains("delete complete"));

    let requests = fixture.letta_requests.lock().await.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_call_id"],
        "call-delete-e2e"
    );
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["status"],
        "success"
    );
}

#[tokio::test]
async fn acp_tool_malformed_args_surface_error_without_registration() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.push(format!(
        "{}{}",
        letta_malformed_tool_request_sse(
            "fs_search_files",
            "call-search-bad-args",
            json!({ "path": "/tmp/acp-workspace" })
        ),
        letta_stop_sse()
    ));

    let res = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-malformed-tool",
        Some(&user_bear.raw_token),
        json!({ "message": "bad search", "client": "zed" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let text = response_text(res).await;
    assert!(text.contains("invalid_tool_arguments"));
    assert!(text.contains("fs_search_files"));
    assert!(text.contains("query"));

    let result = post_tool_result(
        fixture.app,
        &user_bear.bear_slug,
        "session-malformed-tool",
        "call-search-bad-args",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-search-bad-args",
            "tool_name": "fs_search_files",
            "status": "ok",
            "content": "should not be accepted"
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_json: Value = serde_json::from_str(&response_text(result).await).unwrap();
    assert_eq!(result_json["accepted"], false);
    assert_eq!(result_json["reason"], "turn_missing");
}

#[tokio::test]
async fn acp_web_fetch_permission_allow_host_persists_and_continues() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "web_fetch",
                "call-web-fetch-host",
                json!({ "url": "https://example.com/docs" })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"fetch approved\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let prompt_task = tokio::spawn(async move {
        post_prompt(
            app_for_prompt,
            &slug,
            "session-web-fetch-host",
            Some(&token),
            json!({ "message": "fetch docs", "client": "zed" }),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // First response should contain permission_request and generated permission id.
    let prompt = prompt_task.await.unwrap();
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = response_text(prompt).await;
    assert!(text.contains("permission_request"));
    assert!(text.contains("web_fetch"));
    assert!(text.contains("allow_host"));
    let permission_id = Regex::new(r#"\"permission_id\":\"([^\"]+)\""#)
        .unwrap()
        .captures(&text)
        .and_then(|captures| captures.get(1).map(|m| m.as_str().to_string()))
        .expect("permission id in stream");

    let res = post_permission_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-web-fetch-host",
        &permission_id,
        Some(&user_bear.raw_token),
        json!({ "decision": "allow_host" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "{}", response_text(res).await);

    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM bear_web_approvals
        WHERE bear_id = $1
          AND scope_kind = 'host'
          AND scope_value = 'example.com'
          AND revoked_at IS NULL
        "#,
    )
    .bind(user_bear.bear_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("approval count");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn acp_web_fetch_reject_once_continues_as_permission_denied_tool_result() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "web_fetch",
                "call-web-fetch-reject",
                json!({ "url": "https://example.com/reject-me" })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"fetch denied handled\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let mut prompt = post_prompt(
        app_for_prompt,
        &slug,
        "session-web-fetch-reject",
        Some(&token),
        json!({ "message": "fetch docs", "client": "zed" }),
    )
    .await;
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = read_response_until(&mut prompt, "permission_request").await;
    assert!(text.contains("permission_request"));
    let permission_id = Regex::new(r#"\"permission_id\":\"([^\"]+)\""#)
        .unwrap()
        .captures(&text)
        .and_then(|captures| captures.get(1).map(|m| m.as_str().to_string()))
        .expect("permission id in stream");

    let res = post_permission_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-web-fetch-reject",
        &permission_id,
        Some(&user_bear.raw_token),
        json!({ "decision": "reject_once" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "{}", response_text(res).await);

    let body: Value = serde_json::from_str(&response_text(res).await).unwrap();
    assert_eq!(body["accepted"], true);
    assert_eq!(body["reason"], "delivered");

    let remaining_text = response_text(prompt).await;
    assert!(remaining_text.contains("Local tool web_fetch completed with status permission_denied"));
    assert!(remaining_text.contains("fetch denied handled"));

    let requests = fixture.letta_requests.lock().await.clone();
    assert_eq!(
        requests.len(),
        2,
        "expected prompt and tool-result continuation"
    );
    assert_eq!(requests[1]["messages"][0]["type"], "approval");
    assert_eq!(requests[1]["messages"][0]["approve"], false);
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["type"],
        "approval"
    );
    assert_eq!(requests[1]["messages"][0]["approvals"][0]["approve"], false);
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["reason"],
        "web_fetch permission denied"
    );
}

#[tokio::test]
async fn acp_web_fetch_allow_host_approves_future_local_delegation_and_audit() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    let target_base = start_fake_web_server().await;
    let first_url = format!("{target_base}/docs");
    let second_url = format!("{target_base}/docs?again=1");

    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "web_fetch",
                "call-web-fetch-local-first",
                json!({ "url": first_url })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"local fetch returned\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
        format!(
            "{}{}",
            letta_tool_request_sse(
                "web_fetch",
                "call-web-fetch-local-second",
                json!({ "url": second_url })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"second local fetch returned\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let mut prompt = post_prompt(
        app_for_prompt,
        &slug,
        "session-web-fetch-local",
        Some(&token),
        json!({ "message": "fetch local docs", "client": "zed" }),
    )
    .await;
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = read_response_until(&mut prompt, "permission_request").await;
    assert!(text.contains("permission_request"));
    let permission_id = Regex::new(r#"\"permission_id\":\"([^\"]+)\""#)
        .unwrap()
        .captures(&text)
        .and_then(|captures| captures.get(1).map(|m| m.as_str().to_string()))
        .expect("permission id in stream");

    let permission = post_permission_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-web-fetch-local",
        &permission_id,
        Some(&user_bear.raw_token),
        json!({ "decision": "allow_host" }),
    )
    .await;
    assert_eq!(permission.status(), StatusCode::OK);
    let permission_json: Value = serde_json::from_str(&response_text(permission).await).unwrap();
    assert_eq!(permission_json["accepted"], true);
    assert_eq!(permission_json["reason"], "local_tool_required");
    assert_eq!(
        permission_json["local_tool_request"]["tool_name"],
        "local_web_fetch"
    );
    assert_eq!(
        permission_json["local_tool_request"]["tool_call_id"],
        "call-web-fetch-local-first"
    );

    let first_result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-web-fetch-local",
        "call-web-fetch-local-first",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-web-fetch-local-first",
            "tool_name": "local_web_fetch",
            "approval_request_id": "approval-call-web-fetch-local-first",
            "status": "ok",
            "content": "Fake docs\nweb fetch fixture body",
            "structured_content": { "status": 200, "url": first_url }
        }),
    )
    .await;
    assert_eq!(first_result.status(), StatusCode::OK);
    let first_result_json: Value =
        serde_json::from_str(&response_text(first_result).await).unwrap();
    assert_eq!(first_result_json["accepted"], true);
    let first_remaining = response_text(prompt).await;
    assert!(first_remaining.contains("Local tool local_web_fetch completed with status ok"));
    assert!(first_remaining.contains("local fetch returned"));

    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM bear_web_approvals
        WHERE bear_id = $1
          AND scope_kind = 'host'
          AND scope_value LIKE '127.0.0.1:%'
          AND revoked_at IS NULL
        "#,
    )
    .bind(user_bear.bear_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("local host approval count");
    assert_eq!(count, 1);

    let second = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-web-fetch-local",
        Some(&user_bear.raw_token),
        json!({ "message": "fetch local docs again", "client": "zed" }),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    let second_text = response_text(second).await;
    assert!(second_text.contains("\"type\":\"tool_request\""));
    assert!(second_text.contains("local_web_fetch"));
    assert!(!second_text.contains("permission_request"));

    let second_result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-web-fetch-local",
        "call-web-fetch-local-second",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-web-fetch-local-second",
            "tool_name": "local_web_fetch",
            "approval_request_id": "approval-call-web-fetch-local-second",
            "status": "ok",
            "content": "Fake docs second",
            "structured_content": { "status": 200, "url": second_url }
        }),
    )
    .await;
    assert_eq!(second_result.status(), StatusCode::OK);

    sqlx::query(
        r#"
        INSERT INTO bear_web_fetches (
            bear_id, session_id, tool_call_id, url, final_url, host,
            execution_location, approval_kind, http_status, content_type, bytes
        )
        VALUES ($1, 'session-web-fetch-local', 'call-web-fetch-local-first', $2, $2, '127.0.0.1', 'adapter_local', 'user_host', 200, 'text/html', 28),
               ($1, 'session-web-fetch-local', 'call-web-fetch-local-second', $3, $3, '127.0.0.1', 'adapter_local', 'user_host', 200, 'text/html', 16)
        "#,
    )
    .bind(user_bear.bear_id)
    .bind(&first_url)
    .bind(&second_url)
    .execute(&fixture.pool)
    .await
    .expect("insert simulated local fetch audit rows");

    let fetch_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM bear_web_fetches WHERE bear_id = $1 AND execution_location = 'adapter_local'",
    )
    .bind(user_bear.bear_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("local fetch audit count");
    assert_eq!(fetch_count, 2);
}

#[tokio::test]
async fn acp_web_fetch_blocked_source_denies_without_permission_request_and_audits() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    sqlx::query(
        r#"
        INSERT INTO bear_web_sources (bear_id, scope_kind, scope_value, policy)
        VALUES ($1, 'host', 'blocked.example', 'blocked')
        "#,
    )
    .bind(user_bear.bear_id)
    .execute(&fixture.pool)
    .await
    .expect("insert blocked source");
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "web_fetch",
                "call-web-fetch-blocked",
                json!({ "url": "https://blocked.example/docs" })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"blocked handled\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let response = post_prompt(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-web-fetch-blocked",
        Some(&user_bear.raw_token),
        json!({ "message": "fetch blocked", "client": "zed" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let text = response_text(response).await;
    assert!(!text.contains("permission_request"));
    assert!(text.contains("Local tool web_fetch completed with status error"));
    assert!(text.contains("blocked handled"));

    let fetch_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM bear_web_fetches
        WHERE bear_id = $1
          AND host = 'blocked.example'
          AND approval_kind = 'denied'
          AND http_status IS NULL
        "#,
    )
    .bind(user_bear.bear_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("blocked audit count");
    assert_eq!(fetch_count, 1);
}

#[tokio::test]
async fn acp_tool_permission_denied_result_continues_as_error_return() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    fixture.letta_script.lock().await.extend([
        format!(
            "{}{}",
            letta_tool_request_sse(
                "fs_list_directory",
                "call-list-denied",
                json!({ "path": "/tmp/acp-workspace" })
            ),
            letta_stop_sse()
        ),
        "data: {\"message_type\":\"assistant_message\",\"content\":\"permission handled\"}\n\n\
         data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
            .to_string(),
    ]);

    let app_for_prompt = fixture.app.clone();
    let slug = user_bear.bear_slug.clone();
    let token = user_bear.raw_token.clone();
    let prompt_task = tokio::spawn(async move {
        post_prompt(
            app_for_prompt,
            &slug,
            "session-permission-denied",
            Some(&token),
            json!({ "message": "list denied", "client": "zed" }),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = post_tool_result(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-permission-denied",
        "call-list-denied",
        Some(&user_bear.raw_token),
        json!({
            "tool_call_id": "call-list-denied",
            "tool_name": "fs_list_directory",
            "approval_request_id": "approval-call-list-denied",
            "status": "permission_denied",
            "content": "permission denied by client",
            "structured_content": {},
            "diagnostic": { "source": "test" }
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_json: Value = serde_json::from_str(&response_text(result).await).unwrap();
    assert_eq!(result_json["accepted"], true, "{result_json}");

    let prompt = prompt_task.await.unwrap();
    assert_eq!(prompt.status(), StatusCode::OK);
    let text = response_text(prompt).await;
    assert!(text.contains("Local tool fs_list_directory completed with status permission_denied"));
    assert!(text.contains("permission handled"));

    let requests = fixture.letta_requests.lock().await.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["status"],
        "error"
    );
    assert_eq!(
        requests[1]["messages"][0]["approvals"][0]["tool_return"],
        "permission denied by client"
    );
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
async fn acp_session_responses_expose_plan_mode_as_session_mode() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    let session_id = "session-plan-mode-visible";

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

    let entered = den::core::acp_plan_mode::enter_plan_mode(
        &fixture.pool,
        den::core::acp_plan_mode::EnterPlanModeParams {
            user_id: user_bear.user_id,
            bear_id: user_bear.bear_id,
            bear_slug: user_bear.bear_slug.clone(),
            acp_session_id: session_id.to_string(),
            reason: "Need a reviewed implementation plan".to_string(),
            requested_by: den::core::acp_plan_mode::AcpPlanModeRequestedBy::Pair,
            previous_permission_mode: Some("default".to_string()),
        },
    )
    .await
    .expect("enter ACP plan mode");

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
    let list_row = value["sessions"]
        .as_array()
        .expect("sessions array")
        .iter()
        .find(|row| row["acp_session_id"] == session_id)
        .cloned()
        .expect("session row present");
    assert_eq!(list_row["plan_mode"]["id"], entered.id.to_string());
    assert_eq!(list_row["modes"][0]["slug"], "plan");
    assert_eq!(list_row["modes"][0]["kind"], "mutation_gate");
    assert_eq!(list_row["modes"][0]["state"], "review_required");
    assert_eq!(
        list_row["modes"][0]["metadata"]["mutation_gate"]["state"],
        "review_required"
    );
    assert_eq!(
        list_row["session_policy"]["mutation_gate"]["state"],
        "review_required"
    );
    assert_eq!(
        list_row["modes"][1]["metadata"]["plan_mode_id"],
        entered.id.to_string()
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
    assert_eq!(row["plan_mode"]["id"], entered.id.to_string());
    assert_eq!(row["modes"][0]["slug"], "plan");
    assert_eq!(row["modes"][0]["kind"], "mutation_gate");
    assert_eq!(row["modes"][0]["state"], "review_required");
    assert_eq!(row["modes"][0]["source"], "den.session_policy");
    assert_eq!(row["modes"][1]["source"], "den.acp_plan_mode");
    assert_eq!(
        row["session_policy"]["mutation_gate"]["state"],
        "review_required"
    );
}

#[tokio::test]
async fn acp_session_responses_default_to_ask_policy_without_plan_mode() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;
    let session_id = "session-ask-mode-visible";

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
    assert!(row["plan_mode"].is_null() || row.get("plan_mode").is_none());
    assert_eq!(row["modes"][0]["slug"], "ask");
    assert_eq!(row["modes"][0]["kind"], "mutation_gate");
    assert_eq!(row["modes"][0]["state"], "closed");
    assert_eq!(row["modes"][0]["source"], "den.session_policy");
    assert_eq!(row["session_policy"]["mode_label"], "Ask");
    assert_eq!(row["session_policy"]["mutation_gate"]["state"], "closed");
    assert_eq!(
        row["session_policy"]["allowed_tool_classes"],
        json!(["read_only"])
    );
}

#[tokio::test]
async fn acp_adapter_environment_is_stored_and_exposed_in_runtime() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool, true).await;

    acp_sessions::upsert_session(
        &fixture.pool,
        acp_sessions::UpsertAcpSession {
            user_id: user_bear.user_id,
            bear_id: user_bear.bear_id,
            bear_slug: user_bear.bear_slug.clone(),
            acp_session_id: "session-env".to_string(),
            runtime_session_id: "runtime-env".to_string(),
            conversation_id: "conv-env".to_string(),
            resolved_conversation_id: Some("conv-env".to_string()),
            client: "zed".to_string(),
            cwd: Some("/workspace".to_string()),
            current_mode: Some("ask".to_string()),
        },
    )
    .await
    .expect("upsert ACP session");

    let snapshot = json!({
        "bear": { "identity": "Builder Bear" },
        "runtime": { "kind": "acp_adapter" },
        "session": { "id": "session-env", "cwd": "/workspace" },
        "diagnostics": { "status": "ok" }
    });

    let post = post_adapter_environment(
        fixture.app.clone(),
        &user_bear.bear_slug,
        "session-env",
        Some(&user_bear.raw_token),
        json!({ "environment": snapshot }),
    )
    .await;
    assert_eq!(post.status(), StatusCode::OK);

    let runtime = get_acp_session_runtime(
        fixture.app,
        &user_bear.bear_slug,
        "session-env",
        Some(&user_bear.raw_token),
    )
    .await;
    assert_eq!(runtime.status(), StatusCode::OK);
    let body: Value = serde_json::from_str(&response_text(runtime).await).expect("runtime JSON");
    assert_eq!(
        body["adapter_environment"]["runtime"]["kind"],
        "acp_adapter"
    );
    assert_eq!(body["adapter_environment"]["session"]["id"], "session-env");
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
