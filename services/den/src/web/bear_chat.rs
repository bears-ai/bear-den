//! First-party chat shell (Deep Chat component + Den `/v1` APIs).
//! When changing routes wired to this UI, update `src/web/ROUTES.md`.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
};
use minijinja::context;

use crate::{
    auth_backend::AuthSession,
    core::{bears::db as bears_db, user},
    errors::CustomError,
    web::{self, AppState},
};

/// Deep Chat view for one bear (`/bear/{slug}`); membership-checked.
pub async fn bear_page(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }

    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    let u = user::user_by_id(state.sqlx_pool(), user_id).await?;
    if !u.email_verified.unwrap_or(false) {
        return Ok(Redirect::to("/settings/email/verify").into_response());
    }

    let bear = bears_db::bear_for_user_by_slug(state.sqlx_pool(), user_id, slug)
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("Bear not found or you do not have access.".to_string())
        })?;

    web::render_template(
        &state,
        "bear_chat.html",
        auth_session,
        context! {
            bear_id => bear.id.to_string(),
            bear_slug => bear.slug,
            bear_name => bear.name,
        },
    )
    .await
}
