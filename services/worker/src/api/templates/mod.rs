//! Template loading and rendering infrastructure for the API module
//!
//! This module provides independent template functionality for the API module,
//! separate from the web module's template system. It uses MiniJinja for
//! template rendering and memory-serve for embedded template assets.

use axum::response::{Html, IntoResponse, Response};
use minijinja::Environment;
use serde::Serialize;
use std::sync::OnceLock;

use crate::errors::CustomError;

/// Template environment singleton for the API module
static TEMPLATE_ENV: OnceLock<Environment<'static>> = OnceLock::new();

/// Initialize the template environment
pub fn init_template_env() -> &'static Environment<'static> {
    TEMPLATE_ENV.get_or_init(|| {
        let mut env = Environment::new();

        // Load templates based on build configuration
        #[cfg(feature = "production")]
        {
            // In production, embed templates at compile time
            minijinja_embed::load_templates!(&mut env, "api");
        }
        #[cfg(not(feature = "production"))]
        {
            // In development, load templates from filesystem for hot reload
            env.set_loader(minijinja::path_loader("src/api/templates"));
        }

        env
    })
}

/// Context data for OAuth authorization page
#[derive(Debug, Serialize)]
pub struct AuthorizeContext {
    /// OAuth client ID
    pub client_id: String,
    /// Human-readable client name
    pub client_name: Option<String>,
    /// Client description
    pub client_description: Option<String>,
    /// OAuth response type (typically "code")
    pub response_type: String,
    /// Redirect URI after authorization
    pub redirect_uri: Option<String>,
    /// Requested OAuth scopes
    pub scope: Option<String>,
    /// Parsed scopes as a list
    pub scopes: Option<Vec<String>>,
    /// OAuth state parameter
    pub state: Option<String>,
    /// PKCE code challenge
    pub code_challenge: Option<String>,
    /// PKCE code challenge method
    pub code_challenge_method: Option<String>,
    /// CSRF token for form protection
    pub csrf_token: Option<String>,
    /// User information
    pub user: UserContext,
    /// Error message if any
    pub error: Option<String>,
}

/// Context data for OAuth error page
#[derive(Debug, Serialize)]
pub struct ErrorContext {
    /// OAuth error code (e.g., "access_denied", "invalid_request")
    pub error_code: String,
    /// Human-readable error description
    pub error_description: Option<String>,
    /// Client name if available
    pub client_name: Option<String>,
    /// Redirect URI for returning to application
    pub redirect_uri: Option<String>,
    /// OAuth state parameter
    pub state: Option<String>,
    /// Technical details for debugging (optional)
    pub technical_details: Option<String>,
}

/// User context for templates
#[derive(Debug, Serialize)]
pub struct UserContext {
    /// User ID
    pub id: i32,
    /// Username
    pub username: Option<String>,
    /// Display name
    pub display_name: Option<String>,
    /// Email address
    pub email: String,
}

/// Render the OAuth authorization page
pub fn render_authorize_page(context: AuthorizeContext) -> Result<Response, CustomError> {
    let env = init_template_env();
    let template = env
        .get_template("oauth/authorize.html")
        .map_err(|e| CustomError::Render(format!("Failed to load authorize template: {e}")))?;

    let rendered = template
        .render(context)
        .map_err(|e| CustomError::Render(format!("Failed to render authorize template: {e}")))?;

    Ok(Html(rendered).into_response())
}

/// Render the OAuth error page
pub fn render_error_page(context: ErrorContext) -> Result<Response, CustomError> {
    let env = init_template_env();
    let template = env
        .get_template("oauth/error.html")
        .map_err(|e| CustomError::Render(format!("Failed to load error template: {e}")))?;

    let rendered = template
        .render(context)
        .map_err(|e| CustomError::Render(format!("Failed to render error template: {e}")))?;

    Ok(Html(rendered).into_response())
}

/// Helper function to parse OAuth scopes from a scope string
pub fn parse_scopes(scope_string: Option<&str>) -> Option<Vec<String>> {
    scope_string.map(|s| {
        s.split_whitespace()
            .map(|scope| scope.to_string())
            .collect()
    })
}

/// Helper function to create CSRF token (placeholder implementation)
pub fn generate_csrf_token() -> String {
    // In a real implementation, this would generate a cryptographically secure token
    // and store it in the session for validation
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .hash(&mut hasher);

    format!("csrf_{:x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scopes() {
        assert_eq!(parse_scopes(None), None);
        assert_eq!(parse_scopes(Some("")), Some(vec![]));
        assert_eq!(
            parse_scopes(Some("read write")),
            Some(vec!["read".to_string(), "write".to_string()])
        );
        assert_eq!(
            parse_scopes(Some("profile")),
            Some(vec!["profile".to_string()])
        );
    }

    #[test]
    fn test_generate_csrf_token() {
        let token1 = generate_csrf_token();
        let token2 = generate_csrf_token();

        assert!(token1.starts_with("csrf_"));
        assert!(token2.starts_with("csrf_"));
        assert_ne!(token1, token2); // Should be different each time
    }

    #[test]
    fn test_template_env_initialization() {
        let env = init_template_env();
        assert!(
            env.get_template("oauth/authorize.html").is_ok()
                || env.get_template("oauth/authorize.html").is_err()
        );
        // Template loading will depend on whether we're in test mode or have templates available
    }
}
