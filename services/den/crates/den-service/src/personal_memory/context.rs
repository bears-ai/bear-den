//! Server-only preferred-name context authority and references, with no runtime consumer.

mod render;
#[cfg(test)]
mod tests;
mod types;

pub use types::{
    PersonalContextMode, PersonalContextReference, PersonalContextSource, RenderedPersonalContext,
};

use den_core::{DenError, TurnExecutionOrigin};
use den_memory::{
    personal_facts::{self, PersonalFactId, PersonalFactState},
    resolve_authenticated_user_entity, MemoryStoreManager,
};
use sqlx::PgPool;

use super::authority;

fn unauthorized() -> DenError {
    DenError::Authorization("Personal-memory context is stale or no longer authorized.".into())
}

fn require_human_conversation(source: PersonalContextSource) -> Result<(), DenError> {
    match source.origin {
        TurnExecutionOrigin::ChannelConversation
        | TurnExecutionOrigin::BrowserTaskSession
        | TurnExecutionOrigin::ArmatureConversation(_) => Ok(()),
        TurnExecutionOrigin::AuthorizedWorkRun(_)
        | TurnExecutionOrigin::InternalCuration
        | TurnExecutionOrigin::InboundObservation => Err(unauthorized()),
    }
}

fn require_reference_source(
    source: PersonalContextSource,
    reference: &PersonalContextReference,
) -> Result<(), DenError> {
    require_human_conversation(source)?;
    if source != reference.source {
        return Err(unauthorized());
    }
    Ok(())
}

pub(super) fn require_active_context_head(
    expected: Option<(PersonalFactId, PersonalFactState)>,
) -> Result<(), DenError> {
    match expected {
        Some((_, PersonalFactState::Active)) => Ok(()),
        _ => Err(unauthorized()),
    }
}

async fn lock_context_rights(
    pool: &PgPool,
    source: PersonalContextSource,
) -> Result<authority::LockedMember, DenError> {
    let mut locked = authority::lock_member(pool, source.bear_id, source.user_id).await?;
    locked
        .lock_owned_conversation(source.conversation_id)
        .await?;
    Ok(locked)
}

/// Snapshot only a trusted human's owned active conversation, exact binding and current head.
///
/// Disabled mode returns before any I/O or identity allocation. Enabled preparation resolves
/// the binding once and reads the head with that same binding; no preferred-name value is
/// retained in the reference. Rights locks remain held through completion if the caller
/// cancels, since identity resolution may allocate a binding. This does not enable inference,
/// recall, capture, or publication, and the returned reference is not an access grant.
pub async fn prepare_personal_context(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    mode: PersonalContextMode,
    source: PersonalContextSource,
) -> Result<Option<PersonalContextReference>, DenError> {
    if mode == PersonalContextMode::DefaultDisabled {
        return Ok(None);
    }
    require_human_conversation(source)?;
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let locked = lock_context_rights(&pool, source).await?;
        let store = stores.store_for_bear(source.bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, source.user_id).await?;
        let head = personal_facts::current_preferred_name(&store, &binding).await?;
        let reference = PersonalContextReference {
            source,
            binding,
            expected: head.as_ref().map(|head| (head.fact_id, head.state)),
        };
        drop(locked);
        Ok(Some(reference))
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory context preparation is unavailable; retry preparation.".into(),
        )
    })?
}

/// Check freshness of an active retained reference without reading a name or rendering text.
///
/// Disabled mode, a source mismatch, a nonhuman origin or an inactive snapshot denies before
/// I/O. Current rights and the OLD binding's generation and exact head ID/state are rechecked;
/// this never prepares a replacement identity or selects a newer fact. All locks are released
/// before returning. Success is an observation, not a provider lease or an in-flight safeguard.
/// No inference or private-context consumer is enabled by this API.
pub async fn revalidate_personal_context(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    mode: PersonalContextMode,
    source: PersonalContextSource,
    reference: &PersonalContextReference,
) -> Result<(), DenError> {
    if mode == PersonalContextMode::DefaultDisabled {
        return Err(unauthorized());
    }
    require_reference_source(source, reference)?;
    require_active_context_head(reference.expected)?;
    let pool = pool.clone();
    let stores = stores.clone();
    let reference = reference.clone();
    tokio::spawn(async move {
        let locked = lock_context_rights(&pool, source).await?;
        let store = stores.store_for_bear(source.bear_id.as_uuid()).await?;
        let version =
            personal_facts::current_preferred_name_version(&store, &reference.binding).await?;
        let checked = if version.map(|head| (head.fact_id, head.state)) == reference.expected {
            Ok(())
        } else {
            Err(unauthorized())
        };
        drop(locked);
        checked
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory context revalidation is unavailable; retry the check.".into(),
        )
    })?
}

/// Revalidate a reference and render only its still-current, eligible active name.
///
/// Disabled mode returns before any I/O, including for an existing reference. Source rights
/// are checked afresh, while SQLite reads use the OLD retained binding so reconnecting the
/// same account/person tuple cannot revive an old-generation reference. Any head ID/state
/// transition requires new preparation, even when the old or new head has no value.
/// The owning task releases all locks before returning: rendered text is not a provider lease
/// and does not address in-flight requests, transcript replay or secondary-output removal.
pub async fn render_personal_context(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    mode: PersonalContextMode,
    source: PersonalContextSource,
    reference: &PersonalContextReference,
) -> Result<Option<RenderedPersonalContext>, DenError> {
    if mode == PersonalContextMode::DefaultDisabled {
        return Ok(None);
    }
    require_reference_source(source, reference)?;
    let pool = pool.clone();
    let stores = stores.clone();
    let reference = reference.clone();
    tokio::spawn(async move {
        let locked = lock_context_rights(&pool, source).await?;
        let store = stores.store_for_bear(source.bear_id.as_uuid()).await?;
        let head = personal_facts::current_preferred_name(&store, &reference.binding).await?;
        if head.as_ref().map(|head| (head.fact_id, head.state)) != reference.expected {
            return Err(unauthorized());
        }
        let rendered = match head {
            Some(head) if head.state == PersonalFactState::Active => head
                .preferred_name
                .as_ref()
                .map(render::render_name)
                .transpose(),
            _ => Ok(None),
        };
        drop(locked);
        rendered
    })
    .await
    .map_err(|_| {
        DenError::System("Personal-memory context rendering is unavailable; retry the read.".into())
    })?
}
