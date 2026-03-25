//! OAuth 2.0 provider implementation for the standalone API (`src/api/`)
//!
//! This module provides OAuth 2.0 authorization server functionality following RFC 6749.
//! It includes data structures for clients, authorization codes, access tokens, and
//! request/response handling for OAuth endpoints.

pub mod db;
pub mod endpoints;
pub mod error;
pub mod jwt;
pub mod router;
pub mod utils;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// OAuth client registration information
///
/// Represents a registered OAuth client application that can request authorization
/// from users to access their account data per configured scopes.
#[derive(Debug, Clone, Serialize)]
pub struct OAuthClient {
    /// Unique database identifier
    pub id: i32,
    /// Public client identifier used in OAuth flows
    pub client_id: String,
    /// Hashed client secret for authentication (None for public clients)
    pub client_secret: Option<String>,
    /// Human-readable client name
    pub name: String,
    /// Allowed redirect URIs for this client (JSON array)
    pub redirect_uris: serde_json::Value,
    /// Supported OAuth scopes for this client (JSON array)
    pub scopes: serde_json::Value,
    /// Whether this client is currently active
    pub active: bool,
    /// Whether this client is trusted (auto-approve authorization requests)
    pub trusted: bool,
    /// Whether this is a public client (uses PKCE, no client_secret required)
    pub public: bool,
    /// When this client was registered
    pub created_at: OffsetDateTime,
    /// When this client was last updated
    pub updated_at: OffsetDateTime,
}

/// OAuth authorization code for the authorization code flow
///
/// Temporary code issued during the authorization flow that can be exchanged
/// for an access token.
#[derive(Debug, Clone, Serialize)]
pub struct OAuthAuthorizationCode {
    /// Unique database identifier
    pub id: i32,
    /// The authorization code value
    pub code: String,
    /// Client that requested this code
    pub client_id: i32,
    /// User who authorized this code
    pub user_id: i32,
    /// Redirect URI used in the authorization request
    pub redirect_uri: String,
    /// Granted scopes (JSON array)
    pub scopes: serde_json::Value,
    /// When this code expires
    pub expires_at: OffsetDateTime,
    /// Whether this code has been used
    pub used: bool,
    /// When this code was created
    pub created_at: OffsetDateTime,
    /// PKCE code challenge (RFC 7636)
    pub code_challenge: Option<String>,
    /// PKCE code challenge method (S256 or plain)
    pub code_challenge_method: Option<String>,
}

/// OAuth access token for API authentication
///
/// Long-lived token that grants access to protected resources on behalf of a user.
#[derive(Debug, Clone, Serialize)]
pub struct OAuthAccessToken {
    /// Unique database identifier
    pub id: i32,
    /// The access token value
    pub token: String,
    /// Client that owns this token
    pub client_id: i32,
    /// User this token represents
    pub user_id: i32,
    /// Granted scopes (JSON array)
    pub scopes: serde_json::Value,
    /// When this token expires
    pub expires_at: OffsetDateTime,
    /// Whether this token has been revoked
    pub revoked: bool,
    /// When this token was created
    pub created_at: OffsetDateTime,
}

/// OAuth refresh token for obtaining new access tokens
///
/// Long-lived token that allows clients to obtain new access tokens without
/// requiring the user to re-authorize. Refresh tokens are only issued to
/// confidential clients (not public clients) per RFC 6749.
#[derive(Debug, Clone, Serialize)]
pub struct OAuthRefreshToken {
    /// Unique database identifier
    pub id: i32,
    /// The refresh token value
    pub token: String,
    /// Client that owns this token
    pub client_id: i32,
    /// User this token represents
    pub user_id: i32,
    /// Granted scopes (JSON array)
    pub scopes: serde_json::Value,
    /// When this token expires
    pub expires_at: OffsetDateTime,
    /// Whether this token has been revoked
    pub revoked: bool,
    /// When this token was created
    pub created_at: OffsetDateTime,
}

