//! Canonical Bear-owned model configurations and whole-configuration bindings.
//!
//! Resolution is conversation pin → hat override → Bear default → deployment
//! default. Every selected model is revalidated against the database catalog;
//! an invalid explicit choice is an error, never permission to fall back.
//! `bears.default_model` remains a database-maintained compatibility projection.
//! Legacy writes route atomically to canonical configurations; they never mutate
//! named configurations. New callers should use this module, not the raw field.

mod bindings;
mod catalog;
pub(super) mod compatibility;
mod persistence;
mod resolve;
mod types;

pub use bindings::{default_configuration_id, hat_configuration_id, set_default, set_hat_override};
pub use catalog::{validate_model_configuration, validate_thinking_effort, ModelCapabilities};
pub use persistence::{create, delete, get, list, update};
pub use resolve::resolve_primary;
pub use types::{ModelConfiguration, ModelHandle, PrimaryModelSource, ResolvedPrimaryModel};

#[cfg(test)]
mod tests;
