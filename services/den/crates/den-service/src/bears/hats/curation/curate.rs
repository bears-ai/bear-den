//! Internal Curate publication boundary, not a model-facing tool. The selected
//! hat and source come from the verified SQLite proposal columns; both are
//! rechecked against current Postgres state while holding the Work-enable lock.

use den_core::{ids::BearId, DenError};
use den_memory::{
    get_memory_proposal,
    hat_promotion::{self, ReviewedPromotion},
    MemorySource, MemoryStoreManager,
};
use sqlx::PgPool;
use uuid::Uuid;

#[cfg(test)]
#[path = "curate_tests.rs"]
mod tests;

pub async fn promote_curated_proposal(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    proposal_id: Uuid,
    curated_content: &str,
    curator_agent_id: &str,
    curator_reason: &str,
) -> Result<ReviewedPromotion, DenError> {
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    let proposal = get_memory_proposal(&store, &proposal_id.to_string())
        .await?
        .ok_or_else(|| DenError::NotFound("verified hat proposal not found".into()))?;
    let verified = proposal.verified_hat_source.ok_or_else(|| {
        DenError::Authorization("a path or JSON reference is not a verified hat proposal".into())
    })?;
    if proposal.status != "pending" {
        return Err(DenError::ValidationError(
            "hat proposal is no longer pending".into(),
        ));
    }
    let source = hat_promotion::review_candidate(&store, verified.memory_id).await?;
    let MemorySource::Conversation(conversation_id) = source.source else {
        return Err(DenError::Authorization(
            "only canonical conversation notes can enter the current Curate lane".into(),
        ));
    };
    // Work enablement locks this hat before inspecting its historical memory.
    // Once enabled, the same hat audience includes eligible current and future
    // Job runs; Curate cannot choose an audience or grant execution authority.
    let mut tx = pool.begin().await?;
    let auto_curate_enabled = sqlx::query_scalar!(
        "SELECT h.auto_curate_enabled FROM bear_hats h JOIN conversations c
         ON c.hat_id = h.id AND c.bear_id = h.bear_id
         JOIN user_bear member ON member.bear_id = c.bear_id
           AND member.user_id = c.created_by_user_id
         WHERE h.bear_id = $1 AND h.id = $2 AND c.id = $3
           AND c.status = 'active'
         FOR UPDATE OF h, c, member",
        bear_id.as_uuid(),
        verified.hat_id.as_uuid(),
        conversation_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        DenError::Authorization("verified source or Bear hat binding is no longer current".into())
    })?;
    if !auto_curate_enabled {
        return Err(DenError::Authorization(
            "Curate publication requires this hat's automatic memory-sharing opt-in".into(),
        ));
    }
    let outcome = hat_promotion::promote_curated_proposal_to_hat(
        &store,
        proposal_id,
        verified,
        curated_content,
        curator_agent_id,
        curator_reason,
    )
    .await?;
    tx.commit().await?;
    Ok(outcome)
}
