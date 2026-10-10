//! Bear-authenticated gateway authority at the web boundary, never a global catalog fallback.

use den_core::{
    ids::{BearId, ModelConfigurationId},
    model_availability::ModelAvailabilityFailure,
    DenError,
};
use den_service::{
    bears::model_configurations,
    bifrost::{BifrostCatalogEntry, BifrostCatalogSnapshot},
};

use crate::{errors::CustomError, AppState};

pub(crate) enum BearModelCatalog {
    Available(BifrostCatalogSnapshot),
    Unavailable(ModelAvailabilityFailure),
}

impl BearModelCatalog {
    pub(crate) async fn load(state: &AppState, bear_id: BearId) -> Result<Self, CustomError> {
        match state
            .bifrost
            .refresh_bear_catalog_snapshot(
                state.sqlx_pool(),
                bear_id.as_uuid(),
                &state.config.den_secret_encryption_key,
            )
            .await
        {
            Ok(snapshot) => Ok(Self::Available(snapshot)),
            Err(DenError::ModelAvailability(failure)) => Ok(Self::Unavailable(failure)),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn require(&self, model: &str) -> Result<&BifrostCatalogEntry, DenError> {
        match self {
            Self::Available(snapshot) => snapshot.require_available_model(model),
            Self::Unavailable(failure) => Err(DenError::ModelAvailability(
                failure.clone().with_model(model),
            )),
        }
    }

    pub(crate) fn diagnostic(&self) -> Option<String> {
        match self {
            Self::Available(_) => None,
            Self::Unavailable(failure) => Some(failure.to_string()),
        }
    }
}

pub(crate) async fn validate_model(
    state: &AppState,
    bear_id: BearId,
    model: &str,
) -> Result<(), CustomError> {
    state
        .bifrost
        .validate_bear_model_selection(
            state.sqlx_pool(),
            bear_id.as_uuid(),
            model,
            &state.config.den_secret_encryption_key,
        )
        .await?;
    Ok(())
}

/// Execution has native explicit-pin outage continuity; new selections never do.
pub(crate) async fn validate_execution(
    state: &AppState,
    bear_id: BearId,
    primary: &model_configurations::ResolvedPrimaryModel,
) -> Result<(), CustomError> {
    state
        .bifrost
        .validate_bear_model_execution(
            state.sqlx_pool(),
            bear_id.as_uuid(),
            &primary.model_handle,
            &state.config.den_secret_encryption_key,
            primary.source,
        )
        .await?;
    Ok(())
}

/// Clearing a reference is always possible, even when inheritance is currently unavailable.
pub(crate) async fn validate_configuration_selection(
    state: &AppState,
    bear_id: BearId,
    id: Option<ModelConfigurationId>,
) -> Result<(), CustomError> {
    if let Some(id) = id {
        let configuration = model_configurations::get(state.sqlx_pool(), bear_id, id)
            .await?
            .ok_or_else(|| {
                CustomError::NotFound("Model configuration does not belong to this Bear.".into())
            })?;
        model_configurations::validate_model_configuration(
            state.sqlx_pool(),
            configuration.model_handle.as_str(),
            configuration.thinking_effort,
        )
        .await?;
        validate_model(state, bear_id, configuration.model_handle.as_str()).await?;
    }
    Ok(())
}

pub(crate) fn form_failure(error: &CustomError) -> Option<(axum::http::StatusCode, String)> {
    match error {
        CustomError::ValidationError(message) => {
            Some((axum::http::StatusCode::BAD_REQUEST, message.clone()))
        }
        CustomError::ModelAvailability(failure) => {
            Some((availability_status(failure), failure.to_string()))
        }
        _ => None,
    }
}

pub(crate) fn availability_status(failure: &ModelAvailabilityFailure) -> axum::http::StatusCode {
    use den_core::model_availability::ModelAvailabilityFailureKind;
    match failure.kind {
        ModelAvailabilityFailureKind::ModelMissing
        | ModelAvailabilityFailureKind::ModelUnavailable => axum::http::StatusCode::BAD_REQUEST,
        ModelAvailabilityFailureKind::VirtualKeyMissing
        | ModelAvailabilityFailureKind::VirtualKeyRejected => axum::http::StatusCode::CONFLICT,
        ModelAvailabilityFailureKind::CatalogUnavailable => {
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        }
    }
}
