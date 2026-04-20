//! Compile-time build metadata (see `build.rs`) exposed for `GET /version` on web and API.

use axum::{Json, response::IntoResponse};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct VersionBody {
    pub service: &'static str,
    pub version: &'static str,
    pub git_commit: &'static str,
}

pub fn snapshot() -> VersionBody {
    VersionBody {
        service: "den",
        version: env!("CARGO_PKG_VERSION"),
        git_commit: env!("DEN_GIT_COMMIT"),
    }
}

pub async fn json_handler() -> impl IntoResponse {
    Json(snapshot())
}
