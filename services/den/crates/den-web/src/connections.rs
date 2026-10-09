//! Browser mutations for reusable owner-scoped Connections.

use crate::{auth_backend::AuthSession, errors::CustomError, AppState};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use axum_extra::extract::Form;
use den_core::ids::UserId;
use den_service::connections::{self, ConnectionId, Material, Provider};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
struct NewConnection {
    name: String,
    provider: Provider,
    #[serde(default)]
    secret: String,
    #[serde(default)]
    installation: String,
    #[serde(default)]
    allow_write: bool,
    #[serde(default)]
    backend_binding_id: Option<Uuid>,
    #[serde(default)]
    external_secret_id: Option<Uuid>,
    #[serde(default)]
    external_secret_version: Option<i64>,
    #[serde(default)]
    confirm_external_boundary: bool,
}
#[derive(Serialize)]
pub(crate) struct ConnectionDraft {
    pub name: String,
    pub provider: Provider,
    pub installation: String,
    pub allow_write: bool,
}

#[derive(Serialize)]
pub(crate) struct AttachmentFeedback {
    pub account_id: ConnectionId,
    pub surface_id: Uuid,
    pub error: String,
}

#[derive(Deserialize)]
struct Revision {
    revision: i64,
    #[serde(default)]
    confirmed: bool,
}
#[derive(Deserialize)]
struct Repository {
    surface_id: Uuid,
    #[serde(default)]
    confirmed: bool,
}

#[derive(Default, Deserialize)]
struct Confirmation {
    #[serde(default)]
    confirmed: bool,
}

