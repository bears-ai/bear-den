use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::Form;
use axum_login::tower_sessions::Session;
use den_core::UserId;
use den_http::user::db::deletion::{self, UserDeletionError, UserDeletionPreview};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};

const CONFIRMATION_KEY: &str = "admin_user_delete_confirmation";
const CONFIRMATION_MAX_AGE_SECONDS: i64 = 15 * 60;

#[derive(Serialize, Deserialize)]
struct DeleteConfirmation {
    user_id: UserId,
    token: Uuid,
    issued_at: i64,
}

impl DeleteConfirmation {
    fn is_fresh(&self, user_id: UserId, token: Option<Uuid>, now: i64) -> bool {
        let age = now - self.issued_at;
        self.user_id == user_id
            && Some(self.token) == token
            && (0..=CONFIRMATION_MAX_AGE_SECONDS).contains(&age)
    }
}

#[derive(Default, Deserialize)]
pub struct DeleteUserForm {
    #[serde(default)]
    confirm_delete: bool,
    confirmation_token: Option<String>,
}

fn require_operator(auth_session: &AuthSession) -> Result<(), CustomError> {
    if !auth_session.user.as_ref().is_some_and(|user| user.is_admin) {
        return Err(CustomError::Authorization(
            "Operator access required".into(),
        ));
    }
    Ok(())
}

fn boundary_error(error: UserDeletionError) -> CustomError {
    den_core::DenError::from(error).into()
}

async fn render_preview(
    state: &AppState,
    auth_session: AuthSession,
    session: &Session,
    preview: UserDeletionPreview,
    confirmed: bool,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, CustomError> {
    let token = Uuid::new_v4();
    session
        .insert(
            CONFIRMATION_KEY,
            DeleteConfirmation {
                user_id: preview.user_id,
                token,
                issued_at: time::OffsetDateTime::now_utc().unix_timestamp(),
            },
        )
        .await?;
    let mut response = web::render_template(
        state,
        "admin/users/delete.html",
        auth_session,
        context! {
            preview,
            confirmation_token => token,
            confirmed,
            error,
        },
    )
    .await?;
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    Ok(response)
}

pub async fn delete_user_view(
    Path(id): Path<UserId>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    require_operator(&auth_session)?;
    let preview = deletion::preview_user_deletion(state.sqlx_pool(), id)
        .await
        .map_err(boundary_error)?;
    render_preview(
        &state,
        auth_session,
        &session,
        preview,
        false,
        None,
        StatusCode::OK,
    )
    .await
}

pub async fn delete_user_action(
    Path(id): Path<UserId>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
    Form(form): Form<DeleteUserForm>,
) -> Result<Response, CustomError> {
    require_operator(&auth_session)?;
    let preview = deletion::preview_user_deletion(state.sqlx_pool(), id)
        .await
        .map_err(boundary_error)?;
    // Single-use, target-bound confirmation, including on failed attempts. Error rendering
    // issues a new token without losing the checkbox state; no browser script is required.
    let confirmation = session
        .remove::<DeleteConfirmation>(CONFIRMATION_KEY)
        .await?;
    let token = form
        .confirmation_token
        .as_deref()
        .and_then(|value| value.parse::<Uuid>().ok());
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let fresh = confirmation.is_some_and(|confirmation| confirmation.is_fresh(id, token, now));
    if !form.confirm_delete || !fresh {
        return render_preview(
            &state, auth_session, &session, preview, form.confirm_delete,
            Some("Confirm deletion on this page before submitting. If the preview expired or was already submitted, review it and confirm again."),
            StatusCode::BAD_REQUEST,
        ).await;
    }
    match deletion::delete_user(state.sqlx_pool(), id).await {
        Ok(()) => Ok(Redirect::to("/admin/users/").into_response()),
        Err(UserDeletionError::NotFound) => Err(super::user_not_found()),
        Err(UserDeletionError::LastBearAdmin(preview)) => render_preview(
            &state, auth_session, &session, preview, form.confirm_delete,
            Some("Deletion would leave a Bear without an Admin. Grant another person Admin access to every affected Bear, then return here; or keep this account."),
            StatusCode::CONFLICT,
        ).await,
        Err(UserDeletionError::Referenced { constraint }) => {
            tracing::info!(?id, ?constraint, "User deletion blocked by historical references");
            render_preview(
                &state, auth_session, &session, preview, form.confirm_delete,
                Some("Historical records still refer to this account. Keep the account; handing off Bear Admin access alone does not remove those references. No changes were made."),
                StatusCode::CONFLICT,
            ).await
        }
        Err(UserDeletionError::Database(error)) => {
            tracing::error!(?id, ?error, "User deletion failed");
            render_preview(
                &state, auth_session, &session, preview, form.confirm_delete,
                Some("Deletion could not be completed. Check the account's current state, then review and confirm again."),
                StatusCode::SERVICE_UNAVAILABLE,
            ).await
        }
    }
}

#[cfg(test)]
mod tests;
