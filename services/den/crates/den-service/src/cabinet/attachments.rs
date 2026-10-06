//! Cabinet attachment links use the artifact registry's existing link records.

use super::{db_error, pages, violation};
use crate::artifacts::{
    self, ArtifactAccessLevel, ArtifactCitation, ArtifactReader, ArtifactRef, ArtifactStorageKind,
};
use den_cabinet::{
    Actor, ActorScope, AttachmentRole, Authority, CabinetAttachmentRef, CabinetError,
    CabinetItemRef,
};
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct Attachment {
    pub reference: CabinetAttachmentRef,
    pub artifact: ArtifactCitation,
    pub role: AttachmentRole,
    pub created_at: OffsetDateTime,
    pub json_download: bool,
}

pub(crate) fn reader(scope: &ActorScope) -> ArtifactReader {
    match scope.actor {
        Actor::User { user_id } => ArtifactReader::Human(user_id),
        Actor::Bear { bear_id, .. } => ArtifactReader::Bear(bear_id),
    }
}
fn link_id(reference: &CabinetAttachmentRef) -> Result<Uuid, CabinetError> {
    Uuid::parse_str(&reference.as_str()[CabinetAttachmentRef::PREFIX.len()..])
        .map_err(|_| CabinetError::NotFound)
}
fn reference(id: Uuid) -> Result<CabinetAttachmentRef, CabinetError> {
    CabinetAttachmentRef::parse(&format!("{}{}", CabinetAttachmentRef::PREFIX, id.simple()))
        .map_err(violation)
}
fn artifact_error(error: den_core::DenError) -> CabinetError {
    match error {
        den_core::DenError::NotFound(_) | den_core::DenError::Authorization(_) => {
            CabinetError::NotFound
        }
        other => CabinetError::Storage(other.to_string()),
    }
}

pub async fn link(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
    artifact: &ArtifactRef,
    role: AttachmentRole,
) -> Result<CabinetAttachmentRef, CabinetError> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    pages::authorize(pool, scope, page, Authority::Write).await?;
    let metadata = artifacts::authorize_for_reader(
        pool,
        artifact,
        reader(scope),
        ArtifactAccessLevel::Content,
    )
    .await
    .map_err(artifact_error)?;
    sqlx::query!(
        "SELECT id FROM artifacts WHERE id=$1 FOR UPDATE",
        metadata.id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    artifacts::authorize_for_reader(pool, artifact, reader(scope), ArtifactAccessLevel::Content)
        .await
        .map_err(artifact_error)?;
    if metadata.storage_kind == ArtifactStorageKind::DbText {
        artifacts::json_content_for_reader(pool, artifact, reader(scope))
            .await
            .map_err(artifact_error)?;
    }
    let reference = insert_link_in_tx(&mut tx, scope, page, metadata.id, role).await?;
    tx.commit().await.map_err(db_error)?;
    Ok(reference)
}

pub(super) async fn insert_link_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &ActorScope,
    page: &CabinetItemRef,
    artifact_id: Uuid,
    role: AttachmentRole,
) -> Result<CabinetAttachmentRef, CabinetError> {
    let actor =
        serde_json::to_value(scope).map_err(|error| CabinetError::Storage(error.to_string()))?;
    let user = match scope.actor {
        Actor::User { user_id } => Some(user_id.0),
        Actor::Bear { .. } => None,
    };
    let id=sqlx::query_scalar!("INSERT INTO artifact_links(artifact_id,target_kind,target_id,role,metadata,created_by_user_id) VALUES($1,'cabinet_item',$2,$3,$4,$5) ON CONFLICT(artifact_id,target_kind,target_id,role) DO UPDATE SET artifact_id=artifact_links.artifact_id RETURNING id",artifact_id,page.as_str(),role.as_str(),serde_json::json!({"cabinet_actor":actor}),user).fetch_one(&mut **tx).await.map_err(db_error)?;
    reference(id)
}