/// Access token with user and client context
///
/// Extended access token information including user and client details
/// for admin interfaces and detailed token management.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AccessTokenWithContext {
    /// Unique database identifier
    pub token_id: i32,
    /// The access token value
    pub token: String,
    /// Client database ID
    pub client_id: i32,
    /// User database ID
    pub user_id: i32,
    /// Granted scopes (JSON array)
    pub scopes: serde_json::Value,
    /// When this token expires
    pub expires_at: OffsetDateTime,
    /// Whether this token has been revoked
    pub revoked: bool,
    /// When this token was created
    pub token_created_at: OffsetDateTime,
    /// Client identifier
    pub client_identifier: String,
    /// Client name
    pub client_name: String,
    /// Username
    pub username: String,
    /// User email
    pub email: String,
    /// User display name
    pub display_name: String,
}

impl AccessTokenWithContext {
    /// Parse scopes from JSON
    #[allow(dead_code)]
    pub fn parse_scopes(&self) -> Result<Vec<OAuthScope>, crate::api::oauth::error::OAuthError> {
        scopes_from_json(&self.scopes)
    }
}

/// User access token with client information
///
/// Simplified access token view for user-facing interfaces.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct UserAccessToken {
    /// Unique database identifier
    pub id: i32,
    /// The access token value
    pub token: String,
    /// Granted scopes (JSON array)
    pub scopes: serde_json::Value,
    /// When this token expires
    pub expires_at: OffsetDateTime,
    /// When this token was created
    pub created_at: OffsetDateTime,
    /// Client identifier
    pub client_id: String,
    /// Client name
    pub client_name: String,
}

impl UserAccessToken {
    /// Parse scopes from JSON
    #[allow(dead_code)]
    pub fn parse_scopes(&self) -> Result<Vec<OAuthScope>, crate::api::oauth::error::OAuthError> {
        scopes_from_json(&self.scopes)
    }
}

/// Supported OAuth scopes
///
/// Defines the permissions that can be granted to OAuth clients.
/// All scopes follow the resource:action naming pattern for consistency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OAuthScope {
    /// Read access to basic profile information (username, display name)
    ProfileRead,
    /// Read access to user email address
    ProfileEmail,
    /// Read access to API resources you expose (starter placeholder — rename for your product)
    DataRead,
    /// Write access to API resources you expose (starter placeholder — rename for your product)
    DataWrite,
}

impl OAuthScope {
    /// Convert scope to string representation
    ///
    /// Returns the scope in resource:action format (e.g., "profile:read")
    pub fn as_str(&self) -> &'static str {
        match self {
            OAuthScope::ProfileRead => "profile:read",
            OAuthScope::ProfileEmail => "profile:email",
            OAuthScope::DataRead => "data:read",
            OAuthScope::DataWrite => "data:write",
        }
    }

    /// Parse scope from string
    ///
    /// Accepts scopes in resource:action format (e.g., "profile:read")
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "profile:read" => Some(OAuthScope::ProfileRead),
            "profile:email" => Some(OAuthScope::ProfileEmail),
            "data:read" => Some(OAuthScope::DataRead),
            "data:write" => Some(OAuthScope::DataWrite),
            // Legacy scope names from older forks (still accepted when reading stored scopes)
            "hexes:read" => Some(OAuthScope::DataRead),
            "visits:write" => Some(OAuthScope::DataWrite),
            _ => None,
        }
    }

    /// Get all supported scopes
    pub fn all() -> Vec<Self> {
        vec![
            OAuthScope::ProfileRead,
            OAuthScope::ProfileEmail,
            OAuthScope::DataRead,
            OAuthScope::DataWrite,
        ]
    }
}

/// OAuth authorization request parameters
///
/// Parameters sent to the authorization endpoint to initiate the OAuth flow.
#[derive(Debug, Deserialize)]
pub struct AuthorizationRequest {
    /// Response type (must be "code" for authorization code flow)
    pub response_type: String,
    /// Client identifier
    pub client_id: String,
    /// Redirect URI where the authorization response will be sent
    pub redirect_uri: String,
    /// Requested scopes (space-separated)
    pub scope: Option<String>,
    /// Opaque value to prevent CSRF attacks
    pub _state: Option<String>,
}

/// OAuth token request parameters
///
/// Parameters sent to the token endpoint to exchange authorization code for access token.
/// Supports PKCE (RFC 7636) for enhanced security.
#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    /// Grant type (must be "authorization_code")
    pub grant_type: String,
    /// Authorization code from authorization endpoint
    pub code: String,
    /// Redirect URI used in authorization request
    pub redirect_uri: String,
    /// Client identifier
    pub client_id: String,
    /// Client secret for authentication (optional if using PKCE)
    pub client_secret: Option<String>,
    /// PKCE code verifier (RFC 7636)
    pub code_verifier: Option<String>,
}

