//! Secret-free contract for a bounded Den-hosted repository operation.

use super::context::DenToolInvocationContext;
use crate::{Governance, TurnExecutionOrigin};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepositorySurfaceId(pub Uuid);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryHeadArguments {
    pub work_surface_id: RepositorySurfaceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CommitSha(String);
impl CommitSha {
    pub fn parse(raw: &str) -> Result<Self, RepositoryError> {
        if raw.len() != 40
            || !raw
                .bytes()
                .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
            || raw.bytes().all(|ch| ch == b'0')
        {
            return Err(RepositoryError::InvalidProviderResponse);
        }
        Ok(Self(raw.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Serialize)]
pub struct RepositoryHeadResult {
    pub work_surface_id: RepositorySurfaceId,
    pub commit_sha: CommitSha,
}

/// Closed, content-free failures. No provider, backend or parser error is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryError {
    InvalidArguments,
    NotAuthorized,
    ResourceChanged,
    ConnectionUnavailable,
    CredentialUnavailable,
    CredentialRevoked,
    CredentialScopeMismatch,
    DestinationDenied,
    ProviderAuthenticationFailed,
    ProviderUnavailable,
    RateLimited,
    ResponseTooLarge,
    InvalidProviderResponse,
    Timeout,
    PolicyUnavailable,
}
impl RepositoryError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::NotAuthorized => "not_authorized",
            Self::ResourceChanged => "resource_changed",
            Self::ConnectionUnavailable => "connection_unavailable",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::CredentialRevoked => "credential_revoked",
            Self::CredentialScopeMismatch => "credential_scope_mismatch",
            Self::DestinationDenied => "destination_denied",
            Self::ProviderAuthenticationFailed => "provider_authentication_failed",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::RateLimited => "rate_limited",
            Self::ResponseTooLarge => "response_too_large",
            Self::InvalidProviderResponse => "invalid_provider_response",
            Self::Timeout => "timeout",
            Self::PolicyUnavailable => "policy_unavailable",
        }
    }
}
impl std::fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for RepositoryError {}

pub fn error_payload(code: RepositoryError) -> serde_json::Value {
    serde_json::json!({"error": {"code": code}})
}

pub trait RepositoryOps: Send + Sync {
    fn repository_head(
        &self,
        context: &DenToolInvocationContext,
        origin: TurnExecutionOrigin,
        governance: Governance,
        surface: RepositorySurfaceId,
    ) -> impl std::future::Future<Output = Result<RepositoryHeadResult, RepositoryError>> + Send;
}

pub async fn invoke(
    ops: &impl RepositoryOps,
    context: &DenToolInvocationContext,
    origin: TurnExecutionOrigin,
    governance: Governance,
    arguments: serde_json::Value,
) -> serde_json::Value {
    if origin.require_ordinary_session().is_err() {
        return error_payload(RepositoryError::NotAuthorized);
    }
    let result = match serde_json::from_value::<RepositoryHeadArguments>(arguments) {
        Ok(args) => {
            ops.repository_head(context, origin, governance, args.work_surface_id)
                .await
        }
        Err(_) => Err(RepositoryError::InvalidArguments),
    };
    match result {
        Ok(result) => serde_json::json!(result),
        Err(code) => error_payload(code),
    }
}

#[cfg(test)]
mod tests;
