// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
pub mod email;

use std::borrow::Cow::Borrowed;

use axum::{
    Form, Router,
    extract::State,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::routing::RouterExt;
use minijinja::context;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::core::user::{self, UserSettings};
use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(settings_home))
        .route_with_tsr("/edit", get(settings_form_view).post(settings_form_action))
        .nest("/email", email::router())
}

#[derive(Serialize, Deserialize, Validate)]
struct SettingsForm {
    #[validate(length(min = 3, max = 100))]
    display_name: String,
    #[validate(custom(function = "validate_theme"))]
    theme: String,
    #[validate(custom(function = "validate_day_of_week"))]
    week_start_day: i32,
}
impl From<UserSettings> for SettingsForm {
    fn from(user_settings: UserSettings) -> Self {
        SettingsForm {
            display_name: user_settings.display_name,
            theme: user_settings.theme,
            week_start_day: user_settings.week_start_day,
        }
    }
}
impl SettingsForm {
    fn to_user_settings(&self, user_id: i32) -> UserSettings {
        UserSettings {
            id: user_id,

            display_name: self.display_name.clone(),
            theme: self.theme.clone(),
            week_start_day: self.week_start_day,
        }
    }
}
fn validate_theme(theme: &str) -> Result<(), ValidationError> {
    if web::theme_descriptions().get(theme).is_none() {
        return Err(ValidationError::new("invalid_theme").with_message(Borrowed("Invalid theme")));
    }
    Ok(())
}
fn validate_day_of_week(day_of_week: i32) -> Result<(), ValidationError> {
    if web::day_of_week_names().get(&day_of_week).is_none() {
        return Err(ValidationError::new("invalid_day_of_week")
            .with_message(Borrowed("Invalid day of week")));
    }
    Ok(())
}

async fn settings_home(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .clone()
        .ok_or_else(|| CustomError::Authentication("No session user".to_string()))?
        .id;
    let user = user::db::user_by_id(&state.sqlx_pool, user_id)
        .await?
        .unwrap();

    web::render_template(
        state.template_env,
        "settings/view.html",
        auth_session,
        context! {

            username => user.username,
            email => user.email,
            email_verified => user.email_verified.unwrap_or(false),
            // premium_until => user.premium_until,

            display_name => user.display_name,
            theme_descriptions => web::theme_descriptions(),
            day_of_week_names => web::day_of_week_names(),
            theme => user.theme,
            week_start_day => user.week_start_day
        },
    )
    .await
}

async fn settings_form_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .clone()
        .ok_or_else(|| CustomError::Authentication("No session user".to_string()))?
        .id;
    let user_settings = user::db::settings_by_id(&state.sqlx_pool, user_id)
        .await?
        .unwrap();
    let settings_form = SettingsForm::from(user_settings);

    web::render_template(
        state.template_env,
        "settings/edit.html",
        auth_session,
        context! {

            theme_descriptions => web::theme_descriptions(),
            day_of_week_names => web::day_of_week_names(),
            settings_form => settings_form
        },
    )
    .await
}

async fn settings_form_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(settings_form): Form<SettingsForm>,
) -> Result<Response, CustomError> {
    if let Err(form_validation_errors) = settings_form.validate() {
        return Ok(web::render_template(
            state.template_env,
            "settings/edit.html",
            auth_session,
            context! {

                theme_descriptions => web::theme_descriptions(),
                day_of_week_names => web::day_of_week_names(),
                settings_form => context! {
                    errors => form_validation_errors,
                    ..context! {settings_form}
                }
            },
        )
        .await
        .into_response());
    }

    let user_id = auth_session
        .user
        .clone()
        .ok_or_else(|| CustomError::Authentication("No session user".to_string()))?
        .id;
    let user_settings = settings_form.to_user_settings(user_id);

    user::db::update_settings(&state.sqlx_pool, &user_settings).await?;

    Ok(Redirect::to("/settings").into_response())
}
