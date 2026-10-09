//! Secret-free account metadata for already-authorized repository views.

use crate::errors::CustomError;
use den_core::{ids::UserId, DenError};
use den_service::connections::{self, ConnectionId, Provider};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountState {
    Available,
    Revoked,
    Unavailable,
    BackendUnconfigured,
}

#[derive(Debug, Serialize)]
pub(crate) struct LinkedAccount {
    pub surface_id: Uuid,
    pub id: ConnectionId,
    pub name: String,
    pub provider: Provider,
    pub provider_label: &'static str,
    pub state: AccountState,
    pub can_manage: bool,
    pub github_app_write_enabled: Option<bool>,
}

pub(crate) fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::GitHttps => "Git HTTPS token",
        Provider::GitSsh => "Git SSH key",
        Provider::GithubApp => "GitHub App installation",
        Provider::GithubExternal => "GitHub external credential reference (backend unconfigured)",
    }
}

/// Callers must scope ids to repositories the viewer can inspect/manage first.
pub(crate) async fn linked_accounts(
    pool: &sqlx::PgPool,
    viewer: UserId,
    ids: &[Uuid],
) -> Result<Vec<LinkedAccount>, CustomError> {
    let rows = sqlx::query!(
        r#"SELECT g.id AS surface_id, c.id, c.name, c.provider, c.owner_user_id,
        (c.revoked_at IS NOT NULL) AS "revoked!", c.github_app_write_enabled
        FROM git_work_surface_details g JOIN provider_connections c ON c.id = g.connection_id
        WHERE g.id = ANY($1) AND (
            EXISTS (SELECT 1 FROM users u WHERE u.id = $2 AND u.is_admin)
            OR EXISTS (SELECT 1 FROM work_surface_managers m WHERE m.surface_id = g.id AND m.user_id = $2)
        )"#,
        ids,
        viewer.get(),
    )
    .fetch_all(pool)
    .await
    .map_err(DenError::from)?;
    let mut accounts = Vec::new();
    for row in rows {
        let provider: Provider = serde_json::from_value(serde_json::json!(row.provider))
            .map_err(|_| CustomError::System("unknown repository account provider".into()))?;
        let state = if row.revoked {
            AccountState::Revoked
        } else if provider == Provider::GithubExternal {
            AccountState::BackendUnconfigured
        } else {
            match connections::require_live_for_surface(pool, row.surface_id).await {
                Ok(()) => AccountState::Available,
                Err(DenError::Authorization(_)) => AccountState::Unavailable,
                Err(error) => return Err(error.into()),
            }
        };
        accounts.push(LinkedAccount {
            surface_id: row.surface_id,
            id: ConnectionId(row.id),
            name: row.name,
            provider,
            provider_label: provider_label(provider),
            state,
            can_manage: row.owner_user_id == viewer.get(),
            github_app_write_enabled: (provider == Provider::GithubApp)
                .then_some(row.github_app_write_enabled),
        });
    }
    Ok(accounts)
}

/// A trusted Job projection may inspect this boolean without disclosing account metadata.
/// Token/key publication rights still require a provider check; only explicit App read-only
/// configuration is known to deny publication here.
pub(super) async fn publication_available(
    pool: &sqlx::PgPool,
    surface_id: Uuid,
) -> Result<bool, CustomError> {
    match connections::require_live_for_surface(pool, surface_id).await {
        Ok(()) => {}
        Err(DenError::Authorization(_)) => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    sqlx::query_scalar!(
        r#"SELECT CASE WHEN g.connection_id IS NOT NULL
            THEN c.provider <> 'github_app' OR c.github_app_write_enabled
            ELSE g.github_app_installation_id IS NULL OR g.github_app_write_enabled
        END AS "publication_available!"
        FROM git_work_surface_details g LEFT JOIN provider_connections c ON c.id = g.connection_id
        WHERE g.id = $1"#,
        surface_id,
    )
    .fetch_optional(pool)
    .await
    .map(|available| available.unwrap_or(false))
    .map_err(|error| DenError::from(error).into())
}
