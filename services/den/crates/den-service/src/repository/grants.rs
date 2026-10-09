//! Exact repository grant management on the canonical hat grant table.

use den_core::{
    ids::{BearId, HatId, UserId},
    tools::{constants::DEN_REPOSITORY_HEAD, repository::RepositorySurfaceId},
    DenError,
};
use den_repository::{AuthorizedHead, CredentialRequest};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{bears::hats::manage, connections::external, work_surfaces};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryTarget(String);
impl RepositoryTarget {
    pub(crate) fn new(surface: RepositorySurfaceId, request: &CredentialRequest) -> Self {
        Self(AuthorizedHead::target_key(surface, request))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Serialize)]
pub struct RepositoryChoice {
    pub surface_id: Uuid,
    pub name: String,
    pub owner: String,
    pub repository: String,
    pub branch: String,
    pub target_key: String,
}

#[derive(Serialize)]
pub struct RepositoryGrant {
    pub id: Uuid,
    pub target_key: String,
}

pub async fn choices(
    pool: &PgPool,
    bear: BearId,
    hat: HatId,
    actor: UserId,
) -> Result<Vec<RepositoryChoice>, DenError> {
    let allowed = manage::allowed_surfaces(pool, bear, hat).await?;
    let mut choices = Vec::new();
    for surface in work_surfaces::list_surfaces_for_bears(pool, &[bear.as_uuid()]).await? {
        if !allowed.contains(&surface.id) {
            continue;
        }
        let surface_id = RepositorySurfaceId(surface.id);
        match external::for_owner(pool, actor, surface_id).await {
            Ok(bound) => choices.push(RepositoryChoice {
                surface_id: surface.id,
                name: surface.name,
                owner: bound.credential.repository.owner().to_owned(),
                repository: bound.credential.repository.repository().to_owned(),
                branch: bound.credential.repository.branch().to_owned(),
                target_key: RepositoryTarget::new(surface_id, &bound.credential).0,
            }),
            Err(den_core::tools::repository::RepositoryError::PolicyUnavailable) => {
                return Err(DenError::System("repository policy unavailable".into()))
            }
            Err(_) => {}
        }
    }
    Ok(choices)
}

pub async fn list(
    pool: &PgPool,
    bear: BearId,
    hat: HatId,
) -> Result<Vec<RepositoryGrant>, DenError> {
    manage::get_hat(pool, bear, hat).await?;
    Ok(sqlx::query!(
        "SELECT id, target_value FROM bear_hat_access_grants WHERE bear_id = $1 AND hat_id = $2 AND kind = 'tool'
         AND action_key = $3 AND target_kind = 'repository' AND revoked_at IS NULL ORDER BY created_at, id",
        bear.as_uuid(), hat.as_uuid(), DEN_REPOSITORY_HEAD,
    ).fetch_all(pool).await?.into_iter().map(|row| RepositoryGrant { id: row.id, target_key: row.target_value }).collect())
}

pub async fn grant(
    pool: &PgPool,
    bear: BearId,
    hat: HatId,
    actor: UserId,
    surface: RepositorySurfaceId,
    expected: &str,
    consent: bool,
) -> Result<Uuid, DenError> {
    if !consent {
        return Err(DenError::ValidationError(
            "confirm the future owned-conversation and eligible-Job audience".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    // Identity/Bear deletion locks User -> Bear -> dependents. Take the FK
    // parent locks first so granting cannot invert that order at insertion.
    sqlx::query_scalar!(
        "SELECT id FROM users WHERE id = $1 FOR KEY SHARE",
        actor.get()
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DenError::Authorization("current actor required".into()))?;
    sqlx::query_scalar!(
        "SELECT id FROM bears WHERE id = $1 FOR KEY SHARE",
        bear.as_uuid()
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DenError::Authorization("current Bear required".into()))?;
    // Retirement upgrades its Bear lock after fencing membership. Do not wait
    // into that upgrade while holding KEY SHARE; serialize before dependents or retry.
    sqlx::query_scalar!(
        "SELECT id FROM bears WHERE id = $1 FOR NO KEY UPDATE NOWAIT",
        bear.as_uuid()
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(crate::artifacts::snapshot_retirement::locks::source_error)?;
    sqlx::query_scalar!(
        "SELECT g.id FROM git_work_surface_details g JOIN provider_connections c ON c.id = g.connection_id
         WHERE g.id = $1 AND c.owner_user_id = $2 AND c.revoked_at IS NULL FOR UPDATE OF g, c",
        surface.0, actor.get(),
    ).fetch_optional(&mut *tx).await?.ok_or_else(|| DenError::Authorization("owned current repository Connection required".into()))?;
    let bound = external::for_owner_on(&mut *tx, actor, surface)
        .await
        .map_err(|_| {
            DenError::Authorization(
                "an owned external-reference Connection and managed GitHub repository are required"
                    .into(),
            )
        })?;
    let target = RepositoryTarget::new(surface, &bound.credential);
    if target.as_str() != expected {
        return Err(DenError::Authorization(
            "repository or Connection changed; review its current scope".into(),
        ));
    }
    let authorized = sqlx::query_scalar!(
        "SELECT h.id FROM bear_hats h JOIN user_bear ub ON ub.bear_id = h.bear_id AND ub.user_id = $3
         JOIN bear_hat_work_surfaces hs ON hs.bear_id = h.bear_id AND hs.hat_id = h.id AND hs.surface_id = $4
         JOIN work_surface_bears sb ON sb.bear_id = h.bear_id AND sb.surface_id = hs.surface_id
         WHERE h.bear_id = $1 AND h.id = $2 AND lower(btrim(coalesce(ub.role, ''))) = 'admin'
         FOR UPDATE OF h, ub, hs, sb",
        bear.as_uuid(), hat.as_uuid(), actor.get(), surface.0,
    ).fetch_optional(&mut *tx).await?;
    if authorized.is_none() {
        return Err(DenError::Authorization(
            "current Bear admin and Bear/hat repository assignments required".into(),
        ));
    }
    let id = sqlx::query_scalar!(
        "INSERT INTO bear_hat_access_grants (bear_id, hat_id, kind, action_key, target_kind, target_value, created_by_user_id)
         VALUES ($1, $2, 'tool', $3, 'repository', $4, $5)
         ON CONFLICT (bear_id, hat_id, kind, action_key, target_kind, target_value) WHERE revoked_at IS NULL
         DO UPDATE SET target_value = EXCLUDED.target_value RETURNING id",
        bear.as_uuid(), hat.as_uuid(), DEN_REPOSITORY_HEAD, target.as_str(), actor.get(),
    ).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}
