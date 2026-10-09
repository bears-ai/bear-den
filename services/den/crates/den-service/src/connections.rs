//! Owner-scoped, reusable repository credentials. Metadata never contains secret bytes.

use den_core::{ids::UserId, DenError};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub mod external;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    GitHttps,
    GitSsh,
    GithubApp,
    GithubExternal,
}
impl Provider {
    fn parse(value: &str) -> Result<Self, DenError> {
        match value {
            "git_https" => Ok(Self::GitHttps),
            "git_ssh" => Ok(Self::GitSsh),
            "github_app" => Ok(Self::GithubApp),
            "github_external" => Ok(Self::GithubExternal),
            _ => Err(DenError::ValidationError(
                "unknown connection provider".into(),
            )),
        }
    }
}

// Deliberately neither Debug nor Serialize: these values cross only the secret boundary.
pub enum Material {
    HttpsToken(String),
    SshKey(String),
    GithubApp { installation: i64, write: bool },
    ExternalReference(den_repository::ExternalReference),
}

#[derive(Debug, Clone, Serialize)]
pub struct Connection {
    pub id: ConnectionId,
    pub name: String,
    pub provider: Provider,
    pub revision: i64,
    pub revoked: bool,
    pub repository_count: i64,
}

pub async fn list(pool: &PgPool, owner: UserId) -> Result<Vec<Connection>, DenError> {
    let rows = sqlx::query!("SELECT c.id, c.name, c.provider, c.revision, (c.revoked_at IS NOT NULL) AS \"revoked!\", (SELECT count(*) FROM git_work_surface_details g WHERE g.connection_id = c.id) AS \"repository_count!\" FROM provider_connections c WHERE c.owner_user_id = $1 ORDER BY c.name, c.id", owner.get()).fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            Ok(Connection {
                id: ConnectionId(row.id),
                name: row.name,
                provider: Provider::parse(&row.provider)?,
                revision: row.revision,
                revoked: row.revoked,
                repository_count: row.repository_count,
            })
        })
        .collect()
}

pub async fn create(
    pool: &PgPool,
    owner: UserId,
    name: &str,
    material: Material,
    encryption_key: &str,
) -> Result<ConnectionId, DenError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(DenError::ValidationError(
            "connection name must be 1–120 characters".into(),
        ));
    }
    if let Material::ExternalReference(reference) = &material {
        return external::create(pool, owner, name, reference).await;
    }
    let (provider, ciphertext, installation, write) = match material {
        Material::HttpsToken(value) => (
            "git_https",
            Some(crate::secrets::encrypt_secret(&value, encryption_key)?),
            None,
            false,
        ),
        Material::SshKey(value) => (
            "git_ssh",
            Some(crate::secrets::encrypt_secret(&value, encryption_key)?),
            None,
            false,
        ),
        Material::GithubApp {
            installation,
            write,
        } if installation > 0 => ("github_app", None, Some(installation), write),
        Material::ExternalReference(_) => {
            unreachable!("external reference handled before legacy encryption")
        }
        Material::GithubApp { .. } => {
            return Err(DenError::ValidationError(
                "GitHub installation must be positive".into(),
            ))
        }
    };
    let id = sqlx::query_scalar!("INSERT INTO provider_connections (owner_user_id,name,provider,secret_ciphertext,github_app_installation_id,github_app_write_enabled) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id", owner.get(),name,provider,ciphertext,installation,write).fetch_one(pool).await?;
    Ok(ConnectionId(id))
}

pub async fn revoke(
    pool: &PgPool,
    owner: UserId,
    id: ConnectionId,
    revision: i64,
) -> Result<(), DenError> {
    let changed = sqlx::query!("UPDATE provider_connections SET revoked_at = now(), revision = revision + 1, updated_at = now() WHERE id = $1 AND owner_user_id = $2 AND revision = $3 AND revoked_at IS NULL", id.0,owner.get(),revision).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(DenError::NotFound(
            "connection missing, revoked or changed".into(),
        ));
    }
    Ok(())
}

fn validate_upstream(provider: Provider, upstream: &str) -> Result<(), DenError> {
    match provider {
        Provider::GithubExternal => {
            let url = reqwest::Url::parse(upstream).map_err(|_| {
                DenError::ValidationError("GitHub HTTPS repository required".into())
            })?;
            if url.host_str() != Some("github.com") {
                return Err(DenError::ValidationError(
                    "GitHub HTTPS repository required".into(),
                ));
            }
            den_repository::GithubRepository::parse(upstream, "main").map_err(|_| {
                DenError::ValidationError("canonical GitHub HTTPS repository required".into())
            })?;
        }
        Provider::GitHttps | Provider::GithubApp => {
            let url = reqwest::Url::parse(upstream)
                .map_err(|_| DenError::ValidationError("HTTPS repository URL required".into()))?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port_or_known_default() != Some(443)
                || (provider == Provider::GithubApp && url.host_str() != Some("github.com"))
            {
                return Err(DenError::ValidationError("connection requires a plain matching HTTPS upstream without embedded credentials".into()));
            }
        }
        Provider::GitSsh => {
            if let Ok(url) = reqwest::Url::parse(upstream) {
                if url.scheme() != "ssh" || url.password().is_some() || url.host_str().is_none() {
                    return Err(DenError::ValidationError(
                        "SSH upstream required without an embedded password".into(),
                    ));
                }
            } else if upstream.split_once(':').is_none()
                || !upstream.contains('@')
                || upstream.chars().any(char::is_whitespace)
            {
                return Err(DenError::ValidationError(
                    "SSH or SCP-style repository upstream required".into(),
                ));
            }
        }
    }
    Ok(())
}

