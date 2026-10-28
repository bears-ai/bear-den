//! OAuth browser authorization, code exchange, and active-token user information.

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form, Json,
};
use axum_login::tower_sessions::Session;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use url::Url;

use crate::{
    auth::authenticate_bearer_active_with_manager,
    oauth::{
        consent, db,
        error::OAuthError,
        exchange,
        jwt::{create_jwt_manager, JwtManager},
        pkce, scopes_from_json, utils, AuthorizationRequest, OAuthClient, OAuthScope, TokenRequest,
        UserInfoResponse,
    },
    templates::{
        parse_scopes, render_authorize_page, render_error_page, AuthorizeContext, ErrorContext,
        UserContext,
    },
};
use den_http::{auth_backend::AuthSession, errors::CustomError, user::db as user_db};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationRequestWithPKCE {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
}

#[derive(Deserialize)]
pub struct AuthorizationForm {
    pub action: String,
    pub csrf_token: Option<String>,
    pub client_id: String,
    pub response_type: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
}

#[derive(Clone)]
pub struct OAuthState {
    pub pool: PgPool,
    pub web_server_url: String,
    pub api_server_url: String,
    pub jwt_manager: JwtManager,
}

impl OAuthState {
    pub fn new(pool: PgPool, web_server_url: String, api_server_url: String) -> Self {
        Self {
            pool,
            web_server_url,
            api_server_url,
            jwt_manager: create_jwt_manager(),
        }
    }
}

/// Browser authorization. Public clients must supply S256 PKCE, including when trusted.
pub async fn authorize_get(
    Query(params): Query<AuthorizationRequestWithPKCE>,
    State(state): State<OAuthState>,
    auth_session: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let (client, scopes) = match validate_authorization_request(&state.pool, &params).await {
        Ok(validated) => validated,
        Err(error) => return authorization_error(error),
    };
    let Some(user) = auth_session.user.as_ref() else {
        let return_url = build_return_url(&params, &state.api_server_url)?;
        return Ok(Redirect::to(&format!(
            "{}/login?next={}",
            state.web_server_url,
            urlencoding::encode(&return_url)
        ))
        .into_response());
    };
    if client.trusted {
        return authorize_code(&state.pool, &client, user.id, &scopes, &params).await;
    }
    let db_user = user_db::get_user_by_id(&state.pool, user.id)
        .await?
        .ok_or_else(|| CustomError::Authentication("OAuth user is unavailable".into()))?;
    let csrf_token = consent::issue(&session, user.id, &params).await?;
    render_authorize_page(AuthorizeContext {
        client_id: params.client_id,
        client_name: Some(client.name),
        client_description: None,
        response_type: params.response_type,
        redirect_uri: Some(params.redirect_uri),
        scope: params.scope.clone(),
        scopes: parse_scopes(params.scope.as_deref()),
        state: params.state,
        code_challenge: params.code_challenge,
        code_challenge_method: params.code_challenge_method,
        csrf_token: Some(csrf_token),
        user: UserContext {
            id: user.id,
            username: Some(user.username.clone()),
            display_name: Some(db_user.display_name),
            email: db_user.email,
        },
        error: None,
    })
}

/// Consent is bound to the human, login session, and exact request shown on GET.
pub async fn authorize_post(
    State(state): State<OAuthState>,
    auth_session: AuthSession,
    session: Session,
    Form(form): Form<AuthorizationForm>,
) -> Result<Response, CustomError> {
    let Some(user) = auth_session.user.as_ref() else {
        return authorization_error(OAuthError::AccessDenied);
    };
    let params = AuthorizationRequestWithPKCE {
        response_type: form.response_type,
        client_id: form.client_id,
        redirect_uri: form.redirect_uri,
        scope: form.scope,
        state: form.state,
        code_challenge: form.code_challenge,
        code_challenge_method: form.code_challenge_method,
    };
    // Never redirect to a submitted callback before both client and URI validation.
    let (client, scopes) = match validate_authorization_request(&state.pool, &params).await {
        Ok(validated) => validated,
        Err(error) => return authorization_error(error),
    };
    if let Err(error) =
        consent::consume(&session, user.id, form.csrf_token.as_deref(), &params).await
    {
        return authorization_error(error);
    }
    match form.action.as_str() {
        "deny" => generate_authorization_response(
            &params.redirect_uri,
            None,
            Some(OAuthError::AccessDenied),
            params.state.as_deref(),
        ),
        "allow" => authorize_code(&state.pool, &client, user.id, &scopes, &params).await,
        _ => authorization_error(OAuthError::InvalidRequest("Invalid consent action".into())),
    }
}

