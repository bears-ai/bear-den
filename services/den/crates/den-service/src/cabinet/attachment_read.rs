//! Bounded attachment discovery/read over independent page and file policies.

use den_cabinet::{ActorScope, AttachmentRole, CabinetAttachmentRef, CabinetError, CabinetItemRef};
use den_core::DenError;
use serde::Serialize;
use sqlx::PgPool;

use super::attachments;
use crate::artifacts::{
    self, bytes::ArtifactByteReader, ArtifactAccessLevel, ArtifactCitation, ArtifactRef,
    ArtifactStorageKind,
};

pub const DEFAULT_TEXT_LIMIT: usize = 12_000;
pub const MAX_TEXT_LIMIT: usize = 24_000;

#[derive(Debug, Clone, Copy)]
pub struct TextRange {
    offset: usize,
    limit: usize,
}
impl TextRange {
    pub fn new(offset: usize, limit: usize) -> Result<Self, DenError> {
        if !(1..=MAX_TEXT_LIMIT).contains(&limit) {
            return Err(DenError::ValidationError(
                "limit_chars must be between 1 and 24000".into(),
            ));
        }
        Ok(Self { offset, limit })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextFormat {
    Utf8,
    Json,
    Unsupported,
}
impl TextFormat {
    fn parse(content_type: Option<&str>) -> Self {
        let mut parts = content_type.unwrap_or("").split(';');
        let essence = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        for parameter in parts {
            if let Some((name, value)) = parameter.trim().split_once('=') {
                if name.trim().eq_ignore_ascii_case("charset") {
                    let encoding = value.trim().trim_matches('"');
                    if !encoding.eq_ignore_ascii_case("utf-8")
                        && !encoding.eq_ignore_ascii_case("us-ascii")
                    {
                        return Self::Unsupported;
                    }
                }
            }
        }
        match essence.as_str() {
            "application/json" => Self::Json,
            value if value.starts_with("application/") && value.ends_with("+json") => Self::Json,
            value if value.starts_with("text/") => Self::Utf8,
            _ => Self::Unsupported,
        }
    }
}

#[derive(Serialize)]
pub struct AttachmentInfo {
    pub attachment_ref: CabinetAttachmentRef,
    pub artifact: ArtifactCitation,
    pub role: AttachmentRole,
    pub text_readable: bool,
}

pub async fn list(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
    byte_storage_available: bool,
) -> Result<Vec<AttachmentInfo>, CabinetError> {
    super::authorize(pool, scope).await?;
    let visible = attachments::list(pool, scope, page).await?;
    super::authorize(pool, scope).await?;
    Ok(visible
        .into_iter()
        .map(|attachment| {
            let text_readable = attachment.json_download
                || (byte_storage_available
                    && TextFormat::parse(attachment.artifact.content_type.as_deref())
                        != TextFormat::Unsupported
                    && attachment.artifact.content_bytes.is_some_and(|size| {
                        size >= 0 && size as u64 <= super::uploads::MAX_FILE_BYTES as u64
                    }));
            AttachmentInfo {
                attachment_ref: attachment.reference,
                artifact: attachment.artifact,
                role: attachment.role,
                text_readable,
            }
        })
        .collect())
}

#[derive(Serialize)]
pub struct AttachmentText {
    pub cabinet_ref: CabinetItemRef,
    pub attachment_ref: CabinetAttachmentRef,
    pub artifact_ref: ArtifactRef,
    pub content_type: Option<String>,
    pub offset_chars: usize,
    pub total_chars: usize,
    pub next_offset_chars: Option<usize>,
    pub text: String,
}

fn cabinet_error(error: CabinetError) -> DenError {
    error.into()
}

pub async fn read_text(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
    attachment: &CabinetAttachmentRef,
    range: TextRange,
    reader: Option<&dyn ArtifactByteReader>,
) -> Result<AttachmentText, DenError> {
    super::authorize(pool, scope).await.map_err(cabinet_error)?;
    let reference = attachments::artifact_for_link(pool, scope, page, attachment)
        .await
        .map_err(cabinet_error)?;
    let actor = attachments::reader(scope);
    let metadata =
        artifacts::authorize_for_reader(pool, &reference, actor, ArtifactAccessLevel::Content)
            .await?;
    let (source, content_type) = match metadata.storage_kind {
        ArtifactStorageKind::DbText => {
            let value = attachments::json_content(pool, scope, page, attachment)
                .await
                .map_err(cabinet_error)?;
            (
                serde_json::to_string(&value)
                    .map_err(|_| DenError::System("encode attachment JSON".into()))?,
                Some("application/json".into()),
            )
        }
        ArtifactStorageKind::GarageArtifacts => {
            if TextFormat::parse(metadata.content_type.as_deref()) == TextFormat::Unsupported {
                return Err(DenError::ValidationError(
                    "attachment is not a supported UTF-8 text or JSON file".into(),
                ));
            }
            let location = artifacts::content_location_for_reader(pool, &reference, actor).await?;
            if location.content_bytes < 0
                || location.content_bytes as u64 > super::uploads::MAX_FILE_BYTES as u64
            {
                return Err(DenError::ValidationError(
                    "attachment exceeds the 16 MiB read limit".into(),
                ));
            }
            let reader = reader.ok_or_else(|| {
                DenError::ValidationError("attachment byte storage is unavailable".into())
            })?;
            let bytes = reader.read(&location).await?;
            if bytes.len() > super::uploads::MAX_FILE_BYTES {
                return Err(DenError::ValidationError(
                    "attachment exceeds the read limit".into(),
                ));
            }
            artifacts::verify_content_bytes(&location, &bytes)?;
            (
                String::from_utf8(bytes).map_err(|_| {
                    DenError::ValidationError("attachment is not valid UTF-8 text".into())
                })?,
                metadata.content_type,
            )
        }
        ArtifactStorageKind::ExternalGitCommit => {
            return Err(DenError::NotFound("attachment text unavailable".into()))
        }
    };
    super::authorize(pool, scope).await.map_err(cabinet_error)?;
    let current = attachments::artifact_for_link(pool, scope, page, attachment)
        .await
        .map_err(cabinet_error)?;
    if current != reference {
        return Err(DenError::NotFound("attachment changed during read".into()));
    }
    artifacts::authorize_for_reader(pool, &reference, actor, ArtifactAccessLevel::Content).await?;
    let total_chars = source.chars().count();
    if range.offset > total_chars {
        return Err(DenError::ValidationError(
            "offset_chars is beyond the attachment text".into(),
        ));
    }
    let text: String = source
        .chars()
        .skip(range.offset)
        .take(range.limit)
        .collect();
    let end = range.offset + text.chars().count();
    Ok(AttachmentText {
        cabinet_ref: page.clone(),
        attachment_ref: attachment.clone(),
        artifact_ref: reference,
        content_type,
        offset_chars: range.offset,
        total_chars,
        next_offset_chars: (end < total_chars).then_some(end),
        text,
    })
}
