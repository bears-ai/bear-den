//! Human owner management, separate from shared memory and the model tool surface.
mod session_state;
#[cfg(test)]
mod tests;
mod views;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Extension, Router,
};
use axum_extra::{
    extract::{Form, FormRejection},
    routing::RouterExt,
};
use axum_login::tower_sessions::Session;
use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_service::personal_memory::{self, OwnerMemoryAction, PreferredName};
use serde::Deserialize;

use super::settings::{load_session_bear, session_user};
use crate::{auth_backend::AuthSession, errors::CustomError, AppState};
use session_state::{ActionNonce, Intent, Operation, Registry, Scope};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bear/{slug}/memory/about-you", get(page))
        .route_with_tsr(
            "/bear/{slug}/memory/about-you/capture",
            post(submit).layer(Extension(Operation::Capture)),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/about-you/request",
            post(submit).layer(Extension(Operation::Request)),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/about-you/review",
            post(submit).layer(Extension(Operation::Review)),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/about-you/publish",
            post(submit).layer(Extension(Operation::Publish)),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/about-you/retract",
            post(submit).layer(Extension(Operation::Retract)),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/about-you/erase",
            post(submit).layer(Extension(Operation::Erase)),
        )
        .layer(Extension(Registry::default()))
        .layer(middleware::map_response(views::private))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionForm {
    token: ActionNonce,
    name: Option<String>,
    #[serde(default)]
    acknowledged: bool,
}

fn session_id(session: &Session) -> Result<String, CustomError> {
    session.id().map(|id| id.to_string()).ok_or_else(|| {
        CustomError::Authentication("Sign in again before managing your preferred name.".into())
    })
}

async fn page(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
    Extension(registry): Extension<Registry>,
) -> Result<Response, CustomError> {
    let (bear, can_manage) = match load_session_bear(&state, &auth, &slug).await? {
        Ok(value) => value,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let actor = UserId::new(session_user(&auth).await?.id);
    let scope = Scope::new(BearId::new(bear.id), actor, session_id(&session)?);
    let inspection = personal_memory::inspect_personal_memory(
        state.sqlx_pool(),
        state.memory_stores()?,
        BearId::new(bear.id),
        actor,
    )
    .await;
    match inspection {
        Ok(inspection) => {
            views::page(
                &state, auth, &bear, can_manage, inspection, scope, &registry,
            )
            .await
        }
        Err(error) => {
            let (status, message) = failure(&error);
            views::failure(&state, auth, &bear, can_manage, status, message).await
        }
    }
}

fn failure(error: &DenError) -> (StatusCode, &'static str) {
    match error {
        DenError::Authorization(_) => (StatusCode::FORBIDDEN, "Your access or identity changed. Refresh About you before trying again."),
        DenError::ValidationError(_) => (StatusCode::BAD_REQUEST, "This action is stale or could not be accepted. Refresh About you and review the current state."),
        DenError::NotFound(_) => (StatusCode::NOT_FOUND, "This request is no longer available. Refresh About you."),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "The outcome could not be confirmed. Refresh About you to check the current state before making another request."),
    }
}

impl Intent {
    fn action(self, form: ActionForm) -> Result<OwnerMemoryAction, &'static str> {
        if !matches!(self, Self::Request { .. }) && form.name.is_some() {
            return Err("This form could not be accepted. Refresh About you.");
        }
        match self {
            Self::Capture(enabled) => {
                if !form.acknowledged {
                    return Err("Confirm the capture setting and its consequences before saving.");
                }
                Ok(OwnerMemoryAction::SetCapture(enabled))
            }
            Self::Request { request_id } => {
                let name =
                    PreferredName::new(form.name.as_deref().unwrap_or("")).map_err(|_| {
                        "Enter a preferred name of 1–128 UTF-8 bytes without control characters."
                    })?;
                Ok(OwnerMemoryAction::RequestPreferredName { request_id, name })
            }
            Self::Publish {
                candidate_id,
                expected_head,
            } => {
                if !form.acknowledged {
                    return Err("Explicitly approve this preferred name before publishing it.");
                }
                Ok(OwnerMemoryAction::Publish {
                    candidate_id,
                    expected_head,
                })
            }
            Self::Review {
                candidate_id,
                decision,
                expected_review,
            } => Ok(OwnerMemoryAction::Review {
                candidate_id,
                decision,
                expected_review,
            }),
            Self::Retract { expected_head } => {
                if !form.acknowledged {
                    return Err("Confirm retraction. Retraction does not erase retained history.");
                }
                Ok(OwnerMemoryAction::Retract { expected_head })
            }
            Self::ErasePreferredNames { request_id } => {
                if !form.acknowledged {
                    return Err("Separately confirm erasing all stored preferred-name bodies and tracked derived copies, with capture disabled.");
                }
                Ok(OwnerMemoryAction::ErasePreferredNames { request_id })
            }
        }
    }
}

async fn submit(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
    Extension(registry): Extension<Registry>,
    Extension(operation): Extension<Operation>,
    form: Result<Form<ActionForm>, FormRejection>,
) -> Result<Response, CustomError> {
    let (bear, can_manage) = match load_session_bear(&state, &auth, &slug).await? {
        Ok(value) => value,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let actor = UserId::new(session_user(&auth).await?.id);
    let pending = match form {
        Ok(Form(form)) => registry
            .authorize(
                form.token,
                operation,
                BearId::new(bear.id),
                actor,
                &session_id(&session)?,
            )
            .map(|pending| (pending, form)),
        Err(_) => Err("This form could not be accepted. Refresh About you."),
    };
    let (pending, form) = match pending {
        Ok(value) => value,
        Err(message) => {
            return views::failure(
                &state,
                auth,
                &bear,
                can_manage,
                StatusCode::BAD_REQUEST,
                message,
            )
            .await
        }
    };
    let action = match pending.intent.action(form) {
        Ok(action) => action,
        Err(message) => {
            return views::failure(
                &state,
                auth,
                &bear,
                can_manage,
                StatusCode::BAD_REQUEST,
                message,
            )
            .await
        }
    };
    // The service uses the retained OLD binding under its authority fences. Never
    // replace this with an inspection followed by a freshly resolved legacy action.
    match personal_memory::apply_owner_memory_action(
        state.sqlx_pool(),
        state.memory_stores()?,
        BearId::new(bear.id),
        actor,
        &pending.owner,
        action,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to(&format!(
            "/bear/{}/memory/about-you",
            urlencoding::encode(&bear.slug),
        ))
        .into_response()),
        Err(error) => {
            let (status, message) = failure(&error);
            views::failure(&state, auth, &bear, can_manage, status, message).await
        }
    }
}
