//! OAuth 2.0 authorization endpoints
//!
//! This module implements the OAuth 2.0 authorization server endpoints following RFC 6749.
//! It provides handlers for the authorization endpoint that integrates with the existing
//! axum-login authentication system and renders authorization pages using MiniJinja templates.

use axum::{
    Form, Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use url::Url;

use crate::{
    api::{
        oauth::{
            AuthorizationRequest, OAuthScope, TokenRequest, TokenResponse, UserInfoResponse, db,
            error::OAuthError,
            jwt::{JwtManager, create_jwt_manager},
            scopes_from_json, scopes_to_string, utils,
        },
        templates::{
            AuthorizeContext, ErrorContext, UserContext, generate_csrf_token, parse_scopes,
            render_authorize_page, render_error_page,
        },
    },
    auth_backend::AuthSession,
    core::user::db as user_db,
    errors::CustomError,
};

/// OAuth authorization request with PKCE support
///
/// Extends the basic AuthorizationRequest with PKCE parameters for enhanced security.
#[derive(Debug, Deserialize)]
pub struct AuthorizationRequestWithPKCE {
    /// Response type (must be "code" for authorization code flow)
    pub response_type: String,
    /// Client identifier
    pub client_id: String,
    /// Redirect URI where the authorization response will be sent
    pub redirect_uri: String,
    /// Requested scopes (space-separated)
    pub scope: Option<String>,
    /// Opaque value to prevent CSRF attacks
    pub state: Option<String>,
    /// PKCE code challenge
    pub code_challenge: Option<String>,
    /// PKCE code challenge method (S256 or plain)
    pub code_challenge_method: Option<String>,
}

/// OAuth authorization form submission
///
/// Handles the user's response to the authorization request (Allow/Deny).
#[derive(Debug, Deserialize)]
pub struct AuthorizationForm {
    /// User action: "allow" or "deny"
    pub action: String,
    /// CSRF token for form protection
    pub csrf_token: Option<String>,
    /// OAuth parameters (repeated from the authorization request)
    pub client_id: String,
    pub response_type: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
}

/// Application state for OAuth endpoints
#[derive(Clone)]
pub struct OAuthState {
    pub pool: PgPool,
    pub web_server_url: String,
    pub api_server_url: String,
    pub jwt_manager: JwtManager,
}

impl OAuthState {
    /// Create a new OAuth state with JWT manager
    pub fn new(pool: PgPool, web_server_url: String, api_server_url: String) -> Self {
        Self {
            pool,
            web_server_url,
            api_server_url,
            jwt_manager: create_jwt_manager(),
        }
    }
}

/// GET /oauth/authorize - OAuth authorization endpoint
///
/// This endpoint initiates the OAuth authorization flow. It validates the authorization
/// request, checks if the user is authenticated, and renders the authorization page
/// where the user can grant or deny access to the requesting application.
///
/// # Parameters
/// - `response_type`: Must be "code" for authorization code flow
/// - `client_id`: The client identifier of the requesting application
/// - `redirect_uri`: Where to redirect after authorization (must be registered)
/// - `scope`: Requested permissions (space-separated, optional)
/// - `state`: Opaque value for CSRF protection (recommended)
/// - `code_challenge`: PKCE code challenge (optional but recommended)
/// - `code_challenge_method`: PKCE method, "S256" or "plain" (optional)
///
/// # Returns
/// - Authorization page if user is authenticated and request is valid
/// - Redirect to login if user is not authenticated
/// - Error page if request is invalid
///
/// # Security
/// - Validates all OAuth parameters according to RFC 6749
/// - Supports PKCE (RFC 7636) for enhanced security
/// - Integrates with existing axum-login authentication
/// - Validates redirect URIs against registered values
/// - Validates requested scopes against client permissions
pub async fn authorize_get(
    Query(params): Query<AuthorizationRequestWithPKCE>,
    State(state): State<OAuthState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    // Convert to basic authorization request for validation
    let auth_request = AuthorizationRequest {
        response_type: params.response_type.clone(),
        client_id: params.client_id.clone(),
        redirect_uri: params.redirect_uri.clone(),
        scope: params.scope.clone(),
        _state: params.state.clone(),
    };

    // Validate the authorization request
    if let Err(oauth_error) =
        validate_authorization_request(&state.pool, &auth_request, &params).await
    {
        return handle_oauth_error(oauth_error, None, None).await;
    }

    // Check if user is authenticated
    let user = match check_user_authentication(&auth_session) {
        Some(user) => user,
        None => {
            // Redirect to web server login with return URL
            let return_url = build_return_url(&params, &state.api_server_url)?;
            let login_url = format!(
                "{}/login?next={}",
                state.web_server_url,
                urlencoding::encode(&return_url)
            );
            return Ok(Redirect::to(&login_url).into_response());
        }
    };

    // Get client information
    let client = match db::get_oauth_client_by_client_id(&state.pool, &params.client_id).await? {
        Some(client) => client,
        None => {
            return handle_oauth_error(
                OAuthError::InvalidRequest(format!(
                    "Client not found: No client registered with client_id '{}'. Verify the client_id is correct and that the client is active.",
                    params.client_id
                )),
                Some(params.client_id.clone()),
                Some(&params.redirect_uri),
            )
            .await;
        }
    };

    // Parse requested scopes
    let requested_scopes = match auth_request.parse_scopes() {
        Ok(scopes) => scopes,
        Err(oauth_error) => {
            return handle_oauth_error(oauth_error, Some(client.name), Some(&params.redirect_uri))
                .await;
        }
    };

    // Validate scopes against client permissions
    let client_scopes = scopes_from_json(&client.scopes)?;
    if !utils::validate_scopes_for_client(&requested_scopes, &client_scopes) {
        return handle_oauth_error(
            OAuthError::InvalidScope,
            Some(client.name),
            Some(&params.redirect_uri),
        )
        .await;
    }

    // Check if client is trusted - if so, automatically approve the request
    if client.trusted {
        // Generate authorization code automatically for trusted clients
        let authorization_code = utils::generate_authorization_code();
        let expires_at = utils::authorization_code_expiration();

        // Store authorization code in database with PKCE data if provided
        db::create_authorization_code(
            &state.pool,
            &authorization_code,
            client.id,
            user.id,
            &params.redirect_uri,
            &requested_scopes,
            expires_at,
            params.code_challenge.as_deref(),
            params.code_challenge_method.as_deref(),
        )
        .await?;

        // Redirect with authorization code immediately
        return generate_authorization_response(
            &params.redirect_uri,
            Some(&authorization_code),
            None,
            params.state.as_deref(),
        );
    }

    // Get full user information from database to include email
    let db_user = match user_db::get_user_by_id(&state.pool, user.id).await? {
        Some(user) => user,
        None => {
            return handle_oauth_error(
                OAuthError::ServerError("User not found in database".to_string()),
                Some(client.name),
                Some(&params.redirect_uri),
            )
            .await;
        }
    };

    // Render authorization page for non-trusted clients
    let context = AuthorizeContext {
        client_id: params.client_id,
        client_name: Some(client.name),
        client_description: None, // Could be added to client model in future
        response_type: params.response_type,
        redirect_uri: Some(params.redirect_uri),
        scope: params.scope.clone(),
        scopes: parse_scopes(params.scope.as_deref()),
        state: params.state,
        code_challenge: params.code_challenge,
        code_challenge_method: params.code_challenge_method,
        csrf_token: Some(generate_csrf_token()),
        user: UserContext {
            id: user.id,
            username: Some(user.username.clone()),
            display_name: Some(db_user.display_name),
            email: db_user.email,
        },
        error: None,
    };

    render_authorize_page(context)
}

/// POST /oauth/authorize - OAuth authorization form handler
///
/// This endpoint handles the user's response to the authorization request.
/// It processes the form submission where the user either grants or denies
/// access to the requesting application.
///
/// # Form Parameters
/// - `action`: "allow" or "deny"
/// - `csrf_token`: CSRF protection token
/// - All OAuth parameters from the original request
///
/// # Returns
/// - Redirect to client with authorization code if user allows access
/// - Redirect to client with error if user denies access
/// - Error page if form submission is invalid
///
/// # Security
/// - Validates CSRF token (placeholder implementation)
/// - Re-validates all OAuth parameters
/// - Generates secure authorization code with expiration
/// - Stores authorization code in database for token exchange
pub async fn authorize_post(
    State(state): State<OAuthState>,
    auth_session: AuthSession,
    Form(form): Form<AuthorizationForm>,
) -> Result<Response, CustomError> {
    // Check if user is authenticated
    let user = match check_user_authentication(&auth_session) {
        Some(user) => user,
        None => {
            return handle_oauth_error(OAuthError::AccessDenied, None, Some(&form.redirect_uri))
                .await;
        }
    };

    // Validate CSRF token (placeholder - in production, this would validate against session)
    if form.csrf_token.is_none() {
        return handle_oauth_error(
            OAuthError::InvalidRequest("Missing CSRF token".to_string()),
            None,
            Some(&form.redirect_uri),
        )
        .await;
    }

    // Convert form to authorization request for validation
    let auth_request = AuthorizationRequest {
        response_type: form.response_type.clone(),
        client_id: form.client_id.clone(),
        redirect_uri: form.redirect_uri.clone(),
        scope: form.scope.clone(),
        _state: form.state.clone(),
    };

    let pkce_params = AuthorizationRequestWithPKCE {
        response_type: form.response_type.clone(),
        client_id: form.client_id.clone(),
        redirect_uri: form.redirect_uri.clone(),
        scope: form.scope.clone(),
        state: form.state.clone(),
        code_challenge: form.code_challenge.clone(),
        code_challenge_method: form.code_challenge_method.clone(),
    };

    // Re-validate the authorization request
    if let Err(oauth_error) =
        validate_authorization_request(&state.pool, &auth_request, &pkce_params).await
    {
        return handle_oauth_error(oauth_error, None, Some(&form.redirect_uri)).await;
    }

    // Handle user's decision
    match form.action.as_str() {
        "deny" => {
            // User denied access - redirect with error
            generate_authorization_response(
                &form.redirect_uri,
                None,
                Some(OAuthError::AccessDenied),
                form.state.as_deref(),
            )
        }
        "allow" => {
            // User granted access - generate authorization code
            let client = db::get_oauth_client_by_client_id(&state.pool, &form.client_id)
                .await?
                .ok_or_else(|| OAuthError::InvalidRequest(format!(
                    "Client not found: No client registered with client_id '{}'. Verify the client_id is correct.",
                    form.client_id
                )))?;

            let requested_scopes = auth_request.parse_scopes()?;
            let authorization_code = utils::generate_authorization_code();
            let expires_at = utils::authorization_code_expiration();

            // Store authorization code in database with PKCE data if provided
            db::create_authorization_code(
                &state.pool,
                &authorization_code,
                client.id,
                user.id,
                &form.redirect_uri,
                &requested_scopes,
                expires_at,
                form.code_challenge.as_deref(),
                form.code_challenge_method.as_deref(),
            )
            .await?;

            // Redirect with authorization code
            generate_authorization_response(
                &form.redirect_uri,
                Some(&authorization_code),
                None,
                form.state.as_deref(),
            )
        }
        _ => {
            handle_oauth_error(
                OAuthError::InvalidRequest("Invalid action".to_string()),
                None,
                Some(&form.redirect_uri),
            )
            .await
        }
    }
}

/// POST /oauth/token - OAuth token endpoint
///
/// This endpoint handles the token exchange phase of the OAuth authorization flow.
/// It validates the authorization code and client credentials, then issues an access token.
///
/// # Form Parameters
/// - `grant_type`: Must be "authorization_code"
/// - `code`: Authorization code from authorization endpoint
/// - `redirect_uri`: Redirect URI used in authorization request
/// - `client_id`: Client identifier
/// - `client_secret`: Client secret for authentication (optional if using PKCE)
/// - `code_verifier`: PKCE code verifier (optional)
///
/// # Returns
/// - JSON token response with access token if successful
/// - JSON error response if validation fails
///
/// # Security
/// - Validates authorization code (existence, expiration, usage)
/// - Authenticates client using client_id and client_secret
/// - Validates PKCE code_verifier if code_challenge was used
/// - Validates redirect URI matches authorization request
/// - Marks authorization code as used to prevent replay attacks
pub async fn token_post(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    form: Form<TokenRequest>,
) -> Result<Response, CustomError> {
    let token_request = form.0;

    // Log incoming token request (sanitized - don't log secrets)
    tracing::info!(
        "OAuth token exchange request: client_id={}, grant_type={}, has_code={}, has_code_verifier={}, has_client_secret={}, redirect_uri={}",
        token_request.client_id,
        token_request.grant_type,
        !token_request.code.is_empty(),
        token_request.code_verifier.is_some(),
        token_request.client_secret.is_some(),
        token_request.redirect_uri
    );

    // Validate basic token request parameters
    if let Err(oauth_error) = token_request.validate() {
        tracing::warn!(
            "Token request validation failed: client_id={}, error={}",
            token_request.client_id,
            oauth_error
        );
        return Ok(oauth_error_response(oauth_error));
    }

    // Authenticate the client
    let client = match authenticate_client(&state.pool, &token_request, &headers).await {
        Ok(client) => {
            tracing::debug!(
                "Client authenticated successfully: client_id={}, name={}, public={}",
                client.client_id,
                client.name,
                client.public
            );
            client
        }
        Err(oauth_error) => {
            tracing::error!(
                "Client authentication failed: client_id={}, error={}, error_code={}",
                token_request.client_id,
                oauth_error,
                oauth_error.error_code()
            );
            return Ok(oauth_error_response(oauth_error));
        }
    };

    // Get and validate authorization code
    let auth_code = match db::get_and_validate_authorization_code(
        &state.pool,
        &token_request.code,
        client.id,
        &token_request.redirect_uri,
    )
    .await
    {
        Ok(code) => {
            tracing::debug!(
                "Authorization code validated: code_id={}, has_pkce_challenge={}, has_pkce_method={}",
                code.id,
                code.code_challenge.is_some(),
                code.code_challenge_method.is_some()
            );
            code
        }
        Err(oauth_error) => {
            tracing::error!(
                "Authorization code validation failed: client_id={}, code={}, redirect_uri={}, error={}",
                client.client_id,
                &token_request.code[..token_request.code.len().min(20)],
                token_request.redirect_uri,
                oauth_error
            );
            // Enhance error message with redirect_uri context
            let enhanced_error = match oauth_error {
                OAuthError::InvalidGrant => OAuthError::InvalidRequest(format!(
                    "Invalid authorization grant. The authorization code may be invalid, expired, already used, or the redirect_uri '{}' does not match the one used in the authorization request. Ensure you're using the exact same redirect_uri that was used when obtaining the authorization code.",
                    token_request.redirect_uri
                )),
                _ => oauth_error,
            };
            return Ok(oauth_error_response(enhanced_error));
        }
    };

    // Validate PKCE if code_challenge was used
    if let Err(oauth_error) = validate_pkce(&token_request, &auth_code).await {
        tracing::error!(
            "PKCE validation failed: client_id={}, error={}, has_code_verifier={}, has_code_challenge={}, challenge_method={:?}",
            client.client_id,
            oauth_error,
            token_request.code_verifier.is_some(),
            auth_code.code_challenge.is_some(),
            auth_code.code_challenge_method
        );
        return Ok(oauth_error_response(oauth_error));
    }

    // Parse granted scopes
    let granted_scopes = match scopes_from_json(&auth_code.scopes) {
        Ok(scopes) => scopes,
        Err(oauth_error) => return Ok(oauth_error_response(oauth_error)),
    };

    // Generate JWT access token
    let expires_at = utils::access_token_expiration();
    let access_token = match state.jwt_manager.generate_access_token(
        auth_code.user_id,
        &client.client_id,
        &granted_scopes,
        expires_at,
    ) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("Failed to generate JWT access token: {}", e);
            return Ok(oauth_error_response(OAuthError::ServerError(
                "Failed to generate access token".to_string(),
            )));
        }
    };

    // Store access token in database (for revocation and tracking)
    if let Err(e) = db::create_access_token(
        &state.pool,
        &access_token,
        client.id,
        auth_code.user_id,
        &granted_scopes,
        expires_at,
    )
    .await
    {
        tracing::error!(
            "Failed to store access token: {} | token={} client_id={} user_id={} scopes={:?} expires_at={:?}",
            e,
            access_token,
            client.id,
            auth_code.user_id,
            granted_scopes,
            expires_at
        );
        return Ok(oauth_error_response(OAuthError::ServerError(format!(
            "Failed to store access token: {e}"
        ))));
    }

    // Generate and store refresh token for confidential clients (per RFC 6749)
    // Public clients should not receive refresh tokens
    let refresh_token = if !client.public {
        let refresh_token_value = utils::generate_refresh_token();
        let refresh_expires_at = utils::refresh_token_expiration();

        if let Err(e) = db::create_refresh_token(
            &state.pool,
            &refresh_token_value,
            client.id,
            auth_code.user_id,
            &granted_scopes,
            refresh_expires_at,
        )
        .await
        {
            tracing::error!(
                "Failed to store refresh token: {} | client_id={} user_id={}",
                e,
                client.id,
                auth_code.user_id
            );
            // Continue without refresh token - access token was created successfully
            None
        } else {
            Some(refresh_token_value)
        }
    } else {
        None
    };

    // Mark authorization code as used
    if let Err(e) = db::mark_authorization_code_used(&state.pool, auth_code.id).await {
        tracing::error!("Failed to mark authorization code as used: {}", e);
        // Continue anyway - token was created successfully
    }

    // Generate token response
    let token_response = generate_access_token_response(
        &access_token,
        expires_at,
        &granted_scopes,
        refresh_token.as_deref(),
    );

    Ok(Json(token_response).into_response())
}

