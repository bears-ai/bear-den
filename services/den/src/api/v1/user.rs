use axum::{Router, extract::State, response::Json, routing::post};
use password_auth;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::{api::service::ApiState, core::user, errors::CustomError};

#[derive(Serialize, ToSchema)]
pub struct UserResponse {
    /// User's unique identifier
    #[schema(example = 123)]
    pub id: i32,
    /// User's username
    #[schema(example = "johndoe")]
    pub username: String,
    /// User's display name
    #[schema(example = "John Doe")]
    pub display_name: String,
    /// User's email address
    #[schema(example = "john@example.com")]
    pub email: String,
    /// Whether the user's email is verified
    pub email_verified: bool,
    /// When the user account was created
    pub created_at: String,
}

#[derive(Deserialize, Validate, ToSchema)]
pub struct RegisterRequest {
    /// Desired username (must be unique)
    #[validate(length(min = 3, max = 50))]
    #[schema(example = "johndoe")]
    pub username: String,
    /// User's email address
    #[validate(email)]
    #[schema(example = "john@example.com")]
    pub email: String,
    /// User's password
    #[validate(length(min = 8))]
    #[schema(example = "securepassword123")]
    pub password: String,
    /// User's display name
    #[validate(length(min = 1, max = 100))]
    #[schema(example = "John Doe")]
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct RegisterResponse {
    /// Success message
    pub message: String,
    /// User ID of the newly created account
    pub user_id: i32,
}

#[derive(Deserialize, Validate, ToSchema)]
pub struct LoginRequest {
    /// Username or email address
    #[validate(length(min = 1))]
    #[schema(example = "johndoe")]
    pub username_or_email: String,
    /// User's password
    #[validate(length(min = 1))]
    #[schema(example = "securepassword123")]
    pub password: String,
}

#[derive(Serialize, ToSchema)]
pub struct LoginResponse {
    /// Access token for API authentication
    pub access_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Token expiration time in seconds
    pub expires_in: i64,
    /// User information
    pub user: UserResponse,
}

#[derive(Deserialize, Validate, ToSchema)]
pub struct PasswordResetRequest {
    /// Email address for password reset
    #[validate(email)]
    #[schema(example = "john@example.com")]
    pub email: String,
}

#[derive(Serialize, ToSchema)]
pub struct PasswordResetResponse {
    /// Success message
    pub message: String,
}

#[derive(Deserialize, Validate, ToSchema)]
pub struct PasswordResetConfirmRequest {
    /// Password reset token
    #[validate(length(min = 1))]
    pub token: String,
    #[validate(length(min = 8))]
    pub new_password: String,
}

#[derive(Serialize, ToSchema)]
pub struct PasswordResetConfirmResponse {
    /// Success message
    pub message: String,
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/password-reset", post(request_password_reset))
        .route("/password-reset/confirm", post(confirm_password_reset))
}

#[utoipa::path(
    post,
    path = "/v1.0/users/register",
    request_body = RegisterRequest,
    responses(
        (status = 201, description = "User registered successfully", body = RegisterResponse),
        (status = 400, description = "Invalid request data", body = String),
        (status = 409, description = "Username or email already exists", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn register(
    State(state): State<ApiState>,
    Json(request): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, CustomError> {
    // Validate input
    request
        .validate()
        .map_err(|e| CustomError::ValidationError(format!("Validation failed: {e:?}")))?;

    // Check if username already exists
    if user::db::get_user_by_username(&state.sqlx_pool, &request.username)
        .await?
        .is_some()
    {
        return Err(CustomError::ValidationError(
            "Username already exists".to_string(),
        ));
    }

    // Check if email already exists
    if user::db::get_user_by_email(&state.sqlx_pool, &request.email)
        .await?
        .is_some()
    {
        return Err(CustomError::ValidationError(
            "Email already exists".to_string(),
        ));
    }

    // Create the user
    let password_hash = password_auth::generate_hash(&request.password);
    let user_id = user::db::create_user(
        &state.sqlx_pool,
        &request.email,
        &request.username,
        &request.display_name,
        &password_hash,
    )
    .await?;

    Ok(Json(RegisterResponse {
        message: "User registered successfully".to_string(),
        user_id,
    }))
}

#[utoipa::path(
    post,
    path = "/v1.0/users/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = LoginResponse),
        (status = 400, description = "Invalid credentials", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn login(
    State(state): State<ApiState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, CustomError> {
    // Validate input
    request
        .validate()
        .map_err(|e| CustomError::ValidationError(format!("Validation failed: {e:?}")))?;

    // Try to authenticate
    // Check if input is email or username
    let user_auth = if request.username_or_email.contains('@') {
        user::db::get_user_auth_by_email(&state.sqlx_pool, &request.username_or_email).await?
    } else {
        user::db::get_user_by_username(&state.sqlx_pool, &request.username_or_email).await?
    };

    if let Some(user_auth) = user_auth {
        // Verify password
        use password_auth::verify_password;
        if verify_password(request.password, &user_auth.passhash).is_ok() {
            // Generate access token
            use crate::api::oauth::{OAuthScope, jwt::create_jwt_manager, utils};

            let jwt_manager = create_jwt_manager();
            let scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
            let expires_at = utils::access_token_expiration();
            let token =
                jwt_manager.generate_access_token(user_auth.id, "api", &scopes, expires_at)?;

            // Get user details
            let db_user = user::db::user_by_id(&state.sqlx_pool, user_auth.id)
                .await?
                .ok_or_else(|| CustomError::NotFound("User not found".to_string()))?;

            let user_response = UserResponse {
                id: db_user.id,
                username: db_user.username,
                display_name: db_user.display_name,
                email: db_user.email,
                email_verified: db_user.email_verified.unwrap_or(false),
                created_at: db_user.created.to_string(),
            };

            Ok(Json(LoginResponse {
                access_token: token,
                token_type: "Bearer".to_string(),
                expires_in: 3600, // 1 hour
                user: user_response,
            }))
        } else {
            Err(CustomError::Authentication(
                "Invalid credentials".to_string(),
            ))
        }
    } else {
        Err(CustomError::Authentication(
            "Invalid credentials".to_string(),
        ))
    }
}

#[utoipa::path(
    post,
    path = "/v1.0/users/password-reset",
    request_body = PasswordResetRequest,
    responses(
        (status = 200, description = "Password reset email sent", body = PasswordResetResponse),
        (status = 400, description = "Invalid email address", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn request_password_reset(
    State(state): State<ApiState>,
    Json(request): Json<PasswordResetRequest>,
) -> Result<Json<PasswordResetResponse>, CustomError> {
    // Validate input
    request
        .validate()
        .map_err(|e| CustomError::ValidationError(format!("Validation failed: {e:?}")))?;

    // Check if user exists
    if user::db::get_user_by_email(&state.sqlx_pool, &request.email)
        .await?
        .is_none()
    {
        // Don't reveal if email exists or not for security
        return Ok(Json(PasswordResetResponse {
            message: "If an account with this email exists, a password reset link has been sent."
                .to_string(),
        }));
    }

    // TODO: Implement password reset email sending
    // For now, just return success message

    Ok(Json(PasswordResetResponse {
        message: "If an account with this email exists, a password reset link has been sent."
            .to_string(),
    }))
}

#[utoipa::path(
    post,
    path = "/v1.0/users/password-reset/confirm",
    request_body = PasswordResetConfirmRequest,
    responses(
        (status = 200, description = "Password reset successful", body = PasswordResetConfirmResponse),
        (status = 400, description = "Invalid token or password", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn confirm_password_reset(
    State(_state): State<ApiState>,
    Json(request): Json<PasswordResetConfirmRequest>,
) -> Result<Json<PasswordResetConfirmResponse>, CustomError> {
    // Validate input
    request
        .validate()
        .map_err(|e| CustomError::ValidationError(format!("Validation failed: {e:?}")))?;

    // TODO: Implement password reset confirmation
    // For now, just return success message

    Ok(Json(PasswordResetConfirmResponse {
        message: "Password has been reset successfully".to_string(),
    }))
}
