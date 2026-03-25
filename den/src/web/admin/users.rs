// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use serde::{Deserialize, Serialize};

use axum::{
    Router, debug_handler,
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;

use validator::{Validate, ValidationError, ValidationErrors};

use password_auth::generate_hash;

use minijinja::context;

use crate::{
    auth_backend::AuthSession,
    core::{email, user, user::db as user_db},
    errors::CustomError,
    web::{self, AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/users/", get(users_list))
        .route_with_tsr("/users/add", get(add_user_view).post(add_user_action))
        .route_with_tsr("/users/{id}", get(view_user))
        .route_with_tsr("/users/{id}/test_email", get(send_test_email_action))
        .route_with_tsr(
            "/users/{id}/edit",
            get(edit_user_view).post(edit_user_action),
        )
        .route_with_tsr(
            "/users/{id}/change_password",
            get(change_user_password_view).post(change_user_password_action),
        )
        .route_with_tsr("/users/{id}/create_invite", post(create_invite_action))
        .route_with_tsr("/users/{id}/delete", post(delete_user_action))
}

#[derive(Validate, Serialize, Deserialize, Debug)]
pub struct UserForm {
    #[validate(length(min = 1, max = 30))] // uniqueness validated in action
    username: String,
    #[validate(length(max = 255))]
    display_name: String,
    #[validate(email)]
    email: String,
    theme: String,
    #[validate(range(min = 0, max = 6))]
    week_start_day: i32,
}

// create a form from a db record
impl From<crate::core::user::User> for UserForm {
    fn from(record: crate::core::user::User) -> Self {
        Self {
            username: record.username,
            display_name: record.display_name,
            email: record.email,
            theme: record.theme,
            week_start_day: record.week_start_day,
        }
    }
}

#[derive(Validate, Serialize, Deserialize)]
pub struct NewUserForm {
    #[validate(length(min = 3, max = 30))] // uniqueness validated in action
    username: String,
    #[validate(length(max = 255))]
    display_name: String,
    #[validate(email)]
    email: String,
    #[validate(length(min = 8))]
    password: String,
    #[validate(must_match(other = "password"))]
    password_check: String,
}

async fn users_list(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let users = user_db::get_users(&state.sqlx_pool).await?;

    web::render_template(
        state.template_env,
        "admin/users/list.html",
        auth_session,
        context! {
            users
        },
    )
    .await
}

async fn add_user_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    web::render_template(
        state.template_env,
        "admin/users/add.html",
        auth_session,
        context! {},
    )
    .await
}

#[debug_handler]
pub async fn add_user_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewUserForm>,
) -> Result<Response, CustomError> {
    // thanks https://github.com/Keats/validator/issues/191#issuecomment-2346470065
    let mut validation_errors = ValidationErrors::new();
    if let Err(form_validation_errors) = form.validate() {
        validation_errors = form_validation_errors;
    }
    // TODO: abstract this, maybe to an custom macro with a context for a client.transaction: https://github.com/Keats/validator?tab=readme-ov-file#custom

    let users_count = user_db::count_users_by_username(&state.sqlx_pool, &form.username).await?;
    if users_count > 0 {
        validation_errors.add(
            "username",
            ValidationError::new("A user with this username already exists."),
        );
    }

    if validation_errors.is_empty() {
        let _ = user_db::create_user(
            &state.sqlx_pool,
            &form.email,
            &form.username,
            &form.display_name,
            &generate_hash(form.password),
        )
        .await?;

        Ok(Redirect::to("/admin/users/").into_response())
    } else {
        // add_user_view
        // TODO: abstract to share with add_user_view

        web::render_template(
            state.template_env,
            "admin/users/add.html",
            auth_session,
            context! {
                errors => validation_errors,
                user => form,
            },
        )
        .await
    }
}

async fn view_user(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = user::user_by_id(&state.sqlx_pool, id).await?;

    let user_form: UserForm = user.into();

    let invites = user::invites::db::by_user_id(&state.sqlx_pool, id).await?;

    web::render_template(
        state.template_env,
        "admin/users/view.html",
        auth_session,
        context! {
            id,
            user => user_form,
            invites,
        },
    )
    .await
}

