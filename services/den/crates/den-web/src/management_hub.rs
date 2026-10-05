//! Shared Den destinations; all summaries are projections of authorized records.

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{auth_backend::AuthSession, errors::CustomError, web, AppState};
use den_service::{bears::db as bears_db, memory_proposals, work_surfaces};

#[derive(Default, Deserialize)]
struct HubQuery {
    bear: Option<String>,
}

#[derive(Serialize)]
struct BearLink {
    name: String,
    slug: String,
    can_manage: bool,
}

#[derive(Serialize)]
struct ReviewBear {
    name: String,
    slug: String,
    pending: Option<i64>,
}

#[derive(Serialize)]
struct RepositoryConnection {
    id: Uuid,
    name: String,
    credential_configured: bool,
    github_app_configured: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/connections", get(connections))
        .route("/reviews", get(reviews))
}

pub(crate) async fn pending_memory_reviews(
    state: &AppState,
    bear_id: Uuid,
) -> Result<i64, CustomError> {
    let store = state.memory_stores.store_for_bear(bear_id).await?;
    let mut total = 0;
    for status in ["pending", "needs_human_review"] {
        total +=
            memory_proposals::count_for_bear_status(state.sqlx_pool(), bear_id, status).await?;
        total += den_memory::count_memory_proposals(&store, Some(status)).await?;
    }
    Ok(total)
}

async fn connections(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".into()))?;
    if let Some(redirect) =
        crate::bear::member::email_verify_redirect(state.sqlx_pool(), user.id).await?
    {
        return Ok(redirect.into_response());
    }
    let repositories = if user.is_admin {
        work_surfaces::list_all_surfaces(state.sqlx_pool()).await?
    } else {
        work_surfaces::list_surfaces_managed_by(state.sqlx_pool(), user.id).await?
    }
    .into_iter()
    .map(|surface| RepositoryConnection {
        id: surface.id,
        name: surface.name,
        credential_configured: surface.credential_kind.is_some(),
        github_app_configured: surface.github_app_installation_id.is_some(),
    })
    .collect::<Vec<_>>();
    let bears = bears_db::list_bears_for_user(state.sqlx_pool(), user.id)
        .await?
        .into_iter()
        .map(|row| BearLink {
            name: row.bear.name,
            slug: row.bear.slug,
            can_manage: bears_db::role_is_bear_admin(row.membership_role.as_deref()),
        })
        .collect::<Vec<_>>();
    web::render_template(
        &state,
        "connections.html",
        auth_session,
        context! { repositories, bears },
    )
    .await
}

async fn reviews(
    State(state): State<AppState>,
    Query(query): Query<HubQuery>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".into()))?;
    if let Some(redirect) =
        crate::bear::member::email_verify_redirect(state.sqlx_pool(), user.id).await?
    {
        return Ok(redirect.into_response());
    }
    let memberships = bears_db::list_bears_for_user(state.sqlx_pool(), user.id).await?;
    if query
        .bear
        .as_ref()
        .is_some_and(|slug| !memberships.iter().any(|row| row.bear.slug == *slug))
    {
        return Err(CustomError::NotFound(
            "Bear not found or you do not have access.".into(),
        ));
    }
    let mut bears = Vec::new();
    for membership in memberships {
        if !bears_db::role_is_bear_admin(membership.membership_role.as_deref())
            || query
                .bear
                .as_ref()
                .is_some_and(|slug| membership.bear.slug != *slug)
        {
            continue;
        }
        let bear = membership.bear;
        let pending = match pending_memory_reviews(&state, bear.id).await {
            Ok(count) => Some(count),
            Err(error) => {
                tracing::warn!(bear_id = %bear.id, %error, "review summary unavailable");
                None
            }
        };
        bears.push(ReviewBear {
            name: bear.name,
            slug: bear.slug,
            pending,
        });
    }
    web::render_template(
        &state,
        "reviews.html",
        auth_session,
        context! { bears, filtered_bear => query.bear },
    )
    .await
}