fn require_confirmation(confirmed: bool) -> Result<(), CustomError> {
    if confirmed {
        Ok(())
    } else {
        Err(CustomError::ValidationError(
            "Review the affected repositories and confirm the connection change before submitting."
                .into(),
        ))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/connections/create", post(create))
        .route("/connections/{id}/revoke", post(revoke))
        .route("/connections/{id}/repositories", post(attach))
        .route("/connections/repositories/{id}/detach", post(detach))
}

async fn owner(state: &AppState, session: &AuthSession) -> Result<UserId, CustomError> {
    let user = session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".into()))?;
    if crate::bear::member::email_verify_redirect(state.sqlx_pool(), user.id)
        .await?
        .is_some()
    {
        return Err(CustomError::Authorization(
            "verify email before managing connections".into(),
        ));
    }
    Ok(UserId::new(user.id))
}
async fn synced(state: &AppState) -> Response {
    let note = crate::work::surfaces::push_surfaces_best_effort(state).await;
    Redirect::to(if note.is_some() {
        "/connections?sync=pending"
    } else {
        "/connections"
    })
    .into_response()
}
async fn create(
    State(state): State<AppState>,
    session: AuthSession,
    Form(form): Form<NewConnection>,
) -> Result<Response, CustomError> {
    let draft = ConnectionDraft {
        name: form.name.clone(),
        provider: form.provider,
        installation: form.installation.clone(),
        allow_write: form.allow_write,
    };
    match create_inner(&state, &session, form).await {
        Err(CustomError::ValidationError(message)) => {
            let mut response = crate::management_hub::render_connections(
                &state,
                session,
                false,
                Some(&draft),
                Some(&message),
                None,
            )
            .await?;
            *response.status_mut() = axum::http::StatusCode::BAD_REQUEST;
            Ok(response)
        }
        result => result,
    }
}

async fn create_inner(
    state: &AppState,
    session: &AuthSession,
    form: NewConnection,
) -> Result<Response, CustomError> {
    let actor = owner(state, session).await?;
    if form.provider != Provider::GithubExternal
        && (form.backend_binding_id.is_some()
            || form.external_secret_id.is_some()
            || form.external_secret_version.is_some()
            || form.confirm_external_boundary)
    {
        return Err(CustomError::ValidationError(
            "External references cannot be combined with legacy token/key or App material.".into(),
        ));
    }
    if form.secret.len() > 128_000 {
        return Err(CustomError::ValidationError("secret too large".into()));
    }
    if matches!(form.provider, Provider::GitHttps | Provider::GitSsh)
        && form.secret.trim().is_empty()
    {
        return Err(CustomError::ValidationError("Enter the HTTPS access token or SSH private key for the selected provider; no account was created.".into()));
    }
    let material = match form.provider {
        Provider::GitHttps => Material::HttpsToken(form.secret),
        Provider::GitSsh => Material::SshKey(form.secret),
        Provider::GithubExternal => {
            if !form.confirm_external_boundary
                || !form.secret.is_empty()
                || !form.installation.is_empty()
                || form.allow_write
            {
                return Err(CustomError::ValidationError("Confirm the unconfigured external-backend boundary; do not submit token/key or App material with a reference.".into()));
            }
            let reference = den_service::repository::ExternalReference::new(
                form.backend_binding_id.ok_or_else(|| {
                    CustomError::ValidationError("internal backend binding UUID required".into())
                })?,
                form.external_secret_id.ok_or_else(|| {
                    CustomError::ValidationError(
                        "internal credential reference UUID required".into(),
                    )
                })?,
                form.external_secret_version.ok_or_else(|| {
                    CustomError::ValidationError("positive credential version required".into())
                })?,
            )
            .map_err(|_| {
                CustomError::ValidationError(
                    "non-empty UUID bindings and a positive credential version are required".into(),
                )
            })?;
            Material::ExternalReference(reference)
        }
        Provider::GithubApp => Material::GithubApp {
            installation: form.installation.trim().parse().map_err(|_| {
                CustomError::ValidationError("positive GitHub installation ID required".into())
            })?,
            write: form.allow_write,
        },
    };
    connections::create(
        state.sqlx_pool(),
        actor,
        &form.name,
        material,
        &state.config.den_secret_encryption_key,
    )
    .await?;
    Ok(Redirect::to("/connections").into_response())
}
async fn revoke(
    State(state): State<AppState>,
    session: AuthSession,
    Path(id): Path<ConnectionId>,
    Form(form): Form<Revision>,
) -> Result<Response, CustomError> {
    require_confirmation(form.confirmed)?;
    connections::revoke(
        state.sqlx_pool(),
        owner(&state, &session).await?,
        id,
        form.revision,
    )
    .await?;
    Ok(synced(&state).await)
}
async fn attach(
    State(state): State<AppState>,
    session: AuthSession,
    Path(id): Path<ConnectionId>,
    Form(form): Form<Repository>,
) -> Result<Response, CustomError> {
    let actor = owner(&state, &session).await?;
    let result = attach_inner(&state, actor, id, &form).await;
    match result {
        Err(CustomError::ValidationError(error)) => {
            let feedback = AttachmentFeedback {
                account_id: id,
                surface_id: form.surface_id,
                error,
            };
            let mut response = crate::management_hub::render_connections(
                &state,
                session,
                false,
                None,
                None,
                Some(&feedback),
            )
            .await?;
            *response.status_mut() = axum::http::StatusCode::BAD_REQUEST;
            Ok(response)
        }
        Err(error) => Err(error),
        Ok(()) => Ok(synced(&state).await),
    }
}

async fn attach_inner(
    state: &AppState,
    actor: UserId,
    id: ConnectionId,
    form: &Repository,
) -> Result<(), CustomError> {
    require_confirmation(form.confirmed)?;
    if !connections::list(state.sqlx_pool(), actor)
        .await?
        .iter()
        .any(|account| account.id == id && !account.revoked)
    {
        return Err(CustomError::ValidationError("Selected account is unavailable or no longer yours to attach. Choose a current account.".into()));
    }
    if !den_service::work_surfaces::list_surfaces_managed_by(state.sqlx_pool(), actor.get())
        .await?
        .iter()
        .any(|surface| surface.id == form.surface_id)
    {
        return Err(CustomError::ValidationError("Selected repository is unavailable or no longer yours to manage. Choose a currently managed repository.".into()));
    }
    match connections::attach(state.sqlx_pool(), actor, id, form.surface_id).await {
        Ok(()) => Ok(()),
        Err(den_core::DenError::NotFound(_)) => Err(CustomError::ValidationError(
            "Account or repository access changed. Review current choices and confirm again."
                .into(),
        )),
        Err(error) => Err(error.into()),
    }
}
async fn detach(
    State(state): State<AppState>,
    session: AuthSession,
    Path(id): Path<Uuid>,
    Form(form): Form<Confirmation>,
) -> Result<Response, CustomError> {
    require_confirmation(form.confirmed)?;
    connections::detach(state.sqlx_pool(), owner(&state, &session).await?, id).await?;
    Ok(synced(&state).await)
}
