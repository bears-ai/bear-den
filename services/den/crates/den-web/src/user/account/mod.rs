// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use axum::extract::Query;
use axum_login::{login_required, tower_sessions::Session, AuthnBackend};
use serde::{Deserialize, Serialize};

use axum::{
    debug_handler,
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use tokio::runtime::Handle;
use uuid::Uuid;
use validator::{Validate, ValidateArgs, ValidationError, ValidationErrors};

use password_auth::generate_hash;

use minijinja::{context, value::merge_maps};

use std::sync::OnceLock;

use crate::{
    auth_backend::{AuthSession, Backend},
    core::{
        armature_tokens,
        user::{self, email_settings},
    },
    errors::CustomError,
    web::{self, AppState},
};

use super::form_feedback::validation_messages;
use crate::core::user::RESERVED_NAMES;

mod token_view;
use token_view::AccountTokenView;

#[cfg(test)]
mod tests;

const ACCOUNT_NOTICE_KEY: &str = "account_notice";

#[derive(Serialize, Deserialize)]
enum AccountNotice {
    PasswordChanged,
    TokenRevoked,
}

impl AccountNotice {
    fn message(&self) -> &'static str {
        match self {
            Self::PasswordChanged => "Password changed. This session stays signed in; other web sessions will need to sign in again.",
            Self::TokenRevoked => "Editor token revoked. It can no longer be used to connect; earlier actions are unchanged.",
        }
    }
}

#[derive(Serialize)]
struct EditorSetupBear {
    slug: String,
    name: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(view_account))
        // .route_with_tsr("/edit", get(edit_account_view).post(edit_account_action))
        .route_with_tsr(
            "/password",
            get(change_password_view).post(change_password_action),
        )
        .route(
            "/armature-tokens/{token_id}/revoke",
            post(revoke_armature_token_action),
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
    REGEX_ALPHANUMERIC.get_or_init(|| regex::Regex::new(r"^[a-zA-Z0-9]+$").unwrap())
}

// Backwards compatibility
pub use regex_alphanumeric as REGEX_ALPHANUMERIC;

#[derive(Serialize, Deserialize, Validate)]
#[validate(context = ValidateContext)]
pub struct RegisterForm {
    #[validate(custom(function = validate_invite_key))]
    invite_key: String,
    #[validate(length(min = 4, max = 30, message = "Use 4–30 letters and numbers."))]
    #[validate(custom(function = validate_username_format))]
    #[validate(custom(function = validate_username_allowed))]
    #[validate(custom(function = validate_username_unique, use_context))]
    username: String,
    #[validate(length(max = 255, message = "Use no more than 255 characters."))]
    display_name: String,
    #[validate(email(message = "Enter a valid email address."))]
    email: String,
    #[serde(skip_serializing)]
    #[validate(length(min = 8, message = "Use at least 8 characters."))]
    password: String,
    #[serde(skip_serializing)]
    #[validate(length(min = 8, message = "Use at least 8 characters."))]
    #[validate(must_match(other = "password", message = "Passwords must match."))]
    password_check: String,
    #[serde(default)]
    #[validate(custom(function = validate_terms_consent))]
    terms: String,
}
fn validate_terms_consent(terms: &str) -> Result<(), ValidationError> {
    if terms == "on" {
        Ok(())
    } else {
        Err(ValidationError::new("consent")
            .with_message("Confirm the terms acknowledgement to create an account.".into()))
    }
}

fn validate_invite_key(invite_key: &str) -> Result<(), ValidationError> {
    static INVITE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = INVITE_RE
        .get_or_init(|| regex::Regex::new(r"^[a-zA-Z0-9_-]{8,128}$").expect("invite key regex"));
    if !re.is_match(invite_key) {
        return Err(ValidationError::new("invite_format")
            .with_message("Use an invitation code of 8–128 letters, numbers, _ or -.".into()));
    }
    Ok(())
}

fn validate_username_format(username: &str) -> Result<(), ValidationError> {
    if !REGEX_ALPHANUMERIC().is_match(username) {
        return Err(ValidationError::new("username_format")
            .with_message("Use letters and numbers only.".into()));
    }
    Ok(())
}

