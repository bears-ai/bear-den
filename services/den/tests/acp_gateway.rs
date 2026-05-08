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
        acp_tokens,
        bears::{db as bears_db, BearAgentRole},
        work_plans::{
            self, WorkPlanItem, WorkPlanItemStatus, WorkPlanStatus, WorkPlanUpdate, WorkPlanUpsert,
            WorkPlanVisibility,
        },
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
use regex::Regex;

#[derive(Clone)]
struct TestLettaState {
    captured: Arc<Mutex<Option<Value>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    script: Arc<Mutex<Vec<String>>>,
}

struct TestApp {
    app: axum::Router,
    pool: sqlx::PgPool,
    captured_letta_body: Arc<Mutex<Option<Value>>>,
    letta_requests: Arc<Mutex<Vec<Value>>>,
    letta_script: Arc<Mutex<Vec<String>>>,
}

struct TestUserBear {
    user_id: i32,
    bear_id: Uuid,
    bear_slug: String,
    pair_agent_id: String,
    raw_token: String,
}

async fn apply_app_migrations(pool: &sqlx::PgPool) {
    run_sqlx_migrations(pool)
        .await
        .expect("sqlx migrations for ACP integration test");
}

async fn start_fake_letta() -> (
    String,
    Arc<Mutex<Option<Value>>>,
    Arc<Mutex<Vec<Value>>>,
    Arc<Mutex<Vec<String>>>,
) {
    let captured = Arc::new(Mutex::new(None));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let script = Arc::new(Mutex::new(Vec::new()));
    let state = TestLettaState {
        captured: captured.clone(),
        requests: requests.clone(),
        script: script.clone(),
    };
    let app = Router::new()
        .route("/v1/conversations/", post(fake_letta_create_conversation))
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
        axum::serve(listener, app).await.expect("fake Letta server");
    });
    (format!("http://{addr}"), captured, requests, script)
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

    let (letta_base_url, captured_letta_body, letta_requests, letta_script) =
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
        letta_script,
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
    body: Value,
) -> axum::response::Response {
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

async fn post_tool_result(
    app: axum::Router,
    slug: &str,
    session_id: &str,
    tool_call_id: &str,
    token: Option<&str>,
    body: Value,
) -> axum::response::Response {
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

fn letta_tool_request_sse(tool_name: &str, tool_call_id: &str, args: Value) -> String {
    format!(
        "data: {}\n\n",
        json!({
            "id": format!("approval-{tool_call_id}"),
            "message_type": "approval_request_message",
            "tool_call": {
                "name": tool_name,
                "tool_call_id": tool_call_id,
                "arguments": args.to_string(),
            }
        })
    )
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
    assert!(content.starts_with("hello bear"));
    assert!(content.contains("Den workboard context"));
    assert!(content.contains("ACP context plan"));
    assert!(content.contains("den.work_plan.update"));
    assert!(!content.contains("private_path"));
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
