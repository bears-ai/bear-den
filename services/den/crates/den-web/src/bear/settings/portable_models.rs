//! Portable model preferences: fresh IDs, destination catalog validation, no grants.

use std::collections::{HashMap, HashSet};

use den_core::{
    ids::{BearId, HatId, ModelConfigurationId},
    ThinkingEffort,
};
use den_service::bears::model_configurations as service;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::portable_hats::PortableHat;
use crate::errors::CustomError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PortableModelConfiguration {
    pub original_id: ModelConfigurationId,
    pub name: String,
    pub model_handle: String,
    pub thinking_effort: Option<ThinkingEffort>,
}

pub(super) fn validate(
    configurations: Option<&[PortableModelConfiguration]>,
    default: Option<ModelConfigurationId>,
    hats: &[PortableHat],
) -> Result<(), CustomError> {
    let configurations = configurations.unwrap_or_default();
    if configurations.len() > 100 {
        return Err(CustomError::ValidationError(
            "A bundle may contain at most 100 model configurations.".into(),
        ));
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for configuration in configurations {
        if configuration.name.trim().is_empty() || configuration.model_handle.trim().is_empty() {
            return Err(CustomError::ValidationError(
                "Model configuration name and handle are required.".into(),
            ));
        }
        if !ids.insert(configuration.original_id)
            || !names.insert(configuration.name.trim().to_lowercase())
        {
            return Err(CustomError::ValidationError(
                "Duplicate model configuration identity or name in bundle.".into(),
            ));
        }
    }
    for reference in default
        .into_iter()
        .chain(hats.iter().filter_map(|hat| hat.model_configuration_id))
    {
        if !ids.contains(&reference) {
            return Err(CustomError::ValidationError(
                "Model configuration references must name a configuration in this bundle.".into(),
            ));
        }
    }
    Ok(())
}

/// Run before Bear creation, including for legacy raw defaults.
pub(super) async fn validate_catalog(
    pool: &PgPool,
    configurations: Option<&[PortableModelConfiguration]>,
    legacy_default: Option<&str>,
) -> Result<(), CustomError> {
    if let Some(configurations) = configurations {
        for configuration in configurations {
            service::validate_model_configuration(
                pool,
                &configuration.model_handle,
                configuration.thinking_effort,
            )
            .await?;
        }
    } else if let Some(model) = legacy_default
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        service::validate_model_configuration(pool, model, None).await?;
    }
    Ok(())
}

pub(super) async fn export(
    pool: &PgPool,
    bear_id: BearId,
) -> Result<Vec<PortableModelConfiguration>, CustomError> {
    Ok(service::list(pool, bear_id)
        .await?
        .into_iter()
        .map(|configuration| PortableModelConfiguration {
            original_id: configuration.id,
            name: configuration.name,
            model_handle: configuration.model_handle.into_string(),
            thinking_effort: configuration.thinking_effort,
        })
        .collect())
}

pub(super) fn remap(
    mapping: &HashMap<ModelConfigurationId, ModelConfigurationId>,
    original: Option<ModelConfigurationId>,
) -> Result<Option<ModelConfigurationId>, CustomError> {
    original
        .map(|id| {
            mapping.get(&id).copied().ok_or_else(|| {
                CustomError::ValidationError(
                    "Model configuration reference was not imported.".into(),
                )
            })
        })
        .transpose()
}

pub(super) async fn import(
    pool: &PgPool,
    bear_id: BearId,
    configurations: &[PortableModelConfiguration],
    default: Option<ModelConfigurationId>,
    hats: &[PortableHat],
    hat_mapping: &HashMap<HatId, HatId>,
) -> Result<(), CustomError> {
    validate(Some(configurations), default, hats)?;
    let mut mapping = HashMap::new();
    for configuration in configurations {
        let imported = service::create(
            pool,
            bear_id,
            &configuration.name,
            &configuration.model_handle,
            configuration.thinking_effort,
        )
        .await?;
        mapping.insert(configuration.original_id, imported.id);
    }
    service::set_default(pool, bear_id, remap(&mapping, default)?).await?;
    for hat in hats {
        let imported_hat = hat_mapping.get(&hat.original_id).copied().ok_or_else(|| {
            CustomError::ValidationError("Hat for model configuration was not imported.".into())
        })?;
        service::set_hat_override(
            pool,
            bear_id,
            imported_hat,
            remap(&mapping, hat.model_configuration_id)?,
        )
        .await?;
    }
    Ok(())
}
