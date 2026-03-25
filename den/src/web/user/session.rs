// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::{
    extract::{Form, Query},
    routing::RouterExt,
};
use minijinja::context;
use validator::Validate;

use serde::{Deserialize, Serialize};

use crate::web::{self, AppState};
use crate::{
    auth_backend::{AuthSession, Credentials},
    core::user,
    errors::CustomError,
};

// This allows us to extract the "next" field from the query string. We use this
// to redirect after log in.
#[derive(Debug, Deserialize)]
pub struct NextUrl {
    next: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/login/password", post(login_password_action))
        .route_with_tsr("/login", get(login_form))
        .route_with_tsr("/logout", get(logout_action))
        .route_with_tsr("/su/{id}", get(su_action))
}

#[derive(Validate, Serialize, Deserialize)]
pub struct LoginForm {
    #[validate(length(min = 1, max = 255))]
    username: String,
    #[validate(length(min = 8))]
    password: String,
    next: Option<String>,
}
// prepare credntials from form
impl From<LoginForm> for Credentials {
    fn from(form: LoginForm) -> Self {
        Self {
            username: form.username,
            password: form.password,
            next: None,
            su: false,
        }
    }
}
// avoid accidentally logging password
impl std::fmt::Debug for LoginForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .field("next", &self.next)
            .finish()
    }
}

pub async fn login_password_action(
    State(state): State<AppState>,
    mut auth_session: AuthSession,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    let next = form.next.clone();

    if let Err(form_validation_errors) = form.validate() {
        return web::render_template(
            state.template_env,
            "login.html",
            auth_session,
            context! {
                next => next.unwrap_or_default(),
                errors => form_validation_errors
            },
        )
        .await
        .unwrap()
        .into_response();
    }

    let user = match auth_session.authenticate(Credentials::from(form)).await {
        Ok(Some(user)) => user,
        _ => {
            return web::render_template(
                state.template_env,
                "login.html",
                auth_session,
                context! {
                    next => next.unwrap_or_default(),
                    message => "Invalid username or password, try again",
                },
            )
            .await
            .unwrap()
            .into_response();
        }
    };

    if auth_session.login(&user).await.is_err() {
        tracing::error!("Error logging in valid user: {:?}", user);
        return web::render_template(
            state.template_env,
            "login.html",
            auth_session,
            context! {
                next => next.unwrap_or_default(),
                message => "Server error, please try again later",
            },
        )
        .await
        .unwrap()
        .into_response();
    }

    match next {
        Some(target_url) => Redirect::to(&target_url).into_response(),
        None => Redirect::to("/").into_response(),
    }
}

async fn login_form(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(NextUrl { next }): Query<NextUrl>,
) -> Result<Response, CustomError> {
    // If user is already authenticated, redirect to next URL or home
    if auth_session.user.is_some() {
        match next {
            Some(target_url) => return Ok(Redirect::to(&target_url).into_response()),
            None => return Ok(Redirect::to("/").into_response()),
        }
    }

    // User not authenticated, show login form
    web::render_template(
        state.template_env,
        "login.html",
        auth_session,
        context! {
            next => next.unwrap_or_default()
        },
    )
    .await
}

async fn logout_action(mut auth_session: AuthSession) -> impl IntoResponse {
    match auth_session.logout().await {
        Ok(_) => Redirect::to("/").into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(), // TODO: use custom error properly
    }
}

async fn su_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    mut auth_session: AuthSession,
) -> Result<Redirect, CustomError> {
    if let Some(current_user) = auth_session.user.clone() {
        if current_user.is_admin {
            let target_user = user::db::get_user_by_id(&state.sqlx_pool, id)
                .await?
                .ok_or_else(|| CustomError::NotFound(format!("User with id {id} not found")))?;
            let dummy_credentials = Credentials {
                username: target_user.username.clone(),
                password: "[ignored]".to_string(),
                next: None,
                su: true,
            };
            if let Some(new_user) = auth_session.authenticate(dummy_credentials).await? {
                auth_session.logout().await?;
                auth_session.login(&new_user).await?;
                tracing::info!("User {} switched to user {}", current_user.username, id);
                Ok(Redirect::to("/"))
            } else {
                tracing::warn!(
                    "User {} failed to switch to user {}",
                    current_user.username,
                    id
                );
                Err(CustomError::Authorization(format!(
                    "User {} failed to switch to user {}",
                    current_user.username, id
                )))
            }
        } else {
            tracing::error!(
                "Attempt by non-admin user '{}' to use /su",
                current_user.username
            );
            Err(CustomError::Authorization(format!(
                "Attempt by non-admin user '{}' to use /su",
                current_user.username
            )))
        }
    } else {
        tracing::warn!("Anonymous attempt to use /su");
        Err(CustomError::Authorization(
            "Anonymous attempt to use /su".to_string(),
        ))
    }
}
