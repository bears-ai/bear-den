use den_core::{
    ids::{BearId, ModelConfigurationId},
    DenError, ThinkingEffort,
};
use sqlx::PgPool;

use super::{
    types::{write_error, ModelConfigurationRow},
    validate_model_configuration, ModelConfiguration,
};

pub async fn list(pool: &PgPool, bear_id: BearId) -> Result<Vec<ModelConfiguration>, DenError> {
    sqlx::query_as!(
        ModelConfigurationRow,
        r"SELECT id, bear_id, name, model_handle, thinking_effort, created_at, updated_at
          FROM bear_model_configurations WHERE bear_id = $1 ORDER BY lower(name), id",
        bear_id.as_uuid(),
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub async fn get(
    pool: &PgPool,
    bear_id: BearId,
    id: ModelConfigurationId,
) -> Result<Option<ModelConfiguration>, DenError> {
    sqlx::query_as!(
        ModelConfigurationRow,
        r"SELECT id, bear_id, name, model_handle, thinking_effort, created_at, updated_at
          FROM bear_model_configurations WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(),
        id.as_uuid(),
    )
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub async fn create(
    pool: &PgPool,
    bear_id: BearId,
    name: &str,
    model: &str,
    thinking_effort: Option<ThinkingEffort>,
) -> Result<ModelConfiguration, DenError> {
    let name = validate_name(name)?;
    let model = validate_model_configuration(pool, model, thinking_effort).await?;
    sqlx::query_as!(
        ModelConfigurationRow,
        r"INSERT INTO bear_model_configurations (bear_id, name, model_handle, thinking_effort)
          VALUES ($1, $2, $3, $4)
          RETURNING id, bear_id, name, model_handle, thinking_effort, created_at, updated_at",
        bear_id.as_uuid(),
        name,
        model.model_handle.as_str(),
        thinking_effort.map(ThinkingEffort::as_str),
    )
    .fetch_one(pool)
    .await
    .map_err(write_error)?
    .try_into()
}

pub async fn update(
    pool: &PgPool,
    bear_id: BearId,
    id: ModelConfigurationId,
    name: &str,
    model: &str,
    thinking_effort: Option<ThinkingEffort>,
) -> Result<ModelConfiguration, DenError> {
    let name = validate_name(name)?;
    let model = validate_model_configuration(pool, model, thinking_effort).await?;
    sqlx::query_as!(
        ModelConfigurationRow,
        r"UPDATE bear_model_configurations
          SET name = $3, model_handle = $4, thinking_effort = $5, updated_at = now()
          WHERE bear_id = $1 AND id = $2
          RETURNING id, bear_id, name, model_handle, thinking_effort, created_at, updated_at",
        bear_id.as_uuid(),
        id.as_uuid(),
        name,
        model.model_handle.as_str(),
        thinking_effort.map(ThinkingEffort::as_str),
    )
    .fetch_optional(pool)
    .await
    .map_err(write_error)?
    .ok_or_else(|| DenError::NotFound("model configuration not found for this Bear".into()))?
    .try_into()
}

/// Referenced configurations must be unbound explicitly; deletion never turns
/// an explicit selection into inheritance. The database FK closes the race.
pub async fn delete(
    pool: &PgPool,
    bear_id: BearId,
    id: ModelConfigurationId,
) -> Result<(), DenError> {
    let result = sqlx::query!(
        "DELETE FROM bear_model_configurations WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(),
        id.as_uuid(),
    )
    .execute(pool)
    .await
    .map_err(write_error)?;
    if result.rows_affected() == 0 {
        return Err(DenError::NotFound(
            "model configuration not found for this Bear".into(),
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<&str, DenError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DenError::ValidationError(
            "model configuration name is required".into(),
        ));
    }
    Ok(name)
}
