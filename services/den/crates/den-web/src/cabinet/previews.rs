//! Human file inspection; source text is escaped and inline bytes are sandboxed.

mod media;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderValue},
    response::Response,
    routing::get,
    Router,
};
use den_cabinet::CabinetAttachmentRef;
use den_service::artifacts::{ArtifactMetadata, ArtifactStorageKind, ArtifactVisibility};
use minijinja::context;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;

use super::{
    attachments::{protect_response, AuthorizedAttachment},
    item_url, parse_item_ref,
};
use crate::{auth_backend::AuthSession, errors::CustomError, web, AppState};
use media::{Media, TextPreview};

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Preview {
    Text { text: TextPreview },
    Image,
    Pdf,
    DownloadOnly,
    InvalidContent,
    StorageUnavailable,
    TooLarge,
    MissingContent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContentAvailability {
    Available,
    StorageUnavailable,
    TooLarge,
    MissingContent,
}

fn availability(state: &AppState, metadata: &ArtifactMetadata) -> ContentAvailability {
    match metadata.storage_kind {
        ArtifactStorageKind::DbText => ContentAvailability::Available,
        ArtifactStorageKind::ExternalGitCommit => ContentAvailability::MissingContent,
        ArtifactStorageKind::GarageArtifacts => {
            let Some(size) = metadata.content_bytes else {
                return ContentAvailability::MissingContent;
            };
            if size < 0 || metadata.content_sha256.is_none() || metadata.storage_key.is_none() {
                ContentAvailability::MissingContent
            } else if size as u64 > den_service::cabinet::uploads::MAX_FILE_BYTES as u64 {
                ContentAvailability::TooLarge
            } else if state.media.is_none() {
                ContentAvailability::StorageUnavailable
            } else {
                ContentAvailability::Available
            }
        }
    }
}

fn media(metadata: &ArtifactMetadata) -> Media {
    if metadata.storage_kind == ArtifactStorageKind::DbText {
        Media::Json
    } else {
        Media::parse(metadata.content_type.as_deref())
    }
}

#[derive(Serialize)]
struct Details {
    title: String,
    record_type: String,
    content_type: Option<String>,
    content_bytes: Option<i64>,
    reference: String,
    visibility: &'static str,
    creator: String,
    bear: String,
    created_at: String,
    finalized_at: Option<String>,
}

fn timestamp(value: time::OffsetDateTime) -> Result<String, CustomError> {
    value
        .format(&Rfc3339)
        .map_err(|_| CustomError::System("invalid file timestamp".into()))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cabinet/{page}/attachments/{attachment}", get(inspect))
        .route(
            "/cabinet/{page}/attachments/{attachment}/preview",
            get(inline),
        )
}

async fn inspect(
    State(state): State<AppState>,
    session: AuthSession,
    Path((page, attachment)): Path<(String, CabinetAttachmentRef)>,
) -> Result<Response, CustomError> {
    let page = parse_item_ref(&page)?;
    let authorized = AuthorizedAttachment::authorize(&state, &session, &page, &attachment).await?;
    let metadata = &authorized.metadata;
    let availability = availability(&state, metadata);
    let preview = match availability {
        ContentAvailability::StorageUnavailable => Preview::StorageUnavailable,
        ContentAvailability::TooLarge => Preview::TooLarge,
        ContentAvailability::MissingContent => Preview::MissingContent,
        ContentAvailability::Available => {
            let media = media(metadata);
            if media == Media::DownloadOnly {
                Preview::DownloadOnly
            } else {
                let bytes = authorized.bytes(&state).await?;
                match media {
                    Media::Text | Media::Json => match media.text(&bytes) {
                        Some(text) => Preview::Text { text },
                        None => Preview::InvalidContent,
                    },
                    Media::Pdf if media.inline_type(&bytes).is_some() => Preview::Pdf,
                    Media::Png | Media::Jpeg | Media::Gif | Media::Webp
                        if media.inline_type(&bytes).is_some() =>
                    {
                        Preview::Image
                    }
                    _ => Preview::InvalidContent,
                }
            }
        }
    };
    let bear = den_service::bears::db::get_bear(state.sqlx_pool(), metadata.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("file owner unavailable".into()))?;
    let viewer = session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".into()))?
        .id;
    let details = Details {
        title: metadata
            .title
            .clone()
            .unwrap_or_else(|| "Attachment".into()),
        record_type: metadata.kind.clone(),
        content_type: metadata.content_type.clone(),
        content_bytes: metadata.content_bytes,
        reference: authorized.reference.as_str().into(),
        visibility: match metadata.visibility {
            ArtifactVisibility::SameUser => "Private to its creator",
            ArtifactVisibility::PrivateToProfile => "Private source-local file",
            ArtifactVisibility::BearVisible => "Shared with this Bear and its members",
            ArtifactVisibility::HandoffRequested => "Review requested; not shared with the Bear",
        },
        creator: match metadata.created_by_user_id {
            Some(id) if id == viewer => "You".into(),
            Some(id) => format!("Person {id}"),
            None => "Not recorded".into(),
        },
        bear: bear.name,
        created_at: timestamp(metadata.created_at)?,
        finalized_at: metadata.finalized_at.map(timestamp).transpose()?,
    };
    authorized.recheck(&state).await?;
    let base_url = format!("/cabinet/{page}/attachments/{attachment}");
    let mut response = web::render_template(
        &state,
        "cabinet/attachment.html",
        session,
        context! {
            title => details.title,
            details,
            preview,
            page_url => item_url(&page),
            download_url => format!("{base_url}/content"),
            preview_url => format!("{base_url}/preview"),
            can_download => availability == ContentAvailability::Available,
        },
    )
    .await?;
    protect_response(&mut response);
    Ok(response)
}

async fn inline(
    State(state): State<AppState>,
    session: AuthSession,
    Path((page, attachment)): Path<(String, CabinetAttachmentRef)>,
) -> Result<Response, CustomError> {
    let page = parse_item_ref(&page)?;
    let authorized = AuthorizedAttachment::authorize(&state, &session, &page, &attachment).await?;
    let media = media(&authorized.metadata);
    if !matches!(
        media,
        Media::Png | Media::Jpeg | Media::Gif | Media::Webp | Media::Pdf
    ) {
        return Err(CustomError::NotFound("inline preview unavailable".into()));
    }
    let bytes = authorized.bytes(&state).await?;
    let content_type = media
        .inline_type(&bytes)
        .ok_or_else(|| CustomError::NotFound("inline preview unavailable".into()))?;
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_DISPOSITION, authorized.disposition(true))
        .body(Body::from(bytes))
        .map_err(|_| CustomError::System("file preview response failed".into()))?;
    protect_response(&mut response);
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox; frame-ancestors 'self'"),
    );
    Ok(response)
}
