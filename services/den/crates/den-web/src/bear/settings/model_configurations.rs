//! Named primary model settings. Den owns metadata; the Bear's gateway catalog owns availability.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::{
    ids::{BearId, HatId, ModelConfigurationId},
    DenError, ThinkingEffort,
};
use den_service::bears::{model_configurations as service, Bear};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::{load_session_bear_manage, render_models_page};
use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    model_availability::{self, BearModelCatalog},
    AppState,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bear/{slug}/models/configurations", post(create))
        .route_with_tsr("/bear/{slug}/models/configurations/{id}", post(update))
        .route_with_tsr(
            "/bear/{slug}/models/configurations/{id}/delete",
            post(delete),
        )
        .route_with_tsr("/bear/{slug}/models/default", post(set_default))
}

#[derive(Debug, Serialize)]
pub(super) struct CatalogModelOption {
    handle: String,
    label: String,
}

pub(super) async fn catalog_options(
    pool: &PgPool,
    catalog: &BearModelCatalog,
) -> Result<Vec<CatalogModelOption>, CustomError> {
    // Den metadata labels selectable models, but only Bear-authenticated gateway membership
    // makes them usable. No management, static registry, or stale-cache fallback.
    Ok(sqlx::query_as!(CatalogModelOption,
        "SELECT handle, display_name AS label FROM model_selection_options WHERE selectable = TRUE ORDER BY COALESCE(sort_order, 100000), display_name, handle"
    ).fetch_all(pool).await?.into_iter().filter(|option| catalog.require(&option.handle).is_ok()).collect())
}

