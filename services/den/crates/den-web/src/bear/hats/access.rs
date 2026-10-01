//! Bear-admin editing of Den-owned web tool and HTTPS-host policy for one hat.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::{
    ids::{BearId, HatId, UserId},
    tools::constants::{DEN_WEB_FETCH, DEN_WEB_SEARCH},
};
use den_service::bears::hats::access::{
    self as hat_access, HatAccessGrant, HttpsHost, ReadOnlyWorkspaceAction, ToolActionKey,
    WorkspaceRoot,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    super::settings::{load_session_bear_manage, session_user},
    hat_url,
};
use crate::{auth_backend::AuthSession, errors::CustomError, web::AppState};

pub(super) fn router() -> Router<AppState> {
    Router::new().route_with_tsr("/bear/{slug}/hats/{hat_id}/access", post(update))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    EnableFetch,
    EnableSearch,
    AllowHost,
    GrantWorkspaceRead,
    RevokeWorkspace,
    Revoke,
}

#[derive(Debug, Deserialize)]
struct AccessForm {
    action: Action,
    host: Option<String>,
    workspace_root: Option<String>,
    tool_name: Option<String>,
    grant_id: Option<Uuid>,
    #[serde(default)]
    confirm_future_job_audience: bool,
}

async fn update(
    Path((slug, id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<AccessForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let bear_id = BearId::new(bear.id);
    let hat_id = HatId::new(id);
    let actor = UserId::new(session_user(&auth).await?.id);
    match form.action {
        Action::EnableFetch => {
            let tool =
                HatAccessGrant::ToolForHat(ToolActionKey::from_provider_name(DEN_WEB_FETCH)?);
            hat_access::grant(
                state.sqlx_pool(),
                bear_id,
                hat_id,
                actor,
                &tool,
                form.confirm_future_job_audience,
            )
            .await?;
        }
        Action::EnableSearch => {
            let tool =
                HatAccessGrant::ToolForHat(ToolActionKey::from_provider_name(DEN_WEB_SEARCH)?);
            hat_access::grant(
                state.sqlx_pool(),
                bear_id,
                hat_id,
                actor,
                &tool,
                form.confirm_future_job_audience,
            )
            .await?;
        }
        Action::AllowHost => {
            let host = form.host.as_deref().ok_or_else(|| {
                CustomError::ValidationError("exact HTTPS hostname is required".into())
            })?;
            let grant = HatAccessGrant::HttpsHost(HttpsHost::parse(host)?);
            hat_access::grant(
                state.sqlx_pool(),
                bear_id,
                hat_id,
                actor,
                &grant,
                form.confirm_future_job_audience,
            )
            .await?;
        }
        Action::GrantWorkspaceRead => {
            let raw_root = form.workspace_root.as_deref().ok_or_else(|| {
                CustomError::ValidationError("absolute workspace root is required".into())
            })?;
            let raw_tool = form.tool_name.as_deref().ok_or_else(|| {
                CustomError::ValidationError("read-only filesystem action is required".into())
            })?;
            let grant = HatAccessGrant::ReadOnlyToolInWorkspace(
                ReadOnlyWorkspaceAction::from_provider_name(raw_tool)?,
                WorkspaceRoot::parse(raw_root)?,
            );
            hat_access::grant(
                state.sqlx_pool(),
                bear_id,
                hat_id,
                actor,
                &grant,
                form.confirm_future_job_audience,
            )
            .await?;
        }
        Action::RevokeWorkspace => {
            let id = form.grant_id.ok_or_else(|| {
                CustomError::ValidationError("workspace grant ID is required".into())
            })?;
            if !hat_access::workspace_read_grants_for_hat(state.sqlx_pool(), bear_id, hat_id)
                .await?
                .iter()
                .any(|grant| grant.id == id)
            {
                return Err(CustomError::ValidationError(
                    "this is not a current workspace read grant for this hat".into(),
                ));
            }
            hat_access::revoke(state.sqlx_pool(), bear_id, hat_id, actor, id).await?;
        }
        Action::Revoke => {
            let id = form
                .grant_id
                .ok_or_else(|| CustomError::ValidationError("hat grant ID is required".into()))?;
            let current =
                hat_access::web_grants_for_hat(state.sqlx_pool(), bear_id, hat_id).await?;
            if current.fetch_tool_grant_id != Some(id)
                && current.search_tool_grant_id != Some(id)
                && !current.hosts.iter().any(|host| host.id == id)
            {
                return Err(CustomError::ValidationError(
                    "this is not a current web grant for this hat".into(),
                ));
            }
            hat_access::revoke(state.sqlx_pool(), bear_id, hat_id, actor, id).await?;
        }
    }
    Ok(Redirect::to(&hat_url(&bear.slug, hat_id)).into_response())
}
