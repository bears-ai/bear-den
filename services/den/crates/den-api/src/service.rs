//! API service setup and configuration
//!
//! This module assembles the API router and middleware over the process-owned
//! [`DenState`] supplied by the binary composition root.

use axum::{
    extract::{MatchedPath, State},
    http::{HeaderValue, Method, Request, StatusCode},
    routing::get,
    Router,
};
use axum_login::{
    tower_sessions::{cookie::SameSite, Expiry, SessionManagerLayer},
    AuthManagerLayerBuilder,
};
use sqlx::PgPool;
use time::Duration;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tower_sessions_sqlx_store::PostgresStore;
use tracing::info_span;

use den_core::config::Config;
use den_http::auth_backend::Backend;

use den_oauth::oauth::{endpoints::OAuthState, router::create_oauth_router};

// `DenState` lives in `den-service` (below every HTTP edge) per ADR-0043, so the
// JSON/REST edge no longer depends on sibling edges for shared state. Re-exported
// so the retained `crate::service::DenState` paths in `v1`/`docs` resolve.
pub use den_service::DenState;

fn api_readiness_db_error(pool: &PgPool, error: sqlx::Error) -> StatusCode {
    tracing::warn!(
        error = %error,
        pool_size = pool.size(),
        pool_idle = pool.num_idle(),
        "database readiness check failed (api)",
    );
    StatusCode::SERVICE_UNAVAILABLE
}

async fn api_readiness(State(state): State<DenState>) -> Result<&'static str, StatusCode> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.sqlx_pool)
        .await
        .map_err(|error| api_readiness_db_error(&state.sqlx_pool, error))?;
    Ok("OK")
}

/// Create the complete API application with all middleware and routes
///
/// This function creates a fully configured Axum application for the API service
/// including authentication, session management, CORS, logging, and error handling.
/// The API service is designed to be independent from the web service.
///
/// # Arguments
/// * `sqlx_pool` - Database connection pool.
/// * `session_store` - PostgreSQL session store for axum-login.
/// * `config` - Shared process configuration used for URLs, cookies, Bifrost, and memory stores.
/// * `memory_stores` - Clone of the process-wide per-Bear memory store manager.
/// * `peer_routers` - Sibling edge routers mounted by the binary composition root.
///
/// # Returns
/// A configured Axum router ready for serving API requests
///
/// # Features
/// - OAuth 2.0 authorization server endpoints
/// - Session-based authentication via axum-login
/// - CORS support for cross-origin requests
/// - Request tracing and logging
/// - Error handling middleware
/// - Health check endpoint
///
/// # Security
/// - Uses existing authentication backend and session management
/// - Configures secure session cookies with appropriate SameSite policy
/// - Includes CORS headers for OAuth flows
/// - Integrates with existing permission system
///
/// Assemble the JSON/REST + OAuth HTTP app.
///
/// Peer edge surfaces are *injected* by the
/// binary composition root as `(mount_path, router)` pairs rather than mounted
/// here, so this crate has no compile-time dependency on any sibling edge
/// (ADR-0043: edges are peers; the binary wires them together). Injected routers
/// are nested before state/middleware so they share `DenState` and the API CORS
/// and tracing layers.
pub async fn create_api_app(
    api_state: DenState,
    session_store: PostgresStore,
    peer_routers: Vec<(&'static str, Router<DenState>)>,
) -> Result<Router, Box<dyn std::error::Error>> {
    let sqlx_pool = api_state.sqlx_pool.clone();
    let config = api_state.config.clone();
    let web_server_url = config.web_server_url.clone();
    let api_server_url = config.api_server_url.clone();

    // Create OAuth state (separate from main API state for OAuth endpoints)
    let oauth_state = OAuthState::new(sqlx_pool.clone(), web_server_url, api_server_url);

    // Configure session management
    let session_layer = create_session_layer(session_store, config.as_ref());

    // Configure authentication
    let auth_layer = create_auth_layer(sqlx_pool, session_layer);

    // Build the main router with proper authentication layer ordering.
    let mut main_router = Router::new()
        // Health check endpoint (no authentication required)
        .route("/health", get(|| async { "OK" }))
        .route("/version", get(den_http::build_info::json_handler))
        .route("/healthcheck", get(|| async { "API OK" }))
        .route("/health/ready", get(api_readiness))
        // API v1.0 endpoints with Bearer token authentication (no session auth layer needed)
        .nest("/v1.0", crate::v1::router())
        // API documentation (no authentication required)
        .merge(crate::docs::router());

    // Mount peer edge routers injected by the composition root.
    for (mount_path, router) in peer_routers {
        main_router = main_router.nest(mount_path, router);
    }

    let router = main_router
        // Set main API state BEFORE adding middleware layers
        .with_state(api_state.clone())
        // Add CORS middleware for cross-origin API requests
        .layer(create_api_cors_layer(config.as_ref()))
        // Add request tracing
        .layer(create_tracing_layer())
        // OAuth endpoints with their own state and authentication layer
        .nest(
            "/oauth",
            create_oauth_router(config.as_ref())
                .with_state(oauth_state)
                .layer(auth_layer),
        );

    Ok(router)
}

/// Create session management layer
///
/// Configures session management using the same PostgreSQL store as the web service
/// but with API-appropriate settings. Sessions are shared between web and API
/// services to maintain authentication state.
///
/// # Arguments
/// * `session_store` - PostgreSQL session store
///
/// # Returns
/// Configured session manager layer
fn base_session_layer(session_store: PostgresStore) -> SessionManagerLayer<PostgresStore> {
    SessionManagerLayer::new(session_store)
        .with_secure(den_core::config::session_cookie_secure_from_env(true))
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)))
}

