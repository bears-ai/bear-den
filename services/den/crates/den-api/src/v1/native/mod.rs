//! OAuth-authenticated native channel edge. No editor or local execution authority.

mod access;
mod ahp;
mod conversations;
mod error;
mod history;

use axum::{extract::DefaultBodyLimit, routing::get, Router};
use den_service::DenState;

pub fn router() -> Router<DenState> {
    Router::new()
        .route("/bears", get(conversations::bears))
        .route(
            "/chat/conversations",
            get(conversations::list).post(conversations::create),
        )
        .route("/chat/history", get(history::get))
        .route("/agent-host", get(ahp::upgrade))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[cfg(test)]
mod database_tests;
#[cfg(test)]
mod tests;
