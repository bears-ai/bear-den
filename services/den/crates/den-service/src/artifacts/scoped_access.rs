//! Cross-surface artifact reads: linking a file never substitutes Cabinet access for its own ACL.

use super::{
    actor_can_read_artifact, artifact_from_row, garage_artifact_storage_key, ArtifactAccessContext,
    ArtifactAccessLevel, ArtifactContentLocation, ArtifactLifecycle, ArtifactMetadata, ArtifactRow,
    ArtifactStorageKind, ArtifactVisibility,
};
use den_core::ids::{BearId, UserId};
use den_core::DenError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ArtifactRef(String);
impl ArtifactRef {
    pub fn parse(value: &str) -> Result<Self, DenError> {
        den_cabinet::validate_artifact_ref(value)
            .map_err(|_| DenError::ValidationError("invalid artifact reference".into()))?;
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for ArtifactRef {
    type Error = DenError;
    fn try_from(value: String) -> Result<Self, DenError> {
        Self::parse(&value)
    }
}
impl From<ArtifactRef> for String {
    fn from(value: ArtifactRef) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ArtifactReader {
    Human(UserId),
    Bear(BearId),
}

pub async fn authorize_for_reader(
    pool: &PgPool,
    reference: &ArtifactRef,
    reader: ArtifactReader,
    level: ArtifactAccessLevel,
) -> Result<ArtifactMetadata, DenError> {
    let row=sqlx::query_as!(ArtifactRow,"SELECT id,artifact_ref,bear_id,created_by_user_id,owner_profile,kind,title,summary,content_type,storage_kind,storage_key,content_bytes,content_sha256,lifecycle,visibility,provenance,metadata,expires_at,finalized_at,deleted_at,created_at,updated_at FROM artifacts WHERE artifact_ref=$1",reference.as_str()).fetch_optional(pool).await?.ok_or_else(||DenError::NotFound("artifact unavailable".into()))?;
    let artifact = artifact_from_row(row)?;
    let allowed = match reader {
        ArtifactReader::Human(user) => {
            crate::bears::db::membership_role_for_user(pool, user.get(), artifact.bear_id)
                .await?
                .is_some()
                && actor_can_read_artifact(
                    &artifact,
                    &ArtifactAccessContext {
                        bear_id: artifact.bear_id,
                        user_id: Some(user.get()),
                    },
                )
        }
        ArtifactReader::Bear(bear) => {
            bear.as_uuid() == artifact.bear_id
                && artifact.visibility == ArtifactVisibility::BearVisible
        }
    };
    if !allowed
        || matches!(
            artifact.lifecycle,
            ArtifactLifecycle::Deleted | ArtifactLifecycle::Expired
        )
    {
        return Err(DenError::NotFound("artifact unavailable".into()));
    }
    if level == ArtifactAccessLevel::Content {
        if artifact.lifecycle != ArtifactLifecycle::Finalized
            || artifact.storage_kind == ArtifactStorageKind::ExternalGitCommit
        {
            return Err(DenError::NotFound(
                "finalized artifact content unavailable".into(),
            ));
        }
        if artifact
            .expires_at
            .is_some_and(|time| time <= OffsetDateTime::now_utc())
        {
            let retained = sqlx::query_scalar!(
                r#"SELECT artifact_has_cabinet_retention($1) AS "retained!""#,
                artifact.id
            )
            .fetch_one(pool)
            .await?;
            if !retained {
                return Err(DenError::NotFound("artifact expired".into()));
            }
        }
    }
    Ok(artifact)
}

pub async fn content_location_for_reader(
    pool: &PgPool,
    reference: &ArtifactRef,
    reader: ArtifactReader,
) -> Result<ArtifactContentLocation, DenError> {
    let artifact =
        authorize_for_reader(pool, reference, reader, ArtifactAccessLevel::Content).await?;
    if artifact.storage_kind != ArtifactStorageKind::GarageArtifacts
        || artifact.storage_key.as_deref()
            != Some(garage_artifact_storage_key(reference.as_str())?.as_str())
    {
        return Err(DenError::NotFound(
            "artifact byte location unavailable".into(),
        ));
    }
    Ok(ArtifactContentLocation {
        artifact_ref: artifact.artifact_ref,
        storage_kind: artifact.storage_kind,
        storage_key: artifact
            .storage_key
            .ok_or_else(|| DenError::NotFound("artifact content unavailable".into()))?,
        content_type: artifact.content_type,
        content_bytes: artifact
            .content_bytes
            .ok_or_else(|| DenError::NotFound("artifact size unavailable".into()))?,
        content_sha256: artifact
            .content_sha256
            .ok_or_else(|| DenError::NotFound("artifact integrity metadata unavailable".into()))?,
    })
}

pub fn verify_content_bytes(
    location: &ArtifactContentLocation,
    bytes: &[u8],
) -> Result<(), DenError> {
    use sha2::{Digest, Sha256};
    if i64::try_from(bytes.len()).ok() != Some(location.content_bytes)
        || format!("{:x}", Sha256::digest(bytes)) != location.content_sha256
    {
        return Err(DenError::System(
            "artifact bytes do not match finalized integrity metadata".into(),
        ));
    }
    Ok(())
}

pub async fn json_content_for_reader(
    pool: &PgPool,
    reference: &ArtifactRef,
    reader: ArtifactReader,
) -> Result<Value, DenError> {
    let artifact =
        authorize_for_reader(pool, reference, reader, ArtifactAccessLevel::Content).await?;
    if artifact.storage_kind != ArtifactStorageKind::DbText {
        return Err(DenError::ValidationError(
            "artifact is not database-backed JSON".into(),
        ));
    }
    let payload = sqlx::query_scalar!(
        "SELECT payload FROM artifact_json_payloads WHERE artifact_id=$1",
        artifact.id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound("artifact payload unavailable".into()))?;
    authorize_for_reader(pool, reference, reader, ArtifactAccessLevel::Content).await?;
    Ok(payload)
}