async fn authorize_code(
    pool: &PgPool,
    client: &OAuthClient,
    user_id: i32,
    scopes: &[OAuthScope],
    params: &AuthorizationRequestWithPKCE,
) -> Result<Response, CustomError> {
    let code = utils::generate_authorization_code();
    db::create_authorization_code(
        pool,
        &code,
        client.id,
        user_id,
        &params.redirect_uri,
        scopes,
        utils::authorization_code_expiration(),
        params.code_challenge.as_deref(),
        params.code_challenge_method.as_deref(),
    )
    .await
    .map_err(|_| CustomError::System("OAuth authorization storage is unavailable".into()))?;
    generate_authorization_response(
        &params.redirect_uri,
        Some(&code),
        None,
        params.state.as_deref(),
    )
}

/// Authorization-code exchange only. Native/public clients receive no refresh token.
pub async fn token_post(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Form(request): Form<TokenRequest>,
) -> Result<Response, CustomError> {
    let result = async {
        request.validate()?;
        let client = authenticate_client(&state.pool, &request, &headers).await?;
        exchange::exchange_code(&state.pool, &client, &request, &state.jwt_manager).await
    }
    .await;
    let mut response = match result {
        Ok(token) => Json(token).into_response(),
        Err(error) => oauth_error_response(error),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    Ok(response)
}

async fn authenticate_client(
    pool: &PgPool,
    request: &TokenRequest,
    _headers: &HeaderMap,
) -> Result<OAuthClient, OAuthError> {
    let client = db::get_oauth_client_by_client_id(pool, &request.client_id)
        .await
        .map_err(|_| OAuthError::ServerError("OAuth client storage is unavailable".into()))?
        .ok_or(OAuthError::InvalidClient)?;
    if client.public {
        if request.client_secret.is_some() || request.code_verifier.is_none() {
            return Err(OAuthError::InvalidClient);
        }
    } else {
        let valid = request
            .client_secret
            .as_deref()
            .zip(client.client_secret.as_deref())
            .is_some_and(|(secret, hash)| utils::verify_client_secret(secret, hash));
        if !valid {
            return Err(OAuthError::InvalidClient);
        }
    }
    Ok(client)
}

fn oauth_error_response(error: OAuthError) -> Response {
    let status = error.status_code();
    (
        status,
        Json(serde_json::json!({
            "error": error.error_code(),
            "error_description": error.error_description(),
        })),
    )
        .into_response()
}

async fn validate_authorization_request(
    pool: &PgPool,
    params: &AuthorizationRequestWithPKCE,
) -> Result<(OAuthClient, Vec<OAuthScope>), OAuthError> {
    let request = AuthorizationRequest {
        response_type: params.response_type.clone(),
        client_id: params.client_id.clone(),
        redirect_uri: params.redirect_uri.clone(),
        scope: params.scope.clone(),
        _state: params.state.clone(),
    };
    request.validate()?;
    let client = db::get_oauth_client_by_client_id(pool, &params.client_id)
        .await
        .map_err(|_| OAuthError::ServerError("OAuth client storage is unavailable".into()))?
        .ok_or(OAuthError::InvalidClient)?;
    if !utils::validate_redirect_uri(&params.redirect_uri)
        || !utils::is_redirect_uri_allowed(&params.redirect_uri, &client.redirect_uris)
    {
        return Err(OAuthError::InvalidRequest(
            "Redirect URI must exactly match a registered allowed callback".into(),
        ));
    }
    let scopes = request.parse_scopes()?;
    if !utils::validate_scopes_for_client(&scopes, &scopes_from_json(&client.scopes)?) {
        return Err(OAuthError::InvalidScope);
    }
    pkce::validate_challenge(
        client.public,
        params.code_challenge.as_deref(),
        params.code_challenge_method.as_deref(),
    )?;
    Ok((client, scopes))
}

fn generate_authorization_response(
    redirect_uri: &str,
    authorization_code: Option<&str>,
    error: Option<OAuthError>,
    state: Option<&str>,
) -> Result<Response, CustomError> {
    let mut url = Url::parse(redirect_uri)
        .map_err(|_| CustomError::System("Invalid OAuth callback".into()))?;
    {
        let mut pairs = url.query_pairs_mut();
        if let Some(code) = authorization_code {
            pairs.append_pair("code", code);
        }
        if let Some(error) = error {
            pairs.append_pair("error", error.error_code());
            pairs.append_pair("error_description", &error.error_description());
        }
        if let Some(state) = state {
            pairs.append_pair("state", state);
        }
    }
    Ok(Redirect::to(url.as_str()).into_response())
}

fn authorization_error(error: OAuthError) -> Result<Response, CustomError> {
    let status = error.status_code();
    let mut response = render_error_page(ErrorContext {
        error_code: error.error_code().into(),
        error_description: Some(error.error_description()),
        client_name: None,
        redirect_uri: None,
        state: None,
        technical_details: None,
    })?;
    *response.status_mut() = status;
    Ok(response)
}

fn build_return_url(
    params: &AuthorizationRequestWithPKCE,
    api_server_url: &str,
) -> Result<String, CustomError> {
    let mut url = Url::parse(&format!("{api_server_url}/oauth/authorize"))
        .map_err(|_| CustomError::System("Failed to build OAuth return URL".into()))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("response_type", &params.response_type);
        pairs.append_pair("client_id", &params.client_id);
        pairs.append_pair("redirect_uri", &params.redirect_uri);
        for (name, value) in [
            ("scope", &params.scope),
            ("state", &params.state),
            ("code_challenge", &params.code_challenge),
            ("code_challenge_method", &params.code_challenge_method),
        ] {
            if let Some(value) = value {
                pairs.append_pair(name, value);
            }
        }
    }
    Ok(url.into())
}

