use super::{BearDeletionBlocker, BearDeletionPreview};
use crate::artifacts::snapshot_retirement::InventoryFingerprint;
use den_core::{BearId, DenError};
use sqlx::{Postgres, Transaction};

pub(super) async fn read(
    tx: &mut Transaction<'_, Postgres>,
    bear: BearId,
    confirmed: bool,
) -> Result<BearDeletionPreview, DenError> {
    let row = sqlx::query!(r#"
        SELECT b.name,b.slug,
            EXISTS (SELECT 1 FROM bear_jobs j WHERE j.bear_id=b.id AND NOT docket_job_can_release_private_source(j.id))
            OR EXISTS (SELECT 1 FROM bear_job_runs r JOIN bear_jobs j ON j.id=r.job_id
                WHERE j.bear_id=b.id AND r.state NOT IN ('completed','failed','cancelled'))
            OR EXISTS (SELECT 1 FROM bear_work_runs r WHERE r.bear_id=b.id
                AND (r.state NOT IN ('stalled','succeeded','blocked','failed','cancelled','timed_out') OR r.finished_at IS NULL))
            OR EXISTS (SELECT 1 FROM docket_execution_attempts e WHERE e.bear_id=b.id AND e.state NOT IN ('settled','released'))
            OR EXISTS (SELECT 1 FROM docket_turn_claims c WHERE c.bear_id=b.id AND c.state NOT IN ('settled','abandoned'))
            OR EXISTS (SELECT 1 FROM docket_turn_attempts t JOIN docket_routing_decisions d ON d.id=t.routing_decision_id
                WHERE d.bear_id=b.id AND t.state NOT IN ('settled','abandoned'))
            OR EXISTS (SELECT 1 FROM turn_runs r WHERE r.bear_id=b.id AND r.state NOT IN ('completed','failed','cancelled')) AS "live!",
            EXISTS (SELECT 1 FROM artifacts a JOIN artifact_links l ON l.artifact_id=a.id
                WHERE a.bear_id=b.id AND l.target_kind='cabinet_item' AND l.retention_released_at IS NULL) AS "attachments!",
            EXISTS (SELECT 1 FROM artifacts a JOIN artifact_links l ON l.artifact_id=a.id
                WHERE a.bear_id=b.id AND l.target_kind='cabinet_snapshot' AND l.retention_released_at IS NULL) AS "copies!",
            EXISTS (SELECT 1 FROM artifacts a WHERE a.bear_id=b.id AND a.storage_kind='garage_artifacts'
                AND a.content_removed_at IS NULL) AS "bytes!",
            EXISTS (SELECT 1 FROM artifacts a WHERE a.bear_id=b.id AND (
                artifact_has_required_evidence(a.id)
                OR (a.kind='cabinet_document_snapshot' AND NOT cabinet_snapshot_is_simple(a.id))
                OR (a.kind<>'cabinet_document_snapshot' AND (a.lifecycle='finalized'
                    OR a.visibility='bear_visible' OR EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id)))))
            OR EXISTS (SELECT 1 FROM bear_jobs j WHERE j.bear_id=b.id AND (
                            j.visibility NOT IN ('private_to_profile','same_user') OR docket_job_has_foreign_requirements(j.id)))
                        OR EXISTS (SELECT 1 FROM bear_tasks t JOIN bear_jobs j ON j.id=t.job_id WHERE t.bear_id=b.id AND j.bear_id<>b.id)
                        OR EXISTS (SELECT 1 FROM bear_tasks child JOIN bear_tasks parent ON parent.id=child.parent_task_id
                            WHERE (parent.bear_id=b.id AND child.bear_id<>b.id) OR (child.bear_id=b.id AND parent.bear_id<>b.id))
                        OR EXISTS (SELECT 1 FROM bear_work_runs w JOIN bear_jobs j ON j.id=w.job_id WHERE w.bear_id=b.id AND j.bear_id<>b.id)
                        OR EXISTS (SELECT 1 FROM docket_execution_attempts e JOIN bear_tasks t ON t.id=e.task_id WHERE e.bear_id=b.id AND t.bear_id<>b.id)
            OR EXISTS (SELECT 1 FROM bear_jobs j JOIN bear_job_runs r ON r.id=j.current_run_id WHERE j.bear_id=b.id AND r.job_id<>j.id)
            OR EXISTS (SELECT 1 FROM bear_job_runs r JOIN bear_jobs own ON own.id=r.job_id
                WHERE own.bear_id=b.id AND docket_run_has_foreign_requirements(r.id))
            OR EXISTS (SELECT 1 FROM bear_jobs external JOIN bear_jobs own ON external.supersedes_job_id=own.id
                WHERE own.bear_id=b.id AND external.bear_id<>b.id) AS "required!",
            EXISTS (SELECT 1 FROM artifacts a WHERE a.bear_id=b.id AND
                (a.lifecycle='finalized' OR EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id)))
            OR EXISTS (SELECT 1 FROM bear_jobs j WHERE j.bear_id=b.id) AS "audit!",
            jsonb_build_object('bear',to_jsonb(b),
                'members',(SELECT jsonb_agg(to_jsonb(m) ORDER BY m.user_id) FROM user_bear m WHERE m.bear_id=b.id),
                'artifacts',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM artifacts a WHERE a.bear_id=b.id),
                'links',(SELECT jsonb_agg(to_jsonb(l) ORDER BY l.id) FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE a.bear_id=b.id),
                'jobs',(SELECT jsonb_agg(to_jsonb(j) ORDER BY j.id) FROM bear_jobs j WHERE j.bear_id=b.id),
                'tasks',(SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM bear_tasks t WHERE t.bear_id=b.id),
                'docket_runs',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM bear_job_runs r JOIN bear_jobs j ON j.id=r.job_id WHERE j.bear_id=b.id),
                'work',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM bear_work_runs r WHERE r.bear_id=b.id),
                'attempts',(SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM docket_execution_attempts e WHERE e.bear_id=b.id),
                'claims',(SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM docket_turn_claims c WHERE c.bear_id=b.id),
                'task_states',(SELECT jsonb_agg(to_jsonb(s) ORDER BY s.run_id,s.task_id) FROM bear_task_run_state s JOIN bear_tasks t ON t.id=s.task_id WHERE t.bear_id=b.id),
                'criterion_states',(SELECT jsonb_agg(to_jsonb(s) ORDER BY s.run_id,s.criterion_id) FROM bear_job_criteria_state s JOIN bear_job_criteria c ON c.id=s.criterion_id JOIN bear_jobs j ON j.id=c.job_id WHERE j.bear_id=b.id),
                'turns',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM turn_runs r WHERE r.bear_id=b.id))::text AS "material!"
        FROM bears b WHERE b.id=$1
    "#, bear.as_uuid()).fetch_optional(&mut **tx).await?.ok_or_else(|| DenError::NotFound("Bear unavailable".into()))?;
    let mut blockers = Vec::new();
    if row.live {
        blockers.push(BearDeletionBlocker::LiveWork);
    }
    if row.attachments {
        blockers.push(BearDeletionBlocker::CabinetAttachments);
    }
    if row.copies {
        blockers.push(BearDeletionBlocker::SavedCopies);
    }
    if row.required {
        blockers.push(BearDeletionBlocker::RequiredReferences);
    }
    if row.bytes {
        blockers.push(BearDeletionBlocker::ExternalBytes);
    }
    if !confirmed && row.audit {
        blockers.push(BearDeletionBlocker::UnconfirmedAudit);
    }
    let material = format!("{}:{:?}", row.material, blockers);
    Ok(BearDeletionPreview {
        bear_id: bear,
        name: row.name,
        slug: row.slug,
        fingerprint: InventoryFingerprint::of(&material),
        can_delete: blockers.is_empty(),
        blockers,
    })
}
