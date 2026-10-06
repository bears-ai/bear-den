//! Internal, bounded artifact byte transfers. Signed storage URLs never leave Den.

use den_service::{
    artifacts::{self, ArtifactContentLocation},
    cabinet::uploads::MAX_FILE_BYTES,
};

use super::{MediaStore, S3Action};
use crate::errors::CustomError;

fn client() -> Result<reqwest::Client, CustomError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| CustomError::System("artifact storage client unavailable".into()))
}

impl MediaStore {
    pub(crate) async fn read_artifact(
        &self,
        location: &ArtifactContentLocation,
    ) -> Result<Vec<u8>, CustomError> {
        if location.content_bytes < 0 || location.content_bytes as u64 > MAX_FILE_BYTES as u64 {
            return Err(CustomError::ValidationError(
                "file exceeds the 16 MiB limit".into(),
            ));
        }
        let mut response = client()?
            .get(self.presign_internal_download(&location.storage_key))
            .send()
            .await
            .map_err(|_| CustomError::System("artifact storage unavailable".into()))?;
        if !response.status().is_success() {
            return Err(CustomError::System(
                "artifact content unavailable from storage".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| CustomError::System("artifact transfer failed".into()))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_FILE_BYTES {
                return Err(CustomError::ValidationError(
                    "file exceeds the 16 MiB limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        artifacts::verify_content_bytes(location, &bytes)?;
        Ok(bytes)
    }

    pub(crate) async fn write_artifact(
        &self,
        pool: &sqlx::PgPool,
        pending: &den_service::cabinet::uploads::PendingUpload,
        bytes: &[u8],
    ) -> Result<(), CustomError> {
        let location = pending.location();
        if bytes.len() > MAX_FILE_BYTES {
            return Err(CustomError::ValidationError(
                "file exceeds the 16 MiB limit".into(),
            ));
        }
        artifacts::verify_content_bytes(location, bytes)?;
        let content_type = location
            .content_type
            .as_deref()
            .unwrap_or("application/octet-stream");
        let deadline = den_service::cabinet::uploads::authorize_write(pool, pending)
            .await
            .map_err(|error| CustomError::from(den_core::DenError::from(error)))?;
        let seconds = (deadline - time::OffsetDateTime::now_utc()).whole_seconds();
        if seconds <= 0 {
            return Err(CustomError::ValidationError("upload lease expired".into()));
        }
        let expiry =
            std::time::Duration::from_secs(u64::try_from(seconds).expect("positive lease"))
                .min(super::UPLOAD_EXPIRY);
        let mut action = self
            .bucket
            .put_object(Some(&self.credentials), &location.storage_key);
        action.headers_mut().insert("content-type", content_type);
        let signed = action.sign(expiry);
        let response = client()?
            .put(signed)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|_| CustomError::System("file upload could not reach storage".into()))?;
        if !response.status().is_success() {
            return Err(CustomError::System(
                "file upload was refused by storage".into(),
            ));
        }
        self.read_artifact(location).await?;
        Ok(())
    }

    pub(crate) async fn remove_artifact_bytes(
        &self,
        location: &ArtifactContentLocation,
    ) -> Result<(), CustomError> {
        self.remove_artifact_key(&location.storage_key).await
    }

    pub(crate) async fn remove_retired_artifact(
        &self,
        ticket: &den_service::artifacts::cleanup::CleanupTicket,
    ) -> Result<(), CustomError> {
        self.remove_artifact_key(ticket.storage_key()).await
    }

    async fn remove_artifact_key(&self, key: &str) -> Result<(), CustomError> {
        let signed = self
            .bucket
            .delete_object(Some(&self.credentials), key)
            .sign(std::time::Duration::from_secs(60));
        let response = client()?
            .delete(signed)
            .send()
            .await
            .map_err(|_| CustomError::System("unfinished upload cleanup unavailable".into()))?;
        if !response.status().is_success() {
            return Err(CustomError::System(
                "unfinished upload cleanup refused".into(),
            ));
        }
        Ok(())
    }
}
