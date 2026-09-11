//! BearWire JSON-RPC method handlers.
//!
//! The crate-level router owns transport (`/v1/rpc`, SSE pages); this module tree owns typed
//! method parsing and result construction for session/run/client/resource/work operations.

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use den_service::DenState;

pub(crate) mod client;
pub(crate) mod conversation;
pub(crate) mod docket;
pub(crate) mod focused_execution;
pub(crate) mod resource;
pub(crate) mod run;
pub(crate) mod session;
pub(crate) mod work;

#[cfg(test)]
mod tests;

pub(crate) const DEFAULT_CLIENT: &str = "bearwire";

pub(crate) fn initialize_result(_state: &DenState) -> Value {
    json!({
        "protocol": "bearwire",
        "version": 1,
        "server": {
            "name": "den",
            "version": den_http::build_info::snapshot().version,
            "git_sha": den_http::build_info::snapshot().git_sha,
        },
        "bearwire": {
            "rpc": "/v1/rpc",
            "events_page": "/v1/sessions/{session_id}/events/page"
        },
        "legacy_acp_enabled": false,
        "legacy_acp_deprecated": true,
        "legacy_acp_removal_phase": "removed",
    })
}

pub(crate) fn parse_params<T: DeserializeOwned>(
    params: &Value,
) -> Result<T, den_http::errors::CustomError> {
    serde_json::from_value(params.clone()).map_err(|err| {
        den_http::errors::CustomError::ValidationError(format!("invalid BearWire params: {err}"))
    })
}
