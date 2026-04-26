use axum::{
    Router,
    extract::State,
    http::{HeaderMap, header::AUTHORIZATION},
    response::Json,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::{
    api::{
        oauth::{db, jwt::create_jwt_manager},
        service::ApiState,
    },
    errors::CustomError,
};

#[derive(Serialize, ToSchema)]
pub struct AccessTokenResponse {
    /// Token ID
    pub id: i32,
    /// Access token value
    pub token: String,
    /// Granted scopes
    pub scopes: Vec<String>,
    /// Token expiration time
    pub expires_at: String,
    /// Client name
    pub client_name: String,
    /// When token was created
    pub created_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct ListTokensResponse {
    /// List of user's access tokens
    pub tokens: Vec<AccessTokenResponse>,
}

#[derive(Deserialize, Validate, ToSchema)]
pub struct RevokeTokenRequest {
    /// Token ID to revoke
    pub token_id: i32,
}

#[derive(Serialize, ToSchema)]
pub struct RevokeTokenResponse {
    /// Success message
    pub message: String,
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/tokens", get(list_tokens))
        .route("/tokens/revoke", post(revoke_token))
}

#[utoipa::path(
    get,
    path = "/v1.0/oauth/tokens",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of user's tokens", body = ListTokensResponse),
        (status = 401, description = "Unauthorized", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn list_tokens(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<ListTokensResponse>, CustomError> {
    // Extract and validate Bearer token
    let auth_header = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| {
            CustomError::Authentication("Missing or invalid Bearer token".to_string())
        })?;

    // Validate token and get user ID
    let jwt_manager = create_jwt_manager();
    let claims = jwt_manager.validate_access_token(auth_header)?;

    // Get user's tokens
    let user_id: i32 = claims
        .sub
        .parse()
        .map_err(|_| CustomError::Authentication("Invalid user ID in token".to_string()))?;
    let tokens = db::list_user_access_tokens(&state.sqlx_pool, user_id).await?;

    let token_responses = tokens
        .into_iter()
        .map(|token| {
            let scopes = token
                .parse_scopes()
                .unwrap_or_default()
                .iter()
                .map(|s| s.as_str().to_string())
                .collect();

            AccessTokenResponse {
                id: token.id,
                token: token.token,
                scopes,
                expires_at: token.expires_at.to_string(),
                client_name: token.client_name,
                created_at: token.created_at.to_string(),
            }
        })
        .collect();

    Ok(Json(ListTokensResponse {
        tokens: token_responses,
    }))
}

#[utoipa::path(
    post,
    path = "/v1.0/oauth/tokens/revoke",
    security(("bearer_auth" = [])),
    request_body = RevokeTokenRequest,
    responses(
        (status = 200, description = "Token revoked successfully", body = RevokeTokenResponse),
        (status = 400, description = "Invalid request", body = String),
        (status = 401, description = "Unauthorized", body = String),
        (status = 403, description = "Cannot revoke token", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn revoke_token(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<RevokeTokenRequest>,
) -> Result<Json<RevokeTokenResponse>, CustomError> {
    // Extract and validate Bearer token
    let auth_header = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| {
            CustomError::Authentication("Missing or invalid Bearer token".to_string())
        })?;

    // Validate token and get user ID
    let jwt_manager = create_jwt_manager();
    let claims = jwt_manager.validate_access_token(auth_header)?;

    // Get the token to verify ownership
    let token = db::get_oauth_token_by_id(&state.sqlx_pool, request.token_id).await?;
    let user_id: i32 = claims
        .sub
        .parse()
        .map_err(|_| CustomError::Authentication("Invalid user ID in token".to_string()))?;

    match token {
        Some(token) if token.user_id == user_id => {
            // User owns this token, revoke it
            db::revoke_oauth_token(&state.sqlx_pool, request.token_id).await?;

            Ok(Json(RevokeTokenResponse {
                message: "Token revoked successfully".to_string(),
            }))
        }
        Some(_) => {
            // Token exists but user doesn't own it
            Err(CustomError::Authorization(
                "You can only revoke your own tokens".to_string(),
            ))
        }
        None => {
            // Token doesn't exist
            Err(CustomError::NotFound("Token not found".to_string()))
        }
    }
}
