// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
//! First-party chat shell (Loquix from CDN + Den `/v1` APIs).

use axum::response::{Html, IntoResponse};

const APP_HTML: &str = include_str!("static/loquix_app.html");

pub async fn app_page() -> impl IntoResponse {
    Html(APP_HTML)
}
