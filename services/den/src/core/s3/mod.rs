//! S3-compatible object storage (presigned URLs for upload/download).

use std::time::Duration;

use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use url::Url;
use uuid::Uuid;

use crate::config::Config;

/// Presigned URL expiry for browser uploads (PUT).
const UPLOAD_EXPIRY: Duration = Duration::from_secs(15 * 60);

/// Presigned URL expiry for browser downloads / inline display (GET).
const DOWNLOAD_EXPIRY: Duration = Duration::from_secs(60 * 60);

/// Thin wrapper around `rusty_s3` for generating presigned S3 URLs.
///
/// Does not perform HTTP itself — callers (or browsers) use the signed URLs directly.
#[derive(Clone)]
pub struct MediaStore {
    bucket: Bucket,
    /// Bucket pointed at the public-facing origin (for GET URLs that browsers fetch).
    public_bucket: Bucket,
    credentials: Credentials,
}

impl MediaStore {
    pub fn new(config: &Config) -> Option<Self> {
        if config.s3_endpoint.is_empty() {
            return None;
        }

        let endpoint: Url = config
            .s3_endpoint
            .parse()
            .expect("S3_ENDPOINT must be a valid URL");

        let url_style = if config.s3_force_path_style {
            UrlStyle::Path
        } else {
            UrlStyle::VirtualHost
        };

        let bucket = Bucket::new(
            endpoint.clone(),
            url_style.clone(),
            config.s3_bucket.clone(),
            config.s3_region.clone(),
        )
        .expect("S3 bucket configuration is valid");

        let public_endpoint: Url = if config.s3_public_url.is_empty() {
            endpoint
        } else {
            config
                .s3_public_url
                .parse()
                .expect("S3_PUBLIC_URL must be a valid URL")
        };

        let public_bucket = Bucket::new(
            public_endpoint,
            url_style,
            config.s3_bucket.clone(),
            config.s3_region.clone(),
        )
        .expect("S3 public bucket configuration is valid");

        let credentials = Credentials::new(
            config.s3_access_key_id.clone(),
            config.s3_secret_access_key.clone(),
        );

        Some(Self {
            bucket,
            public_bucket,
            credentials,
        })
    }

    pub fn is_enabled(&self) -> bool {
        true
    }

    /// Generate a unique object key for a chat media upload.
    ///
    /// Layout: `chat/{user_id}/{YYYY-MM}/{uuid}.{ext}`
    pub fn chat_object_key(user_id: Uuid, extension: &str) -> String {
        let now = time::OffsetDateTime::now_utc();
        let year_month = format!("{:04}-{:02}", now.year(), now.month() as u8);
        let file_id = Uuid::new_v4();
        let ext = extension.trim_start_matches('.');
        format!("chat/{user_id}/{year_month}/{file_id}.{ext}")
    }

    /// Presigned PUT URL for the browser to upload directly to S3.
    ///
    /// Uses the **internal** endpoint (Den-side or same-network).
    /// If clients upload from outside the network, use `presign_upload_public` instead.
    pub fn presign_upload(&self, object_key: &str, content_type: &str) -> PresignedUpload {
        let mut action = self.bucket.put_object(Some(&self.credentials), object_key);
        action.headers_mut().insert("content-type", content_type);
        let url = action.sign(UPLOAD_EXPIRY);

        PresignedUpload {
            upload_url: url.to_string(),
            object_key: object_key.to_string(),
            download_url: self.presign_download(object_key),
        }
    }

    /// Presigned PUT URL using the **public** endpoint (for browser-direct uploads).
    pub fn presign_upload_public(&self, object_key: &str, content_type: &str) -> PresignedUpload {
        let mut action = self
            .public_bucket
            .put_object(Some(&self.credentials), object_key);
        action.headers_mut().insert("content-type", content_type);
        let url = action.sign(UPLOAD_EXPIRY);

        PresignedUpload {
            upload_url: url.to_string(),
            object_key: object_key.to_string(),
            download_url: self.presign_download(object_key),
        }
    }

    /// Presigned GET URL for the browser to display/download an object.
    ///
    /// Always uses the **public** endpoint so the URL works from the browser.
    pub fn presign_download(&self, object_key: &str) -> String {
        let action = self
            .public_bucket
            .get_object(Some(&self.credentials), object_key);
        action.sign(DOWNLOAD_EXPIRY).to_string()
    }
}

/// Result of a presign-upload request.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresignedUpload {
    /// PUT this URL with the file body and the correct `Content-Type`.
    pub upload_url: String,
    /// The object key in the bucket (pass back to Den when sending the chat message).
    pub object_key: String,
    /// Presigned GET URL the browser can use to display the uploaded object.
    pub download_url: String,
}
