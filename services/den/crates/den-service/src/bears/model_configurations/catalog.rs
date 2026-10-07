use den_core::{DenError, ThinkingEffort};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgConnection, PgPool};

use super::ModelHandle;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub model_handle: ModelHandle,
    /// Database catalog metadata is authoritative. Unknown is not support.
    pub supports_reasoning_effort: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CatalogCapabilities {
    supports_reasoning_effort: Option<bool>,
}

/// Shared UI/service validator. Never infer support from a provider/model name.
/// Passing `None` effort is valid even when the capability is unknown.
pub fn validate_thinking_effort(
    supports_reasoning_effort: Option<bool>,
    thinking_effort: Option<ThinkingEffort>,
) -> Result<(), DenError> {
    if thinking_effort.is_some() && supports_reasoning_effort != Some(true) {
        return Err(DenError::ValidationError(match supports_reasoning_effort {
            Some(false) => "this model does not support explicit thinking effort".into(),
            _ => "thinking effort support is unknown in the model catalog".into(),
        }));
    }
    Ok(())
}

/// Validate against `model_selection_options`, with no static-registry fallback
/// on an empty catalog, removed entry, non-selectable entry or database error.
/// The existing registry is used only to normalize known aliases. Reasoning
/// support comes solely from `metadata_json.supports_reasoning_effort`.
pub async fn validate_model_configuration(
    pool: &PgPool,
    model: &str,
    thinking_effort: Option<ThinkingEffort>,
) -> Result<ModelCapabilities, DenError> {
    let mut connection = pool.acquire().await?;
    validate_on_connection(&mut connection, model, thinking_effort).await
}

pub(super) async fn validate_on_connection(
    connection: &mut PgConnection,
    model: &str,
    thinking_effort: Option<ThinkingEffort>,
) -> Result<ModelCapabilities, DenError> {
    let raw = model.trim();
    if raw.is_empty() || den_llm::model_registry::is_routing_wildcard_model_handle(raw) {
        return Err(DenError::ValidationError(
            "a concrete selectable model is required".into(),
        ));
    }
    let canonical = den_llm::model_registry::resolve_model_handle(raw).unwrap_or(raw);
    let row = sqlx::query!(
        r#"SELECT handle, selectable, metadata_json AS "metadata_json!: Json<CatalogCapabilities>"
           FROM model_selection_options
           WHERE handle = $1 OR handle = $2
           ORDER BY (handle = $1) DESC LIMIT 1"#,
        canonical,
        raw,
    )
    .fetch_optional(connection)
    .await?
    .ok_or_else(|| DenError::ValidationError(format!("model is not in the Den catalog: {raw}")))?;
    if !row.selectable {
        return Err(DenError::ValidationError(format!(
            "model is no longer selectable: {}",
            row.handle
        )));
    }
    let support = row.metadata_json.0.supports_reasoning_effort;
    validate_thinking_effort(support, thinking_effort)?;
    Ok(ModelCapabilities {
        model_handle: ModelHandle::from_catalog(row.handle),
        supports_reasoning_effort: support,
    })
}
