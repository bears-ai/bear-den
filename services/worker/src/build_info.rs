//! Compile-time build metadata (see `build.rs`) exposed for `GET /version` on web and API.

use axum::{Json, response::IntoResponse};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct VersionBody {
    pub service: &'static str,
    pub version: &'static str,
    /// RFC 3339 UTC timestamp from when `build.rs` last ran (`SOURCE_DATE_EPOCH` overrides for reproducible builds).
    pub built_at_utc: &'static str,
    /// Git commit built by CI (`GIT_SHA` build-arg), or `"unknown"` for local builds.
    pub git_sha: &'static str,
}

pub fn snapshot() -> VersionBody {
    VersionBody {
        service: "den",
        version: env!("CARGO_PKG_VERSION"),
        built_at_utc: env!("DEN_BUILT_AT_UTC"),
        git_sha: env!("DEN_GIT_SHA"),
    }
}

pub async fn json_handler() -> impl IntoResponse {
    Json(snapshot())
}