// =============================================================================
// Token Endpoint Helper Functions
// =============================================================================

/// Authenticate OAuth client using client_id and client_secret
///
/// Supports client_secret_post (in request body) method for client authentication.
/// Uses existing password hashing utilities for client secret verification.
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `token_request` - Token request containing client credentials
/// * `_headers` - HTTP headers (unused in current implementation)
///
/// # Returns
/// The authenticated OAuth client or an OAuth error
async fn authenticate_client(
    pool: &PgPool,
    token_request: &TokenRequest,
    _headers: &HeaderMap,
) -> Result<crate::api::oauth::OAuthClient, OAuthError> {
    tracing::debug!(
        "Authenticating client: client_id={}",
        token_request.client_id
    );

    // Get client by client_id
    let client = match db::get_oauth_client_by_client_id(pool, &token_request.client_id).await {
        Ok(Some(client)) => {
            tracing::debug!(
                "Client found: id={}, name={}, public={}, has_secret={}, active={}",
                client.id,
                client.name,
                client.public,
                client.client_secret.is_some(),
                client.active
            );
            client
        }
        Ok(None) => {
            tracing::warn!("Client not found: client_id={}", token_request.client_id);
            return Err(OAuthError::InvalidRequest(format!(
                "Client authentication failed: No client found with client_id '{}'. Verify that the client_id is correct and that the client is registered and active.",
                token_request.client_id
            )));
        }
        Err(e) => {
            tracing::error!(
                "Database error looking up client: client_id={}, error={}",
                token_request.client_id,
                e
            );
            return Err(OAuthError::ServerError(format!("Database error: {e}")));
        }
    };

    // Handle client authentication
    if let Some(secret) = &token_request.client_secret {
        tracing::debug!("Client secret provided in request");
        // Client secret provided - verify it (for confidential clients or public clients using secret)
        if let Some(ref client_secret_hash) = client.client_secret {
            if !utils::verify_client_secret(secret, client_secret_hash) {
                tracing::warn!(
                    "Client secret verification failed: client_id={}, client_public={}",
                    client.client_id,
                    client.public
                );
                return Err(OAuthError::InvalidRequest(
                    "Client authentication failed: The provided client_secret is incorrect. Verify that you're using the correct client_secret for this client_id.".to_string(),
                ));
            }
            tracing::debug!("Client secret verified successfully");
        } else {
            // Client has no secret stored but secret was provided - invalid
            tracing::warn!(
                "Client secret provided but client has no secret stored: client_id={}, client_public={}",
                client.client_id,
                client.public
            );
            return Err(OAuthError::InvalidRequest(format!(
                "This client (client_id: {}) is configured as a public client and does not use client_secret authentication. {}",
                client.client_id,
                if client.public {
                    "Use PKCE (code_verifier) instead of client_secret for authentication."
                } else {
                    "This appears to be a configuration error - the client is not marked as public but has no secret stored."
                }
            )));
        }
    } else {
        tracing::debug!("No client secret provided in request");
        // No client secret provided
        if client.public {
            tracing::debug!("Client is public, checking for PKCE code_verifier");
            // Public client - require PKCE
            if token_request.code_verifier.is_none() {
                tracing::error!(
                    "Public client missing code_verifier: client_id={}",
                    client.client_id
                );
                return Err(OAuthError::InvalidRequest(
                    "This is a public client and requires PKCE (Proof Key for Code Exchange). The code_verifier parameter is required. Ensure you're including the code_verifier that was used to generate the code_challenge in your authorization request.".to_string(),
                ));
            }
            tracing::debug!("Public client has code_verifier, PKCE validation will occur later");
            // PKCE validation will happen later in the flow
        } else {
            // Confidential client - require client secret
            tracing::error!(
                "Confidential client missing client_secret: client_id={}",
                client.client_id
            );
            return Err(OAuthError::InvalidRequest(
                "This is a confidential client and requires client authentication. Include the client_secret parameter in your token request.".to_string(),
            ));
        }
    }

    tracing::debug!(
        "Client authentication successful: client_id={}",
        client.client_id
    );
    Ok(client)
}

