//! Integration tests for web HTTP endpoints. Requires `DATABASE_URL` and applied migrations.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use newapp::{api, config::Config, web};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

async fn web_app() -> axum::Router {
    dotenvy::dotenv().ok();
    let config = Arc::new(Config::load());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .expect("DATABASE_URL must be set with a migrated database for integration tests");
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
        .expect("DATABASE_URL must be set with a migrated database for integration tests");
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
