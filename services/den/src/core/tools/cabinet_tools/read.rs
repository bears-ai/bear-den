//! Typed page/attachment read target at the model argument boundary.

use den_cabinet::{CabinetAttachmentRef, CabinetItemRef, CabinetVersionRef, ReadRequest};
use den_service::{
    artifacts::bytes::ArtifactByteReader,
    cabinet::{
        self,
        attachment_read::{TextRange, DEFAULT_TEXT_LIMIT},
    },
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{actor_scope, cabinet_error, parse_arguments};
use den_core::{tools::context::DenToolInvocationContext, RuntimeContextLabel};
use den_http::errors::CustomError;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    cabinet_ref: CabinetItemRef,
    #[serde(default)]
    version_ref: Option<CabinetVersionRef>,
    #[serde(default)]
    attachment_ref: Option<CabinetAttachmentRef>,
    #[serde(default)]
    offset_chars: Option<usize>,
    #[serde(default)]
    limit_chars: Option<usize>,
}

#[derive(Debug)]
enum Target {
    Page {
        reference: CabinetItemRef,
        version: Option<CabinetVersionRef>,
    },
    Attachment {
        page: CabinetItemRef,
        attachment: CabinetAttachmentRef,
        range: TextRange,
    },
}

impl Arguments {
    fn target(self) -> Result<Target, CustomError> {
        if let Some(attachment) = self.attachment_ref {
            if self.version_ref.is_some() {
                return Err(CustomError::ValidationError(
                    "attachments belong to the current page, not an immutable page version".into(),
                ));
            }
            let range = TextRange::new(
                self.offset_chars.unwrap_or(0),
                self.limit_chars.unwrap_or(DEFAULT_TEXT_LIMIT),
            )?;
            Ok(Target::Attachment {
                page: self.cabinet_ref,
                attachment,
                range,
            })
        } else {
            if self.offset_chars.is_some() || self.limit_chars.is_some() {
                return Err(CustomError::ValidationError(
                    "text ranges require an attachment_ref".into(),
                ));
            }
            Ok(Target::Page {
                reference: self.cabinet_ref,
                version: self.version_ref,
            })
        }
    }
}

pub(super) async fn invoke(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
    reader: Option<&dyn ArtifactByteReader>,
) -> Result<Value, CustomError> {
    let scope = actor_scope(context, role);
    match parse_arguments::<Arguments>(arguments)?.target()? {
        Target::Page { reference, version } => {
            let view = cabinet::read(
                pool,
                ReadRequest {
                    scope: scope.clone(),
                    cabinet_ref: reference.clone(),
                    version_ref: version,
                },
            )
            .await
            .map_err(cabinet_error)?;
            let attachments =
                cabinet::attachment_read::list(pool, &scope, &reference, reader.is_some())
                    .await
                    .map_err(cabinet_error)?;
            Ok(
                json!({"domain":"cabinet","item":view.item,"version":view.version,"sources":view.sources,
                "attachments":attachments,"attachments_scope":"current_page"}),
            )
        }
        Target::Attachment {
            page,
            attachment,
            range,
        } => {
            let text = cabinet::attachment_read::read_text(
                pool,
                &scope,
                &page,
                &attachment,
                range,
                reader,
            )
            .await?;
            Ok(json!({"domain":"cabinet","attachment":text,"content_class":"source_data"}))
        }
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
