use async_trait::async_trait;
use den_core::{
    ids::BearId,
    tools::{
        constants::DEN_REPOSITORY_HEAD,
        context::DenToolInvocationContext,
        descriptor::builtin_den_tool_descriptor_for_provider_name,
        repository::{RepositoryError, RepositorySurfaceId},
    },
    Governance, TurnExecutionOrigin,
};
use den_repository::{AuthorizedHead, RepositoryAuthorizer, GITHUB_API_HOST};
use sqlx::PgPool;

use super::{grants::RepositoryTarget, source};
use crate::{
    bears::hats::{
        access::{self, HatAccessGrant, HttpsHost},
        manage,
    },
    connections::external,
    work_surfaces,
};

pub struct RepositoryHeadPolicy<'a> {
    pub pool: &'a PgPool,
    pub context: &'a DenToolInvocationContext,
    pub origin: TurnExecutionOrigin,
    pub governance: Governance,
}

#[async_trait]
impl RepositoryAuthorizer for RepositoryHeadPolicy<'_> {
    async fn authorize(
        &self,
        surface: RepositorySurfaceId,
    ) -> Result<AuthorizedHead, RepositoryError> {
        let descriptor = builtin_den_tool_descriptor_for_provider_name(DEN_REPOSITORY_HEAD)
            .ok_or(RepositoryError::NotAuthorized)?;
        if !descriptor.allows_origin(self.origin) {
            return Err(RepositoryError::NotAuthorized);
        }
        let source = source::resolve(
            self.pool,
            self.context,
            self.origin,
            self.governance,
            surface,
        )
        .await?;
        let bear = BearId::new(self.context.bear_id);
        if !work_surfaces::bear_may_use_surface(self.pool, self.context.bear_id, surface.0)
            .await
            .map_err(|_| RepositoryError::PolicyUnavailable)?
            || !manage::allowed_surfaces(self.pool, bear, source.hat)
                .await
                .map_err(|_| RepositoryError::NotAuthorized)?
                .contains(&surface.0)
        {
            return Err(RepositoryError::NotAuthorized);
        }
        let bound = external::for_owner(self.pool, source.actor, surface).await?;
        if !bound
            .allowed_outbound_hosts
            .as_slice()
            .iter()
            .any(|host| host == GITHUB_API_HOST)
        {
            return Err(RepositoryError::DestinationDenied);
        }
        let host = HatAccessGrant::HttpsHost(
            HttpsHost::parse(GITHUB_API_HOST).map_err(|_| RepositoryError::DestinationDenied)?,
        );
        if !access::has_current(self.pool, bear, source.hat, &host)
            .await
            .map_err(|_| RepositoryError::PolicyUnavailable)?
        {
            return Err(RepositoryError::DestinationDenied);
        }
        let destination = bound.credential.repository.api_url().to_string();
        let blocked = sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM bear_web_sources WHERE bear_id = $1 AND policy = 'blocked'
             AND ((scope_kind = 'host' AND scope_value = $2) OR (scope_kind = 'url' AND scope_value = $3))) AS \"blocked!\"",
            self.context.bear_id, GITHUB_API_HOST, &destination,
        ).fetch_one(self.pool).await.map_err(|_| RepositoryError::PolicyUnavailable)?;
        if blocked {
            return Err(RepositoryError::DestinationDenied);
        }
        let target = RepositoryTarget::new(surface, &bound.credential);
        let grant_id = sqlx::query_scalar!(
            "SELECT id FROM bear_hat_access_grants WHERE bear_id = $1 AND hat_id = $2 AND kind = 'tool'
             AND action_key = $3 AND target_kind = 'repository' AND target_value = $4 AND revoked_at IS NULL",
            self.context.bear_id, source.hat.as_uuid(), descriptor.name, target.as_str(),
        ).fetch_optional(self.pool).await.map_err(|_| RepositoryError::PolicyUnavailable)?.ok_or(RepositoryError::NotAuthorized)?;
        tracing::info!(audit_event = "repository_head_authorized", bear_id = %self.context.bear_id,
            actor_id = source.actor.get(), source_id = %source.id, hat_id = %source.hat,
            surface_id = %surface.0, grant_id = %grant_id, connection_id = %bound.credential.connection_id,
            connection_revision = bound.credential.connection_revision, credential_version = bound.credential.reference.version(),
            destination = GITHUB_API_HOST, "repository effect authority revalidated");
        Ok(AuthorizedHead {
            surface,
            credential: bound.credential,
            source_id: source.id,
            hat_id: source.hat.as_uuid(),
            grant_id,
        })
    }
}
