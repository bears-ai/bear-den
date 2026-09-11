//! Startup route-construction regression tests.
//!
//! These intentionally do not connect to Postgres. Axum route conflicts, including
//! `route_with_tsr` trailing-slash redirect conflicts, panic while constructing the
//! router, before any request is served. Lazy pools let this test cover the same app
//! composition as startup without needing a live database.

use std::sync::Arc;

use den::{api, config::Config, web};
use sqlx::postgres::PgPoolOptions;
use tower_sessions_sqlx_store::PostgresStore;

fn lazy_pool() -> sqlx::PgPool {
    PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@localhost/den_startup_route_build_test")
        .expect("lazy Postgres pool should not connect during router construction")
}

#[tokio::test]
async fn web_router_builds_without_startup_route_conflicts() {
    let config = Arc::new(Config::test_stub());
    let pool = lazy_pool();
    let store = PostgresStore::new(pool.clone());

    let memory_stores = den_memory::MemoryStoreManager::new(config.as_ref());
    let _app = web::server_with_state(pool, store, config, memory_stores)
        .await
        .expect("web router should build without Axum route conflicts");
}

#[tokio::test]
async fn api_router_builds_without_startup_route_conflicts() {
    let config = Arc::new(Config::test_stub());
    let pool = lazy_pool();
    let store = PostgresStore::new(pool.clone());
    let peer_routers = vec![
        ("/internal", den::internal_tools::router()),
        ("/bearwire", den_bearwire::router()),
    ];

    let memory_stores = den_memory::MemoryStoreManager::new(config.as_ref());
    let state = den_service::DenState::new(
        pool,
        config.clone(),
        Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref())),
        memory_stores,
    );
    let _app = api::create_api_app(state, store, peer_routers)
        .await
        .expect("API router should build without Axum route conflicts");
}
