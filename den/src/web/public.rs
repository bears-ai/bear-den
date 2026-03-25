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

pub fn router() -> Router<AppState> {
    Router::new().fallback(get(serve_404))
}

pub async fn serve_404(State(state): State<AppState>, uri: Uri) -> (StatusCode, Html<String>) {
    let template = state.template_env.get_template("404.html").unwrap();
    let r = template
        .render(context! { error_code => 404, uri => format!("{}", uri) })
        .unwrap();
    (StatusCode::NOT_FOUND, Html(r))
}
