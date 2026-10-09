//! Exact repository-head permissions, never credential ownership or backend setup.

use super::{
    super::settings::{load_session_bear_manage, session_user},
    hat_url,
};
use crate::{auth_backend::AuthSession, errors::CustomError, AppState};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use axum_extra::extract::Form;
use den_core::{
    ids::{BearId, HatId, UserId},
    tools::repository::RepositorySurfaceId,
};
use den_service::{bears::hats::access, repository::grants};
use serde::Deserialize;
use uuid::Uuid;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/bear/{slug}/hats/{hat_id}/repository-access", post(update))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Grant,
    Revoke,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryAccessForm {
    action: Action,
    surface_id: Option<Uuid>,
    expected_target: Option<String>,
    grant_id: Option<Uuid>,
    #[serde(default)]
    confirm_future_job_audience: bool,
}

async fn update(
    Path((slug, id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<RepositoryAccessForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let bear_id = BearId::new(bear.id);
    let hat = HatId::new(id);
    let actor = UserId::new(session_user(&auth).await?.id);
    match form.action {
        Action::Grant => {
            let surface = form.surface_id.ok_or_else(|| {
                CustomError::ValidationError("repository selection required".into())
            })?;
            let expected = form.expected_target.as_deref().ok_or_else(|| {
                CustomError::ValidationError("review the current repository target".into())
            })?;
            grants::grant(
                state.sqlx_pool(),
                bear_id,
                hat,
                actor,
                RepositorySurfaceId(surface),
                expected,
                form.confirm_future_job_audience,
            )
            .await?;
        }
        Action::Revoke => {
            let grant_id = form
                .grant_id
                .ok_or_else(|| CustomError::ValidationError("repository grant required".into()))?;
            if !grants::list(state.sqlx_pool(), bear_id, hat)
                .await?
                .iter()
                .any(|grant| grant.id == grant_id)
            {
                return Err(CustomError::Authorization(
                    "current repository grant for this hat required".into(),
                ));
            }
            access::revoke(state.sqlx_pool(), bear_id, hat, actor, grant_id).await?;
        }
    }
    Ok(Redirect::to(&hat_url(&bear.slug, hat)).into_response())
}
