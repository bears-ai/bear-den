// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
pub mod api;
pub mod bears;
pub mod loop_control;
pub mod membership;
pub mod models;
pub mod oauth_clients;
mod oauth_feedback;
mod oauth_tokens;

#[cfg(test)]
mod oauth_route_tests;
pub mod reflections;
pub mod runs;
pub mod sandbox_images;
pub mod users;
pub mod workers;

#[cfg(test)]
pub(crate) mod usability_tests;

use axum::response::Response;
use axum::{extract::State, routing::get, Router};
use minijinja::context;

use crate::auth_backend::AuthSession;
use crate::errors::CustomError;
use crate::web::{self, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(users::router())
        .merge(oauth_clients::router())
        .merge(bears::router())
        .merge(membership::router())
        .merge(models::router())
        .merge(sandbox_images::router())
        .nest("/loop-control", loop_control::router())
        .nest("/workers", workers::router())
        .nest("/runs", runs::router())
        .nest("/api", api::router())
        .route("/", get(admin_home))
}

async fn admin_home(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    web::render_template(
        &state,
        "admin/menu.html",
        auth_session,
        context! {

            native_runtime => true,
        },
    )
    .await
}
