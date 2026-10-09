//! Shared order: actor User → Bear → source tasks/Jobs/runs → artifact → links.
use super::super::ArtifactRef;
use den_core::{BearId, DenError, UserId};
use sqlx::{Postgres, Transaction};

pub async fn owner(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    bear: BearId,
) -> Result<(), DenError> {
    sqlx::query!(
        "SELECT id FROM users WHERE id=$1 FOR KEY SHARE",
        actor.get()
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(unavailable)?;
    // Membership changes/deletion use FOR UPDATE; this fences them without blocking
    // the FK KEY SHARE locks of unrelated Job writers.
    sqlx::query!(
        "SELECT id FROM bears WHERE id=$1 FOR NO KEY UPDATE",
        bear.as_uuid()
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(unavailable)?;
    sqlx::query!(
        "SELECT user_id FROM user_bear WHERE user_id=$1 AND bear_id=$2 FOR SHARE",
        actor.get(),
        bear.as_uuid()
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(unavailable)?;
    Ok(())
}
pub async fn bear_lock(tx: &mut Transaction<'_, Postgres>, bear: BearId) -> Result<(), DenError> {
    sqlx::query!(
        "SELECT id FROM bears WHERE id=$1 FOR UPDATE",
        bear.as_uuid()
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(unavailable)?;
    Ok(())
}

/// A copy fences only its associated Jobs; deleting a Bear fences all its sources.
/// NOWAIT avoids waiting into legacy source writers' Task/Job/Work lock inversions.
/// A busy source is a safe retry, never authority to remove its evidence.
pub async fn sources(
    tx: &mut Transaction<'_, Postgres>,
    bear: BearId,
    reference: Option<&ArtifactRef>,
) -> Result<(), DenError> {
    let reference = reference.map(ArtifactRef::as_str);
    sqlx::query!(
        r#"SELECT t.id FROM bear_tasks t WHERE t.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id
        WHERE a.artifact_ref=$2 AND l.target_kind='docket_job' AND l.target_id=t.job_id::text))
        ORDER BY t.id FOR UPDATE OF t NOWAIT"#,
        bear.as_uuid(),
        reference
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(source_error)?;
    sqlx::query!(
        r#"SELECT j.id FROM bear_jobs j WHERE j.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id
        WHERE a.artifact_ref=$2 AND l.target_kind='docket_job' AND l.target_id=j.id::text))
        ORDER BY j.id FOR UPDATE OF j NOWAIT"#,
        bear.as_uuid(),
        reference
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(source_error)?;
    sqlx::query!(
        r#"SELECT r.id FROM bear_job_runs r JOIN bear_jobs j ON j.id=r.job_id
        WHERE j.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id
        WHERE a.artifact_ref=$2 AND l.target_kind='docket_job' AND l.target_id=j.id::text))
        ORDER BY r.id FOR UPDATE OF r NOWAIT"#,
        bear.as_uuid(),
        reference
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(source_error)?;
    // Updates of existing proof rows do not reacquire their parent FKs.
    sqlx::query!(
        r#"SELECT s.run_id,s.task_id FROM bear_task_run_state s JOIN bear_tasks t ON t.id=s.task_id
        WHERE t.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id
        WHERE a.artifact_ref=$2 AND l.target_kind='docket_job' AND l.target_id=t.job_id::text))
        ORDER BY s.run_id,s.task_id FOR UPDATE OF s NOWAIT"#,
        bear.as_uuid(),
        reference
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(source_error)?;
    sqlx::query!(
        r#"SELECT s.run_id,s.criterion_id FROM bear_job_criteria_state s
        JOIN bear_job_criteria c ON c.id=s.criterion_id JOIN bear_jobs j ON j.id=c.job_id
        WHERE j.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id
        WHERE a.artifact_ref=$2 AND l.target_kind='docket_job' AND l.target_id=j.id::text))
        ORDER BY s.run_id,s.criterion_id FOR UPDATE OF s NOWAIT"#,
        bear.as_uuid(),
        reference
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(source_error)?;
    sqlx::query!(
        r#"SELECT r.id FROM bear_work_runs r WHERE r.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id
        WHERE a.artifact_ref=$2 AND l.target_kind='docket_job' AND l.target_id=r.job_id::text))
        ORDER BY r.id FOR UPDATE OF r NOWAIT"#,
        bear.as_uuid(),
        reference
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(source_error)?;
    sqlx::query!(r#"SELECT e.id FROM docket_execution_attempts e WHERE e.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM bear_tasks t JOIN artifact_links l ON l.target_kind='docket_job' AND l.target_id=t.job_id::text
        JOIN artifacts a ON a.id=l.artifact_id WHERE a.artifact_ref=$2 AND t.id=e.task_id))
        ORDER BY e.id FOR UPDATE OF e NOWAIT"#,bear.as_uuid(),reference).fetch_all(&mut **tx).await.map_err(source_error)?;
    sqlx::query!(r#"SELECT c.id FROM docket_turn_claims c WHERE c.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id
        WHERE a.artifact_ref=$2 AND l.target_kind='docket_job' AND l.target_id=c.job_id::text))
        ORDER BY c.id FOR UPDATE OF c NOWAIT"#,bear.as_uuid(),reference).fetch_all(&mut **tx).await.map_err(source_error)?;
    sqlx::query!(r#"SELECT t.id FROM docket_turn_attempts t JOIN docket_routing_decisions d ON d.id=t.routing_decision_id
        WHERE d.bear_id=$1 AND ($2::text IS NULL OR EXISTS (
        SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id
        WHERE a.artifact_ref=$2 AND l.target_kind='docket_job' AND l.target_id=d.job_id::text))
        ORDER BY t.id FOR UPDATE OF t NOWAIT"#,bear.as_uuid(),reference).fetch_all(&mut **tx).await.map_err(source_error)?;
    if reference.is_none() {
        sqlx::query!(
            "SELECT id FROM turn_runs WHERE bear_id=$1 ORDER BY id FOR UPDATE NOWAIT",
            bear.as_uuid()
        )
        .fetch_all(&mut **tx)
        .await
        .map_err(source_error)?;
    }
    Ok(())
}
pub async fn artifact(
    tx: &mut Transaction<'_, Postgres>,
    reference: &ArtifactRef,
) -> Result<(), DenError> {
    let id = sqlx::query_scalar!(
        "SELECT id FROM artifacts WHERE artifact_ref=$1 FOR UPDATE",
        reference.as_str()
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(unavailable)?;
    sqlx::query!(
        "SELECT id FROM artifact_links WHERE artifact_id=$1 ORDER BY id FOR UPDATE NOWAIT",
        id
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(source_error)?;
    Ok(())
}
pub(crate) fn source_error(error: sqlx::Error) -> DenError {
    if matches!(&error,sqlx::Error::Database(cause) if matches!(cause.code().as_deref(),Some("55P03" | "40P01")))
    {
        DenError::ValidationError(
            "Work or evidence is changing. Review again after it settles; no changes were made."
                .into(),
        )
    } else {
        error.into()
    }
}
pub fn unavailable() -> DenError {
    DenError::NotFound("Saved copy unavailable".into())
}
