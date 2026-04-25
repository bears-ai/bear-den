//! OAuth 2.0 database operations
//!
//! This module provides database operations for OAuth clients, authorization codes,
//! and access tokens using SQLx following the project patterns.

use crate::api::oauth::{
    AccessTokenWithContext, OAuthAccessToken, OAuthAuthorizationCode, OAuthClient,
    OAuthRefreshToken, OAuthScope, UserAccessToken,
    error::OAuthError,
    utils::{is_expired, scopes_to_json},
};
use crate::errors::CustomError;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

// =============================================================================
// OAuth Client Operations
// =============================================================================

/// Create a new OAuth client
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `client_id` - Unique client identifier
/// * `client_secret_hash` - Hashed client secret (None for public clients)
/// * `name` - Human-readable client name
/// * `redirect_uris` - Allowed redirect URIs (JSON array)
/// * `scopes` - Supported scopes for this client
/// * `public` - Whether this is a public client (uses PKCE, no client_secret required)
///
/// # Returns
/// The database ID of the created client
///
/// # Errors
/// Returns database errors or OAuth errors for validation failures
pub async fn create_oauth_client(
    pool: &PgPool,
    client_id: &str,
    client_secret_hash: Option<&str>,
    name: &str,
    redirect_uris: &serde_json::Value,
    scopes: &[OAuthScope],
    public: bool,
) -> Result<i32, CustomError> {
    let scopes_json = scopes_to_json(scopes);

    let record = sqlx::query!(
        r#"
        INSERT INTO oauth_clients (client_id, client_secret, name, redirect_uris, scopes, trusted, public)
        VALUES ($1, $2, $3, $4, $5, false, $6)
        RETURNING id
        "#,
        client_id,
        client_secret_hash,
        name,
        redirect_uris,
        scopes_json,
        public
    )
    .fetch_one(pool)
    .await?;

    Ok(record.id)
}

