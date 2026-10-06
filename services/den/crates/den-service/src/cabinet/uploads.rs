//! Two-phase human file uploads; no storage I/O while Cabinet policy is locked.

use den_cabinet::{
    Actor, ActorScope, AttachmentRole, Authority, CabinetAttachmentRef, CabinetError,
    CabinetItemRef, Lifecycle,
};
use den_core::{
    ids::{BearId, UserId},
    DenError, RuntimeContextLabel,
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};

use super::{attachments, db_error, pages};
use crate::artifacts::{
    self, ArtifactContentLocation, ArtifactRef, ArtifactStorageKind, ArtifactVisibility,
    FinalizeGarageArtifactInput, ReserveArtifactInput,
};

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub enum UploadAudience {
    Private,
    BearAndMembers,
}

pub struct UploadInput<'a> {
    pub bear_id: BearId,
    pub title: String,
    pub content_type: String,
    pub bytes: &'a [u8],
    pub role: AttachmentRole,
    pub audience: UploadAudience,
}

/// Non-deserializable admission receipt, minted only after current access checks.
/// The browser never receives this receipt or a signed write URL.
pub struct PendingUpload {
    actor: UserId,
    bear: BearId,
    page: CabinetItemRef,
    role: AttachmentRole,
    location: ArtifactContentLocation,
}

impl PendingUpload {
    pub fn location(&self) -> &ArtifactContentLocation {
        &self.location
    }
}

async fn lock_membership(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    bear: BearId,
) -> Result<(), CabinetError> {
    let exists = sqlx::query!(
        "SELECT user_id FROM user_bear WHERE user_id=$1 AND bear_id=$2 FOR SHARE",
        actor.get(),
        bear.as_uuid()
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;
    if exists.is_none() {
        return Err(CabinetError::NotAuthorized);
    }
    Ok(())
}

async fn authorize_active_page(
    tx: &mut Transaction<'_, Postgres>,
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
) -> Result<(), CabinetError> {
    pages::authorize(pool, scope, page, Authority::Write).await?;
    let stored = sqlx::query_scalar!(
        "SELECT lifecycle FROM cabinet_items WHERE cabinet_ref=$1",
        page.as_str()
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(db_error)?;
    let lifecycle: Lifecycle = serde_json::from_value(serde_json::json!(stored))
        .map_err(|_| CabinetError::Storage("invalid stored page lifecycle".into()))?;
    if lifecycle != Lifecycle::Active {
        return Err(CabinetError::Policy(
            "file uploads require an active page".into(),
        ));
    }
    Ok(())
}

fn artifact_error(error: DenError) -> CabinetError {
    match error {
        DenError::ValidationError(message) => CabinetError::Policy(message),
        other => CabinetError::Storage(other.to_string()),
    }
}

pub async fn prepare(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
    input: UploadInput<'_>,
) -> Result<PendingUpload, CabinetError> {
    let Actor::User { user_id } = scope.actor else {
        return Err(CabinetError::NotAuthorized);
    };
    if input.bytes.is_empty() || input.bytes.len() > MAX_FILE_BYTES {
        return Err(CabinetError::Policy(
            "choose a non-empty file of at most 16 MiB".into(),
        ));
    }
    if input.title.trim().is_empty()
        || input.title.len() > 255
        || input.title.chars().any(char::is_control)
    {
        return Err(CabinetError::Policy(
            "choose a file with a valid name of at most 255 bytes".into(),
        ));
    }
    if input.content_type.is_empty()
        || input.content_type.len() > 255
        || !input.content_type.is_ascii()
        || input.content_type.bytes().any(|b| b.is_ascii_control())
    {
        return Err(CabinetError::Policy("invalid file content type".into()));
    }
    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    authorize_active_page(&mut tx, pool, scope, page).await?;
    lock_membership(&mut tx, user_id, input.bear_id).await?;
    let artifact = artifacts::reserve_artifact_in_tx(
        &mut tx,
        ReserveArtifactInput {
            bear_id: input.bear_id.as_uuid(),
            created_by_user_id: Some(user_id.get()),
            owner_profile: RuntimeContextLabel::ChannelConversation,
            kind: "cabinet_file".into(),
            title: Some(input.title),
            summary: None,
            content_type: Some(input.content_type.clone()),
            storage_kind: ArtifactStorageKind::GarageArtifacts,
            visibility: match input.audience {
                UploadAudience::Private => ArtifactVisibility::SameUser,
                UploadAudience::BearAndMembers => ArtifactVisibility::BearVisible,
            },
            provenance: serde_json::json!({"cabinet_ref": page, "uploaded_by_user_id": user_id}),
            metadata: serde_json::json!({}),
            expires_at: Some(OffsetDateTime::now_utc() + Duration::hours(24)),
        },
    )
    .await
    .map_err(artifact_error)?;
    let reference = ArtifactRef::parse(&artifact.artifact_ref).map_err(artifact_error)?;
    let location = ArtifactContentLocation {
        storage_key: artifacts::garage_artifact_storage_key(reference.as_str())
            .map_err(artifact_error)?,
        artifact_ref: artifact.artifact_ref,
        storage_kind: ArtifactStorageKind::GarageArtifacts,
        content_type: Some(input.content_type),
        content_bytes: i64::try_from(input.bytes.len()).expect("file size is bounded"),
        content_sha256: format!("{:x}", Sha256::digest(input.bytes)),
    };
    tx.commit().await.map_err(db_error)?;
    Ok(PendingUpload {
        actor: user_id,
        bear: input.bear_id,
        page: page.clone(),
        role: input.role,
        location,
    })
}

/// Caller must verify the stored bytes against the receipt before publication.
pub async fn publish(
    pool: &PgPool,
    scope: &ActorScope,
    pending: &PendingUpload,
) -> Result<CabinetAttachmentRef, CabinetError> {
    if scope.actor
        != (Actor::User {
            user_id: pending.actor,
        })
    {
        return Err(CabinetError::NotAuthorized);
    }
    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    authorize_active_page(&mut tx, pool, scope, &pending.page).await?;
    lock_membership(&mut tx, pending.actor, pending.bear).await?;
    let artifact = artifacts::finalize_garage_artifact_in_tx(
        &mut tx,
        FinalizeGarageArtifactInput {
            artifact_ref: pending.location.artifact_ref.clone(),
            bear_id: pending.bear.as_uuid(),
            content_type: pending
                .location
                .content_type
                .clone()
                .ok_or_else(|| CabinetError::Storage("upload content type missing".into()))?,
            content_bytes: pending.location.content_bytes,
            content_sha256: pending.location.content_sha256.clone(),
            metadata: serde_json::json!({}),
        },
    )
    .await
    .map_err(artifact_error)?;
    let attachment =
        attachments::insert_link_in_tx(&mut tx, scope, &pending.page, artifact.id, pending.role)
            .await?;
    tx.commit().await.map_err(db_error)?;
    Ok(attachment)
}

/// Failed/interrupted requests never leave a readable attachment. Blob removal
/// is best-effort at the storage boundary; the registry preserves its audit row.
pub async fn abandon(pool: &PgPool, pending: &PendingUpload) -> Result<bool, CabinetError> {
    let result = sqlx::query!("UPDATE artifacts SET lifecycle='deleted',deleted_at=NOW(),updated_at=NOW() WHERE artifact_ref=$1 AND created_by_user_id=$2 AND lifecycle='pending'", pending.location.artifact_ref, pending.actor.get())
        .execute(pool).await.map_err(db_error)?;
    Ok(result.rows_affected() == 1)
}
