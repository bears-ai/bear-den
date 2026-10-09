use super::{ArtifactLifecycle, ArtifactRef, RetirementReceipt};
use den_core::{BearId, DenError, UserId};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct HistoryCursor(pub Uuid);
#[derive(Debug, Serialize)]
pub struct SnapshotSummary {
    pub reference: ArtifactRef,
    pub bear_id: BearId,
    pub bear_name: String,
    pub bear_slug: String,
    pub title: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub retired: bool,
    pub readable: bool,
    pub receipt: Option<RetirementReceipt>,
}
#[derive(Debug, Serialize)]
pub struct SnapshotHistoryPage {
    pub copies: Vec<SnapshotSummary>,
    pub next: Option<HistoryCursor>,
}

/// Owner-only history, including retired receipts; no payload, source-page refs or tool cache.
pub async fn history(
    pool: &PgPool,
    actor: UserId,
    before: Option<HistoryCursor>,
    bear: Option<BearId>,
    job: Option<Uuid>,
) -> Result<SnapshotHistoryPage, DenError> {
    let mut rows = sqlx::query!(r#"
        SELECT a.id,a.artifact_ref,a.bear_id,b.name,b.slug,a.title,a.created_at,a.lifecycle,
            c.retention_released_at,c.retention_released_by_user_id,c.retention_release_reason,
            NOT EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='docket_job'
                AND NOT EXISTS (SELECT 1 FROM bear_jobs j JOIN user_bear access_member ON access_member.bear_id=j.bear_id AND access_member.user_id=$1
                    WHERE j.id::text=l.target_id AND j.bear_id=a.bear_id
                      AND j.visibility IN ('private_to_profile','same_user','bear_visible','handoff_requested')
                      AND (j.created_by_user_id=$1 OR lower(btrim(coalesce(access_member.role,'')))='admin' OR j.visibility='bear_visible'))) AS "job_readable!"
        FROM artifacts a JOIN bears b ON b.id=a.bear_id
        JOIN user_bear ub ON ub.bear_id=a.bear_id AND ub.user_id=$1
        LEFT JOIN LATERAL (SELECT * FROM artifact_links l WHERE l.artifact_id=a.id AND l.target_kind='cabinet_snapshot'
            ORDER BY l.id LIMIT 1) c ON true
        WHERE a.created_by_user_id=$1 AND a.kind='cabinet_document_snapshot' AND a.visibility='same_user'
          AND ($3::uuid IS NULL OR a.bear_id=$3)
          AND ($4::uuid IS NULL OR EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=a.id
              AND l.target_kind='docket_job' AND l.role='source' AND l.target_id=$4::text))
          AND ($2::uuid IS NULL OR (a.created_at,a.id) < (SELECT created_at,id FROM artifacts
              WHERE id=$2 AND created_by_user_id=$1))
        ORDER BY a.created_at DESC,a.id DESC LIMIT 33
    "#, actor.get(), before.map(|cursor| cursor.0), bear.map(|bear| bear.as_uuid()), job).fetch_all(pool).await?;
    let more = rows.len() > 32;
    rows.truncate(32);
    let next = more
        .then(|| rows.last().map(|row| HistoryCursor(row.id)))
        .flatten();
    let copies = rows
        .into_iter()
        .map(|row| {
            let lifecycle = row.lifecycle.parse::<ArtifactLifecycle>()?;
            let receipt = match (
                row.retention_released_at,
                row.retention_released_by_user_id,
                row.retention_release_reason,
            ) {
                (Some(retired_at), Some(actor), Some(reason)) => Some(RetirementReceipt {
                    retired_at,
                    actor: UserId::new(actor),
                    reason,
                }),
                _ => None,
            };
            Ok(SnapshotSummary {
                reference: ArtifactRef::parse(&row.artifact_ref)?,
                bear_id: BearId::new(row.bear_id),
                bear_name: row.name,
                bear_slug: row.slug,
                title: row.title.unwrap_or_else(|| "Saved document".into()),
                created_at: row.created_at,
                retired: lifecycle == ArtifactLifecycle::Deleted && receipt.is_some(),
                readable: row.job_readable
                    && lifecycle == ArtifactLifecycle::Finalized
                    && receipt.is_none(),
                receipt,
            })
        })
        .collect::<Result<Vec<_>, DenError>>()?;
    Ok(SnapshotHistoryPage { copies, next })
}