pub async fn userinfo_get(
    State(state): State<OAuthState>,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let principal =
        match authenticate_bearer_active_with_manager(&headers, &state.pool, &state.jwt_manager)
            .await
        {
            Ok(principal) => principal,
            Err(error) => {
                let oauth = if error.status == StatusCode::SERVICE_UNAVAILABLE {
                    OAuthError::TemporarilyUnavailable
                } else {
                    OAuthError::InvalidToken
                };
                return Ok(bearer_error_response(oauth));
            }
        };
    let user = match user_db::get_user_by_id(&state.pool, principal.user_id).await? {
        Some(user) => user,
        None => return Ok(bearer_error_response(OAuthError::InvalidToken)),
    };
    let mut response = Json(build_user_info_response(&user, &principal.scopes)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn build_user_info_response(user: &user_db::User, scopes: &[OAuthScope]) -> UserInfoResponse {
    UserInfoResponse {
        sub: user.id.to_string(),
        preferred_username: scopes
            .contains(&OAuthScope::ProfileRead)
            .then(|| user.username.clone()),
        name: scopes
            .contains(&OAuthScope::ProfileRead)
            .then(|| user.display_name.clone()),
        email: scopes
            .contains(&OAuthScope::ProfileEmail)
            .then(|| user.email.clone()),
        email_verified: scopes.contains(&OAuthScope::ProfileEmail).then_some(false),
    }
}

fn bearer_error_response(error: OAuthError) -> Response {
    let mut response = oauth_error_response(error);
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
#[path = "tests/endpoints.rs"]
mod tests;
