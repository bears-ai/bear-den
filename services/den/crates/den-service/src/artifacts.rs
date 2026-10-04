//! Artifact registry service.
//!
//! This module owns the small relational registry for durable artifact refs. It
//! does not store blob bytes yet; Phase 1 only reserves refs, finalizes
//! metadata/storage pointers, authorizes reads, and links artifacts to Den
//! objects.
//!
//! Design hint: non-Git external evidence (for example a URL, email message,
//! ticket, or build result) needs one future home. Choose exactly one: model it
//! as an external-reference artifact storage kind, or model it as a distinct
//! Docket work-evidence kind. Do not implement both representations for the
//! same evidence.

use std::{fmt, str::FromStr};

use den_core::{DenError, RuntimeContextLabel};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

const ARTIFACT_REF_PREFIX: &str = "artifact_";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStorageKind {
    DbText,
    GarageArtifacts,
    ExternalGitCommit,
}

impl ArtifactStorageKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::DbText => "db_text",
            Self::GarageArtifacts => "garage_artifacts",
            Self::ExternalGitCommit => "external_git_commit",
        }
    }
}

impl fmt::Display for ArtifactStorageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ArtifactStorageKind {
    type Err = DenError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "db_text" => Ok(Self::DbText),
            "garage_artifacts" => Ok(Self::GarageArtifacts),
            "external_git_commit" => Ok(Self::ExternalGitCommit),
            other => Err(DenError::ValidationError(format!(
                "unknown artifact storage kind: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactLifecycle {
    Pending,
    Finalized,
    Deleted,
    Expired,
}

impl ArtifactLifecycle {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Finalized => "finalized",
            Self::Deleted => "deleted",
            Self::Expired => "expired",
        }
    }
}

impl fmt::Display for ArtifactLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ArtifactLifecycle {
    type Err = DenError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "finalized" => Ok(Self::Finalized),
            "deleted" => Ok(Self::Deleted),
            "expired" => Ok(Self::Expired),
            other => Err(DenError::ValidationError(format!(
                "unknown artifact lifecycle: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactVisibility {
    PrivateToProfile,
    SameUser,
    BearVisible,
    HandoffRequested,
}

impl ArtifactVisibility {
    fn as_str(self) -> &'static str {
        match self {
            Self::PrivateToProfile => "private_to_profile",
            Self::SameUser => "same_user",
            Self::BearVisible => "bear_visible",
            Self::HandoffRequested => "handoff_requested",
        }
    }
}

impl fmt::Display for ArtifactVisibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ArtifactVisibility {
    type Err = DenError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "private_to_profile" => Ok(Self::PrivateToProfile),
            "same_user" => Ok(Self::SameUser),
            "bear_visible" => Ok(Self::BearVisible),
            "handoff_requested" => Ok(Self::HandoffRequested),
            other => Err(DenError::ValidationError(format!(
                "unknown artifact visibility: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReserveArtifactInput {
    pub bear_id: Uuid,
    pub created_by_user_id: Option<i32>,
    pub owner_profile: RuntimeContextLabel,
    pub kind: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content_type: Option<String>,
    pub storage_kind: ArtifactStorageKind,
    pub visibility: ArtifactVisibility,
    pub provenance: Value,
    pub metadata: Value,
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct FinalizeArtifactInput {
    pub artifact_ref: String,
    pub bear_id: Uuid,
    pub storage_key: Option<String>,
    pub content_bytes: Option<i64>,
    pub content_sha256: Option<String>,
    pub metadata: Value,
}

/// Bounded structured content for a finalized `db_text` artifact.
#[derive(Debug, Clone)]
pub struct CreateJsonArtifactInput {
    pub reserve: ReserveArtifactInput,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct FinalizeGarageArtifactInput {
    pub artifact_ref: String,
    pub bear_id: Uuid,
    pub content_type: String,
    pub content_bytes: i64,
    pub content_sha256: String,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct FinalizeGitCommitArtifactInput {
    pub artifact_ref: String,
    pub bear_id: Uuid,
    pub repository: String,
    pub object_format: GitObjectFormat,
    pub commit_oid: String,
    pub output_ref: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitObjectFormat {
    Sha1,
    Sha256,
}

impl GitObjectFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GarageArtifactUploadPath {
    pub storage_key: String,
    pub bucket: String,
    pub object_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactContentLocation {
    pub artifact_ref: String,
    pub storage_kind: ArtifactStorageKind,
    pub storage_key: String,
    pub content_type: Option<String>,
    pub content_bytes: i64,
    pub content_sha256: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactAccessContext {
    pub bear_id: Uuid,
    pub user_id: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactAccessLevel {
    Metadata,
    Content,
}

#[derive(Debug, Clone)]
pub struct AttachArtifactInput {
    pub artifact_ref: String,
    pub bear_id: Uuid,
    pub target_kind: String,
    pub target_id: String,
    pub role: String,
    pub metadata: Value,
    pub created_by_user_id: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocketArtifactTargetKind {
    Job,
    Task,
    Run,
    Criterion,
}

impl DocketArtifactTargetKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Job => "docket_job",
            Self::Task => "docket_task",
            Self::Run => "docket_run",
            Self::Criterion => "docket_criterion",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocketArtifactRole {
    PrimaryOutput,
    Input,
    Source,
    Output,
    Evidence,
    TestReport,
    Diff,
    RuntimeCheckpoint,
    CompletionReceipt,
}

impl DocketArtifactRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryOutput => "primary_output",
            Self::Input => "input",
            Self::Source => "source",
            Self::Output => "output",
            Self::Evidence => "evidence",
            Self::TestReport => "test_report",
            Self::Diff => "diff",
            Self::RuntimeCheckpoint => "runtime_checkpoint",
            Self::CompletionReceipt => "completion_receipt",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttachDocketArtifactInput {
    pub artifact_ref: String,
    pub bear_id: Uuid,
    pub target_kind: DocketArtifactTargetKind,
    pub target_id: Uuid,
    pub role: DocketArtifactRole,
    pub metadata: Value,
    pub created_by_user_id: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct AttachConversationArtifactInput {
    pub artifact_ref: String,
    pub bear_id: Uuid,
    pub conversation_id: String,
    pub role: String,
    pub metadata: Value,
    pub created_by_user_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArtifactCitation {
    pub artifact_ref: String,
    pub kind: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content_type: Option<String>,
    pub content_bytes: Option<i64>,
    pub lifecycle: ArtifactLifecycle,
    pub readable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactMetadata {
    pub id: Uuid,
    pub artifact_ref: String,
    pub bear_id: Uuid,
    pub created_by_user_id: Option<i32>,
    pub owner_profile: RuntimeContextLabel,
    pub kind: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content_type: Option<String>,
    pub storage_kind: ArtifactStorageKind,
    pub storage_key: Option<String>,
    pub content_bytes: Option<i64>,
    pub content_sha256: Option<String>,
    pub lifecycle: ArtifactLifecycle,
    pub visibility: ArtifactVisibility,
    pub provenance: Value,
    pub metadata: Value,
    pub expires_at: Option<OffsetDateTime>,
    pub finalized_at: Option<OffsetDateTime>,
    pub deleted_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactLink {
    pub id: Uuid,
    pub artifact_ref: String,
    pub target_kind: String,
    pub target_id: String,
    pub role: String,
    pub metadata: Value,
    pub created_by_user_id: Option<i32>,
    pub created_at: OffsetDateTime,
}

struct ArtifactRow {
    id: Uuid,
    artifact_ref: String,
    bear_id: Uuid,
    created_by_user_id: Option<i32>,
    owner_profile: String,
    kind: String,
    title: Option<String>,
    summary: Option<String>,
    content_type: Option<String>,
    storage_kind: String,
    storage_key: Option<String>,
    content_bytes: Option<i64>,
    content_sha256: Option<String>,
    lifecycle: String,
    visibility: String,
    provenance: Value,
    metadata: Value,
    expires_at: Option<OffsetDateTime>,
    finalized_at: Option<OffsetDateTime>,
    deleted_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

struct ArtifactLinkRow {
    id: Uuid,
    artifact_ref: String,
    target_kind: String,
    target_id: String,
    role: String,
    metadata: Value,
    created_by_user_id: Option<i32>,
    created_at: OffsetDateTime,
}

pub async fn reserve_artifact(
    pool: &PgPool,
    input: ReserveArtifactInput,
) -> Result<ArtifactMetadata, DenError> {
    validate_non_empty("artifact kind", &input.kind)?;
    validate_json_object("provenance", &input.provenance)?;
    validate_json_object("metadata", &input.metadata)?;

    let artifact_ref = new_artifact_ref();
    let row = sqlx::query_as!(
        ArtifactRow,
        "INSERT INTO artifacts (
            artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, visibility, provenance,
            metadata, expires_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         RETURNING
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at",
        artifact_ref,
        input.bear_id,
        input.created_by_user_id,
        input.owner_profile.as_str(),
        input.kind,
        input.title,
        input.summary,
        input.content_type,
        input.storage_kind.as_str(),
        input.visibility.as_str(),
        input.provenance,
        input.metadata,
        input.expires_at,
    )
    .fetch_one(pool)
    .await?;

    artifact_from_row(row)
}

pub async fn create_json_artifact(
    pool: &PgPool,
    input: CreateJsonArtifactInput,
) -> Result<ArtifactMetadata, DenError> {
    let mut tx = pool.begin().await?;
    let artifact = create_json_artifact_in_tx(&mut tx, input).await?;
    tx.commit().await?;
    Ok(artifact)
}

pub async fn create_json_artifact_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: CreateJsonArtifactInput,
) -> Result<ArtifactMetadata, DenError> {
    if !input.payload.is_object() && !input.payload.is_array() {
        return Err(DenError::ValidationError(
            "artifact JSON payload must be an object or array".to_string(),
        ));
    }
    let payload_bytes = serde_json::to_vec(&input.payload)
        .map_err(|err| DenError::System(format!("serialize artifact JSON payload: {err}")))?;
    if payload_bytes.len() > 262_144 {
        return Err(DenError::ValidationError(
            "artifact JSON payload exceeds 262144 bytes".to_string(),
        ));
    }
    if input.reserve.storage_kind != ArtifactStorageKind::DbText {
        return Err(DenError::ValidationError(
            "JSON artifact payloads require db_text storage".to_string(),
        ));
    }

    validate_non_empty("artifact kind", &input.reserve.kind)?;
    validate_json_object("provenance", &input.reserve.provenance)?;
    validate_json_object("metadata", &input.reserve.metadata)?;
    let artifact_ref = new_artifact_ref();
    let artifact = sqlx::query_as!(
        ArtifactRow,
        "INSERT INTO artifacts (
            artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, visibility, provenance,
            metadata, expires_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'db_text', $9, $10, $11, $12)
         RETURNING
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at",
        artifact_ref,
        input.reserve.bear_id,
        input.reserve.created_by_user_id,
        input.reserve.owner_profile.as_str(),
        input.reserve.kind,
        input.reserve.title,
        input.reserve.summary,
        input.reserve.content_type,
        input.reserve.visibility.as_str(),
        input.reserve.provenance,
        input.reserve.metadata,
        input.reserve.expires_at,
    )
    .fetch_one(&mut **tx)
    .await?;
    let row = sqlx::query_as!(
        ArtifactRow,
        "UPDATE artifacts
         SET lifecycle = 'finalized', content_bytes = $3, metadata = metadata,
             finalized_at = NOW(), updated_at = NOW()
         WHERE id = $1 AND bear_id = $2 AND lifecycle = 'pending'
         RETURNING
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at",
        artifact.id,
        artifact.bear_id,
        i64::try_from(payload_bytes.len()).expect("payload size is bounded"),
    )
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO artifact_json_payloads (artifact_id, payload) VALUES ($1, $2)")
        .bind(artifact.id)
        .bind(input.payload)
        .execute(&mut **tx)
        .await?;
    artifact_from_row(row)
}

pub async fn finalize_metadata_only_artifact(
    pool: &PgPool,
    input: FinalizeArtifactInput,
) -> Result<ArtifactMetadata, DenError> {
    validate_json_object("metadata", &input.metadata)?;
    if let Some(storage_key) = &input.storage_key {
        validate_non_empty("storage key", storage_key)?;
    }
    if let Some(bytes) = input.content_bytes {
        if bytes < 0 {
            return Err(DenError::ValidationError(
                "content_bytes must be non-negative".to_string(),
            ));
        }
    }
    if let Some(hash) = &input.content_sha256 {
        validate_sha256(hash)?;
    }

    let row = sqlx::query_as!(
        ArtifactRow,
        "UPDATE artifacts
         SET lifecycle = 'finalized', storage_key = $3, content_bytes = $4,
             content_sha256 = $5, metadata = $6, finalized_at = NOW(), updated_at = NOW()
         WHERE artifact_ref = $1
           AND bear_id = $2
           AND lifecycle = 'pending'
           AND storage_kind = 'db_text'
         RETURNING
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at",
        input.artifact_ref,
        input.bear_id,
        input.storage_key,
        input.content_bytes,
        input.content_sha256,
        input.metadata,
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => artifact_from_row(row),
        None => Err(DenError::ValidationError(format!(
            "artifact {} is not pending or does not exist",
            input.artifact_ref
        ))),
    }
}

pub async fn get_garage_artifact_upload_path(
    pool: &PgPool,
    bear_id: Uuid,
    artifact_ref: &str,
    bucket: &str,
) -> Result<GarageArtifactUploadPath, DenError> {
    validate_non_empty("bucket", bucket)?;
    let artifact = get_artifact_metadata(pool, bear_id, artifact_ref).await?;
    if artifact.storage_kind != ArtifactStorageKind::GarageArtifacts {
        return Err(DenError::ValidationError(format!(
            "artifact {artifact_ref} is not garage-backed"
        )));
    }
    if artifact.lifecycle != ArtifactLifecycle::Pending {
        return Err(DenError::ValidationError(format!(
            "artifact {artifact_ref} is not pending"
        )));
    }

    let storage_key = garage_artifact_storage_key(artifact_ref)?;
    Ok(GarageArtifactUploadPath {
        object_path: format!("/{bucket}/{storage_key}"),
        bucket: bucket.to_string(),
        storage_key,
    })
}

pub async fn finalize_garage_artifact(
    pool: &PgPool,
    input: FinalizeGarageArtifactInput,
) -> Result<ArtifactMetadata, DenError> {
    validate_non_empty("content type", &input.content_type)?;
    validate_json_object("metadata", &input.metadata)?;
    if input.content_bytes < 0 {
        return Err(DenError::ValidationError(
            "content_bytes must be non-negative".to_string(),
        ));
    }
    validate_sha256(&input.content_sha256)?;

    let storage_key = garage_artifact_storage_key(&input.artifact_ref)?;
    let row = sqlx::query_as!(
        ArtifactRow,
        "UPDATE artifacts
         SET lifecycle = 'finalized', storage_key = $3, content_type = $4,
             content_bytes = $5, content_sha256 = $6, metadata = $7,
             finalized_at = NOW(), updated_at = NOW()
         WHERE artifact_ref = $1
            AND bear_id = $2
            AND lifecycle = 'pending'
            AND storage_kind = 'garage_artifacts'
         RETURNING
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at",
        &input.artifact_ref,
        input.bear_id,
        storage_key,
        input.content_type,
        input.content_bytes,
        input.content_sha256,
        input.metadata,
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => artifact_from_row(row),
        None => Err(DenError::ValidationError(format!(
            "artifact {} is not pending garage-backed content or does not exist",
            input.artifact_ref
        ))),
    }
}

pub async fn finalize_git_commit_artifact(
    pool: &PgPool,
    input: FinalizeGitCommitArtifactInput,
) -> Result<ArtifactMetadata, DenError> {
    validate_non_empty("repository", &input.repository)?;
    validate_non_empty("output ref", &input.output_ref)?;
    validate_git_commit_oid(&input.commit_oid, input.object_format)?;
    validate_json_object("metadata", &input.metadata)?;

    let metadata = merge_json_objects(
        input.metadata,
        serde_json::json!({
            "git": {
                "repository": input.repository,
                "object_format": input.object_format.as_str(),
                "commit_oid": input.commit_oid,
                "output_ref": input.output_ref,
            }
        }),
    )?;
    let row = sqlx::query_as!(
        ArtifactRow,
        "UPDATE artifacts
         SET lifecycle = 'finalized', metadata = $3, finalized_at = NOW(), updated_at = NOW()
         WHERE artifact_ref = $1
            AND bear_id = $2
            AND lifecycle = 'pending'
            AND storage_kind = 'external_git_commit'
         RETURNING
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at",
        &input.artifact_ref,
        input.bear_id,
        metadata,
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => artifact_from_row(row),
        None => Err(DenError::ValidationError(format!(
            "artifact {} is not pending external Git commit or does not exist",
            input.artifact_ref
        ))),
    }
}

pub async fn get_artifact_content_location(
    pool: &PgPool,
    artifact_ref: &str,
    context: ArtifactAccessContext,
) -> Result<ArtifactContentLocation, DenError> {
    let artifact =
        authorize_artifact_access(pool, artifact_ref, context, ArtifactAccessLevel::Content)
            .await?;

    Ok(ArtifactContentLocation {
        artifact_ref: artifact.artifact_ref,
        storage_kind: artifact.storage_kind,
        storage_key: artifact.storage_key.ok_or_else(|| {
            DenError::ValidationError(format!("artifact {artifact_ref} has no storage key"))
        })?,
        content_type: artifact.content_type,
        content_bytes: artifact.content_bytes.ok_or_else(|| {
            DenError::ValidationError(format!("artifact {artifact_ref} has no content size"))
        })?,
        content_sha256: artifact.content_sha256.ok_or_else(|| {
            DenError::ValidationError(format!("artifact {artifact_ref} has no content sha256"))
        })?,
    })
}

pub async fn get_artifact_metadata(
    pool: &PgPool,
    bear_id: Uuid,
    artifact_ref: &str,
) -> Result<ArtifactMetadata, DenError> {
    let row = sqlx::query_as!(
        ArtifactRow,
        "SELECT
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at
         FROM artifacts
         WHERE bear_id = $1 AND artifact_ref = $2",
        bear_id,
        artifact_ref,
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => artifact_from_row(row),
        None => Err(DenError::NotFound(format!(
            "artifact not found: {artifact_ref}"
        ))),
    }
}

pub async fn authorize_artifact_access(
    pool: &PgPool,
    artifact_ref: &str,
    context: ArtifactAccessContext,
    access_level: ArtifactAccessLevel,
) -> Result<ArtifactMetadata, DenError> {
    let artifact = get_artifact_metadata(pool, context.bear_id, artifact_ref).await?;

    if access_level == ArtifactAccessLevel::Content {
        match artifact.lifecycle {
            ArtifactLifecycle::Finalized => {}
            ArtifactLifecycle::Pending => {
                return Err(DenError::Authorization(format!(
                    "artifact {artifact_ref} is pending"
                )))
            }
            ArtifactLifecycle::Deleted | ArtifactLifecycle::Expired => {
                return Err(DenError::Authorization(format!(
                    "artifact {artifact_ref} is not readable"
                )))
            }
        }
        if artifact.storage_kind == ArtifactStorageKind::ExternalGitCommit {
            return Err(DenError::ValidationError(format!(
                "artifact {artifact_ref} is an external Git commit and has no Den content location"
            )));
        }
    } else if matches!(artifact.lifecycle, ArtifactLifecycle::Deleted) {
        return Err(DenError::Authorization(format!(
            "artifact {artifact_ref} is deleted"
        )));
    }

    if actor_can_read_artifact(&artifact, &context) {
        Ok(artifact)
    } else {
        Err(DenError::Authorization(format!(
            "artifact {artifact_ref} is not visible to this context"
        )))
    }
}

pub async fn mark_artifact_deleted(
    pool: &PgPool,
    bear_id: Uuid,
    artifact_ref: &str,
) -> Result<ArtifactMetadata, DenError> {
    let row = sqlx::query_as!(
        ArtifactRow,
        "UPDATE artifacts
         SET lifecycle = 'deleted', deleted_at = NOW(), updated_at = NOW()
         WHERE bear_id = $1 AND artifact_ref = $2 AND lifecycle <> 'deleted'
         RETURNING
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at",
        bear_id,
        artifact_ref,
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => artifact_from_row(row),
        None => Err(DenError::NotFound(format!(
            "artifact not found or already deleted: {artifact_ref}"
        ))),
    }
}

pub async fn attach_artifact(
    pool: &PgPool,
    input: AttachArtifactInput,
) -> Result<ArtifactLink, DenError> {
    let mut tx = pool.begin().await?;
    let link = attach_artifact_in_tx(&mut tx, input).await?;
    tx.commit().await?;
    Ok(link)
}

pub async fn attach_artifact_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: AttachArtifactInput,
) -> Result<ArtifactLink, DenError> {
    validate_non_empty("target kind", &input.target_kind)?;
    validate_non_empty("target id", &input.target_id)?;
    validate_non_empty("artifact link role", &input.role)?;
    validate_json_object("metadata", &input.metadata)?;

    let row = sqlx::query_as!(
        ArtifactLinkRow,
        "WITH artifact AS (
            SELECT id, artifact_ref FROM artifacts
            WHERE bear_id = $1 AND artifact_ref = $2 AND lifecycle <> 'deleted'
         )
         INSERT INTO artifact_links (
            artifact_id, target_kind, target_id, role, metadata, created_by_user_id
         )
         SELECT id, $3, $4, $5, $6, $7 FROM artifact
         ON CONFLICT (artifact_id, target_kind, target_id, role)
         DO UPDATE SET metadata = EXCLUDED.metadata
         RETURNING id,
            (SELECT artifact_ref FROM artifact) AS \"artifact_ref!\",
            target_kind, target_id, role, metadata, created_by_user_id, created_at",
        input.bear_id,
        &input.artifact_ref,
        input.target_kind,
        input.target_id,
        input.role,
        input.metadata,
        input.created_by_user_id,
    )
    .fetch_optional(&mut **tx)
    .await?;

    match row {
        Some(row) => link_from_row(row),
        None => Err(DenError::NotFound(format!(
            "artifact not found or deleted: {}",
            input.artifact_ref
        ))),
    }
}

pub async fn list_artifact_links(
    pool: &PgPool,
    bear_id: Uuid,
    target_kind: &str,
    target_id: &str,
) -> Result<Vec<ArtifactLink>, DenError> {
    let rows = sqlx::query_as!(
        ArtifactLinkRow,
        "SELECT artifact_links.id, artifacts.artifact_ref, artifact_links.target_kind,
            artifact_links.target_id, artifact_links.role, artifact_links.metadata,
            artifact_links.created_by_user_id, artifact_links.created_at
         FROM artifact_links
         JOIN artifacts ON artifacts.id = artifact_links.artifact_id
         WHERE artifacts.bear_id = $1
            AND artifact_links.target_kind = $2
            AND artifact_links.target_id = $3
            AND artifacts.lifecycle <> 'deleted'
         ORDER BY artifact_links.created_at DESC",
        bear_id,
        target_kind,
        target_id,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(link_from_row).collect()
}

pub async fn attach_docket_artifact(
    pool: &PgPool,
    input: AttachDocketArtifactInput,
) -> Result<ArtifactLink, DenError> {
    attach_artifact(
        pool,
        AttachArtifactInput {
            artifact_ref: input.artifact_ref,
            bear_id: input.bear_id,
            target_kind: input.target_kind.as_str().to_string(),
            target_id: input.target_id.to_string(),
            role: input.role.as_str().to_string(),
            metadata: input.metadata,
            created_by_user_id: input.created_by_user_id,
        },
    )
    .await
}

/// Attaches an artifact to a durable conversation without exposing the registry target spelling.
pub async fn attach_conversation_artifact(
    pool: &PgPool,
    input: AttachConversationArtifactInput,
) -> Result<ArtifactLink, DenError> {
    attach_artifact(
        pool,
        AttachArtifactInput {
            artifact_ref: input.artifact_ref,
            bear_id: input.bear_id,
            target_kind: "conversation".to_string(),
            target_id: input.conversation_id,
            role: input.role,
            metadata: input.metadata,
            created_by_user_id: input.created_by_user_id,
        },
    )
    .await
}

pub async fn list_artifact_citations(
    pool: &PgPool,
    bear_id: Uuid,
    target_kind: &str,
    target_id: &str,
    context: ArtifactAccessContext,
) -> Result<Vec<ArtifactCitation>, DenError> {
    validate_non_empty("target kind", target_kind)?;
    validate_non_empty("target id", target_id)?;
    if context.bear_id != bear_id {
        return Err(DenError::Authorization(
            "artifact citation context bear does not match target bear".to_string(),
        ));
    }

    let rows = sqlx::query_as!(
        ArtifactRow,
        "SELECT
            artifacts.id, artifacts.artifact_ref, artifacts.bear_id,
            artifacts.created_by_user_id, artifacts.owner_profile, artifacts.kind,
            artifacts.title, artifacts.summary, artifacts.content_type, artifacts.storage_kind,
            artifacts.storage_key, artifacts.content_bytes, artifacts.content_sha256,
            artifacts.lifecycle, artifacts.visibility, artifacts.provenance, artifacts.metadata,
            artifacts.expires_at, artifacts.finalized_at, artifacts.deleted_at,
            artifacts.created_at, artifacts.updated_at
         FROM artifact_links
         JOIN artifacts ON artifacts.id = artifact_links.artifact_id
         WHERE artifacts.bear_id = $1
            AND artifact_links.target_kind = $2
            AND artifact_links.target_id = $3
            AND artifacts.lifecycle <> 'deleted'
         ORDER BY artifact_links.created_at DESC",
        bear_id,
        target_kind,
        target_id,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            artifact_from_row(row).map(|artifact| citation_from_artifact(&artifact, &context))
        })
        .collect()
}

pub async fn list_docket_artifact_citations(
    pool: &PgPool,
    bear_id: Uuid,
    target_kind: DocketArtifactTargetKind,
    target_id: Uuid,
    context: ArtifactAccessContext,
) -> Result<Vec<ArtifactCitation>, DenError> {
    list_artifact_citations(
        pool,
        bear_id,
        target_kind.as_str(),
        &target_id.to_string(),
        context,
    )
    .await
}

/// Lists citations linked to a durable conversation identifier.
///
/// Keep the target kind here rather than making callers repeat the persistence
/// spelling. Conversation links remain registry-backed and access-filtered.
pub async fn list_conversation_artifact_citations(
    pool: &PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    context: ArtifactAccessContext,
) -> Result<Vec<ArtifactCitation>, DenError> {
    list_artifact_citations(pool, bear_id, "conversation", conversation_id, context).await
}

pub async fn list_expired_artifact_gc_candidates(
    pool: &PgPool,
    bear_id: Uuid,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<ArtifactMetadata>, DenError> {
    if limit <= 0 {
        return Err(DenError::ValidationError(
            "limit must be positive".to_string(),
        ));
    }

    let rows = sqlx::query_as!(
        ArtifactRow,
        "SELECT
            id, artifact_ref, bear_id, created_by_user_id, owner_profile, kind,
            title, summary, content_type, storage_kind, storage_key, content_bytes,
            content_sha256, lifecycle, visibility, provenance, metadata, expires_at,
            finalized_at, deleted_at, created_at, updated_at
         FROM artifacts
         WHERE bear_id = $1
            AND expires_at IS NOT NULL
            AND expires_at <= $2
            AND lifecycle IN ('finalized', 'expired')
         ORDER BY expires_at ASC, created_at ASC
         LIMIT $3",
        bear_id,
        now,
        limit,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(artifact_from_row).collect()
}

fn new_artifact_ref() -> String {
    format!("{ARTIFACT_REF_PREFIX}{}", Uuid::new_v4().simple())
}

pub fn garage_artifact_storage_key(artifact_ref: &str) -> Result<String, DenError> {
    let suffix = artifact_ref
        .strip_prefix(ARTIFACT_REF_PREFIX)
        .filter(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
        .ok_or_else(|| {
            DenError::ValidationError(format!("invalid artifact ref: {artifact_ref}"))
        })?;

    // ponytail: one-level sharding by ref prefix is enough for the initial bucket layout;
    // upgrade to time/bear partitioning if object listings become hot.
    Ok(format!("artifacts/{}/{artifact_ref}", &suffix[..2]))
}

#[cfg(test)]
#[path = "artifacts/access_tests.rs"]
mod access_tests;

fn actor_can_read_artifact(artifact: &ArtifactMetadata, context: &ArtifactAccessContext) -> bool {
    let Some(user_id) = context.user_id else {
        return false;
    };
    match artifact.visibility {
        ArtifactVisibility::PrivateToProfile
        | ArtifactVisibility::SameUser
        | ArtifactVisibility::HandoffRequested => artifact.created_by_user_id == Some(user_id),
        ArtifactVisibility::BearVisible => true,
    }
}

fn citation_from_artifact(
    artifact: &ArtifactMetadata,
    context: &ArtifactAccessContext,
) -> ArtifactCitation {
    let metadata_readable = actor_can_read_artifact(artifact, context);
    ArtifactCitation {
        artifact_ref: artifact.artifact_ref.clone(),
        kind: if metadata_readable {
            artifact.kind.clone()
        } else {
            "unavailable".to_string()
        },
        title: metadata_readable.then(|| artifact.title.clone()).flatten(),
        summary: metadata_readable
            .then(|| artifact.summary.clone())
            .flatten(),
        content_type: metadata_readable
            .then(|| artifact.content_type.clone())
            .flatten(),
        content_bytes: metadata_readable
            .then_some(artifact.content_bytes)
            .flatten(),
        lifecycle: artifact.lifecycle,
        readable: metadata_readable && artifact.lifecycle == ArtifactLifecycle::Finalized,
    }
}

fn artifact_from_row(row: ArtifactRow) -> Result<ArtifactMetadata, DenError> {
    Ok(ArtifactMetadata {
        id: row.id,
        artifact_ref: row.artifact_ref,
        bear_id: row.bear_id,
        created_by_user_id: row.created_by_user_id,
        owner_profile: row
            .owner_profile
            .parse()
            .map_err(|err: String| DenError::ValidationError(err))?,
        kind: row.kind,
        title: row.title,
        summary: row.summary,
        content_type: row.content_type,
        storage_kind: row.storage_kind.parse()?,
        storage_key: row.storage_key,
        content_bytes: row.content_bytes,
        content_sha256: row.content_sha256,
        lifecycle: row.lifecycle.parse()?,
        visibility: row.visibility.parse()?,
        provenance: row.provenance,
        metadata: row.metadata,
        expires_at: row.expires_at,
        finalized_at: row.finalized_at,
        deleted_at: row.deleted_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn link_from_row(row: ArtifactLinkRow) -> Result<ArtifactLink, DenError> {
    Ok(ArtifactLink {
        id: row.id,
        artifact_ref: row.artifact_ref,
        target_kind: row.target_kind,
        target_id: row.target_id,
        role: row.role,
        metadata: row.metadata,
        created_by_user_id: row.created_by_user_id,
        created_at: row.created_at,
    })
}

fn validate_non_empty(name: &str, value: &str) -> Result<(), DenError> {
    if value.trim().is_empty() {
        Err(DenError::ValidationError(format!(
            "{name} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn validate_json_object(name: &str, value: &Value) -> Result<(), DenError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(DenError::ValidationError(format!(
            "{name} must be a JSON object"
        )))
    }
}

fn validate_git_commit_oid(oid: &str, object_format: GitObjectFormat) -> Result<(), DenError> {
    let expected_length = match object_format {
        GitObjectFormat::Sha1 => 40,
        GitObjectFormat::Sha256 => 64,
    };
    if oid.len() == expected_length
        && oid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(DenError::ValidationError(format!(
            "Git {} commit OID must be {expected_length} lowercase hex characters",
            object_format.as_str()
        )))
    }
}

fn merge_json_objects(left: Value, right: Value) -> Result<Value, DenError> {
    let mut left = left
        .as_object()
        .cloned()
        .ok_or_else(|| DenError::ValidationError("metadata must be a JSON object".to_string()))?;
    let right = right
        .as_object()
        .ok_or_else(|| DenError::ValidationError("metadata must be a JSON object".to_string()))?;
    left.extend(right.clone());
    Ok(Value::Object(left))
}

fn validate_sha256(hash: &str) -> Result<(), DenError> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(DenError::ValidationError(
            "content_sha256 must be 64 hex characters".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn test_pool() -> Option<PgPool> {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .ok()?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .ok()?;
        sqlx::migrate!("../../migrations").run(&pool).await.ok()?;
        Some(pool)
    }

    async fn create_user(pool: &PgPool) -> i32 {
        let suffix = Uuid::new_v4().simple().to_string();
        let username = format!("artifactuser{}", &suffix[..12]);
        sqlx::query_scalar!(
            "INSERT INTO users (email, username, display_name, passhash)
             VALUES ($1, $2, $3, 'x') RETURNING id",
            format!("{username}@example.invalid"),
            &username,
            &username,
        )
        .fetch_one(pool)
        .await
        .expect("create user")
    }

    async fn create_bear(pool: &PgPool) -> Uuid {
        let suffix = Uuid::new_v4().simple().to_string();
        sqlx::query_scalar!(
            "INSERT INTO bears (slug, name) VALUES ($1, $2) RETURNING id",
            format!("artifact-bear-{}", &suffix[..12]),
            "Artifact Test Bear",
        )
        .fetch_one(pool)
        .await
        .expect("create bear")
    }

    fn reserve_input(bear_id: Uuid, user_id: i32) -> ReserveArtifactInput {
        ReserveArtifactInput {
            bear_id,
            created_by_user_id: Some(user_id),
            owner_profile: RuntimeContextLabel::ArmatureConversation,
            kind: "tool_output".to_string(),
            title: Some("sample".to_string()),
            summary: None,
            content_type: Some("text/plain".to_string()),
            storage_kind: ArtifactStorageKind::DbText,
            visibility: ArtifactVisibility::SameUser,
            provenance: json!({"test": true}),
            metadata: json!({"phase": 1}),
            expires_at: None,
        }
    }

    fn garage_reserve_input(
        bear_id: Uuid,
        user_id: i32,
        expires_at: Option<OffsetDateTime>,
    ) -> ReserveArtifactInput {
        ReserveArtifactInput {
            bear_id,
            created_by_user_id: Some(user_id),
            owner_profile: RuntimeContextLabel::ArmatureConversation,
            kind: "report".to_string(),
            title: Some("garage sample".to_string()),
            summary: None,
            content_type: None,
            storage_kind: ArtifactStorageKind::GarageArtifacts,
            visibility: ArtifactVisibility::SameUser,
            provenance: json!({"test": true}),
            metadata: json!({"phase": 2}),
            expires_at,
        }
    }

    #[test]
    fn git_commit_oid_validation_accepts_declared_object_format_only() {
        assert!(validate_git_commit_oid(&"a".repeat(40), GitObjectFormat::Sha1).is_ok());
        assert!(validate_git_commit_oid(&"b".repeat(64), GitObjectFormat::Sha256).is_ok());
        assert!(validate_git_commit_oid(&"a".repeat(64), GitObjectFormat::Sha1).is_err());
        assert!(validate_git_commit_oid(&"A".repeat(40), GitObjectFormat::Sha1).is_err());
    }

    #[tokio::test]
    async fn artifact_registry_lifecycle_and_links() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = create_user(&pool).await;
        let other_user_id = create_user(&pool).await;
        let bear_id = create_bear(&pool).await;

        let reserved = reserve_artifact(&pool, reserve_input(bear_id, user_id))
            .await
            .expect("reserve artifact");
        assert!(reserved.artifact_ref.starts_with(ARTIFACT_REF_PREFIX));
        assert_eq!(reserved.lifecycle, ArtifactLifecycle::Pending);

        let pending_content = authorize_artifact_access(
            &pool,
            &reserved.artifact_ref,
            ArtifactAccessContext {
                bear_id,
                user_id: Some(user_id),
            },
            ArtifactAccessLevel::Content,
        )
        .await
        .unwrap_err();
        assert!(pending_content.to_string().contains("pending"));

        let finalized = finalize_metadata_only_artifact(
            &pool,
            FinalizeArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                storage_key: Some("db-text-placeholder".to_string()),
                content_bytes: Some(12),
                content_sha256: Some(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
                ),
                metadata: json!({"phase": 1, "final": true}),
            },
        )
        .await
        .expect("finalize artifact");
        assert_eq!(finalized.lifecycle, ArtifactLifecycle::Finalized);

        let double_finalize = finalize_metadata_only_artifact(
            &pool,
            FinalizeArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                storage_key: None,
                content_bytes: None,
                content_sha256: None,
                metadata: json!({}),
            },
        )
        .await
        .unwrap_err();
        assert!(double_finalize.to_string().contains("not pending"));

        let fetched = get_artifact_metadata(&pool, bear_id, &reserved.artifact_ref)
            .await
            .expect("fetch metadata");
        assert_eq!(fetched.artifact_ref, reserved.artifact_ref);
        assert_eq!(fetched.metadata["final"], true);

        let other_user_access = authorize_artifact_access(
            &pool,
            &reserved.artifact_ref,
            ArtifactAccessContext {
                bear_id,
                user_id: Some(other_user_id),
            },
            ArtifactAccessLevel::Metadata,
        )
        .await
        .unwrap_err();
        assert!(other_user_access.to_string().contains("not visible"));

        let link = attach_artifact(
            &pool,
            AttachArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                target_kind: "bear_task".to_string(),
                target_id: "task-1".to_string(),
                role: "result".to_string(),
                metadata: json!({"slot": "primary"}),
                created_by_user_id: Some(user_id),
            },
        )
        .await
        .expect("attach artifact");
        assert_eq!(link.artifact_ref, reserved.artifact_ref);

        let links = list_artifact_links(&pool, bear_id, "bear_task", "task-1")
            .await
            .expect("list links");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].role, "result");

        mark_artifact_deleted(&pool, bear_id, &reserved.artifact_ref)
            .await
            .expect("delete artifact");
        let deleted_content = authorize_artifact_access(
            &pool,
            &reserved.artifact_ref,
            ArtifactAccessContext {
                bear_id,
                user_id: Some(user_id),
            },
            ArtifactAccessLevel::Content,
        )
        .await
        .unwrap_err();
        assert!(deleted_content.to_string().contains("not readable"));
    }

    #[tokio::test]
    async fn docket_artifact_attach_and_citations_are_model_safe() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = create_user(&pool).await;
        let bear_id = create_bear(&pool).await;
        let task_id = Uuid::new_v4();

        let reserved = reserve_artifact(&pool, reserve_input(bear_id, user_id))
            .await
            .expect("reserve artifact");
        finalize_metadata_only_artifact(
            &pool,
            FinalizeArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                storage_key: Some("db-text-placeholder".to_string()),
                content_bytes: Some(12),
                content_sha256: Some(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
                ),
                metadata: json!({"phase": 3}),
            },
        )
        .await
        .expect("finalize artifact");

        let link = attach_docket_artifact(
            &pool,
            AttachDocketArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                target_kind: DocketArtifactTargetKind::Task,
                target_id: task_id,
                role: DocketArtifactRole::PrimaryOutput,
                metadata: json!({"note": "one finalized task output"}),
                created_by_user_id: Some(user_id),
            },
        )
        .await
        .expect("attach docket artifact");
        assert_eq!(link.target_kind, "docket_task");
        assert_eq!(link.target_id, task_id.to_string());
        assert_eq!(link.role, "primary_output");

        let second_reserved = reserve_artifact(&pool, reserve_input(bear_id, user_id))
            .await
            .expect("reserve second artifact");
        finalize_metadata_only_artifact(
            &pool,
            FinalizeArtifactInput {
                artifact_ref: second_reserved.artifact_ref.clone(),
                bear_id,
                storage_key: Some("db-text-placeholder-2".to_string()),
                content_bytes: Some(12),
                content_sha256: Some(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
                ),
                metadata: json!({}),
            },
        )
        .await
        .expect("finalize second artifact");

        let duplicate_primary_output = attach_docket_artifact(
            &pool,
            AttachDocketArtifactInput {
                artifact_ref: second_reserved.artifact_ref,
                bear_id,
                target_kind: DocketArtifactTargetKind::Task,
                target_id: task_id,
                role: DocketArtifactRole::PrimaryOutput,
                metadata: json!({}),
                created_by_user_id: Some(user_id),
            },
        )
        .await
        .unwrap_err();
        assert!(duplicate_primary_output
            .to_string()
            .contains("artifact_links_one_primary_output_per_docket_task"));

        let citations = list_docket_artifact_citations(
            &pool,
            bear_id,
            DocketArtifactTargetKind::Task,
            task_id,
            ArtifactAccessContext {
                bear_id,
                user_id: Some(user_id),
            },
        )
        .await
        .expect("list citations");
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].artifact_ref, reserved.artifact_ref);
        assert!(citations[0].readable);
        assert_eq!(citations[0].content_bytes, Some(12));
        let rendered = serde_json::to_value(&citations[0]).expect("serialize citation");
        assert!(rendered.get("storage_key").is_none());
        assert!(rendered.get("content_sha256").is_none());
        assert!(rendered.get("provenance").is_none());
        assert!(rendered.get("metadata").is_none());
    }

    #[tokio::test]
    async fn conversation_artifact_attach_and_citations_are_model_safe() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = create_user(&pool).await;
        let bear_id = create_bear(&pool).await;
        let conversation_id = format!("conversation-{}", Uuid::new_v4());

        let reserved = reserve_artifact(&pool, reserve_input(bear_id, user_id))
            .await
            .expect("reserve artifact");
        finalize_metadata_only_artifact(
            &pool,
            FinalizeArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                storage_key: Some("db-text-placeholder".to_string()),
                content_bytes: Some(12),
                content_sha256: Some(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
                ),
                metadata: json!({}),
            },
        )
        .await
        .expect("finalize artifact");

        let link = attach_conversation_artifact(
            &pool,
            AttachConversationArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                conversation_id: conversation_id.clone(),
                role: "output".to_string(),
                metadata: json!({}),
                created_by_user_id: Some(user_id),
            },
        )
        .await
        .expect("attach conversation artifact");
        assert_eq!(link.target_kind, "conversation");
        assert_eq!(link.target_id, conversation_id);

        let citations = list_conversation_artifact_citations(
            &pool,
            bear_id,
            &conversation_id,
            ArtifactAccessContext {
                bear_id,
                user_id: Some(user_id),
            },
        )
        .await
        .expect("list conversation citations");
        assert_eq!(citations.len(), 1);
        let rendered = serde_json::to_value(&citations[0]).expect("serialize citation");
        assert!(rendered.get("storage_key").is_none());
        assert!(rendered.get("content_sha256").is_none());
        assert!(rendered.get("provenance").is_none());
    }

    #[tokio::test]
    async fn garage_artifact_finalize_content_location_and_gc() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = create_user(&pool).await;
        let bear_id = create_bear(&pool).await;
        let now = OffsetDateTime::now_utc();

        let reserved = reserve_artifact(
            &pool,
            garage_reserve_input(bear_id, user_id, Some(now - time::Duration::minutes(5))),
        )
        .await
        .expect("reserve garage artifact");

        let upload = get_garage_artifact_upload_path(
            &pool,
            bear_id,
            &reserved.artifact_ref,
            "den-artifacts",
        )
        .await
        .expect("garage upload path");
        assert_eq!(upload.bucket, "den-artifacts");
        assert!(upload.storage_key.ends_with(&reserved.artifact_ref));
        assert_eq!(
            upload.object_path,
            format!("/den-artifacts/{}", upload.storage_key)
        );

        let finalized = finalize_garage_artifact(
            &pool,
            FinalizeGarageArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                content_type: "application/json".to_string(),
                content_bytes: 42,
                content_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                metadata: json!({"phase": 2, "final": true}),
            },
        )
        .await
        .expect("finalize garage artifact");
        assert_eq!(finalized.lifecycle, ArtifactLifecycle::Finalized);
        assert_eq!(finalized.storage_kind, ArtifactStorageKind::GarageArtifacts);
        assert_eq!(
            finalized.storage_key.as_deref(),
            Some(upload.storage_key.as_str())
        );

        let double_finalize = finalize_garage_artifact(
            &pool,
            FinalizeGarageArtifactInput {
                artifact_ref: reserved.artifact_ref.clone(),
                bear_id,
                content_type: "application/json".to_string(),
                content_bytes: 42,
                content_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                metadata: json!({}),
            },
        )
        .await
        .unwrap_err();
        assert!(double_finalize.to_string().contains("not pending"));

        let location = get_artifact_content_location(
            &pool,
            &reserved.artifact_ref,
            ArtifactAccessContext {
                bear_id,
                user_id: Some(user_id),
            },
        )
        .await
        .expect("content location");
        assert_eq!(location.storage_kind, ArtifactStorageKind::GarageArtifacts);
        assert_eq!(location.storage_key, upload.storage_key);
        assert_eq!(location.content_bytes, 42);

        let candidates = list_expired_artifact_gc_candidates(&pool, bear_id, now, 10)
            .await
            .expect("gc candidates");
        assert!(candidates
            .iter()
            .any(|artifact| artifact.artifact_ref == reserved.artifact_ref));
    }
}