pub async fn unlink(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
    attachment: &CabinetAttachmentRef,
) -> Result<(), CabinetError> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    pages::authorize(pool, scope, page, Authority::Write).await?;
    let id = link_id(attachment)?;
    let artifact=sqlx::query_scalar!("SELECT artifact_id FROM artifact_links WHERE id=$1 AND target_kind='cabinet_item' AND target_id=$2",id,page.as_str()).fetch_optional(&mut *tx).await.map_err(db_error)?.ok_or(CabinetError::NotFound)?;
    sqlx::query!("SELECT id FROM artifacts WHERE id=$1 FOR UPDATE", artifact)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
    sqlx::query!(
        "DELETE FROM artifact_links WHERE id=$1 AND target_kind='cabinet_item' AND target_id=$2",
        id,
        page.as_str()
    )
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

pub async fn list(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
) -> Result<Vec<Attachment>, CabinetError> {
    pages::authorize(pool, scope, page, Authority::Read).await?;
    let rows=sqlx::query!("SELECT l.id,l.role,l.created_at,a.artifact_ref FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE l.target_kind='cabinet_item' AND l.target_id=$1 ORDER BY l.created_at,l.id",page.as_str()).fetch_all(pool).await.map_err(db_error)?;
    let mut visible = Vec::new();
    for row in rows {
        let artifact = ArtifactRef::parse(&row.artifact_ref).map_err(artifact_error)?;
        let metadata = match artifacts::authorize_for_reader(
            pool,
            &artifact,
            reader(scope),
            ArtifactAccessLevel::Content,
        )
        .await
        {
            Ok(value) => value,
            Err(den_core::DenError::NotFound(_) | den_core::DenError::Authorization(_)) => continue,
            Err(error) => return Err(artifact_error(error)),
        };
        let role = serde_json::from_value(serde_json::json!(row.role))
            .map_err(|_| CabinetError::Storage("unknown Cabinet attachment role".into()))?;
        visible.push(Attachment {
            reference: reference(row.id)?,
            role,
            created_at: row.created_at,
            json_download: metadata.storage_kind == ArtifactStorageKind::DbText,
            artifact: ArtifactCitation {
                artifact_ref: metadata.artifact_ref,
                kind: metadata.kind,
                title: metadata.title,
                summary: metadata.summary,
                content_type: metadata.content_type,
                content_bytes: metadata.content_bytes,
                lifecycle: metadata.lifecycle,
                readable: true,
            },
        });
    }
    pages::authorize(pool, scope, page, Authority::Read).await?;
    Ok(visible)
}

pub async fn artifact_for_link(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
    attachment: &CabinetAttachmentRef,
) -> Result<ArtifactRef, CabinetError> {
    pages::authorize(pool, scope, page, Authority::Read).await?;
    let id = link_id(attachment)?;
    let artifact=sqlx::query_scalar!("SELECT a.artifact_ref FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE l.id=$1 AND l.target_kind='cabinet_item' AND l.target_id=$2",id,page.as_str()).fetch_optional(pool).await.map_err(db_error)?.ok_or(CabinetError::NotFound)?;
    let reference = ArtifactRef::parse(&artifact).map_err(artifact_error)?;
    artifacts::authorize_for_reader(
        pool,
        &reference,
        reader(scope),
        ArtifactAccessLevel::Content,
    )
    .await
    .map_err(artifact_error)?;
    Ok(reference)
}

pub async fn json_content(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
    attachment: &CabinetAttachmentRef,
) -> Result<serde_json::Value, CabinetError> {
    pages::authorize(pool, scope, page, Authority::Read).await?;
    let id = link_id(attachment)?;
    let artifact=sqlx::query_scalar!("SELECT a.artifact_ref FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE l.id=$1 AND l.target_kind='cabinet_item' AND l.target_id=$2",id,page.as_str()).fetch_optional(pool).await.map_err(db_error)?.ok_or(CabinetError::NotFound)?;
    let payload = artifacts::json_content_for_reader(
        pool,
        &ArtifactRef::parse(&artifact).map_err(artifact_error)?,
        reader(scope),
    )
    .await
    .map_err(artifact_error)?;
    pages::authorize(pool, scope, page, Authority::Read).await?;
    Ok(payload)
}
