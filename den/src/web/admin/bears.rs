// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use axum::{
    Router,
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;
use uuid::Uuid;
use minijinja::context;
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::AuthSession,
    core::{
        bears::{db as bears_db, provision, Bear},
        letta::{AgentSummary, LettaModelOption},
    },
    errors::CustomError,
    web::{self, AppState},
};

/// When Letta is enabled, `GET /v1/models/` populates the bear model `<select>`.
async fn letta_model_select_context(
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
fn ensure_stored_model_in_options(bear: &Bear, mut options: Vec<LettaModelOption>) -> Vec<LettaModelOption> {
    if let Some(ref h) = bear.default_model {
        let h = h.trim();
        if !h.is_empty() && !options.iter().any(|m| m.handle == h) {
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

fn validate_default_model_for_letta(
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bears/", get(list_view))
        .route_with_tsr("/bears/new", get(new_view).post(new_action))
        .route_with_tsr(
            "/bears/{id}/edit",
            get(edit_view).post(edit_action),
        )
        .route_with_tsr("/bears/{id}/retry-letta", post(retry_letta_action))
        .route_with_tsr("/bears/{id}", get(detail_view))
}

#[derive(Validate, Serialize, Deserialize, Debug, Default)]
pub struct NewBearForm {
    #[validate(length(min = 1, max = 120))]
    slug: String,
    #[validate(length(max = 255))]
    name: String,
    #[validate(length(max = 2000))]
    description: String,
    #[validate(length(max = 100_000))]
    system_prompt: String,
    #[validate(length(max = 255))]
    default_model: String,
    /// Raw JSON for optional tools config; empty = none
    tools_json: String,
}

impl From<&Bear> for NewBearForm {
    fn from(bear: &Bear) -> Self {
        let tools_json = bear
            .tools_enabled
            .as_ref()
            .map(|j| serde_json::to_string_pretty(&j.0).unwrap_or_default())
            .unwrap_or_default();
        Self {
            slug: bear.slug.clone(),
            name: bear.name.clone(),
            description: bear.description.clone(),
            system_prompt: bear.system_prompt.clone(),
            default_model: bear.default_model.clone().unwrap_or_default(),
            tools_json,
        }
    }
}

async fn bear_detail_response(
    state: &AppState,
    auth_session: AuthSession,
    id: Uuid,
    letta_retry_message: Option<String>,
) -> Result<Response, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let member_count = bears_db::count_bear_members(state.sqlx_pool(), id).await?;

    let letta_api_base = state.config.letta_base_url.trim().to_string();
    let letta_configured = state.letta.is_enabled();

    let (letta_agent_summary, letta_agent_fetch_error): (Option<AgentSummary>, Option<String>) =
        if letta_configured {
            if let Some(agent_id) = bear.letta_agent_id.as_deref() {
                match state.letta.fetch_agent(agent_id).await {
                    Ok(v) => (Some(AgentSummary::from_letta_agent_state(&v)), None),
                    Err(e) => (None, Some(e.to_string())),
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

    let tools_json_display = bear
        .tools_enabled
        .as_ref()
        .and_then(|j| serde_json::to_string_pretty(&j.0).ok())
        .filter(|s| !s.trim().is_empty());

    let letta_memory_blocks_label = letta_agent_summary
        .as_ref()
        .and_then(|s| s.memory_block_count)
        .map(|n| n.to_string());
    let letta_tools_count_label = letta_agent_summary
        .as_ref()
        .and_then(|s| s.tool_count)
        .map(|n| n.to_string());

    web::render_template(
        state,
        "admin/bears/detail.html",
        auth_session,
        context! {
            bear,
            member_count,
            letta_api_base,
            letta_configured,
            letta_agent_summary,
            letta_agent_fetch_error,
            letta_retry_message,
            tools_json_display,
            letta_memory_blocks_label,
            letta_tools_count_label,
        },
    )
    .await
}

async fn detail_view(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    bear_detail_response(&state, auth_session, id, None).await
}

async fn list_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bears = bears_db::list_bears(state.sqlx_pool()).await?;
    web::render_template(
        &state,
        "admin/bears/list.html",
        auth_session,
        context! { bears },
    )
    .await
}

async fn new_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (letta_configured, letta_model_options, letta_models_fetch_error) =
        letta_model_select_context(&state).await;
    web::render_template(
        &state,
        "admin/bears/new.html",
        auth_session,
        context! {
            form => NewBearForm::default(),
            letta_configured,
            letta_model_options,
            letta_models_fetch_error,
        },
    )
    .await
}

pub async fn new_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewBearForm>,
) -> Result<Response, CustomError> {
    let letta_fetch = if state.letta.is_enabled() {
        Some(state.letta.list_llm_models().await)
    } else {
        None
    };

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    let tools_enabled = if form.tools_json.trim().is_empty() {
        None
    } else {
        match serde_json::from_str::<serde_json::Value>(form.tools_json.trim()) {
            Ok(v) => Some(Json(v)),
            Err(_) => {
                validation_errors.add(
                    "tools_json",
                    ValidationError::new("tools_json must be valid JSON or empty"),
                );
                None
            }
        }
    };

    let default_model_trim = form.default_model.trim();
    validate_default_model_for_letta(&letta_fetch, default_model_trim, &mut validation_errors);

    let default_model_opt = if default_model_trim.is_empty() {
        None
    } else {
        Some(default_model_trim)
    };

    if bears_db::bear_slug_exists(state.sqlx_pool(), form.slug.trim()).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        let id = bears_db::create_bear(
            state.sqlx_pool(),
            form.slug.trim(),
            form.name.trim(),
            form.description.trim(),
            form.system_prompt.trim(),
            default_model_opt,
            tools_enabled,
        )
        .await?;

        if let Err(e) = provision::provision_bear_if_configured(
            state.sqlx_pool(),
            state.letta.as_ref(),
            id,
        )
        .await
        {
            if state.letta.is_enabled() {
                tracing::warn!(%id, "Letta provision failed: {e}");
                let (letta_configured, letta_model_options, letta_models_fetch_error) =
                    letta_model_select_context(&state).await;
                return web::render_template(
                    &state,
                    "admin/bears/new.html",
                    auth_session,
                    context! {
                        form => form,
                        provision_error => e.to_string(),
                        letta_configured,
                        letta_model_options,
                        letta_models_fetch_error,
                    },
                )
                .await;
            }
        }

        Ok(Redirect::to(&format!("/admin/bears/{id}")).into_response())
    } else {
        let (letta_configured, letta_model_options, letta_models_fetch_error) =
            letta_model_select_context(&state).await;
        web::render_template(
            &state,
            "admin/bears/new.html",
            auth_session,
            context! {
                errors => validation_errors,
                form => form,
                letta_configured,
                letta_model_options,
                letta_models_fetch_error,
            },
        )
        .await
    }
}

async fn edit_view(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    let form = NewBearForm::from(&bear);
    let (letta_configured, letta_model_options, letta_models_fetch_error) =
        letta_model_select_context(&state).await;
    let letta_model_options = if letta_configured {
        ensure_stored_model_in_options(&bear, letta_model_options)
    } else {
        letta_model_options
    };
    web::render_template(
        &state,
        "admin/bears/edit.html",
        auth_session,
        context! {
            bear,
            form,
            letta_configured,
            letta_model_options,
            letta_models_fetch_error,
        },
    )
    .await
}

async fn edit_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewBearForm>,
) -> Result<Response, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let letta_fetch = if state.letta.is_enabled() {
        Some(
            state
                .letta
                .list_llm_models()
                .await
                .map(|opts| ensure_stored_model_in_options(&bear, opts)),
        )
    } else {
        None
    };

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    let tools_enabled = if form.tools_json.trim().is_empty() {
        None
    } else {
        match serde_json::from_str::<serde_json::Value>(form.tools_json.trim()) {
            Ok(v) => Some(Json(v)),
            Err(_) => {
                validation_errors.add(
                    "tools_json",
                    ValidationError::new("tools_json must be valid JSON or empty"),
                );
                None
            }
        }
    };

    let default_model_trim = form.default_model.trim();
    validate_default_model_for_letta(&letta_fetch, default_model_trim, &mut validation_errors);

    let default_model_opt = if default_model_trim.is_empty() {
        None
    } else {
        Some(default_model_trim)
    };

    if bears_db::bear_slug_exists_excluding(state.sqlx_pool(), form.slug.trim(), id).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        bears_db::update_bear(
            state.sqlx_pool(),
            id,
            form.slug.trim(),
            form.name.trim(),
            form.description.trim(),
            form.system_prompt.trim(),
            default_model_opt,
            tools_enabled,
        )
        .await?;

        Ok(Redirect::to(&format!("/admin/bears/{id}")).into_response())
    } else {
        let (letta_configured, letta_model_options, letta_models_fetch_error) =
            letta_model_select_context(&state).await;
        let letta_model_options = if letta_configured {
            ensure_stored_model_in_options(&bear, letta_model_options)
        } else {
            letta_model_options
        };
        web::render_template(
            &state,
            "admin/bears/edit.html",
            auth_session,
            context! {
                errors => validation_errors,
                form => form,
                bear,
                letta_configured,
                letta_model_options,
                letta_models_fetch_error,
            },
        )
        .await
    }
}

async fn retry_letta_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let letta_retry_message = if !state.letta.is_enabled() {
        "Letta is not configured (set LETTA_BASE_URL).".to_string()
    } else if bear.letta_agent_id.is_some() {
        format!(
            "This bear already has a Letta agent ({}). No new agent was created.",
            bear.letta_agent_id.as_deref().unwrap_or("")
        )
    } else {
        match provision::provision_bear_if_configured(
            state.sqlx_pool(),
            state.letta.as_ref(),
            id,
        )
        .await
        {
            Ok(()) => {
                let bear2 = bears_db::get_bear(state.sqlx_pool(), id)
                    .await?
                    .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
                if let Some(agent) = bear2.letta_agent_id.as_deref() {
                    format!("Letta agent provisioned: {agent}.")
                } else {
                    "Provisioning finished but letta_agent_id is still unset.".to_string()
                }
            }
            Err(e) => format!("Letta provisioning failed: {e}"),
        }
    };

    bear_detail_response(
        &state,
        auth_session,
        id,
        Some(letta_retry_message),
    )
    .await
}
