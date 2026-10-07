use den_core::{
    ids::{BearId, HatId, ModelConfigurationId},
    DenError,
};
use sqlx::PgPool;

use super::{get, types::write_error, validate_model_configuration};

pub async fn default_configuration_id(
    pool: &PgPool,
    bear_id: BearId,
) -> Result<Option<ModelConfigurationId>, DenError> {
    let id = sqlx::query_scalar!(
        "SELECT default_model_configuration_id FROM bears WHERE id = $1",
        bear_id.as_uuid(),
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound("Bear not found".into()))?;
    Ok(id.map(ModelConfigurationId::new))
}

/// `None` means inherit the deployment default, not create a model-less config.
pub async fn set_default(
    pool: &PgPool,
    bear_id: BearId,
    id: Option<ModelConfigurationId>,
) -> Result<(), DenError> {
    validate_binding(pool, bear_id, id).await?;
    let result = sqlx::query!(
        r"UPDATE bears SET default_model_configuration_id = $2, updated_at = now()
          WHERE id = $1",
        bear_id.as_uuid(),
        id.map(ModelConfigurationId::as_uuid),
    )
    .execute(pool)
    .await
    .map_err(write_error)?;
    if result.rows_affected() == 0 {
        return Err(DenError::NotFound("Bear not found".into()));
    }
    Ok(())
}

pub async fn hat_configuration_id(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
) -> Result<Option<ModelConfigurationId>, DenError> {
    let id = sqlx::query_scalar!(
        "SELECT model_configuration_id FROM bear_hats WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound("hat not found for this Bear".into()))?;
    Ok(id.map(ModelConfigurationId::new))
}

/// `None` restores whole-configuration inheritance from the Bear. A selected
/// configuration with no effort does not inherit effort from a lower layer.
pub async fn set_hat_override(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    id: Option<ModelConfigurationId>,
) -> Result<(), DenError> {
    validate_binding(pool, bear_id, id).await?;
    let result = sqlx::query!(
        r"UPDATE bear_hats SET model_configuration_id = $3, updated_at = now()
          WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        id.map(ModelConfigurationId::as_uuid),
    )
    .execute(pool)
    .await
    .map_err(write_error)?;
    if result.rows_affected() == 0 {
        return Err(DenError::NotFound("hat not found for this Bear".into()));
    }
    Ok(())
}

async fn validate_binding(
    pool: &PgPool,
    bear_id: BearId,
    id: Option<ModelConfigurationId>,
) -> Result<(), DenError> {
    if let Some(id) = id {
        let configuration = get(pool, bear_id, id).await?.ok_or_else(|| {
            DenError::NotFound("model configuration not found for this Bear".into())
        })?;
        validate_model_configuration(
            pool,
            configuration.model_handle.as_str(),
            configuration.thinking_effort,
        )
        .await?;
    }
    Ok(())
}
