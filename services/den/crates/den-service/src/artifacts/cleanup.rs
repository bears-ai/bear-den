//! Registry-owned recovery for leased Cabinet uploads, not a bucket-wide sweep.

use den_cabinet::CabinetItemRef;
use den_core::{ids::UserId, DenError};
use serde::Deserialize;
use sqlx::{types::Json, PgPool};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::{garage_artifact_storage_key, ArtifactLifecycle, ArtifactRef};

pub const WRITE_GRACE: Duration = Duration::minutes(20);
pub const RETRY_DELAY: Duration = Duration::minutes(1);

/// Minted only after the row is retired under a lock. No caller-provided key.
pub struct CleanupTicket {
    id: Uuid,
    reference: ArtifactRef,
    storage_key: String,
}
impl CleanupTicket {
    pub fn reference(&self) -> &ArtifactRef {
        &self.reference
    }
    pub fn storage_key(&self) -> &str {
        &self.storage_key
    }
}

pub async fn claim_due(
    pool: &PgPool,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<CleanupTicket>, DenError> {
    claim(pool, now, limit, None, None).await
}

pub async fn claim_owned(
    pool: &PgPool,
    now: OffsetDateTime,
    actor: UserId,
    reference: &ArtifactRef,
) -> Result<CleanupTicket, DenError> {
    claim(pool, now, 1, Some(actor), Some(reference))
        .await?
        .pop()
        .ok_or_else(|| DenError::NotFound("upload cleanup unavailable or not yet eligible".into()))
}

async fn claim(
    pool: &PgPool,
    now: OffsetDateTime,
    limit: i64,
    actor: Option<UserId>,
    reference: Option<&ArtifactRef>,
) -> Result<Vec<CleanupTicket>, DenError> {
    if !(1..=50).contains(&limit) {
        return Err(DenError::ValidationError(
            "cleanup batch must contain 1–50 records".into(),
        ));
    }
    let rows = sqlx::query!(r#"
        WITH due AS (
            SELECT a.id FROM artifacts a
            WHERE a.kind='cabinet_file' AND a.storage_kind='garage_artifacts'
              AND a.content_removed_at IS NULL AND a.expires_at <= $1
              AND a.lifecycle IN ('pending','finalized','expired','deleted')
              AND ($3::text IS NULL OR a.artifact_ref=$3)
              AND ($4::int IS NULL OR (a.created_by_user_id=$4 AND EXISTS (
                  SELECT 1 FROM user_bear ub WHERE ub.user_id=$4 AND ub.bear_id=a.bear_id)))
              AND ($3::text IS NOT NULL OR a.updated_at <= $2)
              AND NOT artifact_has_cabinet_retention(a.id)
            ORDER BY a.updated_at,a.id LIMIT $5 FOR UPDATE OF a SKIP LOCKED
        )
        UPDATE artifacts a SET lifecycle=CASE WHEN a.lifecycle='deleted' THEN 'deleted' ELSE 'expired' END,
            updated_at=$6
        FROM due WHERE a.id=due.id
          AND NOT artifact_has_cabinet_retention(a.id)
        RETURNING a.id,a.artifact_ref,a.storage_key
    "#, now-WRITE_GRACE, now-RETRY_DELAY, reference.map(ArtifactRef::as_str), actor.map(|actor| actor.get()), limit, now)
        .fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            let reference = ArtifactRef::parse(&row.artifact_ref)?;
            let key = garage_artifact_storage_key(reference.as_str())?;
            if row
                .storage_key
                .as_ref()
                .is_some_and(|stored| stored != &key)
            {
                return Err(DenError::ValidationError(
                    "cleanup refused a noncanonical artifact storage key".into(),
                ));
            }
            Ok(CleanupTicket {
                id: row.id,
                reference,
                storage_key: key,
            })
        })
        .collect()
}

/// A crash after DELETE but before this acknowledgement is safe: the next
/// attempt repeats the idempotent DELETE and preserves the registry audit row.
pub async fn acknowledge(
    pool: &PgPool,
    ticket: &CleanupTicket,
    now: OffsetDateTime,
) -> Result<(), DenError> {
    let result = sqlx::query!(
        r#"UPDATE artifacts a SET content_removed_at=COALESCE(content_removed_at,$3),updated_at=$3
        WHERE a.id=$1 AND a.artifact_ref=$2 AND a.kind='cabinet_file'
          AND a.storage_kind='garage_artifacts' AND a.lifecycle IN ('expired','deleted')
          AND NOT artifact_has_cabinet_retention(a.id)"#,
        ticket.id,
        ticket.reference.as_str(),
        now
    )
    .execute(pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(DenError::ValidationError(
            "cleanup acknowledgement no longer applies".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct UploadSource {
    #[serde(default)]
    pub cabinet_ref: Option<CabinetItemRef>,
}

pub struct UploadHistory {
    pub reference: ArtifactRef,
    pub title: Option<String>,
    pub bear_name: String,
    pub lifecycle: ArtifactLifecycle,
    pub expires_at: Option<OffsetDateTime>,
    pub content_removed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub retained: bool,
    pub source: UploadSource,
}

/// Explicit owner-only historical projection; retired content is not readable.
pub async fn history(pool: &PgPool, actor: UserId) -> Result<Vec<UploadHistory>, DenError> {
    let rows = sqlx::query!(r#"SELECT a.artifact_ref,a.title,b.name AS bear_name,a.lifecycle,a.expires_at,
        a.content_removed_at,a.created_at,a.provenance AS "source: Json<UploadSource>",
        artifact_has_cabinet_retention(a.id) AS "retained!"
        FROM artifacts a JOIN bears b ON b.id=a.bear_id
        JOIN user_bear ub ON ub.bear_id=a.bear_id AND ub.user_id=$1
        WHERE a.created_by_user_id=$1 AND a.kind='cabinet_file' AND a.storage_kind='garage_artifacts'
        ORDER BY a.created_at DESC,a.id DESC LIMIT 64"#, actor.get()).fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            Ok(UploadHistory {
                reference: ArtifactRef::parse(&row.artifact_ref)?,
                title: row.title,
                bear_name: row.bear_name,
                lifecycle: row.lifecycle.parse()?,
                expires_at: row.expires_at,
                content_removed_at: row.content_removed_at,
                created_at: row.created_at,
                retained: row.retained,
                source: row.source.0,
            })
        })
        .collect()
}
