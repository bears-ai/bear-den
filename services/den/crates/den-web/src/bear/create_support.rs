//! Shared bear create/edit form context and validation (operator admin UI + member-owned bears).

use minijinja::context;
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{errors::CustomError, web::AppState};
use den_llm::ModelOption;
use den_service::bears::{
    context_composition::{
        BearContextProfile, RoleContracts, CONTEXT_PROFILE_VERSION, DEFAULT_ROLE_CONTRACT_VERSION,
    },
    context_profile_from_json, context_profile_to_json, db as bears_db,
    db::BearParams,
    templates::first_bear_template,
    Bear, BearProfile,
};

pub const BEAR_SLUG_VALIDATION_MESSAGE: &str =
    "Use lowercase letters, numbers, and single hyphens.";

pub fn bear_slug_base(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.trim().to_ascii_lowercase().chars() {
        let mapped = if ch.is_ascii_alphanumeric() { ch } else { '-' };
        if mapped == '-' {
            if !last_dash && !out.is_empty() {
                out.push('-');
                last_dash = true;
            }
        } else {
            out.push(mapped);
            last_dash = false;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "imported-bear".to_string()
    } else {
        trimmed
    }
}

pub fn is_valid_bear_slug(raw: &str) -> bool {
    let slug = raw.trim();
    !slug.is_empty()
        && slug.len() <= 120
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--")
}

/// If the bear already has a `default_model` not returned by the catalog, keep it selectable (legacy / BYOK).
pub fn ensure_stored_model_in_options_for_handle(
    stored_model: Option<&str>,
    mut options: Vec<ModelOption>,
) -> Vec<ModelOption> {
    if let Some(h) = stored_model.map(str::trim).filter(|s| !s.is_empty()) {
        if !options.iter().any(|m| m.handle == h) {
            options.insert(
                0,
                ModelOption {
                    handle: h.to_string(),
                    label: format!("{h} (stored on bear)"),
                    context_window: None,
                    max_output_tokens: None,
                },
            );
        }
    }
    options
}

#[derive(Validate, Serialize, Deserialize, Debug, Clone)]
pub struct BearOverviewEditForm {
    #[validate(length(min = 1, max = 120))]
    pub slug: String,
    #[validate(length(max = 255))]
    pub name: String,
    #[validate(length(max = 2000))]
    pub description: String,
}

impl From<&Bear> for BearOverviewEditForm {
    fn from(bear: &Bear) -> Self {
        Self {
            slug: bear.slug.clone(),
            name: bear.name.clone(),
            description: bear.description.clone(),
        }
    }
}

#[derive(Validate, Serialize, Deserialize, Debug, Clone)]
pub struct BearPromptEditForm {
    #[validate(length(max = 100_000))]
    pub system_prompt: String,
}

impl From<&Bear> for BearPromptEditForm {
    fn from(bear: &Bear) -> Self {
        Self {
            system_prompt: bear.system_prompt.clone(),
        }
    }
}

#[derive(Validate, Serialize, Deserialize, Debug, Clone)]
pub struct BearConfigurationEditForm {
    #[validate(length(max = 255))]
    pub default_model: String,
}

impl From<&Bear> for BearConfigurationEditForm {
    fn from(bear: &Bear) -> Self {
        Self {
            default_model: bear.default_model.clone().unwrap_or_default(),
        }
    }
}

/// Model select for `/bear/{slug}/edit/configuration` (Bifrost availability enriched by Den metadata).
pub async fn bear_configuration_page_context(
    state: &AppState,
    _bear: &Bear,
    form: &BearConfigurationEditForm,
) -> minijinja::Value {
    let (model_catalog_configured, model_options, models_fetch_error) =
        model_catalog_select_context(state).await;
    let model_trim = form.default_model.trim();
    let model_handle = (!model_trim.is_empty()).then_some(model_trim);
    let model_options = if model_catalog_configured {
        ensure_stored_model_in_options_for_handle(model_handle, model_options)
    } else {
        model_options
    };

    context! {
        native_runtime => true,
        model_catalog_configured,
        model_options,
        models_fetch_error,
    }
}

#[derive(Validate, Serialize, Deserialize, Debug, Default, Clone)]
pub struct NewBearForm {
    #[validate(length(min = 1, max = 120))]
    pub slug: String,
    #[validate(length(max = 255))]
    pub name: String,
    #[validate(length(max = 2000))]
    pub description: String,
    #[validate(length(max = 100_000))]
    pub system_prompt: String,
    #[validate(length(max = 255))]
    pub default_model: String,
}

impl From<&Bear> for NewBearForm {
    fn from(bear: &Bear) -> Self {
        Self {
            slug: bear.slug.clone(),
            name: bear.name.clone(),
            description: bear.description.clone(),
            system_prompt: bear.system_prompt.clone(),
            default_model: bear.default_model.clone().unwrap_or_default(),
        }
    }
}

/// Operator admin new-bear form: Bifrost availability enriched by Den metadata.
pub async fn admin_bear_new_form_context(state: &AppState, form: &NewBearForm) -> minijinja::Value {
    let (model_catalog_configured, model_options, models_fetch_error) =
        model_catalog_select_context(state).await;
    let model_trim = form.default_model.trim();
    let model_handle = (!model_trim.is_empty()).then_some(model_trim);
    let model_options = if model_catalog_configured {
        ensure_stored_model_in_options_for_handle(model_handle, model_options)
    } else {
        model_options
    };

    context! {
        model_catalog_configured,
        model_options,
        models_fetch_error,
        native_runtime => true,
    }
}

pub(crate) fn model_option_from_bifrost_metadata(
    model: den_service::bifrost::BifrostModelMetadata,
) -> ModelOption {
    den_llm::model_registry::model_option_for_available_handle(
        &model.handle,
        model.display_name.as_deref(),
        (model.context_window > 0).then_some(model.context_window),
        model.max_output_tokens,
    )
}

fn model_options_from_bifrost_models(
    models: Vec<den_service::bifrost::BifrostModelMetadata>,
) -> Vec<ModelOption> {
    let mut options = models
        .into_iter()
        .map(model_option_from_bifrost_metadata)
        .collect::<Vec<_>>();
    options.sort_by(|a, b| a.label.cmp(&b.label));
    options
}

/// Full Bear-scoped Bifrost availability list enriched by Den metadata where known.
pub async fn all_model_catalog_options_context_for_bear(
    state: &AppState,
    bear_id: Uuid,
) -> (bool, Vec<ModelOption>, Option<String>) {
    if !state.bifrost.is_enabled() {
        return (false, Vec::new(), None);
    }

    let snapshot = match state
        .bifrost
        .bear_catalog_snapshot(
            state.sqlx_pool(),
            bear_id,
            &state.config.den_secret_encryption_key,
        )
        .await
    {
        Ok(snapshot) => snapshot,
        Err(err) => {
            return (
                true,
                Vec::new(),
                Some(format!(
                    "Could not load Bear-scoped Bifrost model catalog: {err}."
                )),
            );
        }
    };
    let models = snapshot.models_vec();
    if models.is_empty() {
        return (
            true,
            Vec::new(),
            Some("Bear-scoped Bifrost model catalog snapshot is empty.".into()),
        );
    }
    (true, model_options_from_bifrost_models(models), None)
}

/// Full Bifrost availability list enriched by Den metadata where known.
pub async fn all_model_catalog_options_context(
    state: &AppState,
) -> (bool, Vec<ModelOption>, Option<String>) {
    if !state.bifrost.is_enabled() {
        return (false, Vec::new(), None);
    }

    let models = match state.bifrost_catalog.read() {
        Ok(snapshot) => snapshot.models_vec(),
        Err(_) => {
            return (
                true,
                Vec::new(),
                Some("Could not read Bifrost model catalog snapshot.".into()),
            );
        }
    };
    if models.is_empty() {
        return (
            true,
            Vec::new(),
            Some("Bifrost model catalog snapshot is empty.".into()),
        );
    }
    (true, model_options_from_bifrost_models(models), None)
}

pub fn curated_model_options_from_all(all_options: &[ModelOption]) -> Vec<ModelOption> {
    all_options
        .iter()
        .filter(|option| {
            den_llm::model_registry::entry_for_handle(&option.handle)
                .map(|entry| entry.selectable)
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Stable Den-owned model list for normal Bear Admin/web selectors.
///
/// Bifrost availability is used as status/validation metadata elsewhere; the
/// primary user-facing selector should not flicker when Bifrost's live catalog
/// temporarily shrinks or expands.
pub async fn model_catalog_select_context(
    state: &AppState,
) -> (bool, Vec<ModelOption>, Option<String>) {
    match den_service::model_selection::list_selectable_model_options(state.sqlx_pool()).await {
        Ok(options) if options.is_empty() => (
            true,
            Vec::new(),
            Some("No Den model selection options are configured.".into()),
        ),
        Ok(options) => (true, options, None),
        Err(err) => (
            true,
            den_llm::model_registry::selectable_model_options(),
            Some(format!(
                "Could not load Den model selection options; using static fallback: {err}."
            )),
        ),
    }
}

pub fn default_model_available_in_catalog(models: &[ModelOption], requested: &str) -> bool {
    let requested = requested.trim();
    let requested_resolved = den_llm::model_registry::resolve_model_handle(requested);
    models.iter().any(|model| {
        if model.handle == requested {
            return true;
        }
        let Some(resolved) = requested_resolved else {
            return false;
        };
        resolved == model.handle
            || den_llm::model_registry::resolve_model_handle(&model.handle) == Some(resolved)
    })
}

pub fn validate_default_model_for_catalog(
    catalog_fetch: &Option<Result<Vec<ModelOption>, CustomError>>,
    default_model_trim: &str,
    validation_errors: &mut ValidationErrors,
) {
    let Some(res) = catalog_fetch else {
        return;
    };

    match res {
        Err(_) => {
            if default_model_trim.is_empty() {
                validation_errors.add(
                    "default_model",
                    ValidationError::new(
                        "Model is required when Bifrost is configured. Enter a valid model handle.",
                    ),
                );
            }
        }
        Ok(models) if models.is_empty() => {
            validation_errors.add(
                "default_model",
                ValidationError::new("No Den model selection options are configured."),
            );
        }
        Ok(models) => {
            if default_model_trim.is_empty() {
                validation_errors.add(
                    "default_model",
                    ValidationError::new("Choose a model from the list."),
                );
                return;
            }
            if !default_model_available_in_catalog(models, default_model_trim) {
                validation_errors.add(
                    "default_model",
                    ValidationError::new("Pick a configured Den model selection option."),
                );
            }
        }
    }
}

pub fn canonical_default_model_handle(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(
            den_llm::model_registry::resolve_model_handle(trimmed)
                .unwrap_or(trimmed)
                .to_string(),
        )
    }
}

/// Native model list for the new-bear template, merging stored handles like the edit page.
pub async fn bear_new_form_context(state: &AppState, form: &NewBearForm) -> minijinja::Value {
    let (model_catalog_configured, model_options, models_fetch_error) =
        model_catalog_select_context(state).await;
    let model_trim = form.default_model.trim();
    let model_handle = (!model_trim.is_empty()).then_some(model_trim);
    let model_options = if model_catalog_configured {
        ensure_stored_model_in_options_for_handle(model_handle, model_options)
    } else {
        model_options
    };

    context! {
        native_runtime => true,
        model_catalog_configured,
        model_options,
        models_fetch_error,
    }
}

/// Edit bear template: merged model/tool lists. Per-role diagnostics live on the detail page.
pub async fn bear_edit_page_context(
    state: &AppState,
    _bear: &Bear,
    form: &NewBearForm,
) -> minijinja::Value {
    admin_bear_edit_page_context(state, form).await
}

/// Operator admin edit-bear form: Bifrost catalog for Den-native profiles.
pub async fn admin_bear_edit_page_context(
    state: &AppState,
    form: &NewBearForm,
) -> minijinja::Value {
    let (model_catalog_configured, model_options, models_fetch_error) =
        model_catalog_select_context(state).await;
    let model_trim = form.default_model.trim();
    let model_handle = (!model_trim.is_empty()).then_some(model_trim);
    let model_options = if model_catalog_configured {
        ensure_stored_model_in_options_for_handle(model_handle, model_options)
    } else {
        model_options
    };
    context! {
        native_runtime => true,
        model_catalog_configured,
        model_options,
        models_fetch_error,
    }
}

#[derive(Validate, Serialize, Deserialize, Debug, Clone, Default)]
pub struct AdminBearPromptForm {
    #[validate(length(max = 100_000))]
    pub system_prompt: String,
    #[validate(length(max = 20_000))]
    pub user_steering: String,
    #[validate(length(max = 20_000))]
    pub bear_context: String,
    #[validate(length(max = 20_000))]
    pub role_chat: String,
    #[validate(length(max = 20_000))]
    pub role_pair: String,
    #[validate(length(max = 20_000))]
    pub role_curate: String,
    #[validate(length(max = 20_000))]
    pub role_work: String,
    #[validate(length(max = 20_000))]
    pub role_watch: String,
}

impl AdminBearPromptForm {
    pub fn from_bear(bear: &Bear) -> Result<(Self, bool), CustomError> {
        let context_profile_enabled = bear.context_profile.is_some();
        if let Some(profile) = context_profile_from_json(&bear.context_profile)? {
            Ok((
                Self {
                    system_prompt: bear.system_prompt.clone(),
                    user_steering: profile.user_steering,
                    bear_context: profile.bear_context,
                    role_chat: profile.role_contracts.chat,
                    role_pair: profile.role_contracts.pair,
                    role_curate: profile.role_contracts.curate,
                    role_work: profile.role_contracts.work,
                    role_watch: profile.role_contracts.watch,
                },
                context_profile_enabled,
            ))
        } else {
            Ok((
                Self {
                    system_prompt: bear.system_prompt.clone(),
                    ..Self::default()
                },
                false,
            ))
        }
    }

    pub fn context_profile_for_bear(
        &self,
        bear: &Bear,
        context_profile_enabled: bool,
    ) -> Result<Option<Json<serde_json::Value>>, CustomError> {
        if !context_profile_enabled {
            return Ok(None);
        }
        let existing = context_profile_from_json(&bear.context_profile)?;
        let profile = BearContextProfile {
            composition_version: existing
                .as_ref()
                .map(|p| p.composition_version)
                .unwrap_or(CONTEXT_PROFILE_VERSION),
            template_id: existing.as_ref().and_then(|p| p.template_id.clone()),
            template_version: existing.as_ref().and_then(|p| p.template_version.clone()),
            role_contract_version: existing
                .as_ref()
                .and_then(|p| p.role_contract_version.clone())
                .or_else(|| Some(DEFAULT_ROLE_CONTRACT_VERSION.to_string())),
            role_contracts: RoleContracts {
                chat: self.role_chat.trim().to_string(),
                pair: self.role_pair.trim().to_string(),
                curate: self.role_curate.trim().to_string(),
                work: self.role_work.trim().to_string(),
                watch: self.role_watch.trim().to_string(),
            },
            user_steering: self.user_steering.trim().to_string(),
            bear_context: self.bear_context.trim().to_string(),
            starter_prompts: existing
                .as_ref()
                .map(|p| p.starter_prompts.clone())
                .unwrap_or_default(),
            first_task: existing.as_ref().and_then(|p| p.first_task.clone()),
        };
        let contracts = &profile.role_contracts;
        if contracts.chat.trim().is_empty() {
            return Err(CustomError::ValidationError(
                "Chat role prompt is required.".to_string(),
            ));
        }
        if contracts.pair.trim().is_empty() {
            return Err(CustomError::ValidationError(
                "Pair role prompt is required.".to_string(),
            ));
        }
        if contracts.watch.trim().is_empty() {
            return Err(CustomError::ValidationError(
                "All role prompts are required for role-aware bears.".to_string(),
            ));
        }
        context_profile_to_json(&profile)
            .map(Some)
            .map_err(CustomError::from)
    }

    pub fn resolved_system_prompt(
        &self,
        bear_name: &str,
        context_profile: &Option<Json<serde_json::Value>>,
    ) -> Result<String, CustomError> {
        if let Some(profile) = context_profile {
            composed_system_prompt_for_profile_json(bear_name, profile)
        } else if self.system_prompt.trim().is_empty() {
            Err(CustomError::ValidationError(
                "System prompt is required.".to_string(),
            ))
        } else {
            Ok(self.system_prompt.trim().to_string())
        }
    }
}

#[derive(Validate, Serialize, Deserialize, Debug, Default, Clone)]
pub struct AdminNewBearForm {
    #[serde(flatten)]
    pub bear: NewBearForm,
    #[serde(default)]
    pub grant_user_id: String,
    #[serde(default)]
    pub grant_role: String,
}

pub fn composed_system_prompt_for_profile_json(
    name: &str,
    context_profile: &Json<serde_json::Value>,
) -> Result<String, CustomError> {
    let bear = Bear {
        id: Uuid::nil(),
        slug: "preview".to_string(),
        name: name.to_string(),
        description: String::new(),
        default_model: None,
        default_tool_budget_multiplier: None,
        tools_enabled: None,
        runtime_plan: None,
        context_profile: Some(context_profile.clone()),
        work_enabled: true,
        cabinet_enabled: true,
        provisioning_version: 1,
        system_prompt: String::new(),
        birthday: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
        live_reflection_enabled: true,
        live_reflection_stale_after_minutes: 30,
        live_reflection_activity_threshold: 20,
        live_reflection_sweep_limit: 25,
    };
    den_service::bears::compose_role_context(&bear, BearProfile::Chat, None)
        .map(|context| context.composed_prompt)
        .map_err(CustomError::from)
}

pub fn build_context_profile_json_for_template(
    template_id: &str,
    bear_name: &str,
    user_steering: &str,
    bear_context: &str,
    first_task: Option<&str>,
) -> Result<Json<serde_json::Value>, CustomError> {
    let template = first_bear_template(template_id).ok_or_else(|| {
        CustomError::ValidationError(format!("unknown first-bear template: {template_id}"))
    })?;
    let profile = template.context_profile(bear_name, user_steering, bear_context, first_task);
    context_profile_to_json(&profile).map_err(CustomError::from)
}

/// Shared DB write for creating a bear row (operator or member flow).
pub async fn insert_new_bear_row(
    pool: &sqlx::PgPool,
    form: &NewBearForm,
    default_model_opt: Option<&str>,
) -> Result<Uuid, CustomError> {
    bears_db::create_bear(
        pool,
        BearParams {
            slug: form.slug.trim(),
            name: form.name.trim(),
            description: form.description.trim(),
            system_prompt: form.system_prompt.trim(),
            default_model: default_model_opt,
            tools_enabled: None::<Json<serde_json::Value>>,
            context_profile: None,
        },
    )
    .await
    .map_err(CustomError::from)
}

/// Shared DB write for creating a role-aware context-profile bear row.
pub async fn provision_bifrost_virtual_key_for_bear(
    state: &AppState,
    bear_id: Uuid,
    bear_slug: &str,
) -> Result<bool, CustomError> {
    den_service::bears::bifrost_key::provision_bifrost_virtual_key_for_bear(
        state.sqlx_pool(),
        &state.config,
        bear_id,
        bear_slug,
    )
    .await
    .map_err(CustomError::from)
}

pub async fn insert_new_bear_row_with_context_profile(
    pool: &sqlx::PgPool,
    form: &NewBearForm,
    default_model_opt: Option<&str>,
    context_profile: Json<serde_json::Value>,
) -> Result<Uuid, CustomError> {
    let system_prompt =
        composed_system_prompt_for_profile_json(form.name.trim(), &context_profile)?;
    bears_db::create_bear_with_context_profile(
        pool,
        BearParams {
            slug: form.slug.trim(),
            name: form.name.trim(),
            description: form.description.trim(),
            system_prompt: system_prompt.trim(),
            default_model: default_model_opt,
            tools_enabled: None::<Json<serde_json::Value>>,
            context_profile: Some(context_profile),
        },
    )
    .await
    .map_err(CustomError::from)
}

#[cfg(test)]
mod model_catalog_tests {
    use super::*;

    fn option(handle: &str) -> ModelOption {
        ModelOption {
            handle: handle.to_string(),
            label: handle.to_string(),
            context_window: None,
            max_output_tokens: None,
        }
    }

    fn has_default_model_error(errors: &ValidationErrors) -> bool {
        errors.field_errors().contains_key("default_model")
    }

    #[test]
    fn validation_accepts_alias_when_canonical_handle_is_available() {
        let fetch = Some(Ok(vec![option("openai/gpt-4.1")]));
        let mut errors = ValidationErrors::new();
        validate_default_model_for_catalog(&fetch, "gpt-4.1", &mut errors);
        assert!(!has_default_model_error(&errors));
    }

    #[test]
    fn validation_accepts_bifrost_available_model_without_den_metadata() {
        let fetch = Some(Ok(vec![option("openai/new-model")]));
        let mut errors = ValidationErrors::new();
        validate_default_model_for_catalog(&fetch, "openai/new-model", &mut errors);
        assert!(!has_default_model_error(&errors));
    }

    #[test]
    fn validation_rejects_den_metadata_model_not_available_in_bifrost() {
        let fetch = Some(Ok(vec![option("openai/gpt-4o")]));
        let mut errors = ValidationErrors::new();
        validate_default_model_for_catalog(&fetch, "gpt-4.1", &mut errors);
        assert!(has_default_model_error(&errors));
    }

    #[test]
    fn validation_rejects_unknown_model_when_only_unrelated_unknown_is_available() {
        let fetch = Some(Ok(vec![option("openai/available-but-unknown")]));
        let mut errors = ValidationErrors::new();
        validate_default_model_for_catalog(&fetch, "openai/not-available-and-unknown", &mut errors);
        assert!(has_default_model_error(&errors));
    }
}