/// Validate PKCE code_verifier against stored code_challenge
///
/// Supports both S256 (SHA256) and plain challenge methods following RFC 7636.
///
/// # Arguments
/// * `token_request` - Token request containing code_verifier
/// * `auth_code` - Authorization code containing code_challenge data
///
/// # Returns
/// Ok(()) if PKCE validation passes or is not required, OAuth error otherwise
async fn validate_pkce(
    token_request: &TokenRequest,
    auth_code: &crate::api::oauth::OAuthAuthorizationCode,
) -> Result<(), OAuthError> {
    tracing::debug!(
        "Validating PKCE: has_code_challenge={}, has_code_challenge_method={}, has_code_verifier={}",
        auth_code.code_challenge.is_some(),
        auth_code.code_challenge_method.is_some(),
        token_request.code_verifier.is_some()
    );

    // Check if PKCE was used in the authorization request
    match (&auth_code.code_challenge, &auth_code.code_challenge_method) {
        (Some(code_challenge), Some(code_challenge_method)) => {
            tracing::debug!(
                "PKCE was used in authorization: method={}, challenge_length={}",
                code_challenge_method,
                code_challenge.len()
            );
            // PKCE was used - code_verifier is required
            match &token_request.code_verifier {
                Some(code_verifier) => {
                    tracing::debug!(
                        "Validating PKCE: verifier_length={}, challenge_length={}, method={}",
                        code_verifier.len(),
                        code_challenge.len(),
                        code_challenge_method
                    );
                    // Validate the code_verifier against the stored code_challenge
                    match utils::validate_pkce(code_verifier, code_challenge, code_challenge_method)
                    {
                        Ok(()) => {
                            tracing::info!("PKCE validation successful");
                            Ok(())
                        }
                        Err(e) => {
                            tracing::error!(
                                "PKCE validation failed: method={}, error={}",
                                code_challenge_method,
                                e
                            );
                            // The error from validate_pkce already contains detailed context
                            // Just pass it through, but ensure it's an InvalidRequest
                            match e {
                                OAuthError::InvalidRequest(msg) => {
                                    Err(OAuthError::InvalidRequest(msg))
                                }
                                _ => Err(OAuthError::InvalidRequest(format!(
                                    "PKCE validation failed (method: {}): {}",
                                    code_challenge_method,
                                    e.error_description()
                                ))),
                            }
                        }
                    }
                }
                None => {
                    // PKCE was used in auth request but no verifier provided
                    tracing::error!(
                        "PKCE code_challenge present but no code_verifier provided: method={}, challenge_length={}",
                        code_challenge_method,
                        code_challenge.len()
                    );
                    Err(OAuthError::InvalidRequest(format!(
                        "PKCE was used in the authorization request (method: {}), but no code_verifier was provided in the token request. Include the code_verifier parameter with the same value used to generate the code_challenge.",
                        code_challenge_method
                    )))
                }
            }
        }
        (None, None) => {
            tracing::debug!("PKCE was not used in authorization request");
            // PKCE was not used - code_verifier should not be provided
            if token_request.code_verifier.is_some() {
                tracing::warn!(
                    "code_verifier provided but PKCE was not used in authorization: verifier_length={}",
                    token_request.code_verifier.as_ref().unwrap().len()
                );
                return Err(OAuthError::InvalidRequest(
                    "code_verifier was provided, but PKCE was not used in the authorization request. Either remove the code_verifier parameter, or ensure code_challenge and code_challenge_method were included in the original authorization request.".to_string(),
                ));
            }
            // No PKCE validation needed
            tracing::debug!("No PKCE validation required");
            Ok(())
        }
        _ => {
            // Invalid state - either challenge or method is missing
            tracing::error!(
                "Invalid PKCE state: has_challenge={}, has_method={}",
                auth_code.code_challenge.is_some(),
                auth_code.code_challenge_method.is_some()
            );
            Err(OAuthError::ServerError(
                "Invalid PKCE state in authorization code".to_string(),
            ))
        }
    }
}