pub async fn attach(
    pool: &PgPool,
    owner: UserId,
    id: ConnectionId,
    surface: Uuid,
) -> Result<(), DenError> {
    let mut tx = pool.begin().await?;
    let target = sqlx::query!("SELECT g.upstream_url,c.provider FROM git_work_surface_details g JOIN provider_connections c ON c.id=$2 WHERE g.id=$1 AND c.owner_user_id=$3 AND c.revoked_at IS NULL AND EXISTS(SELECT 1 FROM work_surface_managers m WHERE m.surface_id=g.id AND m.user_id=$3) FOR UPDATE OF g,c",surface,id.0,owner.get()).fetch_optional(&mut *tx).await?.ok_or_else(|| DenError::NotFound("connection or managed repository not found".into()))?;
    validate_upstream(Provider::parse(&target.provider)?, &target.upstream_url)?;
    external::require_idle_attachment(&mut tx, surface).await?;
    let changed = sqlx::query!("UPDATE git_work_surface_details g SET connection_id = c.id, credential_kind = NULL, credential_encrypted = NULL, github_app_installation_id = NULL, github_app_write_enabled = false FROM provider_connections c WHERE g.id = $1 AND c.id = $2 AND c.owner_user_id = $3 AND c.revoked_at IS NULL AND EXISTS (SELECT 1 FROM work_surface_managers m WHERE m.surface_id = g.id AND m.user_id = $3)", surface,id.0,owner.get()).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(DenError::NotFound(
            "connection or managed repository not found".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

pub async fn detach(pool: &PgPool, actor: UserId, surface: Uuid) -> Result<(), DenError> {
    let mut tx = pool.begin().await?;
    sqlx::query_scalar!("SELECT g.id FROM git_work_surface_details g WHERE g.id = $1 AND EXISTS (SELECT 1 FROM work_surface_managers m WHERE m.surface_id = g.id AND m.user_id = $2) FOR UPDATE", surface, actor.get())
        .fetch_optional(&mut *tx).await?.ok_or_else(|| DenError::NotFound("managed repository not found".into()))?;
    external::require_idle_attachment(&mut tx, surface).await?;
    let changed = sqlx::query!("UPDATE git_work_surface_details g SET connection_id = NULL WHERE g.id = $1 AND EXISTS (SELECT 1 FROM work_surface_managers m WHERE m.surface_id = g.id AND m.user_id = $2)",surface,actor.get()).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(DenError::NotFound("managed repository not found".into()));
    }
    tx.commit().await?;
    Ok(())
}

/// Resolved only by provider reconciliation; never returned to a web template.
pub(crate) struct ExecutionConnection {
    pub kind: Option<String>,
    pub ciphertext: Option<String>,
    pub installation: Option<i64>,
    pub write: bool,
}

pub(crate) enum ResolvedConnection {
    Legacy,
    Available(ExecutionConnection),
    Denied,
    ExternalReference,
}

pub(crate) async fn resolve(pool: &PgPool, surface: Uuid) -> Result<ResolvedConnection, DenError> {
    let row = sqlx::query!("SELECT g.connection_id, g.upstream_url, c.provider AS \"provider?\", c.secret_ciphertext, c.github_app_installation_id, c.github_app_write_enabled AS \"github_app_write_enabled?\", (c.revoked_at IS NOT NULL) AS revoked, EXISTS (SELECT 1 FROM work_surface_managers m WHERE m.surface_id = g.id AND m.user_id = c.owner_user_id) AS \"owner_authorized!\" FROM git_work_surface_details g LEFT JOIN provider_connections c ON c.id = g.connection_id WHERE g.id = $1",surface).fetch_optional(pool).await?.ok_or_else(|| DenError::NotFound("repository not found".into()))?;
    if row.connection_id.is_none() {
        return Ok(ResolvedConnection::Legacy);
    }
    if row.revoked != Some(false) || !row.owner_authorized {
        return Ok(ResolvedConnection::Denied);
    }
    let provider = Provider::parse(
        row.provider
            .as_deref()
            .ok_or_else(|| DenError::System("connection provider missing".into()))?,
    )?;
    if validate_upstream(provider, &row.upstream_url).is_err() {
        return Ok(ResolvedConnection::Denied);
    }
    if provider == Provider::GithubExternal {
        return Ok(ResolvedConnection::ExternalReference);
    }
    Ok(ResolvedConnection::Available(ExecutionConnection {
        kind: match provider {
            Provider::GitHttps => Some("https_token".into()),
            Provider::GitSsh => Some("ssh_key".into()),
            Provider::GithubApp => None,
            Provider::GithubExternal => unreachable!("external references are not exported"),
        },
        ciphertext: row.secret_ciphertext,
        installation: row.github_app_installation_id,
        write: row.github_app_write_enabled.unwrap_or(false),
    }))
}

pub async fn linked_repositories(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<Uuid>, DenError> {
    Ok(sqlx::query_scalar!(
        "SELECT id FROM git_work_surface_details WHERE id = ANY($1) AND connection_id IS NOT NULL",
        ids
    )
    .fetch_all(pool)
    .await?)
}

pub async fn require_live_for_surface(pool: &PgPool, surface: Uuid) -> Result<(), DenError> {
    match resolve(pool, surface).await? {
        ResolvedConnection::Denied => Err(DenError::Authorization(
            "repository connection is revoked or no longer authorized".into(),
        )),
        ResolvedConnection::ExternalReference => Err(DenError::Authorization(
            "external-reference Connections are not available to sandbox provisioning or credential export; the repository_head backend is unconfigured".into(),
        )),
        _ => Ok(()),
    }
}
