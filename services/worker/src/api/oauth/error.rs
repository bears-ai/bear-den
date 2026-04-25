//! OAuth 2.0 error handling
//!
//! This module defines OAuth-specific error types following RFC 6749 and integrates
//! with the existing CustomError system.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::fmt;

/// OAuth 2.0 error types as defined in RFC 6749
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthError {
    /// The request is missing a required parameter, includes an invalid parameter value,
    /// includes a parameter more than once, or is otherwise malformed.
    InvalidRequest(String),

    /// Client authentication failed (e.g., unknown client, no client authentication included,
    /// or unsupported authentication method).
    ///
    /// Note: For more specific errors, use InvalidRequest with a descriptive message.
    InvalidClient,

    /// The provided authorization grant (e.g., authorization code, resource owner credentials)
    /// or refresh token is invalid, expired, revoked, does not match the redirection URI
    /// used in the authorization request, or was issued to another client.
    InvalidGrant,

    /// The authenticated client is not authorized to use this authorization grant type.
    UnauthorizedClient,

    /// The authorization grant type is not supported by the authorization server.
    UnsupportedGrantType,

    /// The requested scope is invalid, unknown, or malformed.
    InvalidScope,

    /// The authorization server does not support obtaining an authorization code
    /// using this method.
    UnsupportedResponseType,

    /// The authorization server encountered an unexpected condition that prevented
    /// it from fulfilling the request.
    ServerError(String),

    /// The authorization server is currently unable to handle the request due to
    /// a temporary overloading or maintenance of the server.
    TemporarilyUnavailable,

    /// The resource owner or authorization server denied the request.
    AccessDenied,

    /// The access token provided is expired, revoked, malformed, or invalid (RFC 6750).
    InvalidToken,

    /// The request requires higher privileges than provided by the access token (RFC 6750).
    InsufficientScope,
}

impl OAuthError {
    /// Get the OAuth error code as defined in RFC 6749
    pub fn error_code(&self) -> &'static str {
        match self {
            OAuthError::InvalidRequest(_) => "invalid_request",
            OAuthError::InvalidClient => "invalid_client",
            OAuthError::InvalidGrant => "invalid_grant",
            OAuthError::UnauthorizedClient => "unauthorized_client",
            OAuthError::UnsupportedGrantType => "unsupported_grant_type",
            OAuthError::InvalidScope => "invalid_scope",
            OAuthError::UnsupportedResponseType => "unsupported_response_type",
            OAuthError::ServerError(_) => "server_error",
            OAuthError::TemporarilyUnavailable => "temporarily_unavailable",
            OAuthError::AccessDenied => "access_denied",
            OAuthError::InvalidToken => "invalid_token",
            OAuthError::InsufficientScope => "insufficient_scope",
        }
    }

    /// Get the error description
    pub fn error_description(&self) -> String {
        match self {
            OAuthError::InvalidRequest(msg) => msg.clone(),
            OAuthError::InvalidClient => "Client authentication failed. This could mean: (1) The client_id is incorrect or not registered, (2) The client is inactive, (3) Required authentication credentials (client_secret or code_verifier) are missing or incorrect, or (4) The client type (public/confidential) doesn't match the authentication method used. Check the server logs for more specific details.".to_string(),
            OAuthError::InvalidGrant => "The provided authorization grant is invalid".to_string(),
            OAuthError::UnauthorizedClient => {
                "The client is not authorized to use this grant type".to_string()
            }
            OAuthError::UnsupportedGrantType => "The grant type is not supported".to_string(),
            OAuthError::InvalidScope => "The requested scope is invalid".to_string(),
            OAuthError::UnsupportedResponseType => "The response type is not supported".to_string(),
            OAuthError::ServerError(msg) => msg.clone(),
            OAuthError::TemporarilyUnavailable => {
                "The server is temporarily unavailable".to_string()
            }
            OAuthError::AccessDenied => "The resource owner denied the request".to_string(),
            OAuthError::InvalidToken => {
                "The access token provided is expired, revoked, malformed, or invalid".to_string()
            }
            OAuthError::InsufficientScope => {
                "The request requires higher privileges than provided by the access token"
                    .to_string()
            }
        }
    }

    /// Get the HTTP status code for this error
    pub fn status_code(&self) -> StatusCode {
        match self {
            OAuthError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            OAuthError::InvalidClient => StatusCode::UNAUTHORIZED,
            OAuthError::InvalidGrant => StatusCode::BAD_REQUEST,
            OAuthError::UnauthorizedClient => StatusCode::BAD_REQUEST,
            OAuthError::UnsupportedGrantType => StatusCode::BAD_REQUEST,
            OAuthError::InvalidScope => StatusCode::BAD_REQUEST,
            OAuthError::UnsupportedResponseType => StatusCode::BAD_REQUEST,
            OAuthError::ServerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OAuthError::TemporarilyUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            OAuthError::AccessDenied => StatusCode::FORBIDDEN,
            OAuthError::InvalidToken => StatusCode::UNAUTHORIZED,
            OAuthError::InsufficientScope => StatusCode::FORBIDDEN,
        }
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.error_code(), self.error_description())
    }
}

