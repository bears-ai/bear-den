// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
pub mod api;
pub mod bears;
pub mod membership;
pub mod oauth_clients;
pub mod ops;
pub mod users;

use axum::response::Response;
use axum::{extract::State, routing::get, Router};

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

    let (letta_status, letta_detail) = if !state.letta.is_enabled() {
        (
            "not_configured",
            "Set LETTA_BASE_URL (and LETTA_API_KEY if required) for provisioning and chat."
                .to_string(),
        )
    } else {
        match state.letta.check_health().await {
            Ok(_) => (
                "ok",
                "GET /v1/health succeeded — same check Den uses before provisioning.".to_string(),
            ),
            Err(e) => ("error", e.to_string()),
        }
    };

    let (codepool_status, codepool_detail) = if !state.codepool.is_enabled() {
        (
            "not_configured",
            "Set CODEPOOL_BASE_URL for Letta Code SDK streaming (required when RUN_WEB=true)."
                .to_string(),
        )
    } else {
        match state.codepool.check_health().await {
            Ok(_) => ("ok", "GET /health on Codepool succeeded.".to_string()),
            Err(e) => ("error", e.to_string()),
        }
    };

    web::render_template(
        &state,
        "admin/menu.html",
        auth_session,
        context! {
            users => users,
            letta_status => letta_status,
            letta_detail => letta_detail,
            codepool_status => codepool_status,
            codepool_detail => codepool_detail,
        },
    )
    .await
}
