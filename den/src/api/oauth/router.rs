//! OAuth 2.0 router for API endpoints
//!
//! This module provides the router configuration for OAuth 2.0 endpoints,
//! including authorization, token exchange, and user info endpoints.
//! It integrates with the existing axum-login authentication system and
//! provides CORS support for cross-origin OAuth flows.

use axum::{
    Router,
    http::HeaderValue,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;

use crate::config::Config;

use super::endpoints::{OAuthState, authorize_get, authorize_post, token_post, userinfo_get};

/// Create the OAuth router with all OAuth 2.0 endpoints
///
/// This function creates a complete OAuth 2.0 router that includes:
/// - Authorization endpoint (GET/POST /oauth/authorize)
/// - Token endpoint (POST /oauth/token)
/// - User info endpoint (GET /oauth/userinfo)
///
/// The router includes CORS middleware to support cross-origin requests
/// from web applications implementing OAuth flows.
///
/// # Returns
/// An Axum router configured with OAuth endpoints and middleware
///
/// # Security
/// - Authorization endpoints require user authentication via axum-login
/// - Token endpoint validates client credentials and authorization codes
/// - User info endpoint validates Bearer tokens
/// - CORS is configured to allow cross-origin OAuth flows
pub fn create_oauth_router(config: &Config) -> Router<OAuthState> {
    Router::new()
        // Authorization endpoint - handles OAuth authorization flow initiation
        .route("/authorize", get(authorize_get).post(authorize_post))
        // Token endpoint - exchanges authorization codes for access tokens
        .route("/token", post(token_post))
        // User info endpoint - returns user information for valid tokens
        .route("/userinfo", get(userinfo_get))
        // Add CORS middleware for cross-origin OAuth requests
        .layer(create_oauth_cors_layer(config))
}

/// Create CORS layer for OAuth endpoints
///
/// Configures CORS to allow OAuth flows from web applications.
/// This is essential for OAuth flows where the client application
/// is hosted on a different domain than the authorization server.
///
/// # CORS Configuration
/// - Allows all origins (in production, this should be restricted)
/// - Allows standard OAuth headers (Authorization, Content-Type)
/// - Allows OAuth methods (GET, POST, OPTIONS)
/// - Includes credentials for session-based authentication
///
/// # Returns
/// A configured CORS layer for OAuth endpoints
fn create_oauth_cors_layer(config: &Config) -> CorsLayer {
    #[cfg(feature = "production")]
    {
        let origins: Vec<HeaderValue> = config
            .cors_allowed_origins()
            .into_iter()
            .filter_map(|o| HeaderValue::from_str(&o).ok())
            .collect();
        if origins.is_empty() {
            tracing::error!(
                "OAuth CORS: no allowed origins from WEB_SERVER_URL / API_SERVER_URL."
            );
        }
        CorsLayer::new()
            .allow_origin(origins)
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
                axum::http::header::ACCEPT,
            ])
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PUT,
                axum::http::Method::DELETE,
                axum::http::Method::OPTIONS,
                axum::http::Method::PATCH,
            ])
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
            .allow_origin(dev_origins)
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
                axum::http::header::ACCEPT,
            ])
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PUT,
                axum::http::Method::DELETE,
                axum::http::Method::OPTIONS,
                axum::http::Method::PATCH,
            ])
            .allow_credentials(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn test_oauth_router_creation() {
        let _router = create_oauth_router(&Config::test_stub());

        // Test that the router can be created without panicking
        assert!(true);
    }

    #[test]
    fn test_cors_layer_creation() {
        let _cors_layer = create_oauth_cors_layer(&Config::test_stub());

        // Test that the CORS layer can be created without panicking
        assert!(true);
    }
}
