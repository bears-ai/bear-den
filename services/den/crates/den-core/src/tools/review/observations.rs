//! Retired model-facing observation helper. Internal observation persistence is
//! owned by the runtime and canonical memory stores, not an audit context label.

use crate::{DenError, RuntimeContextLabel};
use serde::Deserialize;
use serde_json::Value;

use crate::tools::context::DenToolInvocationContext;

use super::store::MemoryReviewStore;

#[derive(Debug, Deserialize)]
pub struct ObservationWriteArguments {
    #[serde(default)]
    pub observation_id: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub salience: Option<String>,
    #[serde(default)]
    pub payload_ref: Option<String>,
    #[serde(default)]
    pub source: Option<Value>,
}

pub async fn write_observation(
    _store: &impl MemoryReviewStore,
    _context: &DenToolInvocationContext,
    _role: RuntimeContextLabel,
    _arguments: Value,
) -> Result<Value, DenError> {
    Err(DenError::NotFound(
        "write_observation model helper is retired".to_string(),
    ))
}
