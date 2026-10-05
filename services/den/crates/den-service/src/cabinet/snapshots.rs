//! Immutable, privately owned Cabinet document evidence. No automatic audience promotion.

use super::{db_error, pages};
use crate::artifacts::{
    self, ArtifactRef, ArtifactStorageKind, ArtifactVisibility, CreateJsonArtifactInput,
    ReserveArtifactInput,
};
use den_cabinet::{
    Actor, ActorScope, Authority, CabinetError, CabinetItemRef, CabinetVersionRef, ReadRequest,
    ReviewState,
};
use den_core::ids::BearId;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSnapshot {
    pub cabinet_ref: CabinetItemRef,
    pub version_ref: CabinetVersionRef,
    pub title_at_capture: String,
    pub content: String,
    pub content_sha256: String,
}

pub async fn capture_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    pool: &PgPool,
    scope: &ActorScope,
    bear: BearId,
    page: &CabinetItemRef,
    version: &CabinetVersionRef,
) -> Result<ArtifactRef, CabinetError> {
    let Actor::User { user_id } = scope.actor else {
        return Err(CabinetError::NotAuthorized);
    };
    if crate::bears::db::membership_role_for_user(pool, user_id.0, bear.as_uuid())
        .await
        .map_err(|error| CabinetError::Storage(error.to_string()))?
        .is_none()
    {
        return Err(CabinetError::NotAuthorized);
    }
    pages::lock(tx).await?;
    pages::authorize(pool, scope, page, Authority::Read).await?;
    let view = super::read(
        pool,
        ReadRequest {
            scope: scope.clone(),
            cabinet_ref: page.clone(),
            version_ref: Some(version.clone()),
        },
    )
    .await?;
    if matches!(
        view.version.review(),
        ReviewState::Pending | ReviewState::Rejected
    ) {
        return Err(CabinetError::Policy(
            "only published versions can become document evidence".into(),
        ));
    }
    let payload = DocumentSnapshot {
        cabinet_ref: page.clone(),
        version_ref: version.clone(),
        title_at_capture: view.item.title.clone(),
        content: view.version.content().into(),
        content_sha256: view.version.content_sha256().into(),
    };
    let snapshot = artifacts::create_json_artifact_in_tx(
        tx,
        CreateJsonArtifactInput {
            reserve: ReserveArtifactInput {
                bear_id: bear.as_uuid(),
                created_by_user_id: Some(user_id.0),
                owner_profile: den_core::RuntimeContextLabel::ChannelConversation,
                kind: "cabinet_document_snapshot".into(),
                title: Some(view.item.title),
                summary: Some("Immutable published Cabinet document evidence".into()),
                content_type: Some("application/json".into()),
                storage_kind: ArtifactStorageKind::DbText,
                visibility: ArtifactVisibility::SameUser,
                provenance: serde_json::json!({"cabinet_ref":page,"version_ref":version}),
                metadata: serde_json::json!({}),
                expires_at: None,
            },
            payload: serde_json::to_value(payload)
                .map_err(|error| CabinetError::Storage(error.to_string()))?,
        },
    )
    .await
    .map_err(|error| CabinetError::Storage(error.to_string()))?;
    sqlx::query!("INSERT INTO artifact_links(artifact_id,target_kind,target_id,role,metadata,created_by_user_id) VALUES($1,'cabinet_snapshot',$2,'citation',$3,$4)",snapshot.id,version.as_str(),serde_json::json!({"cabinet_ref":page}),user_id.0).execute(&mut **tx).await.map_err(db_error)?;
    ArtifactRef::parse(&snapshot.artifact_ref)
        .map_err(|error| CabinetError::Storage(error.to_string()))
}
