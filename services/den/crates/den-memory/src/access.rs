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
/// - Erasure-marked source evidence ⇒ hidden, even from its authenticated owner.
/// - Permanent publications must pass eligibility and verified-owner checks before any grant.
/// - No access-rule history and no permanent protection ⇒ visible.
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
    // Scoped readers use base records, so permanent protection must precede the no-ACL grant.
    let protection = record_protection(store, src_memory_id).await?;
    if protection.erased_source {
        return Ok(false);
    }
    if protection.personal_publication {
        let Some(publication) =
            crate::personal_facts::publication_access(store, src_memory_id).await?
        else {
            return Ok(false);
        };
        if !publication.eligible
            || ctx
                .authenticated_user
                .as_ref()
                .is_none_or(|verified| verified.entity_id != publication.person_entity_id)
        {
            return Ok(false);
        }
    }
    let rules = list_access_rules_for_source(store, src_memory_id).await?;
    if rules.is_empty() {
        return Ok(!has_access_rule_history_for_source(store, src_memory_id).await?);
    }
    if ctx.is_empty() {
        return Ok(false);
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

#[derive(sqlx::FromRow)]
struct RecordProtection {
    erased_source: bool,
    personal_publication: bool,
}

/// Body-free classification by exact Bear/source IDs and the immutable erasure boundary.
/// Cleanup completion, current consent and ACL presence cannot undo this protection.
async fn record_protection(
    store: &BearMemoryStore,
    memory_id: &str,
) -> Result<RecordProtection, DenError> {
    let bear_id = store.bear_id().to_string();
    Ok(sqlx::query_as!(
        RecordProtection,
        r#"
        SELECT EXISTS (
            SELECT 1 FROM personal_memory_candidates c
            JOIN personal_memory_erasures e
              ON e.bear_id = c.bear_id AND e.person_entity_id = c.person_entity_id
                AND c.sequence_no < e.sequence_no
            WHERE c.bear_id = ? AND c.source_kind = 'conversation'
              AND c.source_memory_id = ?
        ) AS "erased_source!: bool",
        EXISTS (
            SELECT 1 FROM personal_memory_publications p
            WHERE p.bear_id = ? AND p.memory_id = ?
        ) AS "personal_publication!: bool"
        "#,
        bear_id,
        memory_id,
        bear_id,
        memory_id,
    )
    .fetch_one(store.pool())
    .await?)
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
