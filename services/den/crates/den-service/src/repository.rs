//! Live canonical authorization for the single mediated repository operation.

pub mod grants;
mod policy;
mod source;

pub use den_repository::{ExternalCredentialResolver, ExternalReference};
pub use policy::RepositoryHeadPolicy;

use den_core::{
    tools::{
        context::DenToolInvocationContext,
        repository::{RepositoryError, RepositoryHeadResult, RepositorySurfaceId},
    },
    Governance, TurnExecutionOrigin,
};
use sqlx::PgPool;

/// No production backend has been selected; no environment/ciphertext fallback.
pub async fn head(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    origin: TurnExecutionOrigin,
    governance: Governance,
    surface: RepositorySurfaceId,
) -> Result<RepositoryHeadResult, RepositoryError> {
    head_with_resolver(
        pool,
        context,
        origin,
        governance,
        surface,
        &den_repository::Unavailable,
    )
    .await
}

pub async fn head_with_resolver(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    origin: TurnExecutionOrigin,
    governance: Governance,
    surface: RepositorySurfaceId,
    resolver: &dyn ExternalCredentialResolver,
) -> Result<RepositoryHeadResult, RepositoryError> {
    let start = std::time::Instant::now();
    let policy = RepositoryHeadPolicy {
        pool,
        context,
        origin,
        governance,
    };
    let result = den_repository::repository_head(&policy, resolver, surface).await;
    tracing::info!(audit_event = "repository_head", bear_id = %context.bear_id,
        work_surface_id = %surface.0, code = ?result.as_ref().err(), elapsed_ms = start.elapsed().as_millis(),
        "bounded repository operation completed");
    result
}

#[cfg(test)]
mod tests;
