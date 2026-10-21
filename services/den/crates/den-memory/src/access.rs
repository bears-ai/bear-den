//! Access-bearing enforcement (ADR-0042 §7, §11): the recall gate.
//!
//! Every recall/projection read must thread an `AccessContext`. An empty context hides
//! access-gated records; authenticated-user audiences additionally require a verified
//! binding. Rule history remains protective even after the last active rule is withdrawn.

use std::collections::BTreeSet;

use den_core::ids::{BearId, UserId};
use den_core::DenError;

use super::authenticated_user::{
    user_entity_authority, AuthenticatedUserEntity, UserEntityAuthority,
};
use super::descriptors;
use super::entity::{get_entity, resolve_live_entity};
use super::records::BearMemoryStore;
use super::relations::{has_access_rule_history_for_source, list_access_rules_for_source};

/// The caller's current access context: which entities the Bear is acting with/for
/// (`audience`) and which scopes the run is confined to (`confined_to`).
///
/// Entity ids should be **live** ids; rule targets are resolved through the merge forward
/// pointer before comparison, so a merged non-user audience still matches its survivor.
#[derive(Debug, Clone, Default)]
pub struct AccessContext {
    audience: BTreeSet<String>,
    confinement: BTreeSet<String>,
    authenticated_user: Option<VerifiedAudience>,
}

#[derive(Debug, Clone)]
struct VerifiedAudience {
    bear_id: BearId,
    user_id: UserId,
    entity_id: String,
    authority_generation: i64,
}

impl AccessContext {
    /// Fail-closed context: grants nothing. Any access-gated record is hidden.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A verified user audience, scoped to the Bear that owns the binding.
    /// Protected reads revalidate this binding; keeping this context does not preserve
    /// access after the identity is quarantined.
    pub fn for_authenticated_user(binding: &AuthenticatedUserEntity) -> Self {
        Self {
            audience: BTreeSet::from([binding.entity_id().to_string()]),
            confinement: BTreeSet::new(),
            authenticated_user: Some(VerifiedAudience {
                bear_id: binding.bear_id(),
                user_id: binding.user_id(),
                entity_id: binding.entity_id().to_string(),
                authority_generation: binding.authority_generation,
            }),
        }
    }

    /// Entities the run is acting with/for. Raw ids only satisfy non-user audiences;
    /// authenticated-user audiences require `for_authenticated_user`.
    pub fn with_audience<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.audience.extend(ids.into_iter().map(Into::into));
        self
    }

    /// Scopes the run is confined to — satisfies `confined_to` rules.
    pub fn with_confinement<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.confinement.extend(ids.into_iter().map(Into::into));
        self
    }

    pub fn allow_audience(&mut self, entity_id: impl Into<String>) {
        self.audience.insert(entity_id.into());
    }

    pub fn allow_confinement(&mut self, entity_id: impl Into<String>) {
        self.confinement.insert(entity_id.into());
    }

    pub fn is_empty(&self) -> bool {
        self.audience.is_empty() && self.confinement.is_empty()
    }
}

/// Whether `src_memory_id` may be surfaced under `ctx`.
///
/// - No access-rule history ⇒ always visible.
/// - Historical rules but no active rules ⇒ hidden.
/// - `audience` rules ⇒ at least one addressed entity must match; bound user identities
///   require a verified binding for the same Bear, user, and entity.
/// - `confined_to` rules ⇒ every confinement scope must match.
/// - Both classes must pass. Missing, non-resolved, quarantined, or unknown targets/rules
///   deny access. Verified contexts must still have the same active Bear/user/entity binding.
pub async fn record_visible(
    store: &BearMemoryStore,
    src_memory_id: &str,
    ctx: &AccessContext,
) -> Result<bool, DenError> {
    let rules = list_access_rules_for_source(store, src_memory_id).await?;
    if rules.is_empty() {
        return Ok(!has_access_rule_history_for_source(store, src_memory_id).await?);
    }
    if let Some(verified) = &ctx.authenticated_user {
        if verified.bear_id != BearId::from(store.bear_id()) {
            return Ok(false);
        }
        if !current_verified_binding(store, verified).await? {
            return Ok(false);
        }
    }

    let mut has_audience = false;
    let mut audience_matches = false;

    for rule in &rules {
        let descriptor = match descriptors::relation(&rule.relation) {
            Some(descriptor) if matches!(descriptor.alias, "audience" | "confined_to") => {
                descriptor
            }
            // Unknown access semantics cannot be satisfied by any raw context id.
            _ => return Ok(false),
        };
        // A quarantined identity must never become generic by following a forward pointer.
        // Active bindings also name this exact entity, not a possible merge survivor.
        let authority = user_entity_authority(store, &rule.entity_id).await?;
        let target = match &authority {
            Some(UserEntityAuthority::Quarantined) => return Ok(false),
            Some(UserEntityAuthority::Active(_)) => get_entity(store, &rule.entity_id).await?,
            None => resolve_live_entity(store, &rule.entity_id).await?,
        };
        let target = match target {
            Some(entity)
                if entity.resolution.can_bear_access()
                    && entity.superseded_by_entity_id.is_none() =>
            {
                entity
            }
            _ => return Ok(false),
        };
        let authority = if target.entity_id == rule.entity_id {
            authority
        } else {
            user_entity_authority(store, &target.entity_id).await?
        };
        let bound_target_matches = match authority {
            Some(UserEntityAuthority::Quarantined) => return Ok(false),
            Some(UserEntityAuthority::Active(user_id)) => {
                Some(ctx.authenticated_user.as_ref().is_some_and(|verified| {
                    verified.user_id == user_id && verified.entity_id == target.entity_id
                }))
            }
            None => None,
        };
        match descriptor.alias {
            "audience" => {
                has_audience = true;
                audience_matches |= bound_target_matches
                    .unwrap_or_else(|| ctx.audience.contains(&target.entity_id));
            }
            "confined_to" => {
                if bound_target_matches == Some(false)
                    || !ctx.confinement.contains(&target.entity_id)
                {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }

    Ok(!has_audience || audience_matches)
}

async fn current_verified_binding(
    store: &BearMemoryStore,
    verified: &VerifiedAudience,
) -> Result<bool, DenError> {
    let bear_id = store.bear_id().to_string();
    let user_id = verified.user_id.get();
    let canonical_ref = format!("den:user:{}", verified.user_id);
    Ok(sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM authenticated_user_entities a
            JOIN user_identity_generations g
                ON g.bear_id = a.bear_id AND g.entity_id = a.entity_id
            JOIN entities e ON e.bear_id = a.bear_id AND e.entity_id = a.entity_id
            WHERE a.bear_id = ? AND a.entity_id = ? AND a.user_id = ? AND a.state = 'active'
              AND g.generation = ? AND e.type = 'person' AND e.resolution = 'confirmed'
              AND e.trust = 'asserted' AND e.canonical_ref = ?
              AND e.superseded_by_entity_id IS NULL
        ) AS "matches!: bool"
        "#,
        bear_id,
        verified.entity_id,
        user_id,
        verified.authority_generation,
        canonical_ref,
    )
    .fetch_one(store.pool())
    .await?)
}

#[cfg(test)]
mod tests;
