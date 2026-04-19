// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
pub mod api;
pub mod bears;
pub mod membership;
pub mod oauth_clients;
pub mod ops;
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
        .merge(ops::router())
        .merge(users::router())
        .merge(oauth_clients::router())
        .merge(bears::router())
        .merge(membership::router())
}

async fn admin_home(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let users = user::db::get_users(&state.sqlx_pool).await?;

    let (letta_status, letta_detail) = if !state.letta.is_api_configured() {
        (
            "not_configured",
            "Set LETTA_API_BASE_URL (and LETTA_API_KEY if required) for provisioning and agent APIs; set LETTA_CODE_BASE_URL for web chat."
                .to_string(),
        )
    } else {
        match state.letta.check_health().await {
            Ok(_) => (
                "ok",
                "GET /v1/health on Letta API succeeded — same check Den uses before provisioning.".to_string(),
            ),
            Err(e) => ("error", e.to_string()),
        }
    };

    web::render_template(&state, "admin/menu.html",
        auth_session,
        context! {
            users => users,
            letta_status => letta_status,
            letta_detail => letta_detail,
        },
    )
    .await
}
