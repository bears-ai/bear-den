//! Trusted Rust-only loopback seam for downstream integration tests.
//! Neither this type nor its endpoint/resolver can be deserialized from model input.

use crate::{ExternalCredentialResolver, RepositoryAuthorizer};
use den_core::tools::repository::{RepositoryError, RepositoryHeadResult, RepositorySurfaceId};
use std::{net::SocketAddr, sync::Arc};

pub struct LoopbackRepository {
    address: SocketAddr,
    resolver: Arc<dyn ExternalCredentialResolver>,
}

impl LoopbackRepository {
    pub fn new(
        address: SocketAddr,
        resolver: Arc<dyn ExternalCredentialResolver>,
    ) -> Result<Self, RepositoryError> {
        if !address.ip().is_loopback() {
            return Err(RepositoryError::DestinationDenied);
        }
        Ok(Self { address, resolver })
    }

    pub async fn head(
        &self,
        authority: &dyn RepositoryAuthorizer,
        surface: RepositorySurfaceId,
    ) -> Result<RepositoryHeadResult, RepositoryError> {
        crate::execute(
            authority,
            self.resolver.as_ref(),
            surface,
            crate::http::Transport::Loopback(self.address),
        )
        .await
    }
}