fn validate_username_allowed(username: &str) -> Result<(), ValidationError> {
    if RESERVED_NAMES.contains(&username) {
        return Err(ValidationError::new("username_reserved")
            .with_message("Choose a different username; this one is reserved.".into()));
    }
    Ok(())
}
fn validate_username_unique(
    username: &str,
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
                return Err(ValidationError::new("username_taken")
                    .with_message("This username is already in use.".into()));
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
            tracing::warn!("Invalid invitation in registration link");
            let mut errors = ValidationErrors::new();
            errors.add(
                "invite_key",
                ValidationError::new("invite_unavailable").with_message(
                    "This invitation is invalid or has already been used. Ask for a new one."
                        .into(),
                ),
            );
            template_context = merge_maps([
                template_context,
                context! {
                    invite_error => "This invitation is invalid or has already been used. Ask for a new one.",
                    user => context! { invite_key, errors => validation_messages(&errors) },
                },
            ]);
        }
    }

    web::render_template(
        &state,
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
        web::render_template(
            &state,
            "account/register.html",
            auth_session,
            context! {
                pattern_invite => "^[a-zA-Z0-9_-]{8,128}$",
                pattern_username => REGEX_ALPHANUMERIC().as_str(),
                errors => validation_messages(&form_validation_errors),
                user => context! { errors => validation_messages(&form_validation_errors), ..minijinja::Value::from_serialize(&form) },
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
                &state,
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
                ValidationError::new("invite_unavailable").with_message(
                    "This invitation is invalid or has already been used. Ask for a new one."
                        .into(),
                ),
            );
            // again, this could be abstracted?
            web::render_template(
                &state,
                "account/register.html",
                auth_session,
                context! {
                    pattern_invite => "^[a-zA-Z0-9_-]{8,128}$",
                    pattern_username => REGEX_ALPHANUMERIC().as_str(),
                    errors => validation_messages(&form_validation_errors),
                    user => context! { errors => validation_messages(&form_validation_errors), ..minijinja::Value::from_serialize(&form) },
                },
            )
            .await
        }
    }
}

async fn view_account(
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|user| user.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user = crate::core::user::user_by_id(&state.sqlx_pool, user_id).await?;

    let account_notice = session.remove::<AccountNotice>(ACCOUNT_NOTICE_KEY).await?;
    let editor_setup_bears: Vec<_> =
        den_service::bears::db::list_bears_for_user(&state.sqlx_pool, user_id)
            .await?
            .into_iter()
            .map(|row| EditorSetupBear {
                slug: row.bear.slug,
                name: row.bear.name,
            })
            .collect();
    let invites = user::invites::db::by_user_id(&state.sqlx_pool, user_id).await?;
    let now = time::OffsetDateTime::now_utc();
    let armature_tokens: Vec<_> = armature_tokens::list_for_user(&state.sqlx_pool, user_id)
        .await?
        .into_iter()
        .map(|token| AccountTokenView::at(token, now))
        .collect();
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
        &state,
        "account/view.html",
        auth_session,
        context! {
            user => user,
            // premium_until => user.premium_until,
            invites => invite_contexts,
            armature_tokens => armature_tokens,
            editor_setup_bears,
            account_message => account_notice.as_ref().map(AccountNotice::message),
        },
    )
    .await
}

async fn revoke_armature_token_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
    Path(token_id): Path<Uuid>,
) -> Result<Redirect, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    armature_tokens::revoke_for_user(&state.sqlx_pool, user_id, token_id).await?;
    session
        .insert(ACCOUNT_NOTICE_KEY, AccountNotice::TokenRevoked)
        .await?;
    Ok(Redirect::to("/account"))
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

//     web::render_template(&state, "account/edit.html", auth_session, context! {
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
    #[serde(skip_serializing)]
    #[validate(length(min = 8, message = "Use at least 8 characters."))]
    password: String,
    #[serde(skip_serializing)]
    #[validate(length(min = 8, message = "Use at least 8 characters."))]
    #[validate(must_match(other = "password", message = "Passwords must match."))]
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
        &state,
        "account/password.html",
        auth_session,
        context! {
            target => context!{ username },
            form => context! {},
        },
    )
    .await
}

pub async fn change_password_action(
    State(state): State<AppState>,
    mut auth_session: AuthSession,
    session: Session,
    Form(form): Form<ChangePasswordForm>,
) -> Result<Response, CustomError> {
    let user_id = auth_session.user.clone().unwrap().id;
    if let Err(form_validation_errors) = form.validate() {
        let username = user::db::get_username_by_id(&state.sqlx_pool, user_id)
            .await?
            .ok_or(CustomError::NotFound("User not found".to_string()))?;

        Ok(web::render_template(
            &state,
            "account/password.html",
            auth_session,
            context! {
                form => context! {
                    errors => validation_messages(&form_validation_errors),
                },
                target => context!{ username },
            },
        )
        .await?
        .into_response())
    } else {
        user::db::set_user_passhash_by_id(&state.sqlx_pool, user_id, &generate_hash(form.password))
            .await?;

        // The password hash is also the session auth hash. Refresh this session before redirecting.
        let updated_user = auth_session
            .backend
            .get_user(&user_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("User not found".to_string()))?;
        auth_session.login(&updated_user).await?;
        session
            .insert(ACCOUNT_NOTICE_KEY, AccountNotice::PasswordChanged)
            .await?;
        Ok(Redirect::to("/account").into_response())
    }
}
