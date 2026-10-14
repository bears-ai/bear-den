use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use den_cabinet::ActorScope;
use den_core::ids::UserId;
use den_oauth::oauth::{
    db,
    jwt::{JwtClaims, JwtManager},
    utils::scopes_from_json,
    AccessTokenWithContext, OAuthClient, OAuthScope,
};
use sqlx::PgPool;
use time::OffsetDateTime;

// Deliberately not Debug: a principal is safe to use, not a token-log container.
pub(super) struct Principal {
    pub scope: ActorScope,
    pub client_id: String,
    pub token_id: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthError {
    InvalidToken,
    InsufficientScope,
    UnsafeTransport,
    Unavailable,
    InvalidHeaders,
    NotAcceptable,
    UnsupportedMediaType,
}
impl AuthError {
    pub fn response(self) -> Response {
        let (status, code) = match self {
            Self::InvalidToken => (StatusCode::UNAUTHORIZED, "invalid_token"),
            Self::InsufficientScope => (StatusCode::FORBIDDEN, "insufficient_scope"),
            Self::UnsafeTransport => (StatusCode::FORBIDDEN, "unsafe_transport"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable"),
            Self::InvalidHeaders => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::NotAcceptable => (StatusCode::NOT_ACCEPTABLE, "not_acceptable"),
            Self::UnsupportedMediaType => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type")
            }
        };
        let mut response = (status, Json(serde_json::json!({"error": code}))).into_response();
        if matches!(self, Self::InvalidToken | Self::InsufficientScope) {
            let challenge = if self == Self::InsufficientScope {
                "Bearer error=\"insufficient_scope\", scope=\"data:read\""
            } else {
                "Bearer error=\"invalid_token\""
            };
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(challenge),
            );
        }
        response
    }
}

pub(super) fn protect_transport(uri: &Uri, headers: &HeaderMap) -> Result<(), AuthError> {
    // No browser origins have been provisioned for this native-client release.
    if uri.query().is_some() || headers.contains_key(header::ORIGIN) {
        return Err(AuthError::UnsafeTransport);
    }
    if headers.contains_key("mcp-session-id")
        || headers.get_all(header::AUTHORIZATION).iter().count() > 1
    {
        return Err(AuthError::InvalidHeaders);
    }
    Ok(())
}

pub(super) fn bearer_token(headers: &HeaderMap) -> Result<&str, AuthError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::InvalidToken)?;
    let (scheme, token) = value.split_once(' ').ok_or(AuthError::InvalidToken)?;
    let token = token.trim_matches(' ');
    // HTTP auth schemes are case-insensitive; the credential itself is not.
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(AuthError::InvalidToken);
    }
    Ok(token)
}

pub(super) async fn authenticate(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<Principal, AuthError> {
    let token = bearer_token(headers)?;
    let secret = std::env::var("JWT_SECRET")
        .ok()
        .filter(|secret| !secret.trim().is_empty())
        .ok_or(AuthError::Unavailable)?;
    authenticate_with_manager(pool, token, &JwtManager::new(secret.as_bytes())).await
}

pub(super) async fn authenticate_with_manager(
    pool: &PgPool,
    token: &str,
    manager: &JwtManager,
) -> Result<Principal, AuthError> {
    let claims = manager
        .validate_access_token(token)
        .map_err(|_| AuthError::InvalidToken)?;
    let context = db::get_access_token_with_context(pool, token)
        .await
        .map_err(|_| AuthError::Unavailable)?
        .ok_or(AuthError::InvalidToken)?;
    let client = db::get_oauth_client_by_id(pool, context.client_id)
        .await
        .map_err(|_| AuthError::Unavailable)?
        .ok_or(AuthError::InvalidToken)?;
    // This shared lookup uses SQLx's existing static query metadata and does not
    // initialize den-api, DenState, a Bear run, or an ambient browser session.
    let user = den_http::user::db::settings_by_id(pool, context.user_id)
        .await
        .map_err(|_| AuthError::Unavailable)?
        .ok_or(AuthError::InvalidToken)?;
    validate_current(
        &claims,
        &context,
        &client,
        user.id,
        OffsetDateTime::now_utc(),
    )
}

pub(super) fn validate_current(
    claims: &JwtClaims,
    token: &AccessTokenWithContext,
    client: &OAuthClient,
    live_user_id: i32,
    now: OffsetDateTime,
) -> Result<Principal, AuthError> {
    let subject = claims.user_id().map_err(|_| AuthError::InvalidToken)?;
    if claims.iss != "den-oauth"
        || claims.aud != "den-api"
        || claims.token_type != "access_token"
        || claims.exp <= now.unix_timestamp()
        || claims.nbf > now.unix_timestamp()
        || token.revoked
        || token.expires_at <= now
        || !client.active
        || subject <= 0
        || subject != token.user_id
        || subject != live_user_id
        || token.client_id != client.id
        || token.client_identifier != client.client_id
        || claims.client_id != client.client_id
    {
        return Err(AuthError::InvalidToken);
    }
    let jwt_scopes = claims
        .parse_scopes()
        .map_err(|_| AuthError::InsufficientScope)?;
    let token_scopes = token
        .parse_scopes()
        .map_err(|_| AuthError::InsufficientScope)?;
    let client_scopes =
        scopes_from_json(&client.scopes).map_err(|_| AuthError::InsufficientScope)?;
    if ![jwt_scopes, token_scopes, client_scopes]
        .iter()
        .all(|scopes| scopes.contains(&OAuthScope::DataRead))
    {
        return Err(AuthError::InsufficientScope);
    }
    Ok(Principal {
        scope: ActorScope::user(UserId(subject)),
        client_id: client.client_id.clone(),
        token_id: token.token_id,
    })
}
