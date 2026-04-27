// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
use axum::{extract::State, response::Response, routing::get, Router};
use minijinja::context;

use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/design", get(index))
        .route("/design/chat", get(chat))
}

async fn index(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    web::render_template(&state, "design/index.html", auth_session, context! {}).await
}

async fn chat(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    web::render_template(&state, "design/chat.html", auth_session, context! {}).await
}
