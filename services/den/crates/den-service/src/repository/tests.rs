mod fixture;
mod grant_deletion_races;
mod migrations;
mod policy;
mod work;

use super::*;

use den_repository::{CredentialLease, CredentialRequest, ExternalCredentialResolver};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingUnavailable(AtomicUsize);
#[async_trait::async_trait]
impl ExternalCredentialResolver for CountingUnavailable {
    async fn resolve(&self, _: &CredentialRequest) -> Result<CredentialLease, RepositoryError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(RepositoryError::CredentialUnavailable)
    }
    async fn validate(&self, _: &CredentialLease) -> Result<(), RepositoryError> {
        unreachable!("no lease supplied")
    }
}

async fn assert_denied(
    pool: &sqlx::PgPool,
    context: &den_core::tools::context::DenToolInvocationContext,
    surface: den_core::tools::repository::RepositorySurfaceId,
    origin: den_core::TurnExecutionOrigin,
) {
    let backend = CountingUnavailable(AtomicUsize::new(0));
    assert!(head_with_resolver(
        pool,
        context,
        origin,
        den_core::Governance::Interactive,
        surface,
        &backend
    )
    .await
    .is_err());
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
}
