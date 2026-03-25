use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Json, Response},
    routing::get,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    api::{
        oauth::{error::OAuthError, jwt::create_jwt_manager},
        service::ApiState,
    },
    core::user,
    errors::CustomError,
};

#[derive(Serialize, ToSchema)]
pub struct ProfileResponse {
    /// User's unique identifier
    #[schema(example = 123)]
    pub id: i32,
    /// User's display name
    #[schema(example = "John Doe")]
    pub name: String,
    /// User's email address
    #[schema(example = "john@example.com")]
    pub email: String,
    /// Relative URL to user's profile page
    #[schema(example = "/johndoe")]
    pub profile_url: String,
    /// Whether the user's email is verified
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_verified: Option<bool>,
    /// User's theme preference (system, dark, light)
    #[schema(example = "dark")]
    pub theme: String,
    /// Day of week when user's week starts (1=Monday)
    #[schema(example = 1)]
    pub week_start_day: i32,
}

pub fn router() -> Router<ApiState> {
    Router::new().route("/me", get(get_profile))
}

#[utoipa::path(
    get,
    path = "/v1.0/me",
    responses(
        (status = 200, description = "User profile retrieved successfully", body = ProfileResponse),
        (status = 403, description = "Authentication required")
    ),
    tag = "Profile"
)]
/// Get authenticated user's profile
pub async fn get_profile(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    // Extract Bearer token from Authorization header
    let access_token = match extract_bearer_token(&headers) {
        Ok(token) => token,
        Err(response) => return Ok(response),
    };

    // Validate JWT access token
    let jwt_manager = create_jwt_manager();
    let jwt_claims = match jwt_manager.validate_access_token(&access_token) {
        Ok(claims) => claims,
        Err(oauth_error) => return Ok(bearer_error_response(oauth_error)),
    };

    // Get user ID from JWT claims
    let user_id = match jwt_claims.user_id() {
        Ok(id) => id,
        Err(oauth_error) => return Ok(bearer_error_response(oauth_error)),
    };

    // Get user information from database
    let user = match user::user_by_id(&state.sqlx_pool, user_id).await {
        Ok(user) => user,
        Err(_) => return Ok(internal_server_error("Database error")),
    };

    let profile = ProfileResponse {
        id: user.id,
        name: user.display_name.clone(),
        email: user.email.clone(),
        profile_url: format!("/{}", user.username),
        email_verified: user.email_verified,
        theme: user.theme.clone(),
        week_start_day: user.week_start_day,
    };

    Ok(Json(profile).into_response())
}

/// Extract Bearer token from Authorization header
fn extract_bearer_token(headers: &HeaderMap) -> Result<String, Response> {
    let auth_header = headers.get(AUTHORIZATION).ok_or_else(|| {
        bearer_error_response(OAuthError::InvalidRequest(
            "Missing Authorization header".to_string(),
        ))
    })?;

    let auth_str = auth_header.to_str().map_err(|_| {
        bearer_error_response(OAuthError::InvalidRequest(
            "Invalid Authorization header encoding".to_string(),
        ))
    })?;

    if !auth_str.starts_with("Bearer ") {
        return Err(bearer_error_response(OAuthError::InvalidRequest(
            "Authorization header must use Bearer scheme".to_string(),
        )));
    }

    let token = auth_str[7..].trim();
    if token.is_empty() {
        return Err(bearer_error_response(OAuthError::InvalidToken));
    }

    Ok(token.to_string())
}

/// Create Bearer token error response
fn bearer_error_response(error: OAuthError) -> Response {
    let (status_code, error_code, error_description) = match error {
        OAuthError::InvalidRequest(desc) => (StatusCode::BAD_REQUEST, "invalid_request", desc),
        OAuthError::InvalidToken => (
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "The access token provided is expired, revoked, malformed, or invalid".to_string(),
        ),
        OAuthError::InsufficientScope => (
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "The request requires higher privileges than provided by the access token".to_string(),
        ),
        OAuthError::ServerError(desc) => (StatusCode::INTERNAL_SERVER_ERROR, "server_error", desc),
        _ => (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid request".to_string(),
        ),
    };

    let www_authenticate = if status_code == StatusCode::UNAUTHORIZED {
        format!("Bearer error=\"{error_code}\", error_description=\"{error_description}\"")
    } else {
        "Bearer".to_string()
    };

    let error_response = serde_json::json!({
        "error": error_code,
        "error_description": error_description
    });

    let mut response = (status_code, Json(error_response)).into_response();
    response.headers_mut().insert(
        "WWW-Authenticate",
        www_authenticate
            .parse()
            .unwrap_or_else(|_| "Bearer".parse().unwrap()),
    );

    response
}

/// Create internal server error response
fn internal_server_error(message: &str) -> Response {
    let error_response = serde_json::json!({
        "error": "server_error",
        "error_description": message
    });

    (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)).into_response()
}
