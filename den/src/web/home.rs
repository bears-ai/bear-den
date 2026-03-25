// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};

use crate::{
    auth_backend::AuthSession,
    core::user,
    errors::CustomError,
    web::{self, AppState},
};

use minijinja::context;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(home))
}

async fn home(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    match auth_session.user.clone() {
        Some(session_user) => {
            let user_id = session_user.id;
            let u = user::user_by_id(&state.sqlx_pool, user_id).await?;
            if !u.email_verified.unwrap_or(false) {
                return Ok(Redirect::to("/settings/email/verify").into_response());
            }
            web::render_template(
                state.template_env,
                "dashboard_empty.html",
                auth_session,
                context! {},
            )
            .await
        }
        None => {
            web::render_template(state.template_env, "home.html", auth_session, context! {}).await
        }
    }
}
