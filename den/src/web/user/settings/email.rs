// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
// use axum::extract::Query;
// use regex::Regex;
use serde::{Deserialize, Serialize};

use axum::{
    Router,
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::{extract::Form, routing::RouterExt};
use axum_login::tower_sessions::Session;
use validator::Validate;

use minijinja::context;

use crate::{
    auth_backend::AuthSession,
    core::user::email_settings::{self, UserEmailBasics, UserEmailSettings, VerifyAttemptStatus},
    errors::CustomError,
    web::{self, AppState},
};

static VERIFY_TOKEN: &str = "send_token";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(|| async { Redirect::permanent("/settings") }))
        .route_with_tsr("/edit", get(edit_email_view).post(edit_email_action))
        .route_with_tsr("/verify", get(verify_email_view).post(verify_email_action))
        .route_with_tsr("/verify/{code}", get(verify_email_process))
}

#[derive(Serialize, Deserialize, Debug, Validate)]
pub struct EmailForm {
    #[validate(email)]
    email: String,
}
impl From<UserEmailSettings> for EmailForm {
    fn from(user_email_settings: UserEmailSettings) -> Self {
        EmailForm {
            email: user_email_settings.email,
        }
    }
}
impl EmailForm {
    fn to_user_email_basics(&self, user_id: i32) -> UserEmailBasics {
        UserEmailBasics {
            user_id,

            email: self.email.clone(),
            active: true, // mucking with an email config means activate it
        }
    }
}

async fn edit_email_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session.user.clone().unwrap().id;
    let sqlx_pool = state.sqlx_pool.clone();

    let user_email_settings = email_settings::settings_by_id(&sqlx_pool, user_id).await?;
    let email_form: EmailForm = user_email_settings.into();

    web::render_template(
        state.template_env.clone(),
        "settings/email/edit.html",
        auth_session,
        context! {
            form => email_form,
        },
    )
    .await
}

pub async fn edit_email_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(email_form): Form<EmailForm>,
) -> Result<Response, CustomError> {
    let user_id = auth_session.user.clone().unwrap().id;
    let sqlx_pool = state.sqlx_pool.clone();

    if let Err(form_validation_errors) = email_form.validate() {
        return Ok(web::render_template(
            state.template_env.clone(),
            "settings/email/edit.html",
            auth_session,
            context! {

                email_form => context! {
                    errors => form_validation_errors,
                    ..context! {email_form}
                }
            },
        )
        .await
        .into_response());
    }

    let user_email_basics = email_form.to_user_email_basics(user_id);

    email_settings::update_email_basics(&sqlx_pool, user_email_basics).await?;

    Ok(Redirect::to("/settings/email/verify").into_response())
}

async fn verify_email_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let user_id = auth_session.user.clone().unwrap().id;
    let sqlx_pool = state.sqlx_pool.clone();

    let user_email_settings = email_settings::settings_by_id(&sqlx_pool, user_id).await?;
    if user_email_settings.verified_at.is_some() {
        Err(CustomError::Email("Email is already verified".to_string()))
    } else {
        let token: String = {
            use rand::Rng;
            let mut rng = rand::rng();
            const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
            (0..32)
                .map(|_| A[rng.random_range(0..A.len())] as char)
                .collect()
        };
        session.insert(VERIFY_TOKEN, token.clone()).await?;

        web::render_template(
            state.template_env.clone(),
            "settings/email/verify.html",
            auth_session,
            context! {
                email_address => user_email_settings.email,
                token => token,
            },
        )
        .await
    }
}

#[derive(Deserialize)]
struct SendToken {
    token: String,
}
#[axum::debug_handler]
async fn verify_email_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
    Form(token): Form<SendToken>,
) -> Result<Response, CustomError> {
    if session
        .get::<String>(VERIFY_TOKEN)
        .await
        .unwrap()
        .unwrap_or_default()
        != token.token
    {
        tracing::warn!("Invalid verification sending token");
        return Ok(Redirect::to("/settings/email/verify").into_response());
    }
    let user_id = auth_session.user.clone().unwrap().id;
    let sqlx_pool = state.sqlx_pool.clone();

    let email_sent_to =
        email_settings::send_verify_email_for_user_id(&sqlx_pool, user_id, &state.config).await?;
    session.remove::<String>(VERIFY_TOKEN).await?;

    Ok(web::render_template(
        state.template_env.clone(),
        "settings/email/verify_sent.html",
        auth_session,
        context! {
            email_sent_to => email_sent_to
        },
    )
    .await
    .into_response())
}

async fn verify_email_process(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(code): Path<String>,
) -> Result<Response, CustomError> {
    let user_id = auth_session.user.clone().unwrap().id;
    let sqlx_pool = state.sqlx_pool.clone();

    let verify_outcome = email_settings::mark_email_verified(&sqlx_pool, user_id, code).await?;

    let message_key = match verify_outcome.status {
        VerifyAttemptStatus::Success => "success",
        VerifyAttemptStatus::Redundant => "redundant",
        VerifyAttemptStatus::Expired => "expired",
        VerifyAttemptStatus::Unknown => "invalid",
    };

    tracing::debug!("Verification result: {}", message_key);

    Ok(web::render_template(
        state.template_env.clone(),
        "settings/email/verify_result.html",
        auth_session,
        context! {
            verify_message_key => message_key,
        },
    )
    .await
    .into_response())
}
