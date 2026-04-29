use axum::http::{header, HeaderMap, StatusCode};

use crate::api::oauth::{error::OAuthError, jwt::create_jwt_manager, OAuthScope};
use crate::errors::CustomError;

#[derive(Debug, Clone)]
pub struct BearerPrincipal {
    pub user_id: i32,
    pub scopes: Vec<OAuthScope>,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub error_code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, error_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            error_code,
            message: message.into(),
        }
    }

    pub fn into_custom_error(self) -> CustomError {
        match self.status {
            StatusCode::UNAUTHORIZED => CustomError::Authentication(self.message),
            StatusCode::FORBIDDEN => CustomError::Authorization(self.message),
            StatusCode::BAD_REQUEST => CustomError::ValidationError(self.message),
            _ => CustomError::System(self.message),
        }
    }
}

pub fn extract_bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers.get(header::AUTHORIZATION).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "missing_authorization",
            "missing Authorization header",
        )
    })?;
    let value = value.to_str().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_authorization",
            "invalid Authorization header",
        )
    })?;
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_authorization_scheme",
            "Authorization header must use Bearer scheme",
        ));
    };
    let token = token.trim();
    if token.is_empty() {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "empty_bearer_token",
            "empty bearer token",
        ));
    }
    Ok(token.to_string())
}

pub fn authenticate_bearer(headers: &HeaderMap) -> Result<BearerPrincipal, ApiError> {
    let access_token = extract_bearer_token(headers)?;
    let jwt_manager = create_jwt_manager();
    let claims = jwt_manager
        .validate_access_token(&access_token)
        .map_err(|err| match err {
            OAuthError::InvalidToken | OAuthError::InvalidGrant => ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "invalid or expired bearer token",
            ),
            OAuthError::InsufficientScope => ApiError::new(
                StatusCode::FORBIDDEN,
                "insufficient_scope",
                "bearer token has insufficient scope",
            ),
            other => ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                other.error_description(),
            ),
        })?;
    let scopes = claims.parse_scopes().map_err(|err| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "invalid_scope",
            err.error_description(),
        )
    })?;
    let user_id = claims.user_id().map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_token_subject",
            "bearer token does not contain a user id",
        )
    })?;
    Ok(BearerPrincipal { user_id, scopes })
}

pub fn require_scope(principal: &BearerPrincipal, scope: OAuthScope) -> Result<(), ApiError> {
    if principal.scopes.contains(&scope) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            format!("bearer token requires {} scope", scope.as_str()),
        ))
    }
}
