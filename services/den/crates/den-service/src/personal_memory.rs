//! Server-only personal-memory intake and explicit owner approval, never a model tool.
//!
//! Callers supply a trusted authenticated principal, never an identity selected by
//! a model. Postgres rights/source locks outlive request cancellation and remain
//! held through the owner-bound SQLite operation. Capture stays disabled by default.

mod authority;
mod publication;
#[cfg(test)]
mod tests;

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_memory::{
    personal_candidates::{self, VerifiedPersonalSource},
    resolve_authenticated_user_entity, MemoryStoreManager,
};
use sqlx::PgPool;
use uuid::Uuid;

pub use den_memory::{
    personal_candidates::{PersonalCandidate, PersonalCandidateId, PreferredName},
    personal_facts::{PersonalFactHead, PersonalFactId, PersonalFactState, PublicationReceipt},
};
pub use publication::{current_preferred_name, publish_preferred_name, retract_preferred_name};

/// Stage an explicit, durable preferred-name request without publishing memory.
///
/// `user_id` must come from the server's authenticated principal, not tool/client
/// arguments. Conversation and note IDs are only locators: current ownership,
/// membership, hat existence and the exact canonical SQLite source are checked.
/// This does not enable capture; the owner's setting must already permit it.
/// Once started, staging continues if the caller drops this future. A lost result
/// is not evidence that staging failed; inspect owner-bound candidates before retrying.
pub async fn stage_preferred_name(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
    conversation_id: Uuid,
    source_memory_id: Uuid,
    name: PreferredName,
) -> Result<PersonalCandidate, DenError> {
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let mut locked = authority::lock_member(&pool, bear_id, user_id).await?;
        locked.lock_owned_conversation(conversation_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, user_id).await?;
        let source = VerifiedPersonalSource::for_authorized_conversation(
            &binding,
            conversation_id,
            source_memory_id,
        );
        let staged = personal_candidates::stage_preferred_name(&store, &source, name).await;
        // These are read/authority locks, not a distributed Postgres/SQLite write transaction.
        drop(locked);
        staged
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory staging outcome is unavailable; inspect owned candidates before retrying."
                .into(),
        )
    })?
}

/// Change capture for the authenticated member's resolved person only.
///
/// This is a server-only policy boundary, not a model tool. Enabling capture does
/// not publish facts or enable recall. Disabling atomically withdraws pending candidates
/// and retracts the current fact; reenabling resurrects neither. The operation continues
/// on caller cancellation.
pub async fn set_capture_enabled(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
    enabled: bool,
) -> Result<(), DenError> {
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let locked = authority::lock_member(&pool, bear_id, user_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, user_id).await?;
        let saved = personal_candidates::set_capture_enabled(&store, &binding, enabled).await;
        drop(locked);
        saved
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory capture setting outcome is unavailable; verify the setting before retrying."
                .into(),
        )
    })?
}
