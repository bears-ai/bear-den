// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use axum::extract::Query;
use axum_login::login_required;
use serde::{Deserialize, Serialize};

use axum::{
    Router, debug_handler,
    extract::State,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::{extract::Form, routing::RouterExt};
use tokio::runtime::Handle;
use validator::{Validate, ValidateArgs, ValidationError, ValidationErrors};

use password_auth::generate_hash;

use minijinja::{context, value::merge_maps};

use std::sync::OnceLock;

use crate::{
    auth_backend::{AuthSession, Backend},
    core::user::{self, email_settings},
    errors::CustomError,
    web::{self, AppState},
};

use crate::core::user::RESERVED_NAMES;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(view_account))
        // .route_with_tsr("/edit", get(edit_account_view).post(edit_account_action))
        .route_with_tsr(
            "/password",
            get(change_password_view).post(change_password_action),
        )
        .route_layer(login_required!(Backend, login_url = "/login"))
        .route_with_tsr("/register", get(register_view).post(register_action))
    // .route("/delete", post(delete_user_action))
}

pub struct ValidateContext {
    pub db_pool: sqlx::PgPool,
    pub tokio_handle: Handle,
}

#[derive(Serialize, Deserialize, Debug, Validate)]
pub struct AccountForm {
    #[validate(length(max = 255))]
    display_name: String,
    #[validate(email)]
    email: String,
}

// create a form from a db record
impl From<user::db::User> for AccountForm {
    fn from(record: user::db::User) -> Self {
        Self {
            display_name: record.display_name,
            email: record.email,
        }
    }
}

pub fn regex_alphanumeric() -> &'static regex::Regex {
    static REGEX_ALPHANUMERIC: OnceLock<regex::Regex> = OnceLock::new();
    REGEX_ALPHANUMERIC.get_or_init(|| regex::Regex::new(r"[a-zA-Z0-9]+").unwrap())
}

// Backwards compatibility
pub use regex_alphanumeric as REGEX_ALPHANUMERIC;

