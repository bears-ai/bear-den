use den_core::{
    ids::{BearId, HatId, ModelConfigurationId},
    DenError,
};
use sqlx::PgPool;

use super::{
    default_configuration_id, get, hat_configuration_id, validate_model_configuration,
    PrimaryModelSource, ResolvedPrimaryModel,
};

/// Resolve and validate the entire primary model configuration. A conversation
/// pin is only a raw selected model: it carries no named-config identity or
/// inherited thinking effort. No invalid/removed selection silently falls back.
pub async fn resolve_primary(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: Option<HatId>,
    explicit_conversation_pin: Option<&str>,
    deployment_default: &str,
) -> Result<ResolvedPrimaryModel, DenError> {
    if let Some(model) = explicit_conversation_pin {
        return resolve_raw(pool, model, PrimaryModelSource::ConversationPin).await;
    }
    if let Some(hat_id) = hat_id {
        if let Some(id) = hat_configuration_id(pool, bear_id, hat_id).await? {
            return resolve_configuration(pool, bear_id, id, PrimaryModelSource::HatOverride).await;
        }
    }
    if let Some(id) = default_configuration_id(pool, bear_id).await? {
        return resolve_configuration(pool, bear_id, id, PrimaryModelSource::BearDefault).await;
    }
    resolve_raw(
        pool,
        deployment_default,
        PrimaryModelSource::DeploymentDefault,
    )
    .await
}

async fn resolve_configuration(
    pool: &PgPool,
    bear_id: BearId,
    id: ModelConfigurationId,
    source: PrimaryModelSource,
) -> Result<ResolvedPrimaryModel, DenError> {
    let configuration = get(pool, bear_id, id).await?.ok_or_else(|| {
        DenError::NotFound("bound model configuration not found for this Bear".into())
    })?;
    let model = validate_model_configuration(
        pool,
        configuration.model_handle.as_str(),
        configuration.thinking_effort,
    )
    .await?;
    Ok(ResolvedPrimaryModel {
        configuration_id: Some(configuration.id),
        configuration_name: Some(configuration.name),
        model_handle: model.model_handle.into_string(),
        thinking_effort: configuration.thinking_effort,
        source,
    })
}

async fn resolve_raw(
    pool: &PgPool,
    model: &str,
    source: PrimaryModelSource,
) -> Result<ResolvedPrimaryModel, DenError> {
    let model = validate_model_configuration(pool, model, None).await?;
    Ok(ResolvedPrimaryModel {
        configuration_id: None,
        configuration_name: None,
        model_handle: model.model_handle.into_string(),
        thinking_effort: None,
        source,
    })
}