/// OAuth token response
///
/// Response from the token endpoint containing the access token and metadata.
/// According to RFC 6749, refresh tokens SHOULD be issued when issuing access tokens,
/// unless the client is a public client.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// The access token
    pub access_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Token lifetime in seconds
    pub expires_in: i64,
    /// Granted scopes (space-separated)
    pub scope: String,
    /// Refresh token for obtaining new access tokens (optional, not issued to public clients)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

/// User information response
///
/// Response from the userinfo endpoint containing user profile data.
#[derive(Debug, Serialize)]
pub struct UserInfoResponse {
    /// User's unique identifier
    pub sub: String,
    /// User's username
    pub preferred_username: Option<String>,
    /// User's display name
    pub name: Option<String>,
    /// User's email address (if email scope granted)
    pub email: Option<String>,
    /// Whether email is verified (if email scope granted)
    pub email_verified: Option<bool>,
}

impl AuthorizationRequest {
    /// Validate the authorization request parameters
    pub fn validate(&self) -> Result<(), crate::api::oauth::error::OAuthError> {
        use crate::api::oauth::error::OAuthError;

        // Validate response_type
        if self.response_type != "code" {
            return Err(OAuthError::UnsupportedResponseType);
        }

        // Validate client_id is not empty
        if self.client_id.is_empty() {
            return Err(OAuthError::InvalidRequest(
                "client_id is required".to_string(),
            ));
        }

        // Validate redirect_uri is not empty
        if self.redirect_uri.is_empty() {
            return Err(OAuthError::InvalidRequest(
                "redirect_uri is required".to_string(),
            ));
        }

        // Validate redirect_uri is a valid URL
        if url::Url::parse(&self.redirect_uri).is_err() {
            return Err(OAuthError::InvalidRequest(format!(
                "redirect_uri must be a valid URL, but received: '{}'",
                self.redirect_uri
            )));
        }

        Ok(())
    }

    /// Parse and validate requested scopes
    ///
    /// Expects scopes in resource:action format (e.g., "profile:read data:read")
    pub fn parse_scopes(&self) -> Result<Vec<OAuthScope>, crate::api::oauth::error::OAuthError> {
        use crate::api::oauth::error::OAuthError;

        let scope_str = self.scope.as_deref().unwrap_or("");
        if scope_str.is_empty() {
            return Ok(vec![]);
        }

        let mut scopes = Vec::new();
        for scope_name in scope_str.split_whitespace() {
            match OAuthScope::from_str(scope_name) {
                Some(scope) => scopes.push(scope),
                None => return Err(OAuthError::InvalidScope),
            }
        }

        Ok(scopes)
    }
}

impl TokenRequest {
    /// Validate the token request parameters
    pub fn validate(&self) -> Result<(), crate::api::oauth::error::OAuthError> {
        use crate::api::oauth::error::OAuthError;

        // Validate grant_type
        if self.grant_type != "authorization_code" {
            return Err(OAuthError::UnsupportedGrantType);
        }

        // Validate required fields are not empty
        if self.code.is_empty() {
            return Err(OAuthError::InvalidRequest("code is required".to_string()));
        }

        if self.client_id.is_empty() {
            return Err(OAuthError::InvalidRequest(
                "client_id is required".to_string(),
            ));
        }

        if self.redirect_uri.is_empty() {
            return Err(OAuthError::InvalidRequest(
                "redirect_uri is required".to_string(),
            ));
        }

        // Validate redirect_uri is a valid URL
        if url::Url::parse(&self.redirect_uri).is_err() {
            return Err(OAuthError::InvalidRequest(format!(
                "redirect_uri must be a valid URL, but received: '{}'",
                self.redirect_uri
            )));
        }

        Ok(())
    }
}

