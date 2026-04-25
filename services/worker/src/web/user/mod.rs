// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
pub mod account;
pub mod session;
pub mod settings;

use axum::Router;
use axum_login::login_required;

use crate::{auth_backend::Backend, web::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .nest(
            "/settings",
            settings::router().route_layer(login_required!(Backend, login_url = "/login")),
        )
        .nest("/account", account::router())
        .merge(session::router())
}
