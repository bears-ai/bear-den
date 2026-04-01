// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;
use minijinja::context;
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::AuthSession,
    core::bears::db as bears_db,
    errors::CustomError,
    web::{self, AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bear-templates/", get(list_view))
        .route_with_tsr("/bear-templates/add", get(add_view).post(add_action))
}

#[derive(Validate, Serialize, Deserialize, Debug)]
pub struct TemplateForm {
    #[validate(length(min = 1, max = 120))]
    slug: String,
    #[validate(length(max = 255))]
    name: String,
    #[validate(length(max = 2000))]
    description: String,
    #[validate(length(min = 1, max = 100_000))]
    system_prompt: String,
    #[validate(length(max = 255))]
    default_model: String,
    /// Raw JSON for optional tools config; empty = none
    tools_json: String,
}

async fn list_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let templates = bears_db::list_templates(state.sqlx_pool()).await?;
    web::render_template(
        &state,
        "admin/bear_templates/list.html",
        auth_session,
        context! { templates },
    )
    .await
}

async fn add_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    web::render_template(
        &state,
        "admin/bear_templates/add.html",
        auth_session,
        context! {},
    )
    .await
}

pub async fn add_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<TemplateForm>,
) -> Result<Response, CustomError> {
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

    let default_model = form.default_model.trim();
    let default_model_opt = if default_model.is_empty() {
        None
    } else {
        Some(default_model)
    };

    if bears_db::template_slug_exists(state.sqlx_pool(), form.slug.trim()).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A template with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        let _ = bears_db::create_template(
            state.sqlx_pool(),
            form.slug.trim(),
            form.name.trim(),
            form.description.trim(),
            form.system_prompt.trim(),
            default_model_opt,
            tools_enabled,
        )
        .await?;
        Ok(Redirect::to("/admin/bear-templates/").into_response())
    } else {
        web::render_template(
            &state,
            "admin/bear_templates/add.html",
            auth_session,
            context! {
                errors => validation_errors,
                draft => form,
            },
        )
        .await
    }
}
