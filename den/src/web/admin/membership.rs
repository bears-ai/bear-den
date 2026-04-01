// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::AuthSession,
    core::{bears::db as bears_db, user::db as user_db},
    errors::CustomError,
    web::{self, AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/membership/", get(list_view))
        .route_with_tsr("/membership/grant", get(grant_view).post(grant_action))
}

#[derive(Validate, Serialize, Deserialize, Debug)]
pub struct GrantMembershipForm {
    #[validate(range(min = 1))]
    user_id: i32,
    #[validate(length(min = 1))]
    bear_id: String,
    #[validate(length(max = 64))]
    role: String,
}

async fn list_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let rows = bears_db::list_memberships(state.sqlx_pool()).await?;
    web::render_template(
        &state,
        "admin/membership/list.html",
        auth_session,
        context! { memberships => rows },
    )
    .await
}

async fn grant_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let users = user_db::get_users(state.sqlx_pool()).await?;
    let bears = bears_db::list_bears(state.sqlx_pool()).await?;
    web::render_template(
        &state,
        "admin/membership/grant.html",
        auth_session,
        context! {
            users,
            bears,
        },
    )
    .await
}

pub async fn grant_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<GrantMembershipForm>,
) -> Result<Response, CustomError> {
    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    let bear_id: Option<Uuid> = form.bear_id.trim().parse().ok();
    if bear_id.is_none() {
        validation_errors.add(
            "bear_id",
            ValidationError::new("bear_id must be a valid UUID"),
        );
    }

    if user_db::get_user_by_id(state.sqlx_pool(), form.user_id).await?.is_none() {
        validation_errors.add(
            "user_id",
            ValidationError::new("User not found."),
        );
    }

    if validation_errors.is_empty() {
        let bid = bear_id.expect("checked");
        if bears_db::get_bear(state.sqlx_pool(), bid).await?.is_none() {
            validation_errors.add(
                "bear_id",
                ValidationError::new("Bear not found."),
            );
        }
    }

    if validation_errors.is_empty() {
        let bid = bear_id.expect("checked");
        let role = form.role.trim();
        let role_opt = if role.is_empty() {
            None
        } else {
            Some(role)
        };
        bears_db::grant_membership(state.sqlx_pool(), form.user_id, bid, role_opt).await?;
        Ok(Redirect::to("/admin/membership/").into_response())
    } else {
        let users = user_db::get_users(state.sqlx_pool()).await?;
        let bears = bears_db::list_bears(state.sqlx_pool()).await?;
        web::render_template(
            &state,
            "admin/membership/grant.html",
            auth_session,
            context! {
                errors => validation_errors,
                form => form,
                users,
                bears,
            },
        )
        .await
    }
}
