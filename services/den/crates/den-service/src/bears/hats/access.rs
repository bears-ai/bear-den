//! Canonical, Den-owned hat access grants. Only the configured-hat Den
//! web-fetch decision consumes them today; other effect paths need the shared
//! actor, resource, and egress resolver.

use std::net::IpAddr;

use den_core::{
    client_tools::ClientToolName,
    ids::{BearId, HatId, UserId},
    tools::{constants::DEN_WEB_FETCH, descriptor::builtin_den_tool_descriptor_for_provider_name},
    DenError,
};
use den_sandbox::protocol::AllowedOutboundHosts;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::memory_binding::{self, ResolvedMemoryBinding};
use crate::conversation::viewer::ConversationViewer;

#[cfg(test)]
#[path = "access/tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActionKey(String);

impl ToolActionKey {
    /// Model/provider aliases resolve to a canonical descriptor before storage.
    /// Dynamic MCP names need a trusted server descriptor and are not accepted
    /// as broad grants merely because an armature advertised a string.
    pub fn from_provider_name(name: &str) -> Result<Self, DenError> {
        if let Some(descriptor) = builtin_den_tool_descriptor_for_provider_name(name) {
            return Ok(Self(descriptor.name.to_string()));
        }
        if let Some(tool) = ClientToolName::from_provider_alias(name) {
            if tool != ClientToolName::McpCallTool {
                return Ok(Self(tool.descriptor().canonical_name.to_string()));
            }
        }
        Err(DenError::ValidationError(
            "a hat tool grant requires a known canonical tool descriptor".into(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsHost(String);

impl HttpsHost {
    pub fn parse(raw: &str) -> Result<Self, DenError> {
        let raw = raw.trim();
        if raw.is_empty()
            || raw.len() > 253
            || raw.contains(['/', ':', '?', '#', '@', '*'])
            || raw.chars().any(char::is_whitespace)
        {
            return Err(DenError::ValidationError(
                "a hat network grant requires one exact HTTPS hostname".into(),
            ));
        }
        let parsed = reqwest::Url::parse(&format!("https://{raw}/"))
            .map_err(|err| DenError::ValidationError(format!("invalid HTTPS hostname: {err}")))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| DenError::ValidationError("network grant host is missing".into()))?;
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host == "localhost"
            || host.ends_with(".localhost")
            || host.parse::<IpAddr>().is_ok()
            || !host.contains('.')
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
            })
        {
            return Err(DenError::ValidationError(
                "network grant must name a public DNS hostname, not an IP, local host, or pattern"
                    .into(),
            ));
        }
        Ok(Self(host))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HatAccessGrant {
    ToolForHat(ToolActionKey),
    HttpsHost(HttpsHost),
}

impl HatAccessGrant {
    fn storage(&self) -> (&'static str, &str, &'static str, &str) {
        match self {
            Self::ToolForHat(action) => ("tool", &action.0, "hat", ""),
            Self::HttpsHost(host) => ("network", "https", "host", &host.0),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WebFetchHostGrant {
    pub id: Uuid,
    pub host: String,
}

#[derive(Debug, Serialize)]
pub struct WebFetchGrantSummary {
    pub tool_grant_id: Option<Uuid>,
    pub hosts: Vec<WebFetchHostGrant>,
}

/// Read only the two grant dimensions used by Den's configured-hat web fetch.
/// The Bear-admin page provides the authorization for presenting this summary;
/// this is not a model-facing or effect-time authorization resolver.
pub async fn web_fetch_grants_for_hat(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
) -> Result<WebFetchGrantSummary, DenError> {
    super::manage::get_hat(pool, bear_id, hat_id).await?;
    let action = ToolActionKey::from_provider_name(DEN_WEB_FETCH)?;
    let tool_grant_id = sqlx::query_scalar!(
        "SELECT id FROM bear_hat_access_grants
         WHERE bear_id = $1 AND hat_id = $2 AND kind = 'tool'
           AND action_key = $3 AND target_kind = 'hat' AND target_value = ''
           AND revoked_at IS NULL",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        action.0,
    )
    .fetch_optional(pool)
    .await?;
    let hosts = sqlx::query!(
        "SELECT id, target_value FROM bear_hat_access_grants
         WHERE bear_id = $1 AND hat_id = $2 AND kind = 'network'
           AND action_key = 'https' AND target_kind = 'host'
           AND revoked_at IS NULL ORDER BY target_value",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| WebFetchHostGrant {
        id: row.id,
        host: row.target_value,
    })
    .collect();
    Ok(WebFetchGrantSummary {
        tool_grant_id,
        hosts,
    })
}

/// Only a current Bear admin can persist a positive grant. The membership and
/// Bear-owned hat checks happen inside the write, not in UI text or a stale view.
pub async fn grant(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    actor: UserId,
    access: &HatAccessGrant,
    confirm_future_job_audience: bool,
) -> Result<Uuid, DenError> {
    if !confirm_future_job_audience {
        return Err(DenError::ValidationError(
            "confirm this grant is available to future authorized Job runs wearing the hat".into(),
        ));
    }
    let (kind, action_key, target_kind, target_value) = access.storage();
    let created = sqlx::query_scalar!(
        "INSERT INTO bear_hat_access_grants
            (bear_id, hat_id, kind, action_key, target_kind, target_value, created_by_user_id)
         SELECT h.bear_id, h.id, $5, $6, $7, $8, $3
         FROM bear_hats h JOIN user_bear ub
           ON ub.bear_id = h.bear_id AND ub.user_id = $3
         WHERE h.bear_id = $1 AND h.id = $2
           AND lower(btrim(coalesce(ub.role, ''))) = $4
         ON CONFLICT (bear_id, hat_id, kind, action_key, target_kind, target_value)
           WHERE revoked_at IS NULL DO NOTHING
         RETURNING id",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        actor.get(),
        crate::bears::db::BEAR_ROLE_ADMIN,
        kind,
        action_key,
        target_kind,
        target_value,
    )
    .fetch_optional(pool)
    .await?;
    if let Some(id) = created {
        return Ok(id);
    }
    sqlx::query_scalar!(
        "SELECT g.id FROM bear_hat_access_grants g
         JOIN user_bear ub ON ub.bear_id = g.bear_id AND ub.user_id = $3
         WHERE g.bear_id = $1 AND g.hat_id = $2
           AND lower(btrim(coalesce(ub.role, ''))) = $4
           AND g.kind = $5 AND g.action_key = $6
           AND g.target_kind = $7 AND g.target_value = $8
           AND g.revoked_at IS NULL",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        actor.get(),
        crate::bears::db::BEAR_ROLE_ADMIN,
        kind,
        action_key,
        target_kind,
        target_value,
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        DenError::Authorization("Bear admin and owned hat are required for this grant".into())
    })
}

pub async fn revoke(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    actor: UserId,
    grant_id: Uuid,
) -> Result<(), DenError> {
    sqlx::query_scalar!(
        "UPDATE bear_hat_access_grants g SET revoked_at = now()
         FROM user_bear ub
         WHERE g.bear_id = $1 AND g.hat_id = $2 AND g.id = $3
           AND g.revoked_at IS NULL
           AND ub.bear_id = g.bear_id AND ub.user_id = $4
           AND lower(btrim(coalesce(ub.role, ''))) = $5
         RETURNING g.id",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        grant_id,
        actor.get(),
        crate::bears::db::BEAR_ROLE_ADMIN,
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        DenError::Authorization("grant cannot be revoked by this actor or hat".into())
    })?;
    Ok(())
}

/// Internal storage check, not a complete execution authorization decision.
/// Callers must independently verify the human, source, origin, surface, Job,
/// credential, and platform restrictions before using a grant for an effect.
pub(crate) async fn has_current(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    access: &HatAccessGrant,
) -> Result<bool, DenError> {
    let (kind, action_key, target_kind, target_value) = access.storage();
    Ok(sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM bear_hat_access_grants
         WHERE bear_id = $1 AND hat_id = $2 AND kind = $3
           AND action_key = $4 AND target_kind = $5 AND target_value = $6
           AND revoked_at IS NULL) AS \"allowed!\"",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        kind,
        action_key,
        target_kind,
        target_value,
    )
    .fetch_one(pool)
    .await?)
}

/// Narrow read of hat policy for the authenticated creator of a canonical
/// conversation. This is not an execution grant: descriptor, armature, target,
/// network, credential, and run policy must also pass at the effect boundary.
pub async fn has_grant_for_own_conversation(
    pool: &PgPool,
    bear_id: BearId,
    conversation_id: Uuid,
    human: UserId,
    access: &HatAccessGrant,
) -> Result<bool, DenError> {
    let viewer = ConversationViewer::resolve(pool, bear_id, human)
        .await?
        .ok_or_else(|| DenError::Authorization("current Bear membership is required".into()))?;
    if !viewer.may_read_own_source(pool, conversation_id).await? {
        return Err(DenError::Authorization(
            "hat access requires an active conversation owned by this human".into(),
        ));
    }
    let binding = memory_binding::for_conversation(pool, bear_id, conversation_id).await?;
    let ResolvedMemoryBinding::Bound(bound) = binding else {
        return Ok(false);
    };
    let Some(hat_id) = bound.hat_id() else {
        return Ok(false);
    };
    has_current(pool, bear_id, hat_id, access).await
}

/// Verify both dimensions of a potential HTTPS fetch for this conversation.
/// This does not authorize execution: the descriptor, current actor, Den web
/// safety policy, credentials, and any work-surface bounds still must pass.
pub async fn has_web_fetch_grants_for_own_conversation(
    pool: &PgPool,
    bear_id: BearId,
    conversation_id: Uuid,
    human: UserId,
    raw_url: &str,
) -> Result<bool, DenError> {
    let url = reqwest::Url::parse(raw_url)
        .map_err(|err| DenError::ValidationError(format!("invalid web destination: {err}")))?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Ok(false);
    }
    let host = url
        .host_str()
        .ok_or_else(|| DenError::ValidationError("web destination has no host".into()))?;
    let host = HttpsHost::parse(host)?;
    let tool = HatAccessGrant::ToolForHat(ToolActionKey::from_provider_name(DEN_WEB_FETCH)?);
    let destination = HatAccessGrant::HttpsHost(host);
    if !has_grant_for_own_conversation(pool, bear_id, conversation_id, human, &tool).await? {
        return Ok(false);
    }
    has_grant_for_own_conversation(pool, bear_id, conversation_id, human, &destination).await
}

/// The network destinations a *separately authorized* Job may receive for one
/// managed surface. An intersection is not checkout or credential authority;
/// the caller must first verify its Job, hat, actor, and assigned surface.
pub async fn intersect_surface_outbound_hosts(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    surface_hosts: &AllowedOutboundHosts,
) -> Result<AllowedOutboundHosts, DenError> {
    super::manage::get_hat(pool, bear_id, hat_id).await?;
    let rows = sqlx::query_scalar!(
        "SELECT target_value FROM bear_hat_access_grants
         WHERE bear_id = $1 AND hat_id = $2 AND kind = 'network'
           AND action_key = 'https' AND target_kind = 'host'
           AND revoked_at IS NULL ORDER BY target_value",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
    )
    .fetch_all(pool)
    .await?;
    let hat_hosts = rows
        .into_iter()
        .map(|host| HttpsHost::parse(&host))
        .collect::<Result<Vec<_>, _>>()?;
    AllowedOutboundHosts::new(
        surface_hosts
            .as_slice()
            .iter()
            .filter(|host| hat_hosts.iter().any(|grant| grant.0 == **host))
            .cloned()
            .collect(),
    )
    .map_err(|err| DenError::System(format!("invalid surface egress ceiling: {err}")))
}
