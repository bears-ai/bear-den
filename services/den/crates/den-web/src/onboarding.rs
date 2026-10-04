use axum::{
    extract::State,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::extract::Form;
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{
        bear::create_support::{
            bear_new_form_context, build_context_profile_json_for_template,
            insert_new_bear_row_with_context_profile, is_valid_bear_slug,
            provision_bifrost_virtual_key_for_bear, validate_default_model_for_catalog,
            NewBearForm, BEAR_SLUG_VALIDATION_MESSAGE,
        },
        render_template, AppState,
    },
};
use den_service::bears::{
    db::{self as bears_db, BEAR_ROLE_ADMIN},
    provision,
    templates::FIRST_BEAR_TEMPLATES,
};

#[derive(Debug, Serialize)]
struct FirstBearTemplateView {
    id: &'static str,
    name: &'static str,
    default_bear_name: &'static str,
    description: &'static str,
    default_user_steering: &'static str,
    context_placeholder: &'static str,
    starter_prompts: &'static [&'static str],
}

fn template_views() -> Vec<FirstBearTemplateView> {
    FIRST_BEAR_TEMPLATES
        .iter()
        .map(|template| FirstBearTemplateView {
            id: template.id,
            name: template.name,
            default_bear_name: template.default_bear_name,
            description: template.description,
            default_user_steering: template.default_user_steering,
            context_placeholder: template.context_placeholder,
            starter_prompts: template.starter_prompts,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct FirstBearForm {
    #[validate(length(min = 1, max = 120))]
    pub slug: String,
    #[validate(length(min = 1, max = 255))]
    pub name: String,
    #[validate(length(max = 2000))]
    pub description: String,
    #[validate(length(min = 1, max = 128))]
    pub template_id: String,
    #[validate(length(max = 20_000))]
    pub user_steering: String,
    #[validate(length(max = 20_000))]
    pub bear_context: String,
    #[validate(length(max = 4000))]
    pub first_task: String,
    #[validate(length(max = 255))]
    pub default_model: String,
}

impl Default for FirstBearForm {
    fn default() -> Self {
        let template = &FIRST_BEAR_TEMPLATES[0];
        Self {
            slug: "builder-bear".to_string(),
            name: template.default_bear_name.to_string(),
            description: template.description.to_string(),
            template_id: template.id.to_string(),
            user_steering: template.default_user_steering.to_string(),
            bear_context: String::new(),
            first_task: String::new(),
            default_model: String::new(),
        }
    }
}

fn first_bear_to_new_bear_form(form: &FirstBearForm) -> NewBearForm {
    NewBearForm {
        slug: form.slug.clone(),
        name: form.name.clone(),
        description: form.description.clone(),
        system_prompt: String::new(),
        default_model: form.default_model.clone(),
    }
}

async fn render_first_bear_form(
    state: &AppState,
    auth_session: AuthSession,
    form: FirstBearForm,
    errors: Option<ValidationErrors>,
    provision_error: Option<String>,
    runtime_sync_error: Option<String>,
) -> Result<Response, CustomError> {
    let new_bear_form = first_bear_to_new_bear_form(&form);
    let page = bear_new_form_context(state, &new_bear_form).await;
    render_template(
        state,
        "onboarding/first_bear.html",
        auth_session,
        context! {
            form,
            templates => template_views(),
            errors,
            provision_error,
            runtime_sync_error,
            ..page
        },
    )
    .await
}

async fn user_has_bear(state: &AppState, user_id: i32) -> Result<bool, CustomError> {
    Ok(!bears_db::list_bears_for_user(state.sqlx_pool(), user_id)
        .await?
        .is_empty())
}

async fn email_verified_or_redirect(
    state: &AppState,
    user_id: i32,
) -> Result<Option<Redirect>, CustomError> {
    let u = crate::core::user::user_by_id(state.sqlx_pool(), user_id).await?;
    if !u.email_verified.unwrap_or(false) {
        return Ok(Some(Redirect::to("/settings/email/verify")));
    }
    Ok(None)
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/onboarding/first-bear",
        get(first_bear_get).post(first_bear_post),
    )
}

async fn first_bear_get(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verified_or_redirect(&state, user_id).await? {
        return Ok(r.into_response());
    }
    if user_has_bear(&state, user_id).await? {
        return Ok(Redirect::to("/").into_response());
    }

    render_first_bear_form(
        &state,
        auth_session,
        FirstBearForm::default(),
        None,
        None,
        None,
    )
    .await
}

async fn first_bear_post(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<FirstBearForm>,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verified_or_redirect(&state, user_id).await? {
        return Ok(r.into_response());
    }
    if user_has_bear(&state, user_id).await? {
        return Ok(Redirect::to("/").into_response());
    }

    let model_context =
        crate::web::bear::create_support::model_catalog_select_context(&state).await;
    let model_fetch = model_context
        .0
        .then_some(Ok::<_, CustomError>(model_context.1));

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }
    if den_service::bears::templates::first_bear_template(&form.template_id).is_none() {
        validation_errors.add("template_id", ValidationError::new("Choose a template."));
    }
    if !is_valid_bear_slug(&form.slug) {
        validation_errors.add("slug", ValidationError::new(BEAR_SLUG_VALIDATION_MESSAGE));
    }
    let default_model_trim = form.default_model.trim();
    validate_default_model_for_catalog(&model_fetch, default_model_trim, &mut validation_errors);
    let default_model_opt =
        crate::web::bear::create_support::canonical_default_model_handle(default_model_trim);

    if bears_db::bear_slug_exists(state.sqlx_pool(), form.slug.trim()).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if !validation_errors.is_empty() {
        return render_first_bear_form(
            &state,
            auth_session,
            form,
            Some(validation_errors),
            None,
            None,
        )
        .await;
    }

    let context_profile = build_context_profile_json_for_template(
        &form.template_id,
        form.name.trim(),
        &form.user_steering,
        &form.bear_context,
        Some(&form.first_task),
    )?;
    let new_bear_form = first_bear_to_new_bear_form(&form);
    let id: Uuid = insert_new_bear_row_with_context_profile(
        state.sqlx_pool(),
        &new_bear_form,
        default_model_opt.as_deref(),
        context_profile,
    )
    .await?;

    if let Err(e) =
        provision_bifrost_virtual_key_for_bear(&state, id, new_bear_form.slug.trim()).await
    {
        if let Err(rollback_err) = bears_db::delete_bear(state.sqlx_pool(), id).await {
            tracing::warn!(
                bear_id = %id,
                provision_error = %e,
                error = %rollback_err,
                "failed to roll back Bear after Bifrost virtual key provisioning failure"
            );
        }
        return render_first_bear_form(
            &state,
            auth_session,
            form,
            None,
            Some(format!("Bifrost virtual key provisioning failed: {e}")),
            None,
        )
        .await;
    }

    bears_db::grant_membership(state.sqlx_pool(), user_id, id, Some(BEAR_ROLE_ADMIN)).await?;

    if let Err(e) =
        provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, id).await
    {
        tracing::warn!(%id, "Bear initialization failed during first-bear onboarding: {e}");
        return render_first_bear_form(&state, auth_session, form, None, Some(e.to_string()), None)
            .await;
    }

    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    Ok(Redirect::to(&format!("/bear/{}/hats", bear.slug)).into_response())
}
