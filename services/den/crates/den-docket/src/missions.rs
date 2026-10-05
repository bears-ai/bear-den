//! Docket owns one optional Cabinet-page annotation per Job; Cabinet owns no work state.

use crate::TaskListVisibility;
use den_cabinet::CabinetItemRef;
use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub struct JobReference(pub Uuid);
#[derive(Debug, Serialize)]
pub struct MissionAnnotation {
    pub cabinet_ref: Option<CabinetItemRef>,
    pub revision: i64,
    pub can_edit: bool,
}

fn admission(
    owner: i32,
    visibility: &str,
    admin: bool,
    actor: UserId,
    editing: bool,
) -> Result<bool, DenError> {
    let visibility = TaskListVisibility::parse(visibility)
        .map_err(|_| DenError::NotFound("Job unavailable".into()))?;
    let can_edit = owner == actor.get() || admin;
    if !can_edit && (editing || visibility != TaskListVisibility::BearVisible) {
        return Err(DenError::NotFound("Job unavailable".into()));
    }
    Ok(can_edit)
}

pub async fn get_for_viewer(
    pool: &PgPool,
    bear: BearId,
    job: JobReference,
    actor: UserId,
) -> Result<MissionAnnotation, DenError> {
    let row=sqlx::query!(r#"SELECT j.created_by_user_id,j.visibility,COALESCE(lower(btrim(ub.role))='admin',false) AS "admin!",a.cabinet_ref AS "cabinet_ref?",COALESCE(a.revision,0) AS "revision!" FROM bear_jobs j JOIN user_bear ub ON ub.bear_id=j.bear_id AND ub.user_id=$3 LEFT JOIN job_cabinet_refs a ON a.job_id=j.id WHERE j.id=$1 AND j.bear_id=$2"#,job.0,bear.as_uuid(),actor.get()).fetch_optional(pool).await?.ok_or_else(||DenError::NotFound("Job unavailable".into()))?;
    let can_edit = admission(
        row.created_by_user_id,
        &row.visibility,
        row.admin,
        actor,
        false,
    )?;
    let cabinet_ref = row
        .cabinet_ref
        .as_deref()
        .map(CabinetItemRef::parse)
        .transpose()
        .map_err(|_| DenError::System("stored Job page reference is invalid".into()))?;
    Ok(MissionAnnotation {
        cabinet_ref,
        revision: row.revision,
        can_edit,
    })
}

pub async fn authorize_edit(
    tx: &mut Transaction<'_, Postgres>,
    bear: BearId,
    job: JobReference,
    actor: UserId,
) -> Result<(), DenError> {
    let row=sqlx::query!(r#"SELECT j.created_by_user_id,j.visibility,COALESCE(lower(btrim(ub.role))='admin',false) AS "admin!" FROM bear_jobs j JOIN user_bear ub ON ub.bear_id=j.bear_id AND ub.user_id=$3 WHERE j.id=$1 AND j.bear_id=$2 FOR UPDATE OF j"#,job.0,bear.as_uuid(),actor.get()).fetch_optional(&mut **tx).await?.ok_or_else(||DenError::NotFound("Job unavailable".into()))?;
    admission(
        row.created_by_user_id,
        &row.visibility,
        row.admin,
        actor,
        true,
    )?;
    Ok(())
}

/// Caller must verify the destination page through Cabinet under the shared write fence.
pub async fn set_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    bear: BearId,
    job: JobReference,
    actor: UserId,
    reference: Option<&CabinetItemRef>,
    expected_revision: i64,
) -> Result<(), DenError> {
    authorize_edit(tx, bear, job, actor).await?;
    let current = sqlx::query_scalar!(
        "SELECT revision FROM job_cabinet_refs WHERE job_id=$1",
        job.0
    )
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(0);
    if current != expected_revision {
        return Err(DenError::ValidationError(
            "Job knowledge link changed; reload before saving".into(),
        ));
    }
    sqlx::query!("INSERT INTO job_cabinet_refs(job_id,cabinet_ref,revision) VALUES($1,$2,1) ON CONFLICT(job_id) DO UPDATE SET cabinet_ref=EXCLUDED.cabinet_ref,revision=job_cabinet_refs.revision+1",job.0,reference.map(CabinetItemRef::as_str)).execute(&mut **tx).await?;
    Ok(())
}