/// Generate standardized OAuth token response
///
/// Creates a JSON response containing the access token and metadata according to RFC 6749.
/// Includes refresh token if provided (only for confidential clients).
///
/// # Arguments
/// * `access_token` - The generated access token
/// * `expires_at` - When the token expires
/// * `scopes` - Granted scopes
/// * `refresh_token` - Optional refresh token (None for public clients)
///
/// # Returns
/// TokenResponse struct ready for JSON serialization
fn generate_access_token_response(
    access_token: &str,
    expires_at: OffsetDateTime,
    scopes: &[OAuthScope],
    refresh_token: Option<&str>,
) -> TokenResponse {
    TokenResponse {
        access_token: access_token.to_string(),
        token_type: "Bearer".to_string(),
        expires_in: utils::seconds_until_expiration(expires_at),
        scope: scopes_to_string(scopes),
        refresh_token: refresh_token.map(|s| s.to_string()),
    }
}

/// Convert OAuth error to JSON error response
///
/// Creates a standardized OAuth error response according to RFC 6749.
///
/// # Arguments
/// * `error` - The OAuth error to convert
///
/// # Returns
/// HTTP response with appropriate status code and JSON error body
fn oauth_error_response(error: OAuthError) -> Response {
    // Log the error with full context
    let error_code = error.error_code();
    let error_description = error.error_description();
    tracing::warn!(
        "OAuth error response: code={}, description={}, status={}",
        error_code,
        error_description,
        error.status_code()
    );

    let status_code = match error {
        OAuthError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
        OAuthError::InvalidClient => StatusCode::UNAUTHORIZED,
        OAuthError::InvalidGrant => StatusCode::BAD_REQUEST,
        OAuthError::UnsupportedGrantType => StatusCode::BAD_REQUEST,
        OAuthError::InvalidScope => StatusCode::BAD_REQUEST,
        OAuthError::ServerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };

    // Build error response with helpful context
    let mut error_response = serde_json::json!({
        "error": error.error_code(),
        "error_description": error.error_description()
    });

    // Add additional context for PKCE-related errors
    if error_description.contains("PKCE") || error_description.contains("code_verifier") {
        error_response["error_hint"] = serde_json::json!(
            "For PKCE flows, ensure you're using the same code_verifier that generated the code_challenge in your authorization request. The code_verifier must be 43-128 characters and contain only alphanumeric characters and '-', '.', '_', '~'."
        );
    }

    (status_code, Json(error_response)).into_response()
}