fn create_session_layer(
    session_store: PostgresStore,
    config: &Config,
) -> SessionManagerLayer<PostgresStore> {
    let session_layer = base_session_layer(session_store);
    #[cfg(feature = "production")]
    {
        let session_cookie_domain = config
            .session_cookie_domain
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(domain) = session_cookie_domain {
            return session_layer.with_domain(domain);
        }
    }
    #[cfg(not(feature = "production"))]
    {
        let _ = config;
    }
    session_layer
}

/// Create authentication layer
///
/// Sets up axum-login authentication using the existing Backend implementation.
/// This ensures consistency with the web service authentication system.
///
/// # Arguments
/// * `sqlx_pool` - Database connection pool
/// * `session_layer` - Session management layer
///
/// # Returns
/// Configured authentication manager layer
fn create_auth_layer(
    sqlx_pool: PgPool,
    session_layer: SessionManagerLayer<PostgresStore>,
) -> axum_login::AuthManagerLayer<Backend, PostgresStore> {
    let backend = Backend::new(sqlx_pool);
    AuthManagerLayerBuilder::new(backend, session_layer).build()
}

/// Create CORS layer for API endpoints
///
/// Configures CORS to allow cross-origin requests to the API service.
/// This is essential for OAuth flows and API access from web applications
/// hosted on different domains.
///
/// # CORS Configuration
/// - Allows all origins (should be restricted in production)
/// - Allows standard API headers
/// - Allows all HTTP methods
/// - Includes credentials for session-based authentication
///
/// # Returns
/// Configured CORS layer
fn api_cors_headers() -> [axum::http::HeaderName; 4] {
    [
        axum::http::header::AUTHORIZATION,
        axum::http::header::CONTENT_TYPE,
        axum::http::header::ACCEPT,
        axum::http::header::ORIGIN,
    ]
}

fn api_cors_methods() -> [Method; 6] {
    [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::OPTIONS,
        Method::PATCH,
    ]
}

fn create_api_cors_layer(config: &Config) -> CorsLayer {
    #[cfg(feature = "production")]
    {
        let origins: Vec<HeaderValue> = config
            .cors_allowed_origins()
            .into_iter()
            .filter_map(|o| HeaderValue::from_str(&o).ok())
            .collect();
        if origins.is_empty() {
            tracing::error!(
                "CORS: no allowed origins from WEB_SERVER_URL / API_SERVER_URL; check configuration."
            );
        }
        CorsLayer::new()
            // Restrict allowed origins to configured public web + API URLs
            .allow_origin(origins)
            // Allow standard API headers
            .allow_headers(api_cors_headers())
            // Explicitly list allowed HTTP methods to comply with CORS spec
            .allow_methods(api_cors_methods())
            // Include credentials for session-based authentication
            .allow_credentials(true)
    }
    #[cfg(not(feature = "production"))]
    {
        let mut dev_origins: Vec<HeaderValue> = vec![
            HeaderValue::from_static("http://localhost:3000"),
            HeaderValue::from_static("http://localhost:8080"),
            HeaderValue::from_static("http://127.0.0.1:3000"),
            HeaderValue::from_static("http://127.0.0.1:8080"),
        ];
        for o in config.cors_allowed_origins() {
            if let Ok(h) = HeaderValue::from_str(&o) {
                if !dev_origins.contains(&h) {
                    dev_origins.push(h);
                }
            }
        }
        CorsLayer::new()
            // Typical localhost ports plus any `WEB_SERVER_URL` / `API_SERVER_URL` origins
            .allow_origin(dev_origins)
            // Allow standard API headers
            .allow_headers(api_cors_headers())
            // Explicitly list allowed HTTP methods to comply with CORS spec
            .allow_methods(api_cors_methods())
            // Include credentials for PKCE and session-based authentication
            .allow_credentials(true)
    }
}

/// Create request tracing layer
///
/// Sets up request tracing and logging for API endpoints using the same
/// patterns as the web service. This provides consistent logging across
/// both services.
///
/// # Returns
/// Configured tracing layer
fn create_tracing_layer() -> TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
    impl Fn(&Request<axum::body::Body>) -> tracing::Span + Clone,
> {
    TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
        let matched_path = request
            .extensions()
            .get::<MatchedPath>()
            .map(MatchedPath::as_str);

        info_span!(
            "api_request",
            method = ?request.method(),
            matched_path,
            path = ?request.uri().path(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use den_core::config::Config;

    #[test]
    fn test_cors_layer_creation() {
        // Exercises CORS layer construction; passes as long as it does not panic.
        let _cors_layer = create_api_cors_layer(&Config::test_stub());
    }

    #[test]
    fn test_tracing_layer_creation() {
        // Exercises tracing layer construction; passes as long as it does not panic.
        let _tracing_layer = create_tracing_layer();
    }
}
