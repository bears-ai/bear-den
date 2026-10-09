//! Owner-bound external references. No decryption or credential values here.

use den_core::{
    ids::UserId,
    tools::repository::{RepositoryError, RepositorySurfaceId},
    DenError,
};
use den_repository::{CredentialRequest, ExternalReference, GithubRepository};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::ConnectionId;

pub(super) async fn create(
    pool: &PgPool,
    owner: UserId,
    name: &str,
    reference: &ExternalReference,
) -> Result<ConnectionId, DenError> {
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO provider_connections (id, owner_user_id, name, provider, external_backend_binding_id, external_secret_id, external_secret_version)
         VALUES ($1, $2, $3, 'github_external', $4, $5, $6)",
        id, owner.get(), name, reference.backend_binding_id(), reference.secret_id(), reference.version(),
    ).execute(pool).await?;
    Ok(ConnectionId(id))
}

pub struct BoundRepository {
    pub credential: CredentialRequest,
    pub allowed_outbound_hosts: den_sandbox::protocol::AllowedOutboundHosts,
}

pub async fn for_owner(
    pool: &PgPool,
    owner: UserId,
    surface: RepositorySurfaceId,
) -> Result<BoundRepository, RepositoryError> {
    for_owner_on(pool, owner, surface).await
}

pub(crate) async fn for_owner_on<'e>(
    executor: impl sqlx::Executor<'e, Database = Postgres>,
    owner: UserId,
    surface: RepositorySurfaceId,
) -> Result<BoundRepository, RepositoryError> {
    let row = sqlx::query!(
        "SELECT c.id, c.owner_user_id, c.revision, c.external_backend_binding_id,
                c.external_secret_id, c.external_secret_version, g.upstream_url, g.default_ref, g.allowed_outbound_hosts
         FROM git_work_surface_details g JOIN provider_connections c ON c.id = g.connection_id
         JOIN work_surfaces s ON s.id = g.id AND s.kind = 'git_workspace'
         WHERE g.id = $1 AND c.owner_user_id = $2 AND c.provider = 'github_external'
           AND c.revoked_at IS NULL AND c.secret_ciphertext IS NULL AND c.github_app_installation_id IS NULL
           AND EXISTS (SELECT 1 FROM work_surface_managers m WHERE m.surface_id = g.id AND m.user_id = c.owner_user_id)",
        surface.0, owner.get(),
    ).fetch_optional(executor).await.map_err(|_| RepositoryError::PolicyUnavailable)?
        .ok_or(RepositoryError::ConnectionUnavailable)?;
    let reference = ExternalReference::new(
        row.external_backend_binding_id
            .ok_or(RepositoryError::CredentialScopeMismatch)?,
        row.external_secret_id
            .ok_or(RepositoryError::CredentialScopeMismatch)?,
        row.external_secret_version
            .ok_or(RepositoryError::CredentialScopeMismatch)?,
    )?;
    Ok(BoundRepository {
        credential: CredentialRequest {
            connection_id: row.id,
            owner: UserId::new(row.owner_user_id),
            connection_revision: row.revision,
            reference,
            repository: GithubRepository::parse(&row.upstream_url, &row.default_ref)?,
        },
        allowed_outbound_hosts: den_sandbox::protocol::AllowedOutboundHosts::new(
            row.allowed_outbound_hosts,
        )
        .map_err(|_| RepositoryError::DestinationDenied)?,
    })
}

/// Repository attachment cannot replace authorization beneath an existing queued/live/resumable Work run.
pub(super) async fn require_idle_attachment(
    tx: &mut Transaction<'_, Postgres>,
    surface: Uuid,
) -> Result<(), DenError> {
    let busy = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM job_work_surface_assignments a JOIN bear_work_runs r ON r.job_id = a.job_id
         WHERE a.work_surface_id = $1 AND r.state IN ('queued', 'claimed', 'provisioning', 'running', 'reporting', 'paused')) AS \"busy!\"",
        surface,
    ).fetch_one(&mut **tx).await?;
    if busy {
        return Err(DenError::ValidationError("Stop queued, live or paused Work using this repository before replacing or detaching its Connection; existing distributed credential copies are not erased by this change.".into()));
    }
    Ok(())
}
