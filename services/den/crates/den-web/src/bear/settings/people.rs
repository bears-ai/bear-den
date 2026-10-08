//! Membership form feedback; the shared transactional service owns role validation
//! and the last-admin guard. Never pre-count admins in this HTTP boundary.
use super::{bear_nav_context, load_session_bear_manage, MemberGrantForm};
use crate::{
    auth_backend::AuthSession,
    core::user::db as user_db,
    errors::CustomError,
    web::admin::bears::{membership_role_label, BearMemberAdminRow},
    web::{self, AppState},
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::Form;
use den_core::DenError;
use den_service::bears::db as bears_db;
use minijinja::context;

pub(super) async fn render(
    state: &AppState,
    auth_session: AuthSession,
    bear: den_service::bears::Bear,
    can_manage_bear: bool,
    member_form: MemberGrantForm,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, CustomError> {
    let members: Vec<BearMemberAdminRow> =
        bears_db::list_members_for_bear(state.sqlx_pool(), bear.id)
            .await?
            .into_iter()
            .map(|member| BearMemberAdminRow {
                role_label: membership_role_label(member.role.as_deref()),
                user_id: member.user_id,
                username: member.username,
                display_name: member.display_name,
                role: member.role,
            })
            .collect();
    let status = if error.is_some() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    let response = web::render_template(
        state,
        "bear/settings/access.html",
        auth_session,
        context! {
            members, member_form, message, error, can_manage_bear, native_runtime => true,
            ..bear_nav_context(&bear, "people"),
        },
    )
    .await?;
    Ok((status, response).into_response())
}

async fn rejected(
    state: &AppState,
    auth_session: AuthSession,
    slug: &str,
    form: MemberGrantForm,
    error: String,
) -> Result<Response, CustomError> {
    // Authorization is checked again for the rerender: a racing revocation must
    // not leave a formerly privileged actor with a writable People view.
    let bear = match load_session_bear_manage(state, &auth_session, slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    render(state, auth_session, bear, true, form, None, Some(error)).await
}

pub(super) async fn grant(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(mut form): Form<MemberGrantForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let role = match bears_db::BearMembershipRole::try_from(Some(form.role.as_str())) {
        Ok(role) => role,
        Err(DenError::ValidationError(error)) => {
            return rejected(&state, auth_session, &slug, form, error).await
        }
        Err(error) => return Err(error.into()),
    };
    form.role = role.as_str().into();
    let target = if let Some(id) = form.user_id {
        if id <= 0 {
            return rejected(
                &state,
                auth_session,
                &slug,
                form,
                "Choose an existing user.".into(),
            )
            .await;
        }
        // An ID is a boundary compatibility input, not a bypass for target validation.
        let user = user_db::get_user_by_id(state.sqlx_pool(), id).await?;
        if let Some(user) = &user {
            form.username.clone_from(&user.username);
        }
        user.map(|user| user.id)
    } else {
        if form.username.trim().is_empty() {
            return rejected(
                &state,
                auth_session,
                &slug,
                form,
                "Username is required.".into(),
            )
            .await;
        }
        user_db::get_user_by_username(state.sqlx_pool(), form.username.trim())
            .await?
            .map(|user| user.id)
    };
    let Some(target) = target else {
        return rejected(
            &state,
            auth_session,
            &slug,
            form,
            "User not found. Check the username and try again.".into(),
        )
        .await;
    };
    match bears_db::grant_membership(state.sqlx_pool(), target, bear.id, Some(role.as_str())).await
    {
        Ok(()) => Ok(Redirect::to(&format!(
            "/bear/{}/people?message={}",
            bear.slug,
            urlencoding::encode("Access saved.")
        ))
        .into_response()),
        Err(DenError::ValidationError(error) | DenError::NotFound(error)) => {
            rejected(&state, auth_session, &slug, form, error).await
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn revoke(
    Path((slug, user_id)): Path<(String, i32)>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    match bears_db::revoke_membership(state.sqlx_pool(), user_id, bear.id).await {
        Ok(()) => Ok(Redirect::to(&format!(
            "/bear/{}/people?message={}",
            bear.slug,
            urlencoding::encode("Access removed.")
        ))
        .into_response()),
        Err(DenError::ValidationError(error) | DenError::NotFound(error)) => {
            rejected(
                &state,
                auth_session,
                &slug,
                MemberGrantForm::default(),
                error,
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}