impl std::error::Error for OAuthError {}

/// OAuth error response format as defined in RFC 6749
#[derive(Debug, Serialize)]
pub struct OAuthErrorResponse {
    /// The error code
    pub error: String,
    /// Human-readable error description
    pub error_description: String,
}

impl From<OAuthError> for OAuthErrorResponse {
    fn from(error: OAuthError) -> Self {
        Self {
            error: error.error_code().to_string(),
            error_description: error.error_description(),
        }
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let error_response = OAuthErrorResponse::from(self);
        (status, Json(error_response)).into_response()
    }
}

/// Convert OAuthError to CustomError for integration with existing error handling
impl From<OAuthError> for crate::errors::CustomError {
    fn from(error: OAuthError) -> Self {
        match error {
            OAuthError::InvalidRequest(msg) => crate::errors::CustomError::Authentication(msg),
            OAuthError::InvalidClient => {
                crate::errors::CustomError::Authentication("Invalid client".to_string())
            }
            OAuthError::InvalidGrant => {
                crate::errors::CustomError::Authentication("Invalid grant".to_string())
            }
            OAuthError::UnauthorizedClient => {
                crate::errors::CustomError::Authorization("Unauthorized client".to_string())
            }
            OAuthError::UnsupportedGrantType => {
                crate::errors::CustomError::Authentication("Unsupported grant type".to_string())
            }
            OAuthError::InvalidScope => {
                crate::errors::CustomError::Authentication("Invalid scope".to_string())
            }
            OAuthError::UnsupportedResponseType => {
                crate::errors::CustomError::Authentication("Unsupported response type".to_string())
            }
            OAuthError::ServerError(msg) => crate::errors::CustomError::System(msg),
            OAuthError::TemporarilyUnavailable => {
                crate::errors::CustomError::System("Service temporarily unavailable".to_string())
            }
            OAuthError::AccessDenied => {
                crate::errors::CustomError::Authorization("Access denied".to_string())
            }
            OAuthError::InvalidToken => {
                crate::errors::CustomError::Authentication("Invalid token".to_string())
            }
            OAuthError::InsufficientScope => {
                crate::errors::CustomError::Authorization("Insufficient scope".to_string())
            }
        }
    }
}

/// Convert CustomError to OAuthError where appropriate
impl From<crate::errors::CustomError> for OAuthError {
    fn from(error: crate::errors::CustomError) -> Self {
        match error {
            crate::errors::CustomError::Authentication(_msg) => OAuthError::InvalidClient,
            crate::errors::CustomError::Authorization(_msg) => OAuthError::AccessDenied,
            crate::errors::CustomError::Database(msg) => {
                OAuthError::ServerError(format!("Database error: {msg}"))
            }
            crate::errors::CustomError::System(msg) => OAuthError::ServerError(msg),
            crate::errors::CustomError::Parsing(msg) => {
                OAuthError::InvalidRequest(format!("Parsing error: {msg}"))
            }
            _ => OAuthError::ServerError("Internal server error".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_error_codes() {
        assert_eq!(
            OAuthError::InvalidRequest("test".to_string()).error_code(),
            "invalid_request"
        );
        assert_eq!(OAuthError::InvalidClient.error_code(), "invalid_client");
        assert_eq!(OAuthError::InvalidGrant.error_code(), "invalid_grant");
        assert_eq!(
            OAuthError::UnauthorizedClient.error_code(),
            "unauthorized_client"
        );
        assert_eq!(
            OAuthError::UnsupportedGrantType.error_code(),
            "unsupported_grant_type"
        );
        assert_eq!(OAuthError::InvalidScope.error_code(), "invalid_scope");
        assert_eq!(
            OAuthError::UnsupportedResponseType.error_code(),
            "unsupported_response_type"
        );
        assert_eq!(
            OAuthError::ServerError("test".to_string()).error_code(),
            "server_error"
        );
        assert_eq!(
            OAuthError::TemporarilyUnavailable.error_code(),
            "temporarily_unavailable"
        );
        assert_eq!(OAuthError::AccessDenied.error_code(), "access_denied");
    }

    #[test]
    fn test_oauth_error_status_codes() {
        assert_eq!(
            OAuthError::InvalidRequest("test".to_string()).status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            OAuthError::InvalidClient.status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            OAuthError::ServerError("test".to_string()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            OAuthError::AccessDenied.status_code(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn test_oauth_error_response_conversion() {
        let error = OAuthError::InvalidRequest("Missing parameter".to_string());
        let response = OAuthErrorResponse::from(error);

        assert_eq!(response.error, "invalid_request");
        assert_eq!(response.error_description, "Missing parameter");
    }

    #[test]
    fn test_custom_error_conversion() {
        let oauth_error = OAuthError::InvalidClient;
        let custom_error: crate::errors::CustomError = oauth_error.into();

        match custom_error {
            crate::errors::CustomError::Authentication(_) => (),
            _ => panic!("Expected Authentication error"),
        }
    }
}
