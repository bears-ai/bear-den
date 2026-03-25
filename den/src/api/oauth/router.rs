//! OAuth 2.0 router for API endpoints
//!
//! This module provides the router configuration for OAuth 2.0 endpoints,
//! including authorization, token exchange, and user info endpoints.
//! It integrates with the existing axum-login authentication system and
//! provides CORS support for cross-origin OAuth flows.

use axum::{
    Router,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;

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
pub fn create_oauth_router() -> Router<OAuthState> {
    Router::new()
        // Authorization endpoint - handles OAuth authorization flow initiation
        .route("/authorize", get(authorize_get).post(authorize_post))
        // Token endpoint - exchanges authorization codes for access tokens
        .route("/token", post(token_post))
        // User info endpoint - returns user information for valid tokens
        .route("/userinfo", get(userinfo_get))
        // Add CORS middleware for cross-origin OAuth requests
        .layer(create_oauth_cors_layer())
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
fn create_oauth_cors_layer() -> CorsLayer {
    CorsLayer::new()
        // Restrict allowed origins to production domains
        .allow_origin([
            axum::http::HeaderValue::from_static("https://newapp.example"),
            axum::http::HeaderValue::from_static("https://api.newapp.example"),
        ])
        // Allow standard OAuth headers
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_oauth_router_creation() {
        let _router = create_oauth_router();

        // Test that the router can be created without panicking
        assert!(true);
    }

    #[test]
    fn test_cors_layer_creation() {
        let _cors_layer = create_oauth_cors_layer();

        // Test that the CORS layer can be created without panicking
        assert!(true);
    }
}
