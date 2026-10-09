//! References and leases are internal trusted-adapter values, never tool DTOs.

use async_trait::async_trait;
use den_core::{ids::UserId, tools::repository::RepositoryError};
use secrecy::SecretString;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::GithubRepository;

#[derive(Clone, PartialEq, Eq)]
pub struct ExternalReference {
    backend_binding_id: Uuid,
    secret_id: Uuid,
    version: i64,
}

impl ExternalReference {
    pub fn new(
        backend_binding_id: Uuid,
        secret_id: Uuid,
        version: i64,
    ) -> Result<Self, RepositoryError> {
        if backend_binding_id.is_nil() || secret_id.is_nil() || version <= 0 {
            return Err(RepositoryError::CredentialScopeMismatch);
        }
        Ok(Self {
            backend_binding_id,
            secret_id,
            version,
        })
    }
    pub fn backend_binding_id(&self) -> Uuid {
        self.backend_binding_id
    }
    pub fn secret_id(&self) -> Uuid {
        self.secret_id
    }
    pub fn version(&self) -> i64 {
        self.version
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialRequest {
    pub connection_id: Uuid,
    pub owner: UserId,
    pub connection_revision: i64,
    pub reference: ExternalReference,
    pub repository: GithubRepository,
}

/// A backend must attest the exact owner/Connection/version/read-only repository
/// scope independently of the supplied locator. No lease or plaintext cache is kept.
pub struct CredentialLease {
    request: CredentialRequest,
    pub(crate) token: SecretString,
    expires: Instant,
}

impl CredentialLease {
    pub fn new(
        request: CredentialRequest,
        token: SecretString,
        lifetime: Duration,
    ) -> Result<Self, RepositoryError> {
        if lifetime.is_zero() || lifetime > Duration::from_secs(60) {
            return Err(RepositoryError::CredentialScopeMismatch);
        }
        Ok(Self {
            request,
            token,
            expires: Instant::now() + lifetime,
        })
    }
    pub fn request(&self) -> &CredentialRequest {
        &self.request
    }
    pub fn check(&self, expected: &CredentialRequest) -> Result<(), RepositoryError> {
        if &self.request != expected {
            return Err(RepositoryError::CredentialScopeMismatch);
        }
        if Instant::now() >= self.expires {
            return Err(RepositoryError::CredentialRevoked);
        }
        Ok(())
    }
}

#[async_trait]
pub trait ExternalCredentialResolver: Send + Sync {
    async fn resolve(
        &self,
        request: &CredentialRequest,
    ) -> Result<CredentialLease, RepositoryError>;
    /// Fail closed on backend outage, revoked key/version or lost backend scope.
    async fn validate(&self, lease: &CredentialLease) -> Result<(), RepositoryError>;
}

pub struct Unavailable;

#[async_trait]
impl ExternalCredentialResolver for Unavailable {
    async fn resolve(&self, _: &CredentialRequest) -> Result<CredentialLease, RepositoryError> {
        Err(RepositoryError::CredentialUnavailable)
    }
    async fn validate(&self, _: &CredentialLease) -> Result<(), RepositoryError> {
        Err(RepositoryError::CredentialUnavailable)
    }
}
