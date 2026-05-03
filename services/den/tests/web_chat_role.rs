//! Integration coverage for browser web chat role routing. Requires `DATABASE_URL`.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use den::{
    config::Config,
    core::bears::{db as bears_db, BearAgentRole},
    startup::run_sqlx_migrations,
    web,
};
use http_body_util::BodyExt;
use password_auth::generate_hash;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

#[derive(Clone)]
struct TestCodepoolState {
    captured: Arc<Mutex<Option<Value>>>,
}

struct TestApp {
    app: axum::Router,
    pool: sqlx::PgPool,
    captured_codepool_body: Arc<Mutex<Option<Value>>>,
}

struct TestUserBear {
    username: String,
    password: String,
    bear_id: Uuid,
}

async fn apply_app_migrations(pool: &sqlx::PgPool) {
    run_sqlx_migrations(pool)
        .await
        .expect("sqlx migrations for web chat role test");
}

async fn start_fake_codepool() -> (String, Arc<Mutex<Option<Value>>>) {
    let captured = Arc::new(Mutex::new(None));
    let state = TestCodepoolState {
        captured: captured.clone(),
    };
    let app = Router::new()
        .route(
            "/internal/bear_channel/sessions/{session_id}/messages",
            post(fake_bear_channel),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake codepool");
    let addr: SocketAddr = listener.local_addr().expect("fake codepool local addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("fake codepool server");
    });
    (format!("http://{addr}"), captured)
}

async fn fake_bear_channel(
    State(state): State<TestCodepoolState>,
    Path(_session_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    *state.captured.lock().await = Some(body);
    (
        [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
        concat!(
            "data: {\"type\":\"conversation_resolved\",\"conversation_id\":\"conv-web-role-test\"}\n\n",
            "data: {\"type\":\"assistant_delta\",\"text\":\"hello from fake web codepool\"}\n\n",
            "data: {\"type\":\"done\",\"outcome\":\"ok\"}\n\n"
        ),
    )
        .into_response()
}

async fn test_app() -> TestApp {
    dotenvy::dotenv().ok();
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL for web chat role test");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&database_url)
        .await
        .expect("connect postgres");
    apply_app_migrations(&pool).await;

    let (codepool_base_url, captured_codepool_body) = start_fake_codepool().await;
    let mut config = Config::load();
    config.database_url = database_url;
    config.letta_base_url = "http://fake-letta-for-role-test".to_string();
    config.codepool_base_url = codepool_base_url;

    let store = PostgresStore::new(pool.clone());
    store
        .migrate()
        .await
        .expect("tower-sessions postgres migrate");
    let app = web::server_with_state(pool.clone(), store, Arc::new(config))
        .await
        .expect("build web router");

    TestApp {
        app,
        pool,
        captured_codepool_body,
    }
}

async fn create_test_user_bear(pool: &sqlx::PgPool) -> TestUserBear {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("web{}", &suffix[..20]);
    let email = format!("{username}@example.test");
    let password = "test-password-123".to_string();
    let bear_slug = format!("web-role-test-{}", &suffix[..12]);

    let (user_id,): (i32,) = sqlx::query_as(
        r#"
        INSERT INTO users (email, username, display_name, passhash)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(&email)
    .bind(&username)
    .bind(format!("Web Role Test {username}"))
    .bind(generate_hash(&password))
    .fetch_one(pool)
    .await
    .expect("insert test user");

    let bear_id = bears_db::create_bear(
        pool,
        &bear_slug,
        "Web Role Test Bear",
        "Web chat talk-role integration test bear",
        "",
        None,
        None,
        None,
        sqlx::types::Json(Vec::<String>::new()),
    )
    .await
    .expect("create test bear");

    sqlx::query("UPDATE bears SET letta_agent_id = $2, updated_at = NOW() WHERE id = $1")
        .bind(bear_id)
        .bind("agent-legacy-web")
        .execute(pool)
        .await
        .expect("set legacy agent id");

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
    .bind(BearAgentRole::Talk.as_str())
    .bind("agent-talk-web")
    .execute(pool)
    .await
    .expect("insert talk role agent");

    bears_db::grant_membership(pool, user_id, bear_id, Some(bears_db::BEAR_ROLE_ADMIN))
        .await
        .expect("grant test membership");

    TestUserBear {
        username,
        password,
        bear_id,
    }
}

async fn login_cookie(app: axum::Router, user: &TestUserBear) -> String {
    let body = format!(
        "username={}&password={}",
        urlencoding::encode(&user.username),
        urlencoding::encode(&user.password)
    );
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login/password")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("login set-cookie");
    set_cookie
        .split(';')
        .next()
        .expect("cookie pair")
        .to_string()
}

#[tokio::test]
async fn web_chat_send_uses_talk_role_agent_id_for_codepool() {
    let fixture = test_app().await;
    let user_bear = create_test_user_bear(&fixture.pool).await;
    let cookie = login_cookie(fixture.app.clone(), &user_bear).await;

    let res = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/send")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(
                    json!({
                        "bear_id": user_bear.bear_id,
                        "conversation_id": "default",
                        "message": "hello web chat"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("SSE body is UTF-8");
    assert!(text.contains("hello from fake web codepool"));

    let captured = fixture
        .captured_codepool_body
        .lock()
        .await
        .clone()
        .expect("Codepool request captured");
    assert_eq!(captured["bear"]["letta_agent_id"], "agent-talk-web");
    assert_eq!(captured["bear"]["role_agent_id"], "agent-talk-web");
    assert_eq!(captured["bear"]["agent_role"], "talk");
    assert_eq!(captured["bear"]["runtime_family"], "letta_code_harness");
    assert_ne!(captured["bear"]["letta_agent_id"], "agent-legacy-web");
    assert_eq!(captured["channel"]["family"], "browser_chat");
    assert_eq!(captured["message"], json!({ "type": "text", "content": "hello web chat" }));
}
