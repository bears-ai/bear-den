//! Integration tests for web HTTP endpoints. Requires `DATABASE_URL` (empty DB is fine).
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use den::{api, config::Config, startup::run_sqlx_migrations, web};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

async fn apply_app_migrations(pool: &sqlx::PgPool) {
    run_sqlx_migrations(pool)
        .await
        .expect("sqlx migrations for integration test");
}

async fn web_app() -> axum::Router {
    dotenvy::dotenv().ok();
    let config = Arc::new(Config::load());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .expect("DATABASE_URL must be set for integration tests");
    apply_app_migrations(&pool).await;
    let store = PostgresStore::new(pool.clone());
    store
        .migrate()
        .await
        .expect("tower-sessions postgres migrate");
    web::server_with_state(pool, store, config)
        .await
        .expect("build web router")
}

async fn api_app() -> axum::Router {
    dotenvy::dotenv().ok();
    let config = Arc::new(Config::load());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .expect("DATABASE_URL must be set for integration tests");
    apply_app_migrations(&pool).await;
    let store = PostgresStore::new(pool.clone());
    store
        .migrate()
        .await
        .expect("tower-sessions postgres migrate");
    api::create_api_app(pool, store, config)
        .await
        .expect("build api router")
}

#[tokio::test]
async fn web_health_returns_ok() {
    let app = web_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"OK");
}

#[tokio::test]
async fn web_version_returns_json() {
    let app = web_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/version")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(v["service"], "den");
    assert!(v["version"].as_str().is_some());
    let built = v["built_at_utc"].as_str().expect("built_at_utc string");
    assert!(
        built.len() >= 20 && built.ends_with('Z'),
        "expected RFC3339 UTC, got {built:?}"
    );
    assert!(v["git_sha"].as_str().is_some());
}

#[tokio::test]
async fn web_healthcheck_returns_ok() {
    let app = web_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/healthcheck")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"OK");
}

#[tokio::test]
async fn web_readiness_returns_ok_when_db_up() {
    let app = web_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn web_status_json_is_routed() {
    let app = web_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/status.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        res.status(),
        StatusCode::NOT_FOUND,
        "GET /status.json must be registered"
    );
    assert!(
        matches!(res.status(), StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE),
        "expected 200 or 503 from status JSON, got {}",
        res.status()
    );
}

#[tokio::test]
async fn web_status_page_is_routed() {
    let app = web_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::NOT_FOUND, "GET /status must be registered");
}

#[tokio::test]
async fn web_health_bears_redirects_to_status() {
    let app = web_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/health/bears")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::MOVED_PERMANENTLY);
    let loc = res.headers().get(axum::http::header::LOCATION);
    assert_eq!(loc.map(|v| v.to_str().unwrap()), Some("/status"));
}

/// Bear chat (`/bear/{slug}`) loads Deep Chat from `/assets/deep-chat/*`; these must not 404.
#[tokio::test]
async fn web_deep_chat_vendor_assets_are_served() {
    let app = web_app().await;
    for path in [
        "/assets/deep-chat/deepChat.bundle.js",
    ] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "expected 200 for {path}"
        );
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(
            body.len() > 100,
            "expected non-trivial body for {path}, got {} bytes",
            body.len()
        );
    }
}

#[tokio::test]
async fn api_version_returns_json() {
    let app = api_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/version")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(v["service"], "den");
    assert!(v["version"].as_str().is_some());
    let built = v["built_at_utc"].as_str().expect("built_at_utc string");
    assert!(
        built.len() >= 20 && built.ends_with('Z'),
        "expected RFC3339 UTC, got {built:?}"
    );
    assert!(v["git_sha"].as_str().is_some());
}

#[tokio::test]
async fn api_health_returns_ok() {
    let app = api_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"OK");
}

#[tokio::test]
async fn api_healthcheck_returns_ok() {
    let app = api_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/healthcheck")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_readiness_returns_ok_when_db_up() {
    let app = api_app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
