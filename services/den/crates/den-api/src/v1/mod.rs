mod native;
pub mod oauth;
pub mod profile;
pub mod user;

use crate::service::DenState;
use axum::Router;

pub fn router() -> Router<DenState> {
    Router::new()
        .merge(profile::router())
        .merge(user::router())
        .merge(oauth::router())
        .merge(native::router())
}
