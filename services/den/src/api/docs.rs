use axum::{Router, response::Json, routing::get};
use utoipa::OpenApi;

use crate::api::service::ApiState;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::api::v1::profile::get_profile
    ),
    components(
        schemas(crate::api::v1::profile::ProfileResponse)
    ),
    tags(
        (name = "Profile", description = "User profile management endpoints")
    ),
    info(
        title = "HTTP API",
        version = "1.0.0",
        description = "Standalone API and OAuth2 provider for applications built with this starter"
    )
)]
pub struct ApiDoc;

pub fn router() -> Router<ApiState> {
    Router::new().route("/api-docs/openapi.json", get(serve_openapi))
}

async fn serve_openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
