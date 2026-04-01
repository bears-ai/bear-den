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
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::AuthSession,
    core::bears::db as bears_db,
    errors::CustomError,
    web::{self, AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bears/", get(list_view))
        .route_with_tsr("/bears/new", get(new_view).post(new_action))
}

#[derive(Validate, Serialize, Deserialize, Debug)]
pub struct NewBearFromTemplateForm {
    #[validate(length(min = 1))]
    template_id: String,
    #[validate(length(min = 1, max = 120))]
    slug: String,
    #[validate(length(min = 1, max = 255))]
    name: String,
    #[validate(length(max = 2000))]
    description: String,
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
    let templates = bears_db::list_templates(state.sqlx_pool()).await?;
    web::render_template(
        &state,
        "admin/bears/new.html",
        auth_session,
        context! { templates },
    )
    .await
}

pub async fn new_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewBearFromTemplateForm>,
) -> Result<Response, CustomError> {
    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    let template_id: Option<Uuid> = form.template_id.trim().parse().ok();
    let tid = if form.template_id.trim().is_empty() {
        validation_errors.add(
            "template_id",
            ValidationError::new("Choose a template."),
        );
        None
    } else if template_id.is_none() {
        validation_errors.add(
            "template_id",
            ValidationError::new("template_id must be a valid UUID"),
        );
        None
    } else {
        template_id
    };

    if bears_db::bear_slug_exists(state.sqlx_pool(), form.slug.trim()).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        let template_id = tid.expect("checked");
        let _ = bears_db::create_bear_from_template(
            state.sqlx_pool(),
            template_id,
            form.slug.trim(),
            form.name.trim(),
            form.description.trim(),
        )
        .await?;
        Ok(Redirect::to("/admin/bears/").into_response())
    } else {
        let templates = bears_db::list_templates(state.sqlx_pool()).await?;
        web::render_template(
            &state,
            "admin/bears/new.html",
            auth_session,
            context! {
                errors => validation_errors,
                form => form,
                templates,
            },
        )
        .await
    }
}