pub(crate) async fn selectable_model_options(
    pool: &PgPool,
    catalog: &BearModelCatalog,
) -> Result<Vec<den_llm::ModelOption>, CustomError> {
    Ok(catalog_options(pool, catalog)
        .await?
        .into_iter()
        .map(|option| den_llm::ModelOption {
            handle: option.handle,
            label: option.label,
            context_window: None,
            max_output_tokens: None,
        })
        .collect())
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct ConfigurationForm {
    pub name: String,
    pub model_handle: String,
    #[serde(default)]
    pub thinking_effort: String,
}

impl ConfigurationForm {
    fn effort(&self) -> Result<Option<ThinkingEffort>, CustomError> {
        match self.thinking_effort.trim() {
            "" | "model_default" => Ok(None),
            "low" => Ok(Some(ThinkingEffort::Low)),
            "medium" => Ok(Some(ThinkingEffort::Medium)),
            "high" => Ok(Some(ThinkingEffort::High)),
            _ => Err(CustomError::ValidationError(
                "Choose model default, low, medium, or high reasoning effort.".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PendingConfigurationSelection {
    #[default]
    Unchanged,
    Inherit,
    Configuration(ModelConfigurationId),
}

impl PendingConfigurationSelection {
    pub(crate) fn selected_id(
        self,
        stored: Option<ModelConfigurationId>,
    ) -> Option<ModelConfigurationId> {
        match self {
            Self::Unchanged => stored,
            Self::Inherit => None,
            Self::Configuration(id) => Some(id),
        }
    }
}

impl From<Option<ModelConfigurationId>> for PendingConfigurationSelection {
    fn from(selection: Option<ModelConfigurationId>) -> Self {
        match selection {
            None => Self::Inherit,
            Some(id) => Self::Configuration(id),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct PendingModelsForm {
    pub configuration: Option<(Option<ModelConfigurationId>, ConfigurationForm)>,
    pub default_selection: PendingConfigurationSelection,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ConfigurationSelection {
    #[serde(default, deserialize_with = "optional_configuration_id")]
    pub configuration_id: Option<ModelConfigurationId>,
}

fn optional_configuration_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ModelConfigurationId>, D::Error> {
    let raw = Option::<String>::deserialize(deserializer)?;
    raw.filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<ModelConfigurationId>()
                .map_err(serde::de::Error::custom)
        })
        .transpose()
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Availability {
    Available,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConfigurationView {
    pub configuration: service::ModelConfiguration,
    pub fields: ConfigurationForm,
    pub status: Availability,
    pub status_detail: Option<String>,
    pub effort_label: &'static str,
}

pub(crate) fn effort_label(effort: Option<ThinkingEffort>) -> &'static str {
    match effort {
        None => "Model default",
        Some(ThinkingEffort::Low) => "Low",
        Some(ThinkingEffort::Medium) => "Medium",
        Some(ThinkingEffort::High) => "High",
    }
}

async fn availability(
    pool: &PgPool,
    catalog: &BearModelCatalog,
    model: &str,
    effort: Option<ThinkingEffort>,
) -> Result<(Availability, Option<String>), CustomError> {
    let result = async {
        service::validate_model_configuration(pool, model, effort).await?;
        catalog.require(model)?;
        Ok::<_, DenError>(())
    }
    .await;
    match result {
        Ok(()) => Ok((Availability::Available, None)),
        Err(DenError::ModelAvailability(failure)) => {
            Ok((Availability::Unavailable, Some(failure.to_string())))
        }
        Err(DenError::ValidationError(message)) => Ok((Availability::Unavailable, Some(message))),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn configuration_views(
    pool: &PgPool,
    catalog: &BearModelCatalog,
    bear_id: BearId,
    draft: Option<&(Option<ModelConfigurationId>, ConfigurationForm)>,
) -> Result<Vec<ConfigurationView>, CustomError> {
    let mut views = Vec::new();
    for configuration in service::list(pool, bear_id).await? {
        let (status, status_detail) = availability(
            pool,
            catalog,
            configuration.model_handle.as_str(),
            configuration.thinking_effort,
        )
        .await?;
        let fields = draft
            .filter(|(id, _)| *id == Some(configuration.id))
            .map(|(_, fields)| fields.clone())
            .unwrap_or_else(|| ConfigurationForm {
                name: configuration.name.clone(),
                model_handle: configuration.model_handle.to_string(),
                thinking_effort: configuration
                    .thinking_effort
                    .map(ThinkingEffort::as_str)
                    .unwrap_or("model_default")
                    .into(),
            });
        views.push(ConfigurationView {
            effort_label: effort_label(configuration.thinking_effort),
            configuration,
            fields,
            status,
            status_detail,
        });
    }
    Ok(views)
}

#[derive(Debug, Serialize)]
pub(crate) struct EffectiveModelView {
    pub name: Option<String>,
    pub model_handle: String,
    pub effort_label: &'static str,
    pub source_label: &'static str,
    pub status: Availability,
    pub status_detail: Option<String>,
}

/// Keep the selected configuration inspectable even when canonical resolution rejects it.
pub(crate) async fn effective_model(
    pool: &PgPool,
    catalog: &BearModelCatalog,
    bear_id: BearId,
    hat_id: Option<HatId>,
    deployment: &str,
) -> Result<EffectiveModelView, CustomError> {
    let override_id = match hat_id {
        Some(id) => service::hat_configuration_id(pool, bear_id, id).await?,
        None => None,
    };
    let default_id = service::default_configuration_id(pool, bear_id).await?;
    let (id, source_label) = if override_id.is_some() {
        (override_id, "Hat override")
    } else if default_id.is_some() {
        (default_id, "Bear default")
    } else {
        (None, "Deployment default")
    };
    let configuration = match id {
        Some(id) => Some(service::get(pool, bear_id, id).await?.ok_or_else(|| {
            CustomError::ValidationError("Selected model configuration is missing.".into())
        })?),
        None => None,
    };
    let mut view = EffectiveModelView {
        name: configuration.as_ref().map(|config| config.name.clone()),
        model_handle: configuration
            .as_ref()
            .map(|config| config.model_handle.to_string())
            .unwrap_or_else(|| deployment.into()),
        effort_label: effort_label(
            configuration
                .as_ref()
                .and_then(|config| config.thinking_effort),
        ),
        source_label,
        status: Availability::Available,
        status_detail: None,
    };
    match service::resolve_primary(pool, bear_id, hat_id, None, deployment).await {
        Ok(resolved) => {
            view.model_handle = resolved.model_handle;
            if let Err(error) = catalog.require(&view.model_handle) {
                match error {
                    DenError::ModelAvailability(failure) => {
                        view.status = Availability::Unavailable;
                        view.status_detail = Some(failure.to_string());
                    }
                    error => return Err(error.into()),
                }
            }
        }
        Err(DenError::ValidationError(message)) => {
            view.status = Availability::Unavailable;
            view.status_detail = Some(message);
        }
        Err(error) => return Err(error.into()),
    }
    Ok(view)
}

async fn failed_form(
    state: AppState,
    auth: AuthSession,
    bear: Bear,
    error: CustomError,
    pending: PendingModelsForm,
) -> Result<Response, CustomError> {
    if let Some((status, message)) = model_availability::form_failure(&error) {
        let mut response =
            render_models_page(state, auth, bear, true, None, Some(message), pending).await?;
        *response.status_mut() = status;
        Ok(response)
    } else {
        Err(error)
    }
}

fn saved(slug: &str) -> Response {
    Redirect::to(&format!(
        "/bear/{slug}/models?message=Model%20configuration%20saved."
    ))
    .into_response()
}

async fn save(
    state: AppState,
    auth: AuthSession,
    slug: String,
    id: Option<ModelConfigurationId>,
    form: ConfigurationForm,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let result: Result<(), CustomError> = async {
        let effort = form.effort()?;
        service::validate_model_configuration(state.sqlx_pool(), &form.model_handle, effort)
            .await?;
        model_availability::validate_model(&state, BearId::new(bear.id), &form.model_handle)
            .await?;
        match id {
            Some(id) => {
                service::update(
                    state.sqlx_pool(),
                    BearId::new(bear.id),
                    id,
                    &form.name,
                    &form.model_handle,
                    effort,
                )
                .await?
            }
            None => {
                service::create(
                    state.sqlx_pool(),
                    BearId::new(bear.id),
                    &form.name,
                    &form.model_handle,
                    effort,
                )
                .await?
            }
        };
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(saved(&bear.slug)),
        Err(error) => {
            failed_form(
                state,
                auth,
                bear,
                error,
                PendingModelsForm {
                    configuration: Some((id, form)),
                    ..Default::default()
                },
            )
            .await
        }
    }
}

async fn create(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<ConfigurationForm>,
) -> Result<Response, CustomError> {
    save(state, auth, slug, None, form).await
}

async fn update(
    Path((slug, id)): Path<(String, uuid::Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<ConfigurationForm>,
) -> Result<Response, CustomError> {
    save(state, auth, slug, Some(ModelConfigurationId::new(id)), form).await
}

async fn delete(
    Path((slug, id)): Path<(String, uuid::Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    match service::delete(
        state.sqlx_pool(),
        BearId::new(bear.id),
        ModelConfigurationId::new(id),
    )
    .await
    {
        Ok(()) => Ok(saved(&bear.slug)),
        Err(error) => {
            failed_form(
                state,
                auth,
                bear,
                error.into(),
                PendingModelsForm::default(),
            )
            .await
        }
    }
}

async fn set_default(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<ConfigurationSelection>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let result: Result<(), CustomError> = async {
        model_availability::validate_configuration_selection(
            &state,
            BearId::new(bear.id),
            form.configuration_id,
        )
        .await?;
        service::set_default(
            state.sqlx_pool(),
            BearId::new(bear.id),
            form.configuration_id,
        )
        .await?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(saved(&bear.slug)),
        Err(error) => {
            failed_form(
                state,
                auth,
                bear,
                error,
                PendingModelsForm {
                    default_selection: form.configuration_id.into(),
                    ..Default::default()
                },
            )
            .await
        }
    }
}
