//! Safe, typed failures at the Bear-authenticated model catalog boundary.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelAvailabilityFailureKind {
    ModelMissing,
    ModelUnavailable,
    VirtualKeyMissing,
    VirtualKeyRejected,
    CatalogUnavailable,
}

/// Only a checked model label can enter public errors or diagnostic logs.
/// This is not a model-selection authority or a provider model identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeModelReference(String);

impl SafeModelReference {
    pub fn checked(model: &str) -> Option<Self> {
        let model = model.trim();
        let (provider, name) = model.split_once('/')?;
        let safe_segment = |segment: &str| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
                && !segment.split(['-', '_', '.', ':']).any(|part| {
                    matches!(
                        part.to_ascii_lowercase().as_str(),
                        "sk" | "vk" | "key" | "token" | "secret" | "password" | "bearer"
                    )
                })
        };
        // URL paths, credentials, query strings, markup and unbounded opaque values
        // are not labels. Unsupported representations are omitted, never truncated.
        if model.len() > 128 || !safe_segment(provider) || !safe_segment(name) {
            return None;
        }
        Some(Self(model.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SafeModelReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelAvailabilityDescriptor {
    pub code: &'static str,
    pub summary: &'static str,
    pub recovery: &'static str,
}

impl ModelAvailabilityFailureKind {
    pub fn descriptor(self) -> ModelAvailabilityDescriptor {
        let (code, summary, recovery) = match self {
            Self::ModelMissing => (
                "model_missing",
                "The selected model is missing from this Bear's Bifrost catalog.",
                "Choose a model available to this Bear, or ask an administrator to enable the selected model in Bifrost.",
            ),
            Self::ModelUnavailable => (
                "model_unavailable",
                "The selected model is unavailable to this Bear in Bifrost.",
                "Choose an available model, or ask an administrator to restore this Bear's model access in Bifrost.",
            ),
            Self::VirtualKeyMissing => (
                "virtual_key_missing",
                "This Bear has no Bifrost virtual key for model access.",
                "Ask an administrator to configure this Bear's Bifrost virtual key, then try again.",
            ),
            Self::VirtualKeyRejected => (
                "virtual_key_rejected",
                "This Bear's Bifrost virtual key could not authorize model access.",
                "Ask an administrator to repair this Bear's Bifrost virtual key and model permissions, then try again.",
            ),
            Self::CatalogUnavailable => (
                "catalog_unavailable",
                "The Bifrost model catalog could not be checked.",
                "Try again shortly. If this continues, ask an administrator to check Bifrost connectivity and catalog health.",
            ),
        };
        ModelAvailabilityDescriptor {
            code,
            summary,
            recovery,
        }
    }
}

/// No raw cause, URL, response body or credential is retained in this failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelAvailabilityFailure {
    pub kind: ModelAvailabilityFailureKind,
    pub model: Option<SafeModelReference>,
}

impl ModelAvailabilityFailure {
    pub fn new(kind: ModelAvailabilityFailureKind, model: Option<&str>) -> Self {
        Self {
            kind,
            model: model.and_then(SafeModelReference::checked),
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = SafeModelReference::checked(model);
        self
    }

    pub fn descriptor(&self) -> ModelAvailabilityDescriptor {
        self.kind.descriptor()
    }

    pub fn public_message(&self) -> String {
        let summary = self.descriptor().summary;
        match &self.model {
            Some(model) => format!("{summary} Selected model: {model}."),
            None => summary.to_owned(),
        }
    }
}

impl fmt::Display for ModelAvailabilityFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} {}",
            self.descriptor().code,
            self.public_message(),
            self.descriptor().recovery
        )
    }
}

impl std::error::Error for ModelAvailabilityFailure {}

#[cfg(test)]
#[path = "model_availability_tests.rs"]
mod tests;
