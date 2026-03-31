// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use axum::{
    Router,
    extract::State,
    http::{StatusCode, Uri},
    response::Html,
    routing::get,
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

pub fn router() -> Router<AppState> {
    Router::new().fallback(get(serve_404))
}

pub async fn serve_404(State(state): State<AppState>, uri: Uri) -> (StatusCode, Html<String>) {
    let template = state.template_env.get_template("404.html").unwrap();
    let r = template
        .render(context! {
            error_code => 404,
            uri => format!("{}", uri),
            ..site_context(&state),
        })
        .unwrap();
    (StatusCode::NOT_FOUND, Html(r))
}
