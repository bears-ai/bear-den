//! First-party chat shell (Loquix from CDN + Den `/v1` APIs).
//! When changing routes wired to this UI, update `src/web/ROUTES.md`.

use axum::response::{Html, IntoResponse};

const APP_HTML: &str = include_str!("static/loquix_app.html");

pub async fn app_page() -> impl IntoResponse {
    Html(APP_HTML)
}
