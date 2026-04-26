// ROUTES: When modifying routes in this file, update src/web/ROUTES.md
use axum::{
    extract::State,
    http::{StatusCode, Uri},
    response::Html,
    routing::get,
    Router,
};

use crate::web::AppState;

use minijinja::context;

fn site_context(state: &AppState) -> minijinja::Value {
    context! {
        app_display_name => state.config.app_display_name.clone(),
        app_slug => state.config.app_slug.clone(),
        public_web_origin => state.config.web_public_origin(),
    }
}

const FALLBACK_404_HTML: &str = "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"/><title>Not found</title></head><body><h1>404 Not Found</h1></body></html>";

pub fn router() -> Router<AppState> {
    Router::new().fallback(get(serve_404))
}

pub async fn serve_404(State(state): State<AppState>, uri: Uri) -> (StatusCode, Html<String>) {
    let html = match state.template_env.get_template("404.html") {
        Ok(template) => match template.render(context! {
            error_code => 404,
            uri => format!("{}", uri),
            ..site_context(&state),
        }) {
            Ok(body) => body,
            Err(e) => {
                tracing::warn!(error = %e, "404 template render failed");
                FALLBACK_404_HTML.to_string()
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "404 template not found");
            FALLBACK_404_HTML.to_string()
        }
    };
    (StatusCode::NOT_FOUND, Html(html))
}