/// Get OAuth client by client_id
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `client_id` - Client identifier to look up
///
/// # Returns
/// The OAuth client if found, None otherwise
pub async fn get_oauth_client_by_client_id(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<OAuthClient>, CustomError> {
    let row = sqlx::query!(
        r#"
        SELECT id, client_id, client_secret, name, redirect_uris, scopes,
               active as "active!", trusted as "trusted!", public as "public!", created_at, updated_at
        FROM oauth_clients
        WHERE client_id = $1 AND active = true
        "#,
        client_id
    )
    .fetch_optional(pool)
    .await?;

    let client = row.map(|r| OAuthClient {
        id: r.id,
        client_id: r.client_id,
        client_secret: r.client_secret,
        name: r.name,
        redirect_uris: r.redirect_uris,
        scopes: r.scopes,
        active: r.active,
        trusted: r.trusted,
        public: r.public,
        created_at: r.created_at.unwrap_or_else(time::OffsetDateTime::now_utc),
        updated_at: r.updated_at.unwrap_or_else(time::OffsetDateTime::now_utc),
    });

    Ok(client)
}

/// Get OAuth client by database ID
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `id` - Database ID to look up
///
/// # Returns
/// The OAuth client if found, None otherwise
pub async fn get_oauth_client_by_id(
    pool: &PgPool,
    id: i32,
) -> Result<Option<OAuthClient>, CustomError> {
    let row = sqlx::query!(
        r#"
        SELECT id, client_id, client_secret, name, redirect_uris, scopes,
               active as "active!", trusted as "trusted!", public as "public!", created_at, updated_at
        FROM oauth_clients
        WHERE id = $1 AND active = true
        "#,
        id
    )
    .fetch_optional(pool)
    .await?;

    let client = row.map(|r| OAuthClient {
        id: r.id,
        client_id: r.client_id,
        client_secret: r.client_secret,
        name: r.name,
        redirect_uris: r.redirect_uris,
        scopes: r.scopes,
        active: r.active,
        trusted: r.trusted,
        public: r.public,
        created_at: r.created_at.unwrap_or_else(time::OffsetDateTime::now_utc),
        updated_at: r.updated_at.unwrap_or_else(time::OffsetDateTime::now_utc),
    });

    Ok(client)
}

/// Update OAuth client information
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `id` - Client database ID
/// * `name` - New client name
/// * `redirect_uris` - New redirect URIs
/// * `scopes` - New supported scopes
pub async fn update_oauth_client(
    pool: &PgPool,
    id: i32,
    name: &str,
    redirect_uris: &serde_json::Value,
    scopes: &[OAuthScope],
) -> Result<(), CustomError> {
    let scopes_json = scopes_to_json(scopes);

    sqlx::query!(
        r#"
        UPDATE oauth_clients
        SET name = $2, redirect_uris = $3, scopes = $4, updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        name,
        redirect_uris,
        scopes_json
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Deactivate an OAuth client
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `id` - Client database ID
pub async fn deactivate_oauth_client(pool: &PgPool, id: i32) -> Result<(), CustomError> {
    sqlx::query!(
        r#"
        UPDATE oauth_clients
        SET active = false, updated_at = NOW()
        WHERE id = $1
        "#,
        id
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Update OAuth client trusted status
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `id` - Client database ID
/// * `trusted` - New trusted status
pub async fn update_oauth_client_trusted_status(
    pool: &PgPool,
    id: i32,
    trusted: bool,
) -> Result<(), CustomError> {
    sqlx::query!(
        r#"
        UPDATE oauth_clients
        SET trusted = $2, updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        trusted
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// List all active OAuth clients
///
/// # Arguments
/// * `pool` - Database connection pool
///
/// # Returns
/// Vector of all active OAuth clients
pub async fn list_oauth_clients(pool: &PgPool) -> Result<Vec<OAuthClient>, CustomError> {
    let rows = sqlx::query!(
        r#"
        SELECT id, client_id, client_secret, name, redirect_uris, scopes,
               active as "active!", trusted as "trusted!", public as "public!", created_at, updated_at
        FROM oauth_clients
        WHERE active = true
        ORDER BY created_at DESC
        "#
    )
    .fetch_all(pool)
    .await?;

    let clients = rows
        .into_iter()
        .map(|r| OAuthClient {
            id: r.id,
            client_id: r.client_id,
            client_secret: r.client_secret,
            name: r.name,
            redirect_uris: r.redirect_uris,
            scopes: r.scopes,
            active: r.active,
            trusted: r.trusted,
            public: r.public,
            created_at: r.created_at.unwrap_or_else(time::OffsetDateTime::now_utc),
            updated_at: r.updated_at.unwrap_or_else(time::OffsetDateTime::now_utc),
        })
        .collect();

    Ok(clients)
}

// =============================================================================
// OAuth Authorization Code Operations
// =============================================================================

/// Create a new authorization code
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `code` - The authorization code value
/// * `client_id` - Client database ID
/// * `user_id` - User database ID
/// * `redirect_uri` - Redirect URI used in authorization request
/// * `scopes` - Granted scopes
/// * `expires_at` - When this code expires
///
/// # Returns
/// The database ID of the created authorization code
pub async fn create_authorization_code(
    pool: &PgPool,
    code: &str,
    client_id: i32,
    user_id: i32,
    redirect_uri: &str,
    scopes: &[OAuthScope],
    expires_at: OffsetDateTime,
    code_challenge: Option<&str>,
    code_challenge_method: Option<&str>,
) -> Result<i32, CustomError> {
    let scopes_json = scopes_to_json(scopes);

    let record = sqlx::query!(
        r#"
        INSERT INTO oauth_authorization_codes
        (code, client_id, user_id, redirect_uri, scopes, expires_at, code_challenge, code_challenge_method)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id
        "#,
        code,
        client_id,
        user_id,
        redirect_uri,
        scopes_json,
        expires_at,
        code_challenge,
        code_challenge_method
    )
    .fetch_one(pool)
    .await?;

    Ok(record.id)
}

/// Get and validate authorization code
///
/// This function retrieves an authorization code and validates that it:
/// - Exists and hasn't been used
/// - Hasn't expired
/// - Matches the provided client_id and redirect_uri
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `code` - Authorization code to validate
/// * `client_id` - Expected client database ID
/// * `redirect_uri` - Expected redirect URI
///
/// # Returns
/// The authorization code if valid, or an OAuth error
pub async fn get_and_validate_authorization_code(
    pool: &PgPool,
    code: &str,
    client_id: i32,
    redirect_uri: &str,
) -> Result<OAuthAuthorizationCode, OAuthError> {
    let row = sqlx::query!(
        r#"
        SELECT id, code, client_id, user_id, redirect_uri, scopes,
               expires_at, used as "used!", created_at,
               code_challenge, code_challenge_method
        FROM oauth_authorization_codes
        WHERE code = $1 AND client_id = $2 AND redirect_uri = $3
        "#,
        code,
        client_id,
        redirect_uri
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| OAuthError::ServerError(format!("Database error: {e}")))?;

    let row = row.ok_or(OAuthError::InvalidGrant)?;

    let auth_code = OAuthAuthorizationCode {
        id: row.id,
        code: row.code,
        client_id: row.client_id,
        user_id: row.user_id,
        redirect_uri: row.redirect_uri,
        scopes: row.scopes,
        expires_at: row.expires_at,
        used: row.used,
        created_at: row
            .created_at
            .unwrap_or_else(|| time::OffsetDateTime::now_utc()),
        code_challenge: row.code_challenge,
        code_challenge_method: row.code_challenge_method,
    };

    // Check if code has been used
    if auth_code.used {
        return Err(OAuthError::InvalidGrant);
    }

    // Check if code has expired
    if is_expired(auth_code.expires_at) {
        return Err(OAuthError::InvalidGrant);
    }

    Ok(auth_code)
}

/// Mark authorization code as used
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `code_id` - Authorization code database ID
pub async fn mark_authorization_code_used(pool: &PgPool, code_id: i32) -> Result<(), CustomError> {
    sqlx::query!(
        r#"
        UPDATE oauth_authorization_codes
        SET used = true
        WHERE id = $1
        "#,
        code_id
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Clean up expired authorization codes
///
/// # Arguments
/// * `pool` - Database connection pool
///
/// # Returns
/// Number of codes deleted
#[allow(dead_code)]
pub async fn cleanup_expired_authorization_codes(pool: &PgPool) -> Result<u64, CustomError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM oauth_authorization_codes
        WHERE expires_at < NOW()
        "#
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

// =============================================================================
// OAuth Access Token Operations
// =============================================================================

/// Create a new access token
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token` - The access token value
/// * `client_id` - Client database ID
/// * `user_id` - User database ID
/// * `scopes` - Granted scopes
/// * `expires_at` - When this token expires
///
/// # Returns
/// The database ID of the created access token
pub async fn create_access_token(
    pool: &PgPool,
    token: &str,
    client_id: i32,
    user_id: i32,
    scopes: &[OAuthScope],
    expires_at: OffsetDateTime,
) -> Result<i32, CustomError> {
    let scopes_json = scopes_to_json(scopes);

    let record = sqlx::query!(
        r#"
        INSERT INTO oauth_access_tokens (token, client_id, user_id, scopes, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        token,
        client_id,
        user_id,
        scopes_json,
        expires_at
    )
    .fetch_one(pool)
    .await?;

    Ok(record.id)
}

/// Create a new access token with custom expiration for admin use
///
/// This function allows administrators to create access tokens with custom expiration
/// times for testing and administrative purposes.
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token` - The access token value
/// * `client_id` - Client database ID
/// * `user_id` - User database ID
/// * `scopes` - Granted scopes
/// * `expires_in_hours` - Token expiration in hours (max 720 hours / 30 days)
///
/// # Returns
/// The database ID of the created access token
///
/// # Errors
/// Returns `CustomError` if validation fails or database operation fails
pub async fn create_admin_access_token(
    pool: &PgPool,
    token: &str,
    client_id: i32,
    user_id: i32,
    scopes: &[OAuthScope],
    expires_in_hours: i32,
) -> Result<i32, CustomError> {
    // Validate expiration time
    if expires_in_hours < 1 || expires_in_hours > 720 {
        return Err(CustomError::ValidationError(
            "Expiration must be between 1 and 720 hours".to_string(),
        ));
    }

    let expires_at = OffsetDateTime::now_utc() + Duration::hours(expires_in_hours as i64);
    let scopes_json = scopes_to_json(scopes);

    let record = sqlx::query!(
        r#"
        INSERT INTO oauth_access_tokens (token, client_id, user_id, scopes, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        token,
        client_id,
        user_id,
        scopes_json,
        expires_at
    )
    .fetch_one(pool)
    .await?;

    Ok(record.id)
}

/// Get and validate access token
///
/// This function retrieves an access token and validates that it:
/// - Exists and hasn't been revoked
/// - Hasn't expired
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token` - Access token to validate
///
/// # Returns
/// The access token if valid, or an OAuth error
pub async fn get_and_validate_access_token(
    pool: &PgPool,
    token: &str,
) -> Result<OAuthAccessToken, OAuthError> {
    let row = sqlx::query!(
        r#"
        SELECT id, token, client_id, user_id, scopes,
               expires_at, revoked as "revoked!", created_at
        FROM oauth_access_tokens
        WHERE token = $1
        "#,
        token
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| OAuthError::ServerError(format!("Database error: {e}")))?;

    let row = row.ok_or(OAuthError::InvalidGrant)?;

    let access_token = OAuthAccessToken {
        id: row.id,
        token: row.token,
        client_id: row.client_id,
        user_id: row.user_id,
        scopes: row.scopes,
        expires_at: row.expires_at,
        revoked: row.revoked,
        created_at: row.created_at.unwrap_or_else(time::OffsetDateTime::now_utc),
    };

    // Check if token has been revoked
    if access_token.revoked {
        return Err(OAuthError::InvalidGrant);
    }

    // Check if token has expired
    if is_expired(access_token.expires_at) {
        return Err(OAuthError::InvalidGrant);
    }

    Ok(access_token)
}

/// Get access token with user and client information
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token` - Access token to look up
///
/// # Returns
/// Access token with associated user and client data
pub async fn get_access_token_with_context(
    pool: &PgPool,
    token: &str,
) -> Result<Option<AccessTokenWithContext>, CustomError> {
    let row = sqlx::query!(
        r#"
        SELECT
            t.id as token_id,
            t.token,
            t.client_id,
            t.user_id,
            t.scopes,
            t.expires_at,
            t.revoked as "revoked!",
            t.created_at as token_created_at,
            c.client_id as client_identifier,
            c.name as client_name,
            u.username,
            u.email,
            u.display_name
        FROM oauth_access_tokens t
        INNER JOIN oauth_clients c ON t.client_id = c.id
        INNER JOIN users u ON t.user_id = u.id
        WHERE t.token = $1 AND NOT t.revoked AND t.expires_at > NOW()
        "#,
        token
    )
    .fetch_optional(pool)
    .await?;

    let token_context = row.map(|r| AccessTokenWithContext {
        token_id: r.token_id,
        token: r.token,
        client_id: r.client_id,
        user_id: r.user_id,
        scopes: r.scopes,
        expires_at: r.expires_at,
        revoked: r.revoked,
        token_created_at: r
            .token_created_at
            .unwrap_or_else(time::OffsetDateTime::now_utc),
        client_identifier: r.client_identifier,
        client_name: r.client_name,
        username: r.username,
        email: r.email,
        display_name: r.display_name,
    });

    Ok(token_context)
}

/// Revoke an access token
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token` - Access token to revoke
#[allow(dead_code)]
pub async fn revoke_access_token(pool: &PgPool, token: &str) -> Result<(), CustomError> {
    sqlx::query!(
        r#"
        UPDATE oauth_access_tokens
        SET revoked = true
        WHERE token = $1
        "#,
        token
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Revoke access token by ID
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token_id` - Token database ID
pub async fn revoke_oauth_token(pool: &PgPool, token_id: i32) -> Result<(), CustomError> {
    sqlx::query!(
        r#"
        UPDATE oauth_access_tokens
        SET revoked = true
        WHERE id = $1
        "#,
        token_id
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Revoke all access tokens for a user and client
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `user_id` - User database ID
/// * `client_id` - Client database ID
#[allow(dead_code)]
pub async fn revoke_user_client_tokens(
    pool: &PgPool,
    user_id: i32,
    client_id: i32,
) -> Result<u64, CustomError> {
    let result = sqlx::query!(
        r#"
        UPDATE oauth_access_tokens
        SET revoked = true
        WHERE user_id = $1 AND client_id = $2 AND NOT revoked
        "#,
        user_id,
        client_id
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Clean up expired access tokens
///
/// # Arguments
/// * `pool` - Database connection pool
///
/// # Returns
/// Number of tokens deleted
#[allow(dead_code)]
pub async fn cleanup_expired_access_tokens(pool: &PgPool) -> Result<u64, CustomError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM oauth_access_tokens
        WHERE expires_at < NOW()
        "#
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// List active access tokens for a user
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `user_id` - User database ID
///
/// # Returns
/// Vector of active access tokens with client information
pub async fn list_user_access_tokens(
    pool: &PgPool,
    user_id: i32,
) -> Result<Vec<UserAccessToken>, CustomError> {
    let rows = sqlx::query!(
        r#"
        SELECT
            t.id,
            t.token,
            t.scopes,
            t.expires_at,
            t.created_at,
            c.client_id,
            c.name as client_name
        FROM oauth_access_tokens t
        INNER JOIN oauth_clients c ON t.client_id = c.id
        WHERE t.user_id = $1 AND NOT t.revoked AND t.expires_at > NOW()
        ORDER BY t.created_at DESC
        "#,
        user_id
    )
    .fetch_all(pool)
    .await?;

    let tokens = rows
        .into_iter()
        .map(|r| UserAccessToken {
            id: r.id,
            token: r.token,
            scopes: r.scopes,
            expires_at: r.expires_at,
            created_at: r.created_at.unwrap_or_else(time::OffsetDateTime::now_utc),
            client_id: r.client_id,
            client_name: r.client_name,
        })
        .collect();

    Ok(tokens)
}

/// Get access token with context by ID
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token_id` - Token database ID
///
/// # Returns
/// Access token with context if found
pub async fn get_oauth_token_by_id(
    pool: &PgPool,
    token_id: i32,
) -> Result<Option<AccessTokenWithContext>, CustomError> {
    let row = sqlx::query!(
        r#"
        SELECT
            t.id as token_id,
            t.token,
            t.client_id,
            t.user_id,
            t.scopes,
            t.expires_at,
            t.revoked as "revoked!",
            t.created_at as token_created_at,
            c.client_id as client_identifier,
            c.name as client_name,
            u.username,
            u.email,
            u.display_name
        FROM oauth_access_tokens t
        INNER JOIN oauth_clients c ON t.client_id = c.id
        INNER JOIN users u ON t.user_id = u.id
        WHERE t.id = $1
        "#,
        token_id
    )
    .fetch_optional(pool)
    .await?;

    let token_context = row.map(|r| AccessTokenWithContext {
        token_id: r.token_id,
        token: r.token,
        client_id: r.client_id,
        user_id: r.user_id,
        scopes: r.scopes,
        expires_at: r.expires_at,
        revoked: r.revoked,
        token_created_at: r
            .token_created_at
            .unwrap_or_else(time::OffsetDateTime::now_utc),
        client_identifier: r.client_identifier,
        client_name: r.client_name,
        username: r.username,
        email: r.email,
        display_name: r.display_name,
    });

    Ok(token_context)
}

// =============================================================================
// OAuth Refresh Token Operations
// =============================================================================

/// Create a new refresh token
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token` - The refresh token value
/// * `client_id` - Client database ID
/// * `user_id` - User database ID
/// * `scopes` - Granted scopes
/// * `expires_at` - When this token expires
///
/// # Returns
/// The database ID of the created refresh token
pub async fn create_refresh_token(
    pool: &PgPool,
    token: &str,
    client_id: i32,
    user_id: i32,
    scopes: &[OAuthScope],
    expires_at: OffsetDateTime,
) -> Result<i32, CustomError> {
    let scopes_json = scopes_to_json(scopes);

    let record = sqlx::query!(
        r#"
        INSERT INTO oauth_refresh_tokens (token, client_id, user_id, scopes, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        token,
        client_id,
        user_id,
        scopes_json,
        expires_at
    )
    .fetch_one(pool)
    .await?;

    Ok(record.id)
}

/// Get and validate refresh token
///
/// This function retrieves a refresh token and validates that it:
/// - Exists and hasn't been revoked
/// - Hasn't expired
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token` - Refresh token to validate
///
/// # Returns
/// The refresh token if valid, or an OAuth error
pub async fn get_and_validate_refresh_token(
    pool: &PgPool,
    token: &str,
) -> Result<OAuthRefreshToken, OAuthError> {
    let row = sqlx::query!(
        r#"
        SELECT id, token, client_id, user_id, scopes,
               expires_at, revoked as "revoked!", created_at
        FROM oauth_refresh_tokens
        WHERE token = $1
        "#,
        token
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| OAuthError::ServerError(format!("Database error: {e}")))?;

    let row = row.ok_or(OAuthError::InvalidGrant)?;

    let refresh_token = OAuthRefreshToken {
        id: row.id,
        token: row.token,
        client_id: row.client_id,
        user_id: row.user_id,
        scopes: row.scopes,
        expires_at: row.expires_at,
        revoked: row.revoked,
        created_at: row.created_at.unwrap_or_else(time::OffsetDateTime::now_utc),
    };

    // Check if token has been revoked
    if refresh_token.revoked {
        return Err(OAuthError::InvalidGrant);
    }

    // Check if token has expired
    if is_expired(refresh_token.expires_at) {
        return Err(OAuthError::InvalidGrant);
    }

    Ok(refresh_token)
}

/// Revoke a refresh token
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token` - Refresh token to revoke
pub async fn revoke_refresh_token(pool: &PgPool, token: &str) -> Result<(), CustomError> {
    sqlx::query!(
        r#"
        UPDATE oauth_refresh_tokens
        SET revoked = true
        WHERE token = $1
        "#,
        token
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Revoke refresh token by ID
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token_id` - Token database ID
pub async fn revoke_oauth_refresh_token(pool: &PgPool, token_id: i32) -> Result<(), CustomError> {
    sqlx::query!(
        r#"
        UPDATE oauth_refresh_tokens
        SET revoked = true
        WHERE id = $1
        "#,
        token_id
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Revoke all refresh tokens for a user and client
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `user_id` - User database ID
/// * `client_id` - Client database ID
pub async fn revoke_user_client_refresh_tokens(
    pool: &PgPool,
    user_id: i32,
    client_id: i32,
) -> Result<u64, CustomError> {
    let result = sqlx::query!(
        r#"
        UPDATE oauth_refresh_tokens
        SET revoked = true
        WHERE user_id = $1 AND client_id = $2 AND NOT revoked
        "#,
        user_id,
        client_id
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Clean up expired refresh tokens
///
/// # Arguments
/// * `pool` - Database connection pool
///
/// # Returns
/// Number of tokens deleted
#[allow(dead_code)]
pub async fn cleanup_expired_refresh_tokens(pool: &PgPool) -> Result<u64, CustomError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM oauth_refresh_tokens
        WHERE expires_at < NOW()
        "#
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

// =============================================================================
// Admin Operations
// =============================================================================

/// List all access tokens with context for admin interface
///
/// # Arguments
/// * `pool` - Database connection pool
///
/// # Returns
/// List of all access tokens with user and client context
pub async fn list_all_access_tokens_with_context(
    pool: &PgPool,
) -> Result<Vec<AccessTokenWithContext>, CustomError> {
    let rows = sqlx::query!(
        r#"
        SELECT
            t.id as token_id,
            t.token,
            t.client_id,
            t.user_id,
            t.scopes,
            t.expires_at,
            t.revoked as "revoked!",
            t.created_at as token_created_at,
            c.client_id as client_identifier,
            c.name as client_name,
            u.username,
            u.email,
            u.display_name
        FROM oauth_access_tokens t
        INNER JOIN oauth_clients c ON t.client_id = c.id
        INNER JOIN users u ON t.user_id = u.id
        ORDER BY t.created_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let tokens = rows
        .into_iter()
        .map(|r| AccessTokenWithContext {
            token_id: r.token_id,
            token: r.token,
            client_id: r.client_id,
            user_id: r.user_id,
            scopes: r.scopes,
            expires_at: r.expires_at,
            revoked: r.revoked,
            token_created_at: r
                .token_created_at
                .unwrap_or_else(time::OffsetDateTime::now_utc),
            client_identifier: r.client_identifier,
            client_name: r.client_name,
            username: r.username,
            email: r.email,
            display_name: r.display_name,
        })
        .collect();

    Ok(tokens)
}

// =============================================================================
// Cleanup Operations
// =============================================================================

/// Run cleanup operations for expired codes and tokens
///
/// This function should be called periodically to clean up expired
/// authorization codes and access tokens.
///
/// # Arguments
/// * `pool` - Database connection pool
///
/// # Returns
/// Tuple of (codes_deleted, tokens_deleted)
pub async fn _cleanup_expired_oauth_data(pool: &PgPool) -> Result<(u64, u64), CustomError> {
    let codes_deleted = cleanup_expired_authorization_codes(pool).await?;
    let tokens_deleted = cleanup_expired_access_tokens(pool).await?;

    Ok((codes_deleted, tokens_deleted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::oauth::utils::{
        access_token_expiration, authorization_code_expiration, generate_access_token,
        generate_authorization_code, generate_client_id, hash_client_secret,
    };
    use sqlx::PgPool;

    // Note: These tests would require a test database setup
    // They are provided as examples of how the functions should be tested

    #[sqlx::test]
    async fn test_create_and_get_oauth_client(
        pool: PgPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let client_id = generate_client_id();
        let client_secret = "test_secret";
        let client_secret_hash = hash_client_secret(client_secret)?;
        let name = "Test Client";
        let redirect_uris = serde_json::json!(["https://example.com/callback"]);
        let scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];

        let db_id = create_oauth_client(
            &pool,
            &client_id,
            Some(client_secret_hash.as_str()),
            name,
            &redirect_uris,
            &scopes,
            false,
        )
        .await?;

        let retrieved_client = get_oauth_client_by_client_id(&pool, &client_id)
            .await?
            .expect("Client should exist");

        assert_eq!(retrieved_client.id, db_id);
        assert_eq!(retrieved_client.client_id, client_id);
        assert_eq!(retrieved_client.name, name);
        assert_eq!(retrieved_client.redirect_uris, redirect_uris);

        Ok(())
    }

    #[sqlx::test]
    async fn test_authorization_code_flow(pool: PgPool) -> Result<(), Box<dyn std::error::Error>> {
        // Create test user first
        let user = sqlx::query_as::<_, (i32,)>(
            r#"
            INSERT INTO users (email, username, passhash)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
        )
        .bind("testuser1@example.com")
        .bind("testuser1")
        .bind("hashed_password")
        .fetch_one(&pool)
        .await?;
        let user_id = user.0;

        // Create test OAuth client
        let client_id_str = generate_client_id();
        let client_secret_hash = hash_client_secret("test_secret")?;
        let redirect_uris = serde_json::json!(["https://example.com/callback"]);
        let scopes = vec![OAuthScope::ProfileRead];

        let client_db_id = create_oauth_client(
            &pool,
            &client_id_str,
            Some(client_secret_hash.as_str()),
            "Test Client",
            &redirect_uris,
            &scopes,
            false,
        )
        .await?;

        // Now create authorization code
        let code = generate_authorization_code();
        let redirect_uri = "https://example.com/callback";
        let expires_at = authorization_code_expiration();

        let code_id = create_authorization_code(
            &pool,
            &code,
            client_db_id,
            user_id,
            redirect_uri,
            &scopes,
            expires_at,
            Some("some_code_challenge"), // PKCE code challenge
            Some("S256"),                // PKCE code challenge method
        )
        .await?;

        let retrieved_code =
            get_and_validate_authorization_code(&pool, &code, client_db_id, redirect_uri).await?;

        assert_eq!(retrieved_code.id, code_id);
        assert_eq!(retrieved_code.code, code);
        assert!(!retrieved_code.used);

        mark_authorization_code_used(&pool, code_id).await?;

        // Should fail after being marked as used
        let result =
            get_and_validate_authorization_code(&pool, &code, client_db_id, redirect_uri).await;

        assert!(matches!(result, Err(OAuthError::InvalidGrant)));

        Ok(())
    }

    #[sqlx::test]
    async fn test_access_token_operations(pool: PgPool) -> Result<(), Box<dyn std::error::Error>> {
        // Create test user first
        let user = sqlx::query_as::<_, (i32,)>(
            r#"
            INSERT INTO users (email, username, passhash)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
        )
        .bind("testtokenuser@example.com")
        .bind("testtokenuser")
        .bind("hashed_password")
        .fetch_one(&pool)
        .await?;
        let user_id = user.0;

        // Create test OAuth client
        let client_id_str = generate_client_id();
        let client_secret_hash = hash_client_secret("test_secret")?;
        let redirect_uris = serde_json::json!(["https://example.com/callback"]);
        let scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];

        let client_db_id = create_oauth_client(
            &pool,
            &client_id_str,
            Some(client_secret_hash.as_str()),
            "Test Token Client",
            &redirect_uris,
            &scopes,
            false,
        )
        .await?;

        // Now create access token
        let token = generate_access_token();
        let expires_at = access_token_expiration();

        let token_id =
            create_access_token(&pool, &token, client_db_id, user_id, &scopes, expires_at).await?;

        let retrieved_token = get_and_validate_access_token(&pool, &token).await?;

        assert_eq!(retrieved_token.id, token_id);
        assert_eq!(retrieved_token.token, token);
        assert!(!retrieved_token.revoked);

        revoke_access_token(&pool, &token).await?;

        // Should fail after being revoked
        let result = get_and_validate_access_token(&pool, &token).await;
        assert!(matches!(result, Err(OAuthError::InvalidGrant)));

        Ok(())
    }
}
