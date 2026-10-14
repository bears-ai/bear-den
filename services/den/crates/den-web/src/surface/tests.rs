use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use axum_login::tower_sessions::{MemoryStore, Session};
use tower::ServiceExt;

fn state() -> AppState {
    let mut config = Config::test_stub();
    config.cabinet_web_origin = Some(Url::parse("https://cabinet.example").unwrap());
    config.session_cookie_domain = Some(".example".into());
    config.run_api = true;
    config.run_workers = true;
    config.run_sandbox = true;
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("cabinet-runtime-absent-{}", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(10))
        .connect_lazy("postgres://localhost:1/not-used")
        .unwrap();
    let mut state = cabinet_state(pool, Arc::new(config)).unwrap();
    let mut env = minijinja::Environment::new();
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
    env.add_template("login.html", "{{ cabinet_surface }}|{{ cabinet_home_url }}|{{ den_home_url }}|{{ public_web_origin }}|{{ native_runtime }}|{{ next }}").unwrap();
    state.template_env = env;
    state
}

fn app(state: AppState) -> Router {
    let store = PostgresStore::new(state.sqlx_pool().clone());
    cabinet_router(state, store)
}

async fn request(app: &Router, method: &str, path: &str, origin: Option<&str>) -> Response {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn cabinet_has_no_native_runtime_and_does_not_open_sqlite() {
    let state = state();
    assert_eq!(state.surface, WebSurface::Cabinet);
    assert!(!state.has_native_runtime());
    assert!(state.memory_stores().is_err());
    assert!(!std::path::Path::new(&state.config.bear_sqlite_data_dir).exists());
    assert!(state.bifrost_catalog.read().unwrap().fetched_at.is_none());
    // A legacy Den test state still has its native memory capability.
    let den = AppState::test_with_template_env(
        state.sqlx_pool().clone(),
        minijinja::Environment::new(),
        state.config.clone(),
    );
    assert!(den.has_native_runtime());
    assert!(den.memory_stores().is_ok());
}

#[tokio::test]
async fn cabinet_routes_are_isolated_from_den_and_require_local_login() {
    let app = app(state());
    for path in [
        "/admin",
        "/admin/bears",
        "/v1/chat",
        "/bear/test",
        "/bear/test/memory",
        "/work",
        "/connections",
        "/onboarding",
        "/settings",
        "/account",
        "/su/1",
        "/bearwire",
        "/api",
        "/status",
        "/metrics",
    ] {
        assert_eq!(
            request(&app, "GET", path, None).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    for path in [
        "/health",
        "/healthcheck",
        "/version",
        "/manifest.json",
        "/login",
    ] {
        assert_eq!(
            request(&app, "GET", path, None).await.status(),
            StatusCode::OK,
            "{path}"
        );
    }
    for path in [
        "/cabinet",
        "/cabinet/new",
        "/cabinet/page/history",
        "/cabinet/page/discussion",
    ] {
        let response = request(&app, "GET", path, None).await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert!(response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/login"));
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/cabinet")
                .header(header::COOKIE, "id=not-a-cabinet-session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert!(response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .starts_with("/login"));
    assert_eq!(
        request(&app, "GET", "/health/ready", None).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn cabinet_template_contract_and_login_return_url_are_local() {
    let app = app(state());
    let response = request(&app, "GET", "/login?next=https%3A%2F%2Fevil.example", None).await;
    let body =
        String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert_eq!(
        body,
        "true|/cabinet|http://localhost:3000|https://cabinet.example|false|/cabinet"
    );
    assert_eq!(local_next(Some("//evil.example")), "/cabinet");
    assert_eq!(
        local_next(Some("/cabinet/page?version=v1")),
        "/cabinet/page?version=v1"
    );
    let response = request(&app, "GET", "/manifest.json", None).await;
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["start_url"], "/cabinet");
    assert_eq!(body["short_name"], "Cabinet");
}

#[tokio::test]
async fn cabinet_mutations_reject_missing_foreign_and_null_origins_before_auth() {
    let app = app(state());
    for path in [
        "/cabinet/new",
        "/cabinet/page/review",
        "/cabinet/page/discussion",
        "/login/password",
        "/logout",
    ] {
        for origin in [
            None,
            Some("https://den.example"),
            Some("null"),
            Some("https://cabinet.example.evil"),
            Some("https://user@cabinet.example"),
        ] {
            assert_eq!(
                request(&app, "POST", path, origin).await.status(),
                StatusCode::FORBIDDEN,
                "{path}: {origin:?}"
            );
        }
    }
    assert_eq!(
        request(
            &app,
            "POST",
            "/cabinet/new",
            Some("https://cabinet.example")
        )
        .await
        .status(),
        StatusCode::TEMPORARY_REDIRECT
    );
    assert_eq!(
        request(&app, "GET", "/logout", None).await.status(),
        StatusCode::FORBIDDEN
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cabinet/new")
                .header(header::REFERER, "https://cabinet.example/cabinet/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cabinet/new")
                .header(header::ORIGIN, "https://den.example")
                .header(header::REFERER, "https://cabinet.example/cabinet/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn den_legacy_cabinet_reads_redirect_without_auth_and_mutations_are_retired() {
    let state = state();
    // Dedicated links and retired writers must need no authentication extension.
    let router = den_cabinet_router(&state.config).with_state(state.clone());
    let response = request(&router, "GET", "/cabinet/page/history?version=v1", None).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://cabinet.example/cabinet/page/history?version=v1"
    );
    for path in [
        "/cabinet",
        "/cabinet/",
        "/cabinet/new",
        "/cabinet/page/edit",
        "/cabinet/page/discussion",
        "/cabinet/page/delete",
        "/cabinet/future/writer",
    ] {
        for method in ["POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
            let response = request(&router, method, path, Some("https://den.example")).await;
            assert_eq!(response.status(), StatusCode::GONE, "{method} {path}");
            assert!(!response.headers().contains_key(header::LOCATION));
        }
        let response = request(&router, "HEAD", path, None).await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert!(to_bytes(response.into_body(), 4096)
            .await
            .unwrap()
            .is_empty());
    }
    assert_eq!(
        request(&router, "GET", "/cabinetish", None).await.status(),
        StatusCode::NOT_FOUND
    );
    let mut config = (*state.config).clone();
    config.cabinet_web_origin = None;
    let layer = AuthManagerLayerBuilder::new(
        Backend::new(state.sqlx_pool().clone()),
        SessionManagerLayer::new(MemoryStore::default()).with_secure(false),
    )
    .build();
    let router = den_cabinet_router(&config).layer(layer).with_state(state);
    let response = request(&router, "POST", "/cabinet/new", None).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert!(response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .starts_with("/login"));
    let response = request(&router, "GET", "/cabinet", None).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert!(response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .starts_with("/login"));
}

#[tokio::test]
async fn cabinet_mcp_bypasses_browser_sessions_and_keeps_its_transport_guard() {
    let app = app(state());
    // A real-looking Cabinet cookie would attempt a session DB read if the
    // browser auth layer leaked onto MCP. The lazy DB here is unreachable.
    let cookie = format!(
        "{CABINET_SESSION_COOKIE}={}",
        axum_login::tower_sessions::session::Id::default()
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers()[header::WWW_AUTHENTICATE]
        .to_str()
        .unwrap()
        .starts_with("Bearer "));
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert!(!response.headers().contains_key(header::LOCATION));
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"], "invalid_token");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::AUTHORIZATION, "Bearer invalid-credential")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::SERVICE_UNAVAILABLE
    ));
    assert!(!response.headers().contains_key(header::SET_COOKIE));

    for (path, origin) in [
        ("/mcp", Some("https://cabinet.example")),
        ("/mcp", Some("https://foreign.example")),
        ("/mcp?access_token=invalid", None),
    ] {
        let response = request(&app, "POST", path, origin).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["error"], "unsafe_transport");
    }
}

#[tokio::test]
async fn dedicated_den_keeps_admin_and_native_memory_but_never_runs_legacy_writers() {
    let cabinet = state();
    let den = AppState::test_with_template_env(
        cabinet.sqlx_pool().clone(),
        minijinja::Environment::new(),
        cabinet.config.clone(),
    );
    assert!(den.has_native_runtime());
    assert!(!den.config.owns_cabinet_recovery(den.surface));
    assert!(cabinet.config.owns_cabinet_recovery(cabinet.surface));
    let store = PostgresStore::new(den.sqlx_pool().clone());
    let app = crate::server(den, store).await.unwrap();
    for path in ["/admin/bears", "/bear/test/memory", "/reviews"] {
        let response = request(&app, "GET", path, None).await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert!(response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/login"));
    }
    // Even a valid-shaped Den cookie cannot trigger a session database read on
    // these legacy routes. They stay available during Den auth/storage outages.
    let cookie = format!("id={}", axum_login::tower_sessions::session::Id::default());
    for method in ["GET", "POST"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/cabinet/new")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if method == "GET" {
                StatusCode::TEMPORARY_REDIRECT
            } else {
                StatusCode::GONE
            }
        );
    }
}

#[tokio::test]
async fn cabinet_cookie_is_distinct_host_only_http_only_and_secure() {
    let state = state();
    let router = Router::new()
        .route(
            "/",
            get(|session: Session| async move {
                session.insert("test", true).await.unwrap();
                "OK"
            }),
        )
        .layer(cabinet_session_layer(&state, MemoryStore::default()));
    let response = request(&router, "GET", "/", None).await;
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("cabinet_session="), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("Secure"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(!cookie.contains("Domain="), "{cookie}");
}
