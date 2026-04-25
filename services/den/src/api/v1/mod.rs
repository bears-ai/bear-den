pub mod oauth;
pub mod profile;
pub mod user;

use crate::api::service::ApiState;
use axum::Router;

pub fn router() -> Router<ApiState> {
    Router::new()
        .merge(profile::router())
        .merge(user::router())
        .merge(oauth::router())
}
