//! Direct owner intake from trusted human settings, separate from publication approval.

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_memory::{
    personal_candidates::{self, PersonalCandidate, PreferredName, VerifiedOwnerSource},
    resolve_authenticated_user_entity, MemoryStoreManager,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::authority;

/// Stage an explicit HUMAN settings request without a conversation or source note.
///
/// `user_id` must be the trusted authenticated principal, never a client/model-selected
/// owner. The owner must already have enabled capture. This does not publish a fact or
/// enable recall; publication/correction needs a separate explicit owner approval.
/// Reuse the request ID only for an identical retry, including after a lost result.
/// Authority locks and the operation survive caller cancellation.
pub async fn stage_owner_preferred_name(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    user_id: UserId,
    request_id: Uuid,
    name: PreferredName,
) -> Result<PersonalCandidate, DenError> {
    let pool = pool.clone();
    let stores = stores.clone();
    tokio::spawn(async move {
        let locked = authority::lock_member(&pool, bear_id, user_id).await?;
        let store = stores.store_for_bear(bear_id.as_uuid()).await?;
        let binding = resolve_authenticated_user_entity(&store, user_id).await?;
        let source = VerifiedOwnerSource::for_authorized_owner_request(&binding, request_id);
        let staged = personal_candidates::stage_owner_preferred_name(&store, &source, name).await;
        drop(locked);
        staged
    })
    .await
    .map_err(|_| {
        DenError::System(
            "Personal-memory staging outcome is unavailable; retry the identical owner request to recover its candidate."
                .into(),
        )
    })?
}
