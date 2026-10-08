//! One-use, operator-bound feedback. Credentials never travel in redirect URLs.

use axum::{
    extract::Request,
    http::{header, HeaderValue},
    middleware::Next,
    response::Response,
};
use axum_login::tower_sessions::Session;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{auth_backend::AuthSession, errors::CustomError};

const FEEDBACK_LIFETIME_SECONDS: i64 = 300;
const TOKEN_FEEDBACK_KEY: &str = "admin.oauth.issued_token";

#[derive(Serialize, Deserialize)]
struct Feedback<T> {
    operator_id: i32,
    expires_at: i64,
    value: T,
}

impl<T> Feedback<T> {
    fn consume_for(self, operator_id: i32, now: i64) -> Option<T> {
        (self.operator_id == operator_id && self.expires_at > now).then_some(self.value)
    }
}

#[derive(Serialize, Deserialize)]
pub(super) enum ClientFeedback {
    Created { secret: Option<String> },
    Regenerated { secret: String },
}

fn operator_id(auth: &AuthSession) -> Result<i32, CustomError> {
    auth.user
        .as_ref()
        .filter(|user| user.is_admin)
        .map(|user| user.id)
        .ok_or_else(|| CustomError::Authorization("Den administrator access required".into()))
}

async fn put<T: Serialize>(
    session: &Session,
    auth: &AuthSession,
    key: &str,
    value: T,
) -> Result<(), CustomError> {
    session
        .insert(
            key,
            Feedback {
                operator_id: operator_id(auth)?,
                expires_at: OffsetDateTime::now_utc().unix_timestamp() + FEEDBACK_LIFETIME_SECONDS,
                value,
            },
        )
        .await
        .map_err(|_| {
            CustomError::System(
                "Could not save one-time OAuth feedback. Inspect the record before retrying."
                    .into(),
            )
        })?;
    session.save().await.map_err(|_| {
        CustomError::System(
            "Could not persist one-time OAuth feedback. Inspect the record before retrying.".into(),
        )
    })
}

async fn take<T: DeserializeOwned>(
    session: &Session,
    auth: &AuthSession,
    key: &str,
) -> Result<Option<T>, CustomError> {
    let operator_id = operator_id(auth)?;
    let feedback = session
        .remove::<Feedback<T>>(key)
        .await
        .map_err(|_| CustomError::System("Could not read one-time OAuth feedback.".into()))?;
    // Persist consumption before rendering a credential, including on render failure.
    session
        .save()
        .await
        .map_err(|_| CustomError::System("Could not consume one-time OAuth feedback.".into()))?;
    Ok(feedback.and_then(|feedback| {
        feedback.consume_for(operator_id, OffsetDateTime::now_utc().unix_timestamp())
    }))
}

pub(super) async fn put_client(
    session: &Session,
    auth: &AuthSession,
    client_id: i32,
    value: ClientFeedback,
) -> Result<(), CustomError> {
    put(
        session,
        auth,
        &format!("admin.oauth.client.{client_id}"),
        value,
    )
    .await
}

pub(super) async fn take_client(
    session: &Session,
    auth: &AuthSession,
    client_id: i32,
) -> Result<Option<ClientFeedback>, CustomError> {
    take(session, auth, &format!("admin.oauth.client.{client_id}")).await
}

pub(super) async fn put_token(
    session: &Session,
    auth: &AuthSession,
    token_id: i32,
) -> Result<(), CustomError> {
    put(session, auth, TOKEN_FEEDBACK_KEY, token_id).await
}

pub(super) async fn take_token(
    session: &Session,
    auth: &AuthSession,
) -> Result<Option<i32>, CustomError> {
    take(session, auth, TOKEN_FEEDBACK_KEY).await
}

#[cfg(test)]
#[path = "oauth_feedback_tests.rs"]
mod tests;

pub(super) async fn protect(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}
