//! Shared bear create/edit form context and validation (operator admin UI + member-owned bears).

use minijinja::context;
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    core::{
        bears::{db as bears_db, Bear},
        letta::{LettaAgentDiagnostics, LettaModelOption, LettaToolOption},
    },
    errors::CustomError,
    web::AppState,
};

/// Operator and member `<select>` for Letta `agent_type` (subset of Letta `AgentType`; empty = server default).
#[derive(Serialize)]
pub struct AgentTypeSelectRow {
    pub value: &'static str,
    pub label: &'static str,
}

pub const LETTA_AGENT_TYPE_ROWS: &[AgentTypeSelectRow] = &[
    AgentTypeSelectRow {
        value: "",
        label: "Letta default",
    },
    AgentTypeSelectRow {
        value: "memgpt_agent",
        label: "memgpt_agent",
    },
    AgentTypeSelectRow {
        value: "memgpt_v2_agent",
        label: "memgpt_v2_agent",
    },
    AgentTypeSelectRow {
        value: "letta_v1_agent",
        label: "letta_v1_agent",
    },
    AgentTypeSelectRow {
        value: "react_agent",
        label: "react_agent",
    },
    AgentTypeSelectRow {
        value: "workflow_agent",
        label: "workflow_agent",
    },
    AgentTypeSelectRow {
        value: "split_thread_agent",
        label: "split_thread_agent",
    },
    AgentTypeSelectRow {
        value: "voice_convo_agent",
        label: "voice_convo_agent",
    },
];

/// When Letta is enabled, `GET /v1/models/` populates the bear model `<select>`.
pub async fn letta_model_select_context(
    state: &AppState,
) -> (bool, Vec<LettaModelOption>, Option<String>) {
    if !state.letta.is_enabled() {
        return (false, Vec::new(), None);
    }
    match state.letta.list_llm_models().await {
        Ok(models) if models.is_empty() => (
            true,
            models,
            Some("Letta returned no LLM models.".into()),
        ),
        Ok(models) => (true, models, None),
        Err(e) => (
            true,
            Vec::new(),
            Some(format!(
                "Could not load models from Letta: {e}. You can still type a model handle below if you know it."
            )),
        ),
    }
}

/// If the bear already has a `default_model` not returned by Letta, keep it selectable (legacy / BYOK).
pub fn ensure_stored_model_in_options_for_handle(
    stored_model: Option<&str>,
    mut options: Vec<LettaModelOption>,
) -> Vec<LettaModelOption> {
    if let Some(h) = stored_model.map(str::trim).filter(|s| !s.is_empty()) {
        if !options.iter().any(|m| m.handle == h) {
            options.insert(
                0,
                LettaModelOption {
                    handle: h.to_string(),
                    label: format!("{h} (stored on bear)"),
                },
            );
        }
    }
    options
}

/// When Letta is enabled, `GET /v1/tools/` populates the bear tools `<select multiple>`.
pub async fn letta_tool_select_context(
    state: &AppState,
) -> (bool, Vec<LettaToolOption>, Option<String>) {
    if !state.letta.is_enabled() {
        return (false, Vec::new(), None);
    }
    match state.letta.list_tools().await {
        Ok(tools) => (true, tools, None),
        Err(e) => (
            true,
            Vec::new(),
            Some(format!("Could not load tools from Letta: {e}")),
        ),
    }
}

pub fn ensure_stored_tools_in_options_ids(
    stored_ids: &[String],
    mut options: Vec<LettaToolOption>,
) -> Vec<LettaToolOption> {
    for id in stored_ids {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        if !options.iter().any(|t| t.id == id) {
            options.insert(
                0,
                LettaToolOption {
                    id: id.to_string(),
                    label: format!("{id} (stored on bear)"),
                },
            );
        }
    }
    options.sort_by(|a, b| a.label.cmp(&b.label));
    options
}