/// Helper function to convert scopes to space-separated string
pub fn scopes_to_string(scopes: &[OAuthScope]) -> String {
    scopes
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Helper function to convert scopes to JSON value for database storage
pub fn _scopes_to_json(scopes: &[OAuthScope]) -> serde_json::Value {
    let scope_strings: Vec<String> = scopes.iter().map(|s| s.as_str().to_string()).collect();
    serde_json::Value::Array(
        scope_strings
            .into_iter()
            .map(serde_json::Value::String)
            .collect(),
    )
}

/// Helper function to parse scopes from JSON value
pub fn scopes_from_json(
    json: &serde_json::Value,
) -> Result<Vec<OAuthScope>, crate::api::oauth::error::OAuthError> {
    match json {
        serde_json::Value::Array(arr) => {
            let mut scopes = Vec::new();
            for item in arr {
                if let serde_json::Value::String(scope_str) = item {
                    match OAuthScope::from_str(scope_str) {
                        Some(scope) => scopes.push(scope),
                        None => return Err(crate::api::oauth::error::OAuthError::InvalidScope),
                    }
                } else {
                    return Err(crate::api::oauth::error::OAuthError::InvalidScope);
                }
            }
            Ok(scopes)
        }
        _ => Err(crate::api::oauth::error::OAuthError::InvalidScope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_scope_conversion() {
        assert_eq!(OAuthScope::ProfileRead.as_str(), "profile:read");
        assert_eq!(OAuthScope::ProfileEmail.as_str(), "profile:email");
        assert_eq!(OAuthScope::DataRead.as_str(), "data:read");
        assert_eq!(OAuthScope::DataWrite.as_str(), "data:write");

        assert_eq!(
            OAuthScope::from_str("profile:read"),
            Some(OAuthScope::ProfileRead)
        );
        assert_eq!(
            OAuthScope::from_str("profile:email"),
            Some(OAuthScope::ProfileEmail)
        );
        assert_eq!(
            OAuthScope::from_str("data:read"),
            Some(OAuthScope::DataRead)
        );
        assert_eq!(
            OAuthScope::from_str("data:write"),
            Some(OAuthScope::DataWrite)
        );
        assert_eq!(
            OAuthScope::from_str("hexes:read"),
            Some(OAuthScope::DataRead)
        );
        assert_eq!(
            OAuthScope::from_str("visits:write"),
            Some(OAuthScope::DataWrite)
        );
        assert_eq!(OAuthScope::from_str("invalid"), None);
        assert_eq!(OAuthScope::from_str("profile"), None); // Old format no longer supported
        assert_eq!(OAuthScope::from_str("email"), None); // Old format no longer supported
    }

    #[test]
    fn test_scopes_to_json() {
        let scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
        let json = super::_scopes_to_json(&scopes);

        assert_eq!(json, serde_json::json!(["profile:read", "profile:email"]));
    }

    #[test]
    fn test_scopes_from_json() {
        let json = serde_json::json!(["profile:read", "profile:email"]);
        let scopes = super::scopes_from_json(&json).unwrap();

        assert_eq!(
            scopes,
            vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail]
        );
    }

    #[test]
    fn test_scopes_to_string() {
        let scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
        let scope_string = scopes_to_string(&scopes);

        assert_eq!(scope_string, "profile:read profile:email");
    }

    #[test]
    fn test_authorization_request_validation() {
        let valid_request = AuthorizationRequest {
            response_type: "code".to_string(),
            client_id: "test_client".to_string(),
            redirect_uri: "https://example.com/callback".to_string(),
            scope: Some("profile email".to_string()),
            _state: Some("random_state".to_string()),
        };

        assert!(valid_request.validate().is_ok());

        let invalid_request = AuthorizationRequest {
            response_type: "token".to_string(),
            client_id: "test_client".to_string(),
            redirect_uri: "https://example.com/callback".to_string(),
            scope: None,
            _state: None,
        };

        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn test_token_request_validation() {
        let valid_request = TokenRequest {
            grant_type: "authorization_code".to_string(),
            code: "test_code".to_string(),
            redirect_uri: "https://example.com/callback".to_string(),
            client_id: "test_client".to_string(),
            client_secret: Some("test_secret".to_string()),
            code_verifier: None,
        };

        assert!(valid_request.validate().is_ok());

        let invalid_request = TokenRequest {
            grant_type: "password".to_string(),
            code: "test_code".to_string(),
            redirect_uri: "https://example.com/callback".to_string(),
            client_id: "test_client".to_string(),
            client_secret: None,
            code_verifier: Some("test_verifier".to_string()),
        };

        assert!(invalid_request.validate().is_err());
    }
}
