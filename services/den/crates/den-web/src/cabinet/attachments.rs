//! Cabinet attachment forms and permission-rechecking content proxy.

use super::{cabinet_error, item_url, parse_item_ref, require_user_scope};
use crate::{auth_backend::AuthSession, errors::CustomError, AppState};
use axum::{
    body::Body,
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use den_cabinet::{AttachmentRole, CabinetAttachmentRef};
use den_service::{
    artifacts::{self, ArtifactAccessLevel, ArtifactReader, ArtifactRef, ArtifactStorageKind},
    cabinet,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct LinkForm {
    artifact_ref: ArtifactRef,
    role: AttachmentRole,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cabinet/{page}/attachments", post(link))
        .route(
            "/cabinet/{page}/attachments/{attachment}/remove",
            post(unlink),
        )
        .route(
            "/cabinet/{page}/attachments/{attachment}/content",
            get(content),
        )
}
async fn link(
    State(state): State<AppState>,
    session: AuthSession,
    Path(page): Path<String>,
    Form(form): Form<LinkForm>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    let page = parse_item_ref(&page)?;
    cabinet::attachments::link(
        state.sqlx_pool(),
        &scope,
        &page,
        &form.artifact_ref,
        form.role,
    )
    .await
    .map_err(cabinet_error)?;
    Ok(Redirect::to(&item_url(&page)).into_response())
}
async fn unlink(
    State(state): State<AppState>,
    session: AuthSession,
    Path((page, attachment)): Path<(String, CabinetAttachmentRef)>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    let page = parse_item_ref(&page)?;
    cabinet::attachments::unlink(state.sqlx_pool(), &scope, &page, &attachment)
        .await
        .map_err(cabinet_error)?;
    Ok(Redirect::to(&item_url(&page)).into_response())
}
async fn content(
    State(state): State<AppState>,
    session: AuthSession,
    Path((page, attachment)): Path<(String, CabinetAttachmentRef)>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    let page = parse_item_ref(&page)?;
    let actor = ArtifactReader::Human(den_core::ids::UserId::new(
        session
            .user
            .as_ref()
            .ok_or_else(|| CustomError::Authentication("login required".into()))?
            .id,
    ));
    let reference =
        cabinet::attachments::artifact_for_link(state.sqlx_pool(), &scope, &page, &attachment)
            .await
            .map_err(cabinet_error)?;
    let metadata = artifacts::authorize_for_reader(
        state.sqlx_pool(),
        &reference,
        actor,
        ArtifactAccessLevel::Content,
    )
    .await?;
    let (bytes, content_type) = match metadata.storage_kind {
        ArtifactStorageKind::DbText => {
            let value =
                cabinet::attachments::json_content(state.sqlx_pool(), &scope, &page, &attachment)
                    .await
                    .map_err(cabinet_error)?;
            (
                serde_json::to_vec(&value)
                    .map_err(|_| CustomError::System("encode artifact content".into()))?,
                "application/json",
            )
        }
        ArtifactStorageKind::GarageArtifacts => {
            let media = state.media.as_ref().ok_or_else(|| {
                CustomError::ValidationError("artifact byte storage is not configured".into())
            })?;
            let location =
                artifacts::content_location_for_reader(state.sqlx_pool(), &reference, actor)
                    .await?;
            (
                media.read_artifact(&location).await?,
                "application/octet-stream",
            )
        }
        ArtifactStorageKind::ExternalGitCommit => {
            return Err(CustomError::NotFound("artifact has no file payload".into()))
        }
    };
    cabinet::attachments::artifact_for_link(state.sqlx_pool(), &scope, &page, &attachment)
        .await
        .map_err(cabinet_error)?;
    let fallback = format!(
        "{}.{}",
        reference.as_str(),
        if metadata.storage_kind == ArtifactStorageKind::DbText {
            "json"
        } else {
            "bin"
        }
    );
    let filename = if metadata.storage_kind == ArtifactStorageKind::GarageArtifacts {
        metadata.title.as_deref().unwrap_or(&fallback)
    } else {
        &fallback
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "attachment; filename=\"{fallback}\"; filename*=UTF-8''{}",
                urlencoding::encode(filename)
            ),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(bytes))
        .map_err(|_| CustomError::System("artifact response failed".into()))
}
