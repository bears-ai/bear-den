//! Expiring, actor- and request-bound browser consent stored in the login session.

use axum_login::tower_sessions::Session;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{endpoints::AuthorizationRequestWithPKCE, error::OAuthError};
use crate::templates::generate_csrf_token;

const SESSION_KEY: &str = "oauth.pending_consents";
const MAX_PENDING: usize = 8;
const LIFETIME_SECONDS: i64 = 600;

#[derive(Clone, Serialize, Deserialize)]
struct PendingConsent {
    token: String,
    user_id: i32,
    issued_at: i64,
    request: AuthorizationRequestWithPKCE,
}

impl PendingConsent {
    fn current(&self, now: i64) -> bool {
        now >= self.issued_at && now - self.issued_at < LIFETIME_SECONDS
    }
}

fn session_error(_: impl std::fmt::Display) -> OAuthError {
    OAuthError::ServerError("OAuth consent session is unavailable".into())
}

pub(super) async fn issue(
    session: &Session,
    user_id: i32,
    request: &AuthorizationRequestWithPKCE,
) -> Result<String, OAuthError> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut pending: Vec<PendingConsent> = session
        .get(SESSION_KEY)
        .await
        .map_err(session_error)?
        .unwrap_or_default();
    pending.retain(|entry| entry.current(now));
    if pending.len() >= MAX_PENDING {
        pending.drain(..=pending.len() - MAX_PENDING);
    }
    let token = generate_csrf_token();
    pending.push(PendingConsent {
        token: token.clone(),
        user_id,
        issued_at: now,
        request: request.clone(),
    });
    session
        .insert(SESSION_KEY, pending)
        .await
        .map_err(session_error)?;
    session.save().await.map_err(session_error)?;
    Ok(token)
}

pub(super) async fn consume(
    session: &Session,
    user_id: i32,
    token: Option<&str>,
    request: &AuthorizationRequestWithPKCE,
) -> Result<(), OAuthError> {
    let denied =
        || OAuthError::InvalidRequest("Consent expired or changed; restart authorization".into());
    let token = token.ok_or_else(denied)?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut pending: Vec<PendingConsent> = session
        .get(SESSION_KEY)
        .await
        .map_err(session_error)?
        .unwrap_or_default();
    let index = pending
        .iter()
        .position(|entry| {
            entry.current(now)
                && entry.user_id == user_id
                && &entry.request == request
                && same_token(&entry.token, token)
        })
        .ok_or_else(denied)?;
    pending.remove(index);
    session
        .insert(SESSION_KEY, pending)
        .await
        .map_err(session_error)?;
    session.save().await.map_err(session_error)?;
    Ok(())
}

fn same_token(expected: &str, supplied: &str) -> bool {
    expected.len() == supplied.len()
        && expected
            .bytes()
            .zip(supplied.bytes())
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

#[cfg(test)]
#[path = "tests/consent.rs"]
mod tests;
