//! Bind hats to durable conversation and Job identities, never to a client session.
//!
//! A binding only narrows the existing authority. Work must recheck the Job's
//! assigned surfaces and the hat's Work eligibility at dispatch/execution time.

use den_core::{
    ids::{BearId, HatId},
    DenError,
};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn bind_conversation_hat(
    pool: &PgPool,
    bear_id: BearId,
    conversation_id: Uuid,
    hat_id: HatId,
) -> Result<(), DenError> {
    let bound = sqlx::query_scalar!(
        r#"UPDATE conversations AS c SET hat_id = $3
           FROM bear_hats AS h
           WHERE c.id = $2 AND c.bear_id = $1 AND c.status = 'active'
             AND h.id = $3 AND h.bear_id = c.bear_id
             AND (c.hat_id = $3 OR (c.hat_id IS NULL
                 AND NOT EXISTS (SELECT 1 FROM conversation_messages m WHERE m.conversation_id = c.id)
                 AND NOT EXISTS (
                     SELECT 1 FROM client_sessions s
                     WHERE s.bear_id = c.bear_id AND s.closed_at IS NULL
                       AND (s.conversation_id = c.external_conversation_id
                            OR s.resolved_conversation_id = c.external_conversation_id)
                 )
             ))
           RETURNING c.hat_id AS "hat_id!: Uuid""#,
        bear_id.as_uuid(),
        conversation_id,
        hat_id.as_uuid(),
    )
    .fetch_optional(pool)
    .await?;
    if bound.is_none() {
        return Err(DenError::Authorization(
            "conversation is unavailable or already bound to another hat".to_string(),
        ));
    }
    Ok(())
}

pub async fn conversation_can_bind_hat(
    pool: &PgPool,
    bear_id: BearId,
    conversation_id: Uuid,
) -> Result<bool, DenError> {
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (
            SELECT 1 FROM conversations c
            WHERE c.bear_id = $1 AND c.id = $2 AND c.status = 'active' AND c.hat_id IS NULL
              AND NOT EXISTS (SELECT 1 FROM conversation_messages m WHERE m.conversation_id = c.id)
              AND NOT EXISTS (
                  SELECT 1 FROM client_sessions s
                  WHERE s.bear_id = c.bear_id AND s.closed_at IS NULL
                    AND (s.conversation_id = c.external_conversation_id
                         OR s.resolved_conversation_id = c.external_conversation_id)
              )
        ) AS "can_bind!""#,
        bear_id.as_uuid(),
        conversation_id,
    )
    .fetch_one(pool)
    .await?)
}

pub async fn conversation_hat(
    pool: &PgPool,
    bear_id: BearId,
    conversation_id: Uuid,
) -> Result<Option<HatId>, DenError> {
    let hat = sqlx::query_scalar!(
        "SELECT hat_id FROM conversations WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(),
        conversation_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(hat.flatten().map(HatId::new))
}

pub async fn job_hat(
    pool: &PgPool,
    bear_id: BearId,
    job_id: Uuid,
) -> Result<Option<HatId>, DenError> {
    let hat = sqlx::query_scalar!(
        "SELECT hat_id FROM bear_jobs WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(),
        job_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound("Job not found for this Bear".into()))?;
    Ok(hat.map(HatId::new))
}

/// Bind a draft Job only when all its assigned surfaces are on a Work-enabled
/// hat. This is not a Work authorization: dispatch must re-evaluate grants.
pub async fn bind_job_hat(
    pool: &PgPool,
    bear_id: BearId,
    job_id: Uuid,
    hat_id: HatId,
) -> Result<(), DenError> {
    let bound = sqlx::query_scalar!(
        r#"UPDATE bear_jobs AS j SET hat_id = $3
           FROM bear_hats AS h
           WHERE j.id = $2 AND j.bear_id = $1
             AND j.lifecycle_intent IS NULL AND j.current_run_id IS NULL
             AND NOT EXISTS (SELECT 1 FROM bear_job_runs r WHERE r.job_id = j.id)
             AND h.id = $3 AND h.bear_id = j.bear_id AND h.work_enabled
             AND (j.hat_id IS NULL OR j.hat_id = $3)
             AND EXISTS (SELECT 1 FROM job_work_surface_assignments a WHERE a.job_id = j.id)
             AND NOT EXISTS (
                 SELECT 1 FROM job_work_surface_assignments a
                 WHERE a.job_id = j.id AND NOT EXISTS (
                     SELECT 1 FROM bear_hat_work_surfaces hs
                     WHERE hs.bear_id = j.bear_id AND hs.hat_id = h.id
                       AND hs.surface_id = a.work_surface_id
                 )
             )
           RETURNING j.hat_id AS "hat_id!: Uuid""#,
        bear_id.as_uuid(),
        job_id,
        hat_id.as_uuid(),
    )
    .fetch_optional(pool)
    .await?;
    if bound.is_none() {
        return Err(DenError::Authorization(
            "unexecuted Job is unavailable or not eligible for this hat's Work surfaces"
                .to_string(),
        ));
    }
    Ok(())
}

/// Re-derive the Job's eligible hat from current grants, not from a stale
/// hat_id snapshot. Call this before a Work run may use hat-scoped resources.
pub async fn eligible_job_hat(
    pool: &PgPool,
    bear_id: BearId,
    job_id: Uuid,
) -> Result<Option<HatId>, DenError> {
    let hat = sqlx::query_scalar!(
        r#"SELECT h.id
           FROM bear_jobs j JOIN bear_hats h ON h.id = j.hat_id AND h.bear_id = j.bear_id
           WHERE j.bear_id = $1 AND j.id = $2 AND h.work_enabled
             AND EXISTS (SELECT 1 FROM job_work_surface_assignments a WHERE a.job_id = j.id)
             AND NOT EXISTS (
                 SELECT 1 FROM job_work_surface_assignments a
                 WHERE a.job_id = j.id AND NOT EXISTS (
                     SELECT 1 FROM bear_hat_work_surfaces hs
                     WHERE hs.bear_id = j.bear_id AND hs.hat_id = h.id
                       AND hs.surface_id = a.work_surface_id
                 )
             )"#,
        bear_id.as_uuid(),
        job_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(hat.map(HatId::new))
}