async fn edit_user_view(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = user::user_by_id(&state.sqlx_pool, id).await?;

    let user_form: UserForm = user.into();

    web::render_template(
        state.template_env,
        "admin/users/edit.html",
        auth_session,
        context! {
            id,
            user => user_form,
            theme_descriptions => web::theme_descriptions(),
            day_of_week_names => web::day_of_week_names(),
        },
    )
    .await
}

#[debug_handler]
pub async fn edit_user_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    Form(form): Form<UserForm>,
) -> Result<Redirect, CustomError> {
    // TODO: validate

    user_db::update_user_by_id(
        &state.sqlx_pool,
        id,
        &form.email,
        &form.username,
        &form.display_name,
        &form.theme,
        form.week_start_day,
    )
    .await?;

    // 303 redirect to users list
    Ok(Redirect::to("/admin/users/"))
}

#[derive(Validate, Serialize, Deserialize)]
pub struct ChangePasswordForm {
    #[validate(length(min = 8))]
    password: String,
    #[validate(must_match(other = "password"))]
    password_check: String,
}

pub async fn change_user_password_view(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let username = user_db::get_username_by_id(&state.sqlx_pool, id)
        .await?
        .ok_or_else(|| CustomError::NotFound("User not found".to_string()))?;

    web::render_template(
        state.template_env,
        "admin/users/change_password.html",
        auth_session,
        context! {
            id,
            target => context!{ username },
            // errors => validation_errors,
        },
    )
    .await
}

pub async fn change_user_password_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<ChangePasswordForm>,
) -> Result<Response, CustomError> {
    if let Err(form_validation_errors) = form.validate() {
        let username = user_db::get_username_by_id(&state.sqlx_pool, id)
            .await?
            .ok_or_else(|| CustomError::NotFound("User not found".to_string()))?;

        Ok(web::render_template(
            state.template_env,
            "admin/users/change_password.html",
            auth_session,
            context! {
                id,
                form => context! {
                    error => form_validation_errors,
                },
                target => context!{ username },
            },
        )
        .await?
        .into_response())
    } else {
        user_db::set_user_passhash_by_id(&state.sqlx_pool, id, &generate_hash(form.password))
            .await?;

        // TODO redirect to user detail page with success message
        Ok(Redirect::to("/admin/users/").into_response())
    }
}

pub async fn send_test_email_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    let sqlx_pool = state.sqlx_pool;
    let user = user_db::get_user_by_id(&sqlx_pool, id)
        .await?
        .ok_or_else(|| CustomError::NotFound("User not found".to_string()))?;

    // if ! user.email_verified.unwrap_or(false) {
    //     return Err(CustomError::Email("User email is not verified".to_string()));
    // }

    let mut minijinja_env = minijinja::Environment::new();
    minijinja_contrib::add_to_environment(&mut minijinja_env); // needed for the 'datetimeformat' filter
    #[cfg(feature = "production")]
    minijinja_embed::load_templates!(&mut minijinja_env, "email");
    #[cfg(not(feature = "production"))]
    minijinja_env.set_loader(minijinja::path_loader("src/core/email/templates"));

    let cfg = match email::get_current_config(&sqlx_pool, user.id).await {
        Ok(c) => c,
        Err(_) => {
            sqlx::query(
                r#"INSERT INTO email_configs (user_id, email_address, active, verified_at)
                   VALUES ($1, $2, true, NOW())"#,
            )
            .bind(user.id)
            .bind(&user.email)
            .execute(&sqlx_pool)
            .await?;
            email::get_current_config(&sqlx_pool, user.id).await?
        }
    };

    email::send_email_template(
        &sqlx_pool,
        &state.config,
        cfg,
        "Test email".to_string(),
        minijinja_env,
        "test_email.html",
        minijinja::context! { display_name => user.display_name },
        None,
    )
    .await?;

    // 303 redirect to users list
    Ok(Redirect::to("/admin/users/"))
}

pub async fn delete_user_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    user_db::delete_user_by_id(&state.sqlx_pool, id).await?;

    // 303 redirect to users list
    Ok(Redirect::to("/admin/users/"))
}

pub async fn create_invite_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    let code: String = {
        use rand::Rng;
        let mut rng = rand::rng();
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        (0..24)
            .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
            .collect()
    };
    user::invites::db::create(&state.sqlx_pool, id, &code).await?;

    // 303 redirect to user detail page
    Ok(Redirect::to(&format!("/admin/users/{}", id)))
}
