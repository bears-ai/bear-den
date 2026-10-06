//! Server-rendered Cabinet file uploads; storage authority stays inside Den.

use axum::{
    extract::{multipart::Field, DefaultBodyLimit, Multipart, Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use den_cabinet::AttachmentRole;
use den_core::ids::BearId;
use den_service::cabinet::{
    self,
    uploads::{UploadAudience, UploadInput, MAX_FILE_BYTES},
};
use serde::Deserialize;

use super::{cabinet_error, item_url, parse_item_ref, require_user_scope};
use crate::{auth_backend::AuthSession, errors::CustomError, AppState};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum UploadField {
    BearId,
    Role,
    File,
    ShareWithBear,
}

struct File {
    name: String,
    content_type: String,
    bytes: Vec<u8>,
}
struct UploadForm {
    bear: BearId,
    file: File,
    role: AttachmentRole,
    audience: UploadAudience,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/cabinet/{page}/attachments/upload",
        post(upload).layer(DefaultBodyLimit::max(MAX_FILE_BYTES + 64 * 1024)),
    )
}

async fn read_field(mut field: Field<'_>, limit: usize) -> Result<Vec<u8>, CustomError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| CustomError::ValidationError("invalid file upload".into()))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(CustomError::ValidationError(
                "upload exceeds the permitted size".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_form(mut multipart: Multipart) -> Result<UploadForm, CustomError> {
    let mut bear = None;
    let mut role = None;
    let mut file = None;
    let mut share = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| CustomError::ValidationError("invalid file upload".into()))?
    {
        let key: UploadField = serde_json::from_value(serde_json::json!(field.name()))
            .map_err(|_| CustomError::ValidationError("unexpected upload field".into()))?;
        match key {
            UploadField::File => {
                if file.is_some() {
                    return Err(CustomError::ValidationError("choose one file".into()));
                }
                let name = field
                    .file_name()
                    .unwrap_or("")
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_owned();
                let bytes = read_field(field, MAX_FILE_BYTES).await?;
                file = Some(File {
                    name,
                    content_type,
                    bytes,
                });
            }
            other => {
                let bytes = read_field(field, 512).await?;
                let text = String::from_utf8(bytes)
                    .map_err(|_| CustomError::ValidationError("invalid upload field".into()))?;
                match other {
                    UploadField::BearId => {
                        if bear.is_some() {
                            return Err(CustomError::ValidationError("choose one Bear".into()));
                        }
                        bear = Some(BearId::new(uuid::Uuid::parse_str(&text).map_err(|_| {
                            CustomError::ValidationError("choose a Bear".into())
                        })?));
                    }
                    UploadField::Role => {
                        if role.is_some() {
                            return Err(CustomError::ValidationError(
                                "choose one file role".into(),
                            ));
                        }
                        role = Some(serde_json::from_value(serde_json::json!(text)).map_err(
                            |_| CustomError::ValidationError("choose a file role".into()),
                        )?);
                    }
                    UploadField::ShareWithBear => {
                        if share.is_some() {
                            return Err(CustomError::ValidationError(
                                "choose one file audience".into(),
                            ));
                        }
                        share = Some(text.parse::<bool>().map_err(|_| {
                            CustomError::ValidationError("invalid sharing acknowledgement".into())
                        })?);
                    }
                    UploadField::File => unreachable!("file handled before text parsing"),
                }
            }
        }
    }
    Ok(UploadForm {
        bear: bear
            .ok_or_else(|| CustomError::ValidationError("choose a Bear for this file".into()))?,
        file: file.ok_or_else(|| CustomError::ValidationError("choose a file".into()))?,
        role: role.unwrap_or(AttachmentRole::Other),
        audience: if share.unwrap_or(false) {
            UploadAudience::BearAndMembers
        } else {
            UploadAudience::Private
        },
    })
}

async fn upload(
    State(state): State<AppState>,
    session: AuthSession,
    Path(page): Path<String>,
    multipart: Multipart,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    let page = parse_item_ref(&page)?;
    let access = cabinet::pages::metadata(state.sqlx_pool(), &scope, &page)
        .await
        .map_err(cabinet_error)?;
    if !access.can_write {
        return Err(cabinet_error(den_cabinet::CabinetError::NotAuthorized));
    }
    let media = state.media.as_ref().ok_or_else(|| {
        CustomError::ValidationError("file storage is not configured for this Den".into())
    })?;
    let form = read_form(multipart).await?;
    let pending = cabinet::uploads::prepare(
        state.sqlx_pool(),
        &scope,
        &page,
        UploadInput {
            bear_id: form.bear,
            title: form.file.name,
            content_type: form.file.content_type,
            bytes: &form.file.bytes,
            role: form.role,
            audience: form.audience,
        },
    )
    .await
    .map_err(cabinet_error)?;
    let result = async {
        media
            .write_artifact(state.sqlx_pool(), &pending, &form.file.bytes)
            .await?;
        cabinet::uploads::publish(state.sqlx_pool(), &scope, &pending)
            .await
            .map_err(cabinet_error)?;
        Ok::<_, CustomError>(())
    }
    .await;
    if let Err(error) = result {
        // Only delete bytes when the registry confirms this request is still
        // pending: an ambiguous commit outcome must not destroy retained content.
        match cabinet::uploads::abandon(state.sqlx_pool(), &pending).await {
            Ok(true) => {
                if media
                    .remove_artifact_bytes(pending.location())
                    .await
                    .is_err()
                {
                    tracing::warn!(artifact_ref = %pending.location().artifact_ref, "unfinished upload byte cleanup needs retry");
                }
            }
            Ok(false) => {}
            Err(_) => {
                tracing::warn!(artifact_ref = %pending.location().artifact_ref, "unfinished upload registry cleanup needs retry");
            }
        }
        return Err(error);
    }
    Ok(Redirect::to(&item_url(&page)).into_response())
}
