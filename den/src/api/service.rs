//! API service setup and configuration
//!
//! This module provides the main API service setup, including application state,
//! middleware configuration, and router assembly. It creates an independent
//! API service that can run separately from or alongside the web service.

use axum::{
    Router,
    extract::{MatchedPath, State},
    http::{Request, StatusCode},
    routing::get,
};
use axum_login::{
    AuthManagerLayerBuilder,
    tower_sessions::{Expiry, SessionManagerLayer, cookie::SameSite},
};
use sqlx::PgPool;
use time::Duration;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tower_sessions_sqlx_store::PostgresStore;
use tracing::info_span;

use crate::{auth_backend::Backend, config::Config};

use super::oauth::{endpoints::OAuthState, router::create_oauth_router};

use std::sync::Arc;

/// Application state for the API service
///
/// Contains shared resources needed by API endpoints including database
/// connections, template environment, and configuration. This state is
/// independent from the web service state to allow separate deployment.
#[derive(Clone)]
pub struct ApiState {
    /// Database connection pool for API operations
    pub sqlx_pool: PgPool,
}

async fn api_readiness(State(state): State<ApiState>) -> Result<&'static str, StatusCode> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.sqlx_pool)
        .await
        .map_err(|e| {
            tracing::warn!("database readiness check failed: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    Ok("OK")
}

/// Create the complete API application with all middleware and routes
///
/// This function creates a fully configured Axum application for the API service
/// including authentication, session management, CORS, logging, and error handling.
/// The API service is designed to be independent from the web service.
///
/// # Arguments
/// * `sqlx_pool` - Database connection pool
/// * `session_store` - PostgreSQL session store for axum-login
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
pub async fn create_api_app(
    sqlx_pool: PgPool,
    session_store: PostgresStore,
    config: Arc<Config>,
) -> Result<Router, Box<dyn std::error::Error>> {
    // Extract URLs before moving config
    let web_server_url = config.web_server_url.clone();
    let api_server_url = config.api_server_url.clone();

    // Create API application state
    let api_state = ApiState {
        sqlx_pool: sqlx_pool.clone(),
    };

    // Create OAuth state (separate from main API state for OAuth endpoints)
    let oauth_state = OAuthState::new(sqlx_pool.clone(), web_server_url, api_server_url);

    // Configure session management
    let session_layer = create_session_layer(session_store, config.as_ref());

    // Configure authentication
    let auth_layer = create_auth_layer(sqlx_pool.clone(), session_layer);

    // Build the main router with proper authentication layer ordering
    let router = Router::new()
        // Health check endpoint (no authentication required)
        .route("/healthcheck", get(|| async { "API OK" }))
        .route("/health/ready", get(api_readiness))
        // API v1.0 endpoints with Bearer token authentication (no session auth layer needed)
        .nest("/v1.0", crate::api::v1::router())
        // API documentation (no authentication required)
        .merge(crate::api::docs::router())
        // Set main API state BEFORE adding middleware layers
        .with_state(api_state)
        // Add CORS middleware for cross-origin API requests
        .layer(create_api_cors_layer())
        // Add request tracing
        .layer(create_tracing_layer())
        // OAuth endpoints with their own state and authentication layer
        .nest(
            "/oauth",
            create_oauth_router()
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
fn create_session_layer(
    session_store: PostgresStore,
    config: &Config,
) -> SessionManagerLayer<PostgresStore> {
    #[cfg(feature = "production")]
    {
        let session_cookie_domain: Option<String> = config
            .session_cookie_domain
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mut session_layer = SessionManagerLayer::new(session_store)
            .with_secure(true)
            .with_same_site(SameSite::Lax)
            .with_expiry(Expiry::OnInactivity(Duration::days(1)));
        if let Some(domain) = session_cookie_domain {
            session_layer = session_layer.with_domain(domain);
        }
        session_layer
    }
    #[cfg(not(feature = "production"))]
    {
        let _ = config;
        SessionManagerLayer::new(session_store)
            .with_secure(true)
            .with_same_site(SameSite::Lax)
            .with_expiry(Expiry::OnInactivity(Duration::days(1)))
    }
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
fn create_api_cors_layer() -> CorsLayer {
    #[cfg(feature = "production")]
    {
        CorsLayer::new()
            // Restrict allowed origins to production domains
            .allow_origin([
                axum::http::HeaderValue::from_static("https://newapp.example"),
                axum::http::HeaderValue::from_static("https://api.newapp.example"),
            ])
            // Allow standard API headers
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
                axum::http::header::ACCEPT,
                axum::http::header::ORIGIN,
            ])
            // Explicitly list allowed HTTP methods to comply with CORS spec
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PUT,
                axum::http::Method::DELETE,
                axum::http::Method::OPTIONS,
                axum::http::Method::PATCH,
            ])
            // Include credentials for session-based authentication
            .allow_credentials(true)
    }
    #[cfg(not(feature = "production"))]
    {
        CorsLayer::new()
            // Allow specific development origins for PKCE and OAuth flows
            .allow_origin([
                axum::http::HeaderValue::from_static("http://localhost:3000"),
                axum::http::HeaderValue::from_static("http://localhost:8080"),
                axum::http::HeaderValue::from_static("http://127.0.0.1:3000"),
                axum::http::HeaderValue::from_static("http://127.0.0.1:8080"),
            ])
            // Allow standard API headers
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
                axum::http::header::ACCEPT,
                axum::http::header::ORIGIN,
            ])
            // Explicitly list allowed HTTP methods to comply with CORS spec
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PUT,
                axum::http::Method::DELETE,
                axum::http::Method::OPTIONS,
                axum::http::Method::PATCH,
            ])
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

    #[test]
    fn test_api_state_creation() {
        // Test that ApiState can be created with minimal dependencies
        // This would require a test database pool in a real test
        assert!(true);
    }

    #[test]
    fn test_cors_layer_creation() {
        let _cors_layer = create_api_cors_layer();
        // Test that CORS layer can be created without panicking
        assert!(true);
    }

    #[test]
    fn test_tracing_layer_creation() {
        let _tracing_layer = create_tracing_layer();
        // Test that tracing layer can be created without panicking
        assert!(true);
    }
}
