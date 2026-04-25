// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};

use serde::Serialize;

use crate::{
    auth_backend::AuthSession,
    core::bears::db::{self as bears_db, role_is_bear_admin},
    core::user,
    errors::CustomError,
    web::{self, AppState},
};

use minijinja::context;

#[derive(Serialize)]
struct DashboardBear {
    slug: String,
    name: String,
    description: String,
    is_admin: bool,
}

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
            let rows = bears_db::list_bears_for_user(&state.sqlx_pool, user_id).await?;
            let bears: Vec<DashboardBear> = rows
                .into_iter()
                .map(|row| DashboardBear {
                    slug: row.bear.slug,
                    name: row.bear.name,
                    description: row.bear.description,
                    is_admin: role_is_bear_admin(row.membership_role.as_deref()),
                })
                .collect();
            web::render_template(
                &state,
                "dashboard.html",
                auth_session,
                context! { bears },
            )
            .await
        }
        None => {
            web::render_template(&state, "home.html", auth_session, context! {}).await
        }
    }
}