#[derive(Serialize, Deserialize, Validate)]
#[validate(context = ValidateContext)]
pub struct RegisterForm {
    #[validate(custom(function = validate_invite_key))]
    invite_key: String,
    #[validate(length(min = 4, max = 30))]
    #[validate(custom(function = validate_username_format))]
    #[validate(custom(function = validate_username_allowed))]
    #[validate(custom(function = validate_username_unique, use_context))]
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
fn validate_invite_key(invite_key: &str) -> Result<(), ValidationError> {
    static INVITE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = INVITE_RE
        .get_or_init(|| regex::Regex::new(r"^[a-zA-Z0-9_-]{8,128}$").expect("invite key regex"));
    if !re.is_match(invite_key) {
        return Err(ValidationError::new("Invalid key"));
    }
    Ok(())
}

fn validate_username_format(username: &str) -> Result<(), ValidationError> {
    if !REGEX_ALPHANUMERIC().is_match(username) {
        return Err(ValidationError::new("Must be alphanumeric"));
    }
    Ok(())
}

fn validate_username_allowed(username: &str) -> Result<(), ValidationError> {
    if RESERVED_NAMES.contains(&username) {
        return Err(ValidationError::new("Username reserved"));
    }
    Ok(())
}
fn validate_username_unique(
    username: &String,
    context: &ValidateContext,
) -> Result<(), ValidationError> {
    tokio::task::block_in_place(|| {
        // let db_client = context.db_client;
        if let Ok(users_count) = context
            .tokio_handle
            .block_on(user::db::count_users_by_username(
                &context.db_pool,
                username,
            ))
        {
            if users_count > 0 {
                return Err(ValidationError::new("Username already in use."));
            }
        }
        Ok(())
    })
}

#[derive(Deserialize)]
struct RegisterQuery {
    invite: Option<String>,
}
async fn register_view(
    State(state): State<AppState>,
    Query(query): Query<RegisterQuery>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    if auth_session.user.is_some() {
        return Ok(Redirect::to("/").into_response());
    }

    let mut template_context = context! {
        pattern_invite => "^[a-zA-Z0-9_-]{8,128}$",
        pattern_username => REGEX_ALPHANUMERIC().as_str(),
    };

    if let Some(invite_key) = query.invite {
        if let Some(invite_record) = user::invites::db::check(&state.sqlx_pool, &invite_key).await?
        {
            let inviting_username = invite_record.inviting_username;
            let inviting_display_name = invite_record.inviting_display_name;
            template_context = merge_maps([
                template_context,
                context! {
                    user => context! {
                        invite_key => invite_key,
                    },
                    invite => context! {
                        key => invite_key,
                        username => inviting_username,
                        display_name => inviting_display_name
                    }
                },
            ]);
        } else {
            tracing::warn!("Invalid invite key in querystring: {}", invite_key);
        }
    }

    web::render_template(
        state.template_env,
        "account/register.html",
        auth_session,
        template_context,
    )
    .await
}

#[debug_handler]
pub async fn register_action(
    State(state): State<AppState>,
    mut auth_session: AuthSession,
    Form(form): Form<RegisterForm>,
) -> Result<Response, CustomError> {
    auth_session.logout().await?;

    let validate_context = ValidateContext {
        db_pool: state.sqlx_pool.clone(),
        tokio_handle: Handle::current(),
    };

    if let Err(form_validation_errors) = form.validate_with_args(&validate_context) {
        // TODO: abstract to share with add_user_view?
        web::render_template(
            state.template_env,
            "account/register.html",
            auth_session,
            context! {
                pattern_invite => "^[a-zA-Z0-9_-]{8,128}$",
                pattern_username => REGEX_ALPHANUMERIC().as_str(),
                errors => form_validation_errors,
                user => form,
            },
        )
        .await
    } else {
        // not using validator to check for invite, because we need to remove it

        if (user::invites::db::check(&state.sqlx_pool, &form.invite_key).await?).is_some() {
            let tx = state.sqlx_pool.begin().await?;

            let new_user_id = user::db::create_user(
                &state.sqlx_pool,
                &form.email,
                &form.username,
                &form.display_name,
                &generate_hash(form.password),
            )
            .await?;

            user::invites::db::consume(&state.sqlx_pool, &form.invite_key, new_user_id).await?;

            tx.commit().await?;

            let sqlx_pool = state.sqlx_pool.clone();

            // below mimics email::send_verify_email_for_user_idverify_email_action
            let email_sent_to = email_settings::send_verify_email_for_user_id(
                &sqlx_pool,
                new_user_id,
                &state.config,
            )
            .await?;

            Ok(web::render_template(
                state.template_env,
                "settings/email/verify_sent.html",
                auth_session,
                context! {
                email_sent_to => email_sent_to
                },
            )
            .await
            .into_response())
        } else {
            let mut form_validation_errors = ValidationErrors::new();
            form_validation_errors.add(
                "invite_key",
                ValidationError::new("This key isn't valid or has already been used"),
            );
            // again, this could be abstracted?
            web::render_template(
                state.template_env,
                "account/register.html",
                auth_session,
                context! {
                    pattern_invite => "^[a-zA-Z0-9_-]{8,128}$",
                    pattern_username => REGEX_ALPHANUMERIC().as_str(),
                    errors => form_validation_errors,
                    user => form,
                },
            )
            .await
        }
    }
}

async fn view_account(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session.user.clone().unwrap().id;
    let user = crate::core::user::user_by_id(&state.sqlx_pool, user_id).await?;

    let invites = user::invites::db::by_user_id(&state.sqlx_pool, user_id).await?;
    let invite_contexts: Vec<_> = invites
        .iter()
        .map(|invite| {
            context! {
                key => invite.code,
                new_username => invite.new_username.as_deref().unwrap_or(""),
                new_display_name => invite.new_display_name.as_deref().unwrap_or(""),
            }
        })
        .collect();

    web::render_template(
        state.template_env,
        "account/view.html",
        auth_session,
        context! {
            user => user,
            // premium_until => user.premium_until,
            invites => invite_contexts,
        },
    )
    .await
}

// async fn edit_account_view(
//     State(state): State<AppState>,
//     auth_session: AuthSession
// ) -> Result<Response, CustomError> {
//     let db_client = state.db_pool.get().await?;
//     let user_id = auth_session.user.clone().unwrap().id;
//     let user = db::queries::users::get_user_by_id()
//         .bind(&db_client, &user_id)
//         .one().await?;

//     let user_form: AccountForm = user.into();

//     web::render_template(state.template_env, "account/edit.html", auth_session, context! {
//         user => user_form,
//     }).await
// }

// pub async fn edit_account_action(
//     State(state): State<AppState>,
//     auth_session: AuthSession,
//     Form(form): Form<AccountForm>
// ) -> Result<Redirect, CustomError> {
//     let db_client = state.db_pool.get().await.unwrap();
//     let user_id = auth_session.user.clone().unwrap().id;

//     // TODO: validate

//     let email = form.email;
//     let display_name = form.display_name;
//     let _ = db::queries::users::update_account_by_id()
//         .bind(
//             &db_client,
//             &email.as_str(),
//             &display_name.as_str(),
//             &user_id,
//         )
//         .await?;

//     Ok(Redirect::to("/account"))
// }

#[derive(Validate, Serialize, Deserialize)]
pub struct ChangePasswordForm {
    #[validate(length(min = 8))]
    password: String,
    #[validate(must_match(other = "password"))]
    password_check: String,
}

pub async fn change_password_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session.user.clone().unwrap().id;
    let username = user::db::get_username_by_id(&state.sqlx_pool, user_id)
        .await?
        .ok_or(CustomError::NotFound("User not found".to_string()))?;

    web::render_template(
        state.template_env,
        "account/password.html",
        auth_session,
        context! {
            target => context!{ username },
            // errors => validation_errors,
        },
    )
    .await
}

pub async fn change_password_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<ChangePasswordForm>,
) -> Result<Response, CustomError> {
    let user_id = auth_session.user.clone().unwrap().id;
    if let Err(form_validation_errors) = form.validate() {
        let username = user::db::get_username_by_id(&state.sqlx_pool, user_id)
            .await?
            .ok_or(CustomError::NotFound("User not found".to_string()))?;

        Ok(web::render_template(
            state.template_env,
            "account/password.html",
            auth_session,
            context! {
                form => context! {
                    error => form_validation_errors,
                },
                target => context!{ username },
            },
        )
        .await?
        .into_response())
    } else {
        user::db::set_user_passhash_by_id(&state.sqlx_pool, user_id, &generate_hash(form.password))
            .await?;

        // TODO redirect to user detail page with success message
        Ok(Redirect::to("/account").into_response())
    }
}
