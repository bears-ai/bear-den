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
use serde::Deserialize;
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
}
#[derive(Deserialize)]
struct Revision {
    revision: i64,
}
#[derive(Deserialize)]
struct Repository {
    surface_id: Uuid,
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
    let actor = owner(&state, &session).await?;
    if form.secret.len() > 128_000 {
        return Err(CustomError::ValidationError("secret too large".into()));
    }
    let material = match form.provider {
        Provider::GitHttps => Material::HttpsToken(form.secret),
        Provider::GitSsh => Material::SshKey(form.secret),
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
    connections::attach(
        state.sqlx_pool(),
        owner(&state, &session).await?,
        id,
        form.surface_id,
    )
    .await?;
    Ok(synced(&state).await)
}
async fn detach(
    State(state): State<AppState>,
    session: AuthSession,
    Path(id): Path<Uuid>,
) -> Result<Response, CustomError> {
    connections::detach(state.sqlx_pool(), owner(&state, &session).await?, id).await?;
    Ok(synced(&state).await)
}
