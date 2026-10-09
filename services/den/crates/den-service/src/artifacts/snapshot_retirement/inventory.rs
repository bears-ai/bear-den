use super::{
    locks, InventoryFingerprint, RetirementBlocker, RetirementReceipt, SnapshotRetirementPreview,
};
use crate::{
    artifacts::{ArtifactLifecycle, ArtifactRef},
    cabinet::snapshots::DocumentSnapshot,
};
use den_core::{BearId, DenError, UserId};
use sha2::{Digest, Sha256};
use sqlx::{types::Json, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

pub(super) struct SnapshotInventory {
    pub id: Uuid,
    pub citation_id: Option<Uuid>,
    pub reference: ArtifactRef,
    pub bear_id: BearId,
    pub bear_name: String,
    pub bear_slug: String,
    pub title: String,
    pub created_at: OffsetDateTime,
    pub lifecycle: ArtifactLifecycle,
    pub valid: bool,
    pub simple: bool,
    pub job_authorized: bool,
    pub job_readable: bool,
    pub settled: bool,
    pub material: String,
    pub receipt: Option<RetirementReceipt>,
    pub retirement_fingerprint: Option<String>,
}

pub(super) async fn owned_bear(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    reference: &ArtifactRef,
) -> Result<BearId, DenError> {
    sqlx::query_scalar!("SELECT bear_id FROM artifacts WHERE artifact_ref=$1 AND created_by_user_id=$2 AND kind='cabinet_document_snapshot' AND visibility='same_user'", reference.as_str(), actor.get())
        .fetch_optional(&mut **tx).await?.map(BearId::new).ok_or_else(locks::unavailable)
}

pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    reference: &ArtifactRef,
) -> Result<SnapshotInventory, DenError> {
    let row = sqlx::query!(r#"
        SELECT a.id,a.artifact_ref,a.bear_id,b.name,b.slug,a.title,a.created_at,a.lifecycle,
            p.payload AS "payload: Json<DocumentSnapshot>",
            c.id AS "citation_id?",c.retention_released_at,c.retention_released_by_user_id,
            c.retention_release_reason,c.retirement_fingerprint,
            cabinet_snapshot_is_simple(a.id) AS "simple!",
            cabinet_snapshot_job_settled(a.id) AS "settled!",
            NOT EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job'
                AND NOT EXISTS (SELECT 1 FROM bear_jobs j JOIN user_bear ub ON ub.bear_id=j.bear_id AND ub.user_id=$2
                    WHERE j.id::text=l.target_id AND j.bear_id=a.bear_id
                      AND j.visibility IN ('private_to_profile','same_user','bear_visible','handoff_requested')
                      AND (j.created_by_user_id=$2 OR lower(btrim(coalesce(ub.role,'')))='admin'))) AS "job_authorized!",
            NOT EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job'
                AND NOT EXISTS (SELECT 1 FROM bear_jobs j JOIN user_bear ub ON ub.bear_id=j.bear_id AND ub.user_id=$2
                    WHERE j.id::text=l.target_id AND j.bear_id=a.bear_id
                      AND j.visibility IN ('private_to_profile','same_user','bear_visible','handoff_requested')
                      AND (j.created_by_user_id=$2 OR lower(btrim(coalesce(ub.role,'')))='admin' OR j.visibility='bear_visible'))) AS "job_readable!",
            jsonb_build_object('artifact',to_jsonb(a),'links',
                (SELECT jsonb_agg(to_jsonb(l) ORDER BY l.id) FROM artifact_links l WHERE l.artifact_id=a.id),
                'jobs',(SELECT jsonb_agg(to_jsonb(j) ORDER BY j.id) FROM bear_jobs j
                    JOIN artifact_links l ON l.target_kind='docket_job' AND l.target_id=j.id::text WHERE l.artifact_id=a.id),
                'runs',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM bear_job_runs r JOIN bear_jobs j ON j.id=r.job_id
                    WHERE j.bear_id=a.bear_id AND EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job' AND l.target_id=j.id::text)),
                'work',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM bear_work_runs r WHERE r.bear_id=a.bear_id
                    AND EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job' AND l.target_id=r.job_id::text)),
                'attempts',(SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM docket_execution_attempts e JOIN bear_tasks t ON t.id=e.task_id
                    WHERE e.bear_id=a.bear_id AND EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job' AND l.target_id=t.job_id::text)),
                'claims',(SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM docket_turn_claims c WHERE c.bear_id=a.bear_id
                    AND EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job' AND l.target_id=c.job_id::text)),
                'task_states',(SELECT jsonb_agg(to_jsonb(s) ORDER BY s.run_id,s.task_id) FROM bear_task_run_state s JOIN bear_tasks t ON t.id=s.task_id
                    WHERE t.bear_id=a.bear_id AND EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job' AND l.target_id=t.job_id::text)),
                'criterion_states',(SELECT jsonb_agg(to_jsonb(s) ORDER BY s.run_id,s.criterion_id) FROM bear_job_criteria_state s JOIN bear_job_criteria c ON c.id=s.criterion_id
                    WHERE EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job' AND l.target_id=c.job_id::text)),
                'simple',cabinet_snapshot_is_simple(a.id),'settled',cabinet_snapshot_job_settled(a.id))::text AS "material!"
        FROM artifacts a JOIN bears b ON b.id=a.bear_id JOIN user_bear ub ON ub.bear_id=a.bear_id AND ub.user_id=$2
        JOIN artifact_json_payloads p ON p.artifact_id=a.id
        LEFT JOIN LATERAL (SELECT * FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='cabinet_snapshot'
            ORDER BY l.id LIMIT 1) c ON true
        WHERE a.artifact_ref=$1 AND a.created_by_user_id=$2 AND a.kind='cabinet_document_snapshot' AND a.visibility='same_user'
    "#, reference.as_str(), actor.get()).fetch_optional(&mut **tx).await?.ok_or_else(locks::unavailable)?;
    let lifecycle = row.lifecycle.parse::<ArtifactLifecycle>()?;
    let payload = row.payload.0;
    let valid =
        format!("{:x}", Sha256::digest(payload.content.as_bytes())) == payload.content_sha256;
    let receipt = match (
        row.retention_released_at,
        row.retention_released_by_user_id,
        row.retention_release_reason,
    ) {
        (Some(retired_at), Some(user), Some(reason)) => Some(RetirementReceipt {
            retired_at,
            actor: UserId::new(user),
            reason,
        }),
        (None, None, None) => None,
        _ => {
            return Err(DenError::System(
                "Incomplete snapshot retirement receipt".into(),
            ))
        }
    };
    Ok(SnapshotInventory {
        id: row.id,
        citation_id: row.citation_id,
        reference: ArtifactRef::parse(&row.artifact_ref)?,
        bear_id: BearId::new(row.bear_id),
        bear_name: row.name,
        bear_slug: row.slug,
        title: row.title.unwrap_or(payload.title_at_capture),
        created_at: row.created_at,
        lifecycle,
        valid,
        simple: row.simple,
        job_authorized: row.job_authorized,
        job_readable: row.job_readable,
        settled: row.settled,
        material: row.material,
        receipt,
        retirement_fingerprint: row.retirement_fingerprint,
    })
}

pub(super) fn project(row: SnapshotInventory) -> Result<SnapshotRetirementPreview, DenError> {
    let blocker = if !row.valid
        || !matches!(
            row.lifecycle,
            ArtifactLifecycle::Finalized | ArtifactLifecycle::Deleted
        )
        || (row.lifecycle == ArtifactLifecycle::Deleted && row.receipt.is_none())
    {
        Some(RetirementBlocker::InvalidSnapshot)
    } else if !row.simple {
        Some(RetirementBlocker::RequiredReferences)
    } else if !row.job_authorized {
        Some(RetirementBlocker::JobAuthority)
    } else if !row.settled {
        Some(RetirementBlocker::JobNotSettled)
    } else {
        None
    };
    Ok(SnapshotRetirementPreview {
        reference: row.reference,
        bear_id: row.bear_id,
        bear_name: row.bear_name,
        bear_slug: row.bear_slug,
        title: row.title,
        created_at: row.created_at,
        fingerprint: InventoryFingerprint::of(&row.material),
        blocker,
        readable: row.valid
            && row.job_readable
            && row.lifecycle == ArtifactLifecycle::Finalized
            && row.receipt.is_none(),
        receipt: row.receipt,
    })
}
