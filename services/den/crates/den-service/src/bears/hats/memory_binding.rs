//! Resolve a durable memory source and hat from Den-owned conversation/Work state.
//! A revoked Work hat is an error, never a fallback to the legacy profile branch.

use den_core::{
    ids::{BearId, HatId},
    DenError,
};
use den_memory::{scoped::MemoryReadGrant, MemorySource};
use sqlx::PgPool;
use uuid::Uuid;

use super::bindings::{eligible_job_hat, job_hat};
use crate::conversation::persistence::get_conversation_for_external_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedMemoryBinding {
    Bound(MemoryReadGrant),
}

pub fn missing_binding() -> DenError {
    DenError::Authorization(
        "a named hat is required; start a conversation or Job explicitly bound to a hat".into(),
    )
}

pub async fn for_conversation(
    pool: &PgPool,
    bear_id: BearId,
    canonical_conversation_id: Uuid,
) -> Result<ResolvedMemoryBinding, DenError> {
    let hat_id = sqlx::query_scalar!(
        "SELECT hat_id FROM conversations WHERE bear_id = $1 AND id = $2 AND status = 'active'",
        bear_id.as_uuid(),
        canonical_conversation_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound("active conversation is not bound to this Bear".into()))?;
    match hat_id {
        Some(id) => {
            let hat = super::manage::get_hat(pool, bear_id, id.into()).await?;
            Ok(ResolvedMemoryBinding::Bound(MemoryReadGrant::new(
                MemorySource::Conversation(canonical_conversation_id),
                Some(hat.id),
            )))
        }
        None => Err(missing_binding()),
    }
}

pub async fn for_external_conversation(
    pool: &PgPool,
    bear_id: BearId,
    external_conversation_id: &str,
) -> Result<ResolvedMemoryBinding, DenError> {
    let conversation =
        get_conversation_for_external_id(pool, bear_id.as_uuid(), external_conversation_id)
            .await?
            .ok_or_else(|| DenError::NotFound("canonical conversation not found".into()))?;
    for_conversation(pool, bear_id, conversation.id).await
}

async fn require_job_hat(
    pool: &PgPool,
    bear_id: BearId,
    job_id: Uuid,
    hat_id: Option<HatId>,
) -> Result<(), DenError> {
    let hat_id = hat_id.ok_or_else(missing_binding)?;
    if eligible_job_hat(pool, bear_id, job_id).await? != Some(hat_id) {
        return Err(DenError::Authorization(
            "Work run's hat or surface grant is no longer eligible".into(),
        ));
    }
    Ok(())
}

/// Check a Job before the automatic dispatcher queues another run for it.
pub async fn require_eligible_job(
    pool: &PgPool,
    bear_id: BearId,
    job_id: Uuid,
) -> Result<(), DenError> {
    let hat_id = job_hat(pool, bear_id, job_id).await?;
    require_job_hat(pool, bear_id, job_id, hat_id).await
}

pub async fn for_work_run(
    pool: &PgPool,
    bear_id: BearId,
    work_run_id: Uuid,
) -> Result<ResolvedMemoryBinding, DenError> {
    let row = sqlx::query!(
        "SELECT r.job_id, j.hat_id FROM bear_work_runs r
         JOIN bear_jobs j ON j.id = r.job_id AND j.bear_id = r.bear_id
         WHERE r.bear_id = $1 AND r.id = $2",
        bear_id.as_uuid(),
        work_run_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound("Work run is not bound to this Bear".into()))?;
    let hat_id = row.hat_id.map(HatId::new).ok_or_else(missing_binding)?;
    require_job_hat(pool, bear_id, row.job_id, Some(hat_id)).await?;
    Ok(ResolvedMemoryBinding::Bound(MemoryReadGrant::new(
        MemorySource::WorkRun(work_run_id),
        Some(hat_id),
    )))
}
