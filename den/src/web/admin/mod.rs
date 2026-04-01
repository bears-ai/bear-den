// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
pub mod api;
pub mod bear_templates;
pub mod bears;
pub mod membership;
pub mod oauth_clients;
pub mod users;

use axum::response::Response;
use axum::{Router, extract::State, routing::get};

use minijinja::context;

use crate::errors::CustomError;
use crate::web::{self, AppState};
use crate::{auth_backend::AuthSession, core::user};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(admin_home))
        .nest("/api", api::router())
        .merge(users::router())
        .merge(oauth_clients::router())
        .merge(bear_templates::router())
        .merge(bears::router())
        .merge(membership::router())
}

async fn admin_home(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let users = user::db::get_users(&state.sqlx_pool).await?;

    web::render_template(&state, "admin/menu.html",
        auth_session,
        context! {
            users => users,
        },
    )
    .await
}
