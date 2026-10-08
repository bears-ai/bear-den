//! Typed display values for OAuth inspection; templates do not call Rust methods
//! on serialized records or compare serialized timestamps.

use den_oauth::oauth::{AccessTokenWithContext, OAuthClient};
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) enum TokenState {
    Active,
    Expired,
    Revoked,
}

#[derive(Debug, Serialize)]
pub(super) struct TokenView {
    #[serde(flatten)]
    record: AccessTokenWithContext,
    state: TokenState,
    can_revoke: bool,
    scope_labels: Vec<String>,
    scope_error: Option<String>,
    expires_label: String,
    created_label: String,
}

impl TokenView {
    fn at(record: AccessTokenWithContext, now: OffsetDateTime) -> Self {
        let state = if record.revoked {
            TokenState::Revoked
        } else if record.expires_at <= now {
            TokenState::Expired
        } else {
            TokenState::Active
        };
        let can_revoke = matches!(state, TokenState::Active);
        let (scope_labels, scope_error) = match record.parse_scopes() {
            Ok(scopes) => (
                scopes
                    .iter()
                    .map(|scope| scope.as_str().to_string())
                    .collect(),
                None,
            ),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        Self {
            expires_label: record.expires_at.to_string(),
            created_label: record.token_created_at.to_string(),
            record,
            state,
            can_revoke,
            scope_labels,
            scope_error,
        }
    }
}

impl From<AccessTokenWithContext> for TokenView {
    fn from(record: AccessTokenWithContext) -> Self {
        Self::at(record, OffsetDateTime::now_utc())
    }
}

#[derive(Serialize)]
pub(super) struct ClientOption {
    id: i32,
    client_id: String,
    name: String,
    active: bool,
    trusted: bool,
    created_label: String,
}

impl From<OAuthClient> for ClientOption {
    fn from(client: OAuthClient) -> Self {
        Self {
            id: client.id,
            client_id: client.client_id,
            name: client.name,
            active: client.active,
            trusted: client.trusted,
            created_label: client.created_at.to_string(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct TokenUserOption {
    id: i32,
    username: String,
    display_name: String,
    email: String,
}

impl From<(i32, String, String, String)> for TokenUserOption {
    fn from((id, username, display_name, email): (i32, String, String, String)) -> Self {
        Self {
            id,
            username,
            display_name,
            email,
        }
    }
}

#[cfg(test)]
#[path = "oauth_token_tests.rs"]
mod tests;
