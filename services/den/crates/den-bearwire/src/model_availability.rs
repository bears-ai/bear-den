//! Sanitized model availability projection at the BearWire boundary.

use den_core::ModelAvailabilityFailure;
use serde_json::{json, Value};

pub(crate) fn error_data(failure: &ModelAvailabilityFailure) -> Value {
    let descriptor = failure.descriptor();
    json!({
        "error": failure.to_string(),
        "error_code": descriptor.code,
        "unavailable_model": failure.model.as_ref().map(|model| model.as_str()),
        "recovery": descriptor.recovery,
    })
}
