//! Cabinet attachment forms and permission-rechecking content access.

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
use den_cabinet::{ActorScope, AttachmentRole, CabinetAttachmentRef, CabinetItemRef};
use den_service::{
    artifacts::{
        self, ArtifactAccessLevel, ArtifactMetadata, ArtifactReader, ArtifactRef,
        ArtifactStorageKind,
    },
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

pub(super) struct AuthorizedAttachment {
    pub reference: ArtifactRef,
    pub metadata: ArtifactMetadata,
    scope: ActorScope,
    page: CabinetItemRef,
    attachment: CabinetAttachmentRef,
    reader: ArtifactReader,
}

impl AuthorizedAttachment {
    pub async fn authorize(
        state: &AppState,
        session: &AuthSession,
        page: &CabinetItemRef,
        attachment: &CabinetAttachmentRef,
    ) -> Result<Self, CustomError> {
        let scope = require_user_scope(session)?;
        let user = session
            .user
            .as_ref()
            .ok_or_else(|| CustomError::Authentication("login required".into()))?;
        let reader = ArtifactReader::Human(den_core::ids::UserId::new(user.id));
        let reference =
            cabinet::attachments::artifact_for_link(state.sqlx_pool(), &scope, page, attachment)
                .await
                .map_err(cabinet_error)?;
        let metadata = artifacts::authorize_for_reader(
            state.sqlx_pool(),
            &reference,
            reader,
            ArtifactAccessLevel::Content,
        )
        .await?;
        Ok(Self {
            reference,
            metadata,
            scope,
            page: page.clone(),
            attachment: attachment.clone(),
            reader,
        })
    }

    pub async fn recheck(&self, state: &AppState) -> Result<(), CustomError> {
        let current = cabinet::attachments::artifact_for_link(
            state.sqlx_pool(),
            &self.scope,
            &self.page,
            &self.attachment,
        )
        .await
        .map_err(cabinet_error)?;
        if current != self.reference {
            return Err(CustomError::NotFound("attachment changed".into()));
        }
        Ok(())
    }

    pub async fn bytes(&self, state: &AppState) -> Result<Vec<u8>, CustomError> {
        let bytes = match self.metadata.storage_kind {
            ArtifactStorageKind::DbText => {
                let value = cabinet::attachments::json_content(
                    state.sqlx_pool(),
                    &self.scope,
                    &self.page,
                    &self.attachment,
                )
                .await
                .map_err(cabinet_error)?;
                serde_json::to_vec(&value)
                    .map_err(|_| CustomError::System("encode artifact content".into()))?
            }
            ArtifactStorageKind::GarageArtifacts => {
                let media = state.media.as_ref().ok_or_else(|| {
                    CustomError::ValidationError("artifact byte storage is not configured".into())
                })?;
                let location = artifacts::content_location_for_reader(
                    state.sqlx_pool(),
                    &self.reference,
                    self.reader,
                )
                .await?;
                media.read_artifact(&location).await?
            }
            ArtifactStorageKind::ExternalGitCommit => {
                return Err(CustomError::NotFound("artifact has no file payload".into()))
            }
        };
        self.recheck(state).await?;
        Ok(bytes)
    }

    pub fn disposition(&self, inline: bool) -> String {
        let fallback = format!(
            "{}.{}",
            self.reference.as_str(),
            if self.metadata.storage_kind == ArtifactStorageKind::DbText {
                "json"
            } else {
                "bin"
            }
        );
        let filename = if self.metadata.storage_kind == ArtifactStorageKind::GarageArtifacts {
            self.metadata.title.as_deref().unwrap_or(&fallback)
        } else {
            &fallback
        };
        format!(
            "{}; filename=\"{fallback}\"; filename*=UTF-8''{}",
            if inline { "inline" } else { "attachment" },
            urlencoding::encode(filename)
        )
    }
}

pub(super) fn protect_response(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("no-referrer"),
    );
}

async fn content(
    State(state): State<AppState>,
    session: AuthSession,
    Path((page, attachment)): Path<(String, CabinetAttachmentRef)>,
) -> Result<Response, CustomError> {
    let page = parse_item_ref(&page)?;
    let authorized = AuthorizedAttachment::authorize(&state, &session, &page, &attachment).await?;
    let bytes = authorized.bytes(&state).await?;
    let content_type = match authorized.metadata.storage_kind {
        ArtifactStorageKind::DbText => "application/json",
        ArtifactStorageKind::GarageArtifacts | ArtifactStorageKind::ExternalGitCommit => {
            "application/octet-stream"
        }
    };
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_DISPOSITION, authorized.disposition(false))
        .body(Body::from(bytes))
        .map_err(|_| CustomError::System("artifact response failed".into()))?;
    protect_response(&mut response);
    Ok(response)
}