pub fn validate_default_model_for_letta(
    letta_fetch: &Option<Result<Vec<LettaModelOption>, CustomError>>,
    default_model_trim: &str,
    validation_errors: &mut ValidationErrors,
) {
    let Some(res) = letta_fetch else {
        return;
    };

    match res {
        Err(_) => {
            if default_model_trim.is_empty() {
                validation_errors.add(
                    "default_model",
                    ValidationError::new(
                        "Model is required when Letta is configured. Enter a valid model handle.",
                    ),
                );
            }
        }
        Ok(models) if models.is_empty() => {
            validation_errors.add(
                "default_model",
                ValidationError::new(
                    "Letta has no LLM models available; configure models in Letta before creating bears.",
                ),
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
            if !models.iter().any(|m| m.handle == default_model_trim) {
                validation_errors.add(
                    "default_model",
                    ValidationError::new("Pick a model from the list."),
                );
            }
        }
    }
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
    #[validate(length(max = 64))]
    pub letta_agent_type: String,
    #[serde(default)]
    pub letta_tool_ids: Vec<String>,
}

impl From<&Bear> for BearConfigurationEditForm {
    fn from(bear: &Bear) -> Self {
        Self {
            default_model: bear.default_model.clone().unwrap_or_default(),
            letta_agent_type: bear.letta_agent_type.clone().unwrap_or_default(),
            letta_tool_ids: bear.letta_tool_ids.0.clone(),
        }
    }
}

/// Model + tool `<select>`s for `/bear/{slug}/details/edit/configuration`.
pub async fn bear_configuration_page_context(
    state: &AppState,
    bear: &Bear,
    form: &BearConfigurationEditForm,
) -> minijinja::Value {
    let (letta_configured, letta_model_options, letta_models_fetch_error) =
        letta_model_select_context(state).await;
    let model_trim = form.default_model.trim();
    let model_handle = (!model_trim.is_empty()).then_some(model_trim);
    let letta_model_options = if letta_configured {
        ensure_stored_model_in_options_for_handle(model_handle, letta_model_options)
    } else {
        letta_model_options
    };
    let form_tool_ids: Vec<String> = form
        .letta_tool_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let (letta_tools_configured, mut letta_tool_options, letta_tools_fetch_error) =
        letta_tool_select_context(state).await;
    if letta_tools_configured {
        letta_tool_options = ensure_stored_tools_in_options_ids(&form_tool_ids, letta_tool_options);
    }

    let (letta_diagnostics, letta_agent_fetch_warn): (
        Option<LettaAgentDiagnostics>,
        Option<String>,
    ) = if state.letta.is_enabled() {
        if let Some(agent_id) = bear
            .letta_agent_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match state.letta.fetch_agent(agent_id).await {
                Ok(v) => (Some(LettaAgentDiagnostics::from_agent_json(&v)), None),
                Err(e) => (None, Some(e.to_string())),
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    context! {
        letta_configured,
        letta_model_options,
        letta_models_fetch_error,
        letta_tools_configured,
        letta_tool_options,
        letta_tools_fetch_error,
        letta_agent_type_rows => LETTA_AGENT_TYPE_ROWS,
        letta_diagnostics => letta_diagnostics,
        letta_agent_fetch_warn => letta_agent_fetch_warn,
    }
}

#[derive(Validate, Serialize, Deserialize, Debug, Default)]
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
    /// Letta `agent_type` (`<select>` value); empty string = Letta default.
    #[validate(length(max = 64))]
    pub letta_agent_type: String,
    /// Letta `tool_ids` from `<select name="letta_tool_ids" multiple>`.
    #[serde(default)]
    pub letta_tool_ids: Vec<String>,
    /// When set (hidden field), Den links this Letta agent instead of calling `POST /v1/agents`.
    #[serde(default)]
    #[validate(length(max = 256))]
    pub attach_letta_agent_id: String,
}

impl From<&Bear> for NewBearForm {
    fn from(bear: &Bear) -> Self {
        Self {
            slug: bear.slug.clone(),
            name: bear.name.clone(),
            description: bear.description.clone(),
            system_prompt: bear.system_prompt.clone(),
            default_model: bear.default_model.clone().unwrap_or_default(),
            letta_agent_type: bear.letta_agent_type.clone().unwrap_or_default(),
            letta_tool_ids: bear.letta_tool_ids.0.clone(),
            attach_letta_agent_id: String::new(),
        }
    }
}

/// Letta model + tool lists for the new-bear template, merging stored handles like the edit page.
pub async fn bear_new_form_context(state: &AppState, form: &NewBearForm) -> minijinja::Value {
    let (letta_configured, letta_model_options, letta_models_fetch_error) =
        letta_model_select_context(state).await;
    let model_trim = form.default_model.trim();
    let model_handle = (!model_trim.is_empty()).then_some(model_trim);
    let letta_model_options = if letta_configured {
        ensure_stored_model_in_options_for_handle(model_handle, letta_model_options)
    } else {
        letta_model_options
    };

    let form_tool_ids: Vec<String> = form
        .letta_tool_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let (letta_tools_configured, mut letta_tool_options, letta_tools_fetch_error) =
        letta_tool_select_context(state).await;
    if letta_tools_configured {
        letta_tool_options = ensure_stored_tools_in_options_ids(&form_tool_ids, letta_tool_options);
    }

    context! {
        letta_configured,
        letta_model_options,
        letta_models_fetch_error,
        letta_tools_configured,
        letta_tool_options,
        letta_tools_fetch_error,
        letta_agent_type_rows => LETTA_AGENT_TYPE_ROWS,
    }
}

/// Edit bear template: merged model/tool lists + optional Letta agent diagnostics.
pub async fn bear_edit_page_context(
    state: &AppState,
    bear: &Bear,
    form: &NewBearForm,
) -> minijinja::Value {
    let (letta_configured, letta_model_options, letta_models_fetch_error) =
        letta_model_select_context(state).await;
    let model_trim = form.default_model.trim();
    let model_handle = (!model_trim.is_empty()).then_some(model_trim);
    let letta_model_options = if letta_configured {
        ensure_stored_model_in_options_for_handle(model_handle, letta_model_options)
    } else {
        letta_model_options
    };
    let form_tool_ids: Vec<String> = form
        .letta_tool_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let (letta_tools_configured, mut letta_tool_options, letta_tools_fetch_error) =
        letta_tool_select_context(state).await;
    if letta_tools_configured {
        letta_tool_options = ensure_stored_tools_in_options_ids(&form_tool_ids, letta_tool_options);
    }

    let (letta_diagnostics, letta_agent_fetch_warn): (
        Option<LettaAgentDiagnostics>,
        Option<String>,
    ) = if state.letta.is_enabled() {
        if let Some(agent_id) = bear
            .letta_agent_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match state.letta.fetch_agent(agent_id).await {
                Ok(v) => (Some(LettaAgentDiagnostics::from_agent_json(&v)), None),
                Err(e) => (None, Some(e.to_string())),
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    context! {
        letta_configured,
        letta_model_options,
        letta_models_fetch_error,
        letta_tools_configured,
        letta_tool_options,
        letta_tools_fetch_error,
        letta_agent_type_rows => LETTA_AGENT_TYPE_ROWS,
        letta_diagnostics => letta_diagnostics,
        letta_agent_fetch_warn => letta_agent_fetch_warn,
    }
}

/// Shared DB write for creating a bear row (operator or member flow).
pub async fn insert_new_bear_row(
    pool: &sqlx::PgPool,
    form: &NewBearForm,
    letta_tool_ids: Vec<String>,
    letta_agent_type_db: Option<String>,
    default_model_opt: Option<&str>,
) -> Result<Uuid, CustomError> {
    bears_db::create_bear(
        pool,
        form.slug.trim(),
        form.name.trim(),
        form.description.trim(),
        form.system_prompt.trim(),
        default_model_opt,
        None::<Json<serde_json::Value>>,
        letta_agent_type_db.as_deref(),
        Json(letta_tool_ids),
    )
    .await
}