// =============================================================================
// Authorization Endpoint Helper Functions
// =============================================================================

/// Validate an OAuth authorization request
///
/// Performs comprehensive validation of the authorization request including:
/// - Basic parameter validation (response_type, client_id, redirect_uri)
/// - Client existence and active status
/// - Redirect URI validation against registered URIs
/// - PKCE parameter validation if present
async fn validate_authorization_request(
    pool: &PgPool,
    auth_request: &AuthorizationRequest,
    pkce_params: &AuthorizationRequestWithPKCE,
) -> Result<(), OAuthError> {
    // Basic parameter validation
    auth_request.validate()?;

    // Validate client exists and is active
    let client = db::get_oauth_client_by_client_id(pool, &auth_request.client_id)
        .await
        .map_err(|e| OAuthError::ServerError(format!("Database error: {e}")))?
        .ok_or(OAuthError::InvalidClient)?;

    // Validate redirect URI
    if !utils::validate_redirect_uri(&auth_request.redirect_uri) {
        return Err(OAuthError::InvalidRequest(format!(
            "Invalid redirect URI format: '{}'. The redirect_uri must be a valid URL with an allowed scheme (https for web, http for localhost, or custom protocols like myapp:// for mobile apps).",
            auth_request.redirect_uri
        )));
    }

    if !utils::is_redirect_uri_allowed(&auth_request.redirect_uri, &client.redirect_uris) {
        return Err(OAuthError::InvalidRequest(format!(
            "Redirect URI not registered for this client: '{}'. The redirect_uri must exactly match one of the redirect URIs registered for this client. Check the client configuration in the admin panel.",
            auth_request.redirect_uri
        )));
    }

    // Validate PKCE parameters if present
    if let Some(code_challenge) = &pkce_params.code_challenge {
        if code_challenge.is_empty() {
            return Err(OAuthError::InvalidRequest(
                "Empty code_challenge".to_string(),
            ));
        }

        if let Some(method) = &pkce_params.code_challenge_method {
            if method != "S256" && method != "plain" {
                return Err(OAuthError::InvalidRequest(
                    "Unsupported code_challenge_method".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Generate an OAuth authorization response
///
/// Creates the appropriate redirect response based on the authorization result.
/// Handles both success (with authorization code) and error cases.
fn generate_authorization_response(
    redirect_uri: &str,
    authorization_code: Option<&str>,
    error: Option<OAuthError>,
    state: Option<&str>,
) -> Result<Response, CustomError> {
    let mut url = Url::parse(redirect_uri).map_err(|_| {
        CustomError::System(format!(
            "Invalid redirect URI format when generating response: '{}'",
            redirect_uri
        ))
    })?;

    {
        let mut query_pairs = url.query_pairs_mut();

        if let Some(code) = authorization_code {
            query_pairs.append_pair("code", code);
        }

        if let Some(oauth_error) = error {
            query_pairs.append_pair("error", oauth_error.error_code());
            query_pairs.append_pair("error_description", &oauth_error.error_description());
        }

        if let Some(state_value) = state {
            query_pairs.append_pair("state", state_value);
        }
    }

    Ok(Redirect::to(url.as_str()).into_response())
}

/// Handle OAuth errors with appropriate responses
///
/// Determines whether to redirect to the client with an error or render
/// an error page based on the error type and available information.
async fn handle_oauth_error(
    error: OAuthError,
    client_name: Option<String>,
    redirect_uri: Option<&str>,
) -> Result<Response, CustomError> {
    // For certain errors, we can redirect back to the client
    if let (Some(redirect_uri), true) = (redirect_uri, can_redirect_error(&error)) {
        return generate_authorization_response(redirect_uri, None, Some(error), None);
    }

    // Otherwise, render error page
    let context = ErrorContext {
        error_code: error.error_code().to_string(),
        error_description: Some(error.error_description()),
        client_name,
        redirect_uri: redirect_uri.map(|s| s.to_string()),
        state: None,
        technical_details: None,
    };

    render_error_page(context)
}

/// Check if an error can be safely redirected to the client
///
/// Some errors (like invalid redirect URI) cannot be redirected since
/// we don't trust the redirect URI itself.
fn can_redirect_error(error: &OAuthError) -> bool {
    match error {
        OAuthError::InvalidRequest(_) => false, // Might include invalid redirect_uri
        OAuthError::InvalidClient => false,     // Client not trusted
        OAuthError::AccessDenied => true,       // User decision, safe to redirect
        OAuthError::InvalidScope => true,       // Client error, safe to redirect
        OAuthError::ServerError(_) => false,    // Server issue, show error page
        _ => false,                             // Conservative default
    }
}

/// Check user authentication status
///
/// Integrates with the existing axum-login authentication system to
/// determine if the current user is authenticated.
fn check_user_authentication(
    auth_session: &AuthSession,
) -> Option<&crate::auth_backend::SessionUser> {
    auth_session.user.as_ref()
}

/// Build return URL for login redirect
///
/// Constructs the URL that the user should be redirected to after login
/// to continue the OAuth authorization flow.
fn build_return_url(
    params: &AuthorizationRequestWithPKCE,
    api_server_url: &str,
) -> Result<String, CustomError> {
    let api_oauth_url = format!("{api_server_url}/oauth/authorize");
    let mut url = Url::parse(&api_oauth_url)
        .map_err(|_| CustomError::System("Failed to build return URL".to_string()))?;

    {
        let mut query_pairs = url.query_pairs_mut();
        query_pairs.append_pair("response_type", &params.response_type);
        query_pairs.append_pair("client_id", &params.client_id);
        query_pairs.append_pair("redirect_uri", &params.redirect_uri);

        if let Some(scope) = &params.scope {
            query_pairs.append_pair("scope", scope);
        }
        if let Some(state) = &params.state {
            query_pairs.append_pair("state", state);
        }
        if let Some(code_challenge) = &params.code_challenge {
            query_pairs.append_pair("code_challenge", code_challenge);
        }
        if let Some(code_challenge_method) = &params.code_challenge_method {
            query_pairs.append_pair("code_challenge_method", code_challenge_method);
        }
    }

    Ok(url.as_str().to_string())
}

// =============================================================================
// User Info Endpoint
// =============================================================================

/// GET /oauth/userinfo - OAuth user info endpoint
///
/// This endpoint returns user information based on the provided Bearer token
/// following RFC 6749 and RFC 6750 specifications. It validates the access token
/// and returns user information based on the granted scopes.
///
/// # Authentication
/// Requires a valid Bearer token in the Authorization header:
/// `Authorization: Bearer <access_token>`
///
/// # Scopes
/// - `profile`: Returns username and display_name
/// - `email`: Returns email address
///
/// # Returns
/// - JSON user information if token is valid and has appropriate scopes
/// - 401 Unauthorized if token is invalid, expired, or revoked
/// - 403 Forbidden if token lacks required scopes
/// - 500 Internal Server Error for database or system errors
///
/// # Error Responses
/// Follows RFC 6750 Bearer Token specification:
/// - Returns WWW-Authenticate header with error details for 401 responses
/// - Uses standard OAuth error codes (invalid_token, insufficient_scope)
pub async fn userinfo_get(
    State(state): State<OAuthState>,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    // Extract Bearer token from Authorization header
    let access_token = match extract_bearer_token(&headers) {
        Ok(token) => token,
        Err(oauth_error) => return Ok(bearer_error_response(oauth_error)),
    };

    // Validate JWT access token
    let jwt_claims = match state.jwt_manager.validate_access_token(&access_token) {
        Ok(claims) => claims,
        Err(oauth_error) => return Ok(bearer_error_response(oauth_error)),
    };

    // Parse granted scopes from JWT claims
    let granted_scopes = match jwt_claims.parse_scopes() {
        Ok(scopes) => scopes,
        Err(oauth_error) => return Ok(bearer_error_response(oauth_error)),
    };

    // Get user ID from JWT claims
    let user_id = match jwt_claims.user_id() {
        Ok(id) => id,
        Err(oauth_error) => return Ok(bearer_error_response(oauth_error)),
    };

    // Check if token is revoked in database (optional security check)
    if let Ok(Some(_)) = db::get_access_token_with_context(&state.pool, &access_token).await {
        // Token exists in database, check if it's revoked
        match db::get_and_validate_access_token(&state.pool, &access_token).await {
            Err(OAuthError::InvalidGrant) => {
                return Ok(bearer_error_response(OAuthError::InvalidToken));
            }
            Err(_) => {
                // Token might be revoked or expired in database
                return Ok(bearer_error_response(OAuthError::InvalidToken));
            }
            Ok(_) => {
                // Token is valid in database, continue
            }
        }
    }

    // Get user information from database
    let user = match user_db::get_user_by_id(&state.pool, user_id).await? {
        Some(user) => user,
        None => {
            // User no longer exists - token should be considered invalid
            return Ok(bearer_error_response(OAuthError::InvalidToken));
        }
    };

    // Build user info response based on granted scopes
    let user_info = build_user_info_response(&user, &granted_scopes);

    Ok(Json(user_info).into_response())
}

/// Extract Bearer token from Authorization header
///
/// Parses the Authorization header and extracts the Bearer token following RFC 6750.
///
/// # Arguments
/// * `headers` - HTTP headers from the request
///
/// # Returns
/// The extracted access token or an OAuth error
///
/// # Errors
/// - `InvalidRequest` if Authorization header is missing or malformed
/// - `InvalidToken` if the header doesn't contain a valid Bearer token
fn extract_bearer_token(headers: &HeaderMap) -> Result<String, OAuthError> {
    // Get Authorization header
    let auth_header = headers
        .get(AUTHORIZATION)
        .ok_or_else(|| OAuthError::InvalidRequest("Missing Authorization header".to_string()))?;

    // Convert to string
    let auth_str = auth_header.to_str().map_err(|_| {
        OAuthError::InvalidRequest("Invalid Authorization header encoding".to_string())
    })?;

    // Check for Bearer prefix
    if !auth_str.starts_with("Bearer ") {
        return Err(OAuthError::InvalidRequest(
            "Authorization header must use Bearer scheme".to_string(),
        ));
    }

    // Extract token (everything after "Bearer ")
    let token = auth_str[7..].trim(); // "Bearer " is 7 characters

    if token.is_empty() {
        return Err(OAuthError::InvalidToken);
    }

    Ok(token.to_string())
}

/// Build user info response based on granted scopes
///
/// Creates a UserInfoResponse containing only the user information that the
/// client is authorized to access based on the granted OAuth scopes.
///
/// # Arguments
/// * `user` - User information from the database
/// * `granted_scopes` - OAuth scopes granted to the access token
///
/// # Returns
/// UserInfoResponse with appropriate fields populated based on scopes
fn build_user_info_response(
    user: &user_db::User,
    granted_scopes: &[OAuthScope],
) -> UserInfoResponse {
    let mut response = UserInfoResponse {
        sub: user.id.to_string(),
        preferred_username: None,
        name: None,
        email: None,
        email_verified: None,
    };

    // Add profile information if profile scope is granted
    if granted_scopes.contains(&OAuthScope::ProfileRead) {
        response.preferred_username = Some(user.username.clone());
        response.name = Some(user.display_name.clone());
    }

    // Add email information if email scope is granted
    if granted_scopes.contains(&OAuthScope::ProfileEmail) {
        response.email = Some(user.email.clone());
        // Note: We don't have email verification status in the current User struct
        // This would need to be added if email verification is implemented
        response.email_verified = Some(false);
    }

    response
}

/// Create Bearer token error response following RFC 6750
///
/// Generates an HTTP response with appropriate status code and WWW-Authenticate
/// header for Bearer token authentication errors.
///
/// # Arguments
/// * `error` - The OAuth error to convert to a Bearer token error response
///
/// # Returns
/// HTTP response with Bearer token error format
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

    // Build WWW-Authenticate header value
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

    // Add WWW-Authenticate header
    response.headers_mut().insert(
        "WWW-Authenticate",
        www_authenticate
            .parse()
            .unwrap_or_else(|_| "Bearer".parse().unwrap()),
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_redirect_error() {
        assert!(!can_redirect_error(&OAuthError::InvalidClient));
        assert!(!can_redirect_error(&OAuthError::InvalidRequest(
            "test".to_string()
        )));
        assert!(can_redirect_error(&OAuthError::AccessDenied));
        assert!(can_redirect_error(&OAuthError::InvalidScope));
        assert!(!can_redirect_error(&OAuthError::ServerError(
            "test".to_string()
        )));
    }

    #[test]
    fn test_build_return_url() {
        let params = AuthorizationRequestWithPKCE {
            response_type: "code".to_string(),
            client_id: "test_client".to_string(),
            redirect_uri: "https://example.com/callback".to_string(),
            scope: Some("profile email".to_string()),
            state: Some("random_state".to_string()),
            code_challenge: Some("challenge".to_string()),
            code_challenge_method: Some("S256".to_string()),
        };

        let return_url = build_return_url(&params, "http://localhost:3001").unwrap();
        assert!(return_url.contains("response_type=code"));
        assert!(return_url.contains("client_id=test_client"));
        assert!(return_url.contains("scope=profile+email"));
        assert!(return_url.contains("state=random_state"));
        assert!(return_url.contains("code_challenge=challenge"));
        assert!(return_url.contains("code_challenge_method=S256"));
    }

    #[test]
    fn test_generate_authorization_response_success() {
        let response = generate_authorization_response(
            "https://example.com/callback",
            Some("test_code"),
            None,
            Some("test_state"),
        )
        .unwrap();

        // In a real test, we'd extract the redirect URL and verify parameters
        // For now, just verify it doesn't error
        assert!(response.status().is_redirection());
    }

    #[test]
    fn test_generate_authorization_response_error() {
        let response = generate_authorization_response(
            "https://example.com/callback",
            None,
            Some(OAuthError::AccessDenied),
            Some("test_state"),
        )
        .unwrap();

        assert!(response.status().is_redirection());
    }
}
