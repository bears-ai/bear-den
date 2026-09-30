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
    // The admin Work-enablement paths lock the same hat row. Publishing into a
    // Work-off hat first makes subsequent enablement review its new memory;
    // Work-on first denies this publication. No model flag widens that audience.
    let mut tx = pool.begin().await?;
    let work_enabled = sqlx::query_scalar!(
        "SELECT h.work_enabled FROM bear_hats h JOIN conversations c
         ON c.hat_id = h.id AND c.bear_id = h.bear_id
         WHERE h.bear_id = $1 AND h.id = $2 AND c.id = $3
           AND c.status = 'active' AND c.created_by_user_id IS NOT NULL
         FOR UPDATE OF h, c",
        bear_id.as_uuid(),
        verified.hat_id.as_uuid(),
        conversation_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        DenError::Authorization("verified source or Bear hat binding is no longer current".into())
    })?;
    if work_enabled {
        return Err(DenError::Authorization(
            "autonomous Curate publication into a Work-enabled hat is not supported".into(),
        ));
    }
    let outcome = hat_promotion::promote_curated_proposal_to_hat(
        &store,
        proposal_id,
        verified,
        curated_content,
        curator_agent_id,
    )
    .await?;
    tx.commit().await?;
    Ok(outcome)
}
