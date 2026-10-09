//! One bounded HTTP operation. No subprocess, filesystem, ambient credential or KMS backend.

mod coordinates;
mod credentials;
mod http;
#[cfg(feature = "test-util")]
pub mod test_util;

pub use coordinates::{GithubRepository, GITHUB_API_HOST};
pub use credentials::{
    CredentialLease, CredentialRequest, ExternalCredentialResolver, ExternalReference, Unavailable,
};

use async_trait::async_trait;
use den_core::tools::repository::{RepositoryError, RepositoryHeadResult, RepositorySurfaceId};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Ephemeral comparison snapshot, not a reusable execution capability or policy cache.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizedHead {
    pub surface: RepositorySurfaceId,
    pub credential: CredentialRequest,
    pub source_id: Uuid,
    pub hat_id: Uuid,
    pub grant_id: Uuid,
}

impl AuthorizedHead {
    pub fn target_key(surface: RepositorySurfaceId, request: &CredentialRequest) -> String {
        let mut hash = Sha256::new();
        hash.update(b"repository-head-v1\0");
        hash.update(surface.0.as_bytes());
        hash.update(request.connection_id.as_bytes());
        hash.update(request.owner.get().to_le_bytes());
        hash.update(request.connection_revision.to_le_bytes());
        hash.update(request.reference.backend_binding_id().as_bytes());
        hash.update(request.reference.secret_id().as_bytes());
        hash.update(request.reference.version().to_le_bytes());
        for component in [
            request.repository.owner(),
            request.repository.repository(),
            request.repository.branch(),
        ] {
            hash.update(component.as_bytes());
            hash.update([0]);
        }
        format!("{}:{:x}", surface.0, hash.finalize())
    }
}

#[async_trait]
pub trait RepositoryAuthorizer: Send + Sync {
    async fn authorize(
        &self,
        surface: RepositorySurfaceId,
    ) -> Result<AuthorizedHead, RepositoryError>;
}

pub async fn repository_head(
    authority: &dyn RepositoryAuthorizer,
    resolver: &dyn ExternalCredentialResolver,
    surface: RepositorySurfaceId,
) -> Result<RepositoryHeadResult, RepositoryError> {
    execute(authority, resolver, surface, http::Transport::Public).await
}

async fn execute(
    authority: &dyn RepositoryAuthorizer,
    resolver: &dyn ExternalCredentialResolver,
    surface: RepositorySurfaceId,
    transport: http::Transport,
) -> Result<RepositoryHeadResult, RepositoryError> {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let authorized = authority.authorize(surface).await?;
        if authorized.surface != surface {
            return Err(RepositoryError::NotAuthorized);
        }
        let lease = resolver.resolve(&authorized.credential).await?;
        lease.check(&authorized.credential)?;
        resolver.validate(&lease).await?;
        if authority.authorize(surface).await? != authorized {
            return Err(RepositoryError::ResourceChanged);
        }
        let prepared = transport.prepare(&authorized.credential.repository).await?;
        resolver.validate(&lease).await?;
        if authority.authorize(surface).await? != authorized {
            return Err(RepositoryError::ResourceChanged);
        }
        lease.check(&authorized.credential)?;
        let response = prepared
            .send(&authorized.credential.repository, &lease)
            .await;
        resolver.validate(&lease).await?;
        lease.check(&authorized.credential)?;
        if authority.authorize(surface).await? != authorized {
            return Err(RepositoryError::ResourceChanged);
        }
        Ok(RepositoryHeadResult {
            work_surface_id: surface,
            commit_sha: response?,
        })
    })
    .await
    .map_err(|_| RepositoryError::Timeout)?
}

#[cfg(test)]
mod tests;
