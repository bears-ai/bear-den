use std::collections::BTreeMap;

use serde::Serialize;
use validator::ValidationErrors;

#[derive(Serialize)]
pub(super) struct FieldError {
    message: String,
}

/// Only messages cross the template boundary; validator parameters can contain passwords.
pub(super) fn validation_messages(errors: &ValidationErrors) -> BTreeMap<String, Vec<FieldError>> {
    errors
        .field_errors()
        .into_iter()
        .map(|(field, errors)| {
            let messages = errors
                .iter()
                .map(|error| FieldError {
                    message: error
                        .message
                        .as_deref()
                        .unwrap_or("Check this field.")
                        .to_string(),
                })
                .collect();
            (field.to_string(), messages)
        })
        .collect()
}
