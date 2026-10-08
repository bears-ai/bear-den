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
struct ConnectionCatalogView {
    #[serde(flatten)]
    account: den_service::connections::Connection,
    repositories: Vec<RepositoryLabel>,
    other_repository_count: i64,
    github_app_write_enabled: Option<bool>,
}

#[derive(Serialize)]
struct RepositoryLabel {
    id: Uuid,
    name: String,
}

#[derive(Serialize)]
struct RepositoryConnection {
    id: Uuid,
    name: String,
    credential_configured: bool,
    github_app_configured: bool,
    linked_account: Option<crate::work::connection_view::LinkedAccount>,
    can_link: bool,
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

#[derive(Default, Deserialize)]
struct ConnectionQuery {
    sync: Option<String>,
}

async fn connections(
    State(state): State<AppState>,
    Query(query): Query<ConnectionQuery>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    render_connections(
        &state,
        auth_session,
        query.sync.as_deref() == Some("pending"),
        None,
        None,
        None,
    )
    .await
}

pub(crate) async fn render_connections(
    state: &AppState,
    auth_session: AuthSession,
    sync_pending: bool,
    draft: Option<&crate::connections::ConnectionDraft>,
    error: Option<&str>,
    attachment_feedback: Option<&crate::connections::AttachmentFeedback>,
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
    let connection_catalog =
        den_service::connections::list(state.sqlx_pool(), den_core::ids::UserId::new(user.id))
            .await?;
    let app_permissions = sqlx::query!(
        "SELECT id, github_app_write_enabled FROM provider_connections WHERE owner_user_id = $1 AND provider = 'github_app'",
        user.id,
    ).fetch_all(state.sqlx_pool()).await.map_err(den_core::DenError::from)?;
    let linkable = work_surfaces::list_surfaces_managed_by(state.sqlx_pool(), user.id).await?;
    let mut repositories = if user.is_admin {
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
        linked_account: None,
        can_link: linkable.iter().any(|managed| managed.id == surface.id),
    })
    .collect::<Vec<_>>();
    let linked = crate::work::connection_view::linked_accounts(
        state.sqlx_pool(),
        den_core::ids::UserId::new(user.id),
        &repositories.iter().map(|row| row.id).collect::<Vec<_>>(),
    )
    .await?;
    for account in linked {
        if let Some(row) = repositories
            .iter_mut()
            .find(|row| row.id == account.surface_id)
        {
            row.linked_account = Some(account);
        }
    }
    let connection_catalog: Vec<_> = connection_catalog
        .into_iter()
        .map(|account| {
            let visible: Vec<_> = repositories
                .iter()
                .filter(|row| {
                    row.linked_account
                        .as_ref()
                        .is_some_and(|linked| linked.id == account.id)
                })
                .map(|row| RepositoryLabel {
                    id: row.id,
                    name: row.name.clone(),
                })
                .collect();
            ConnectionCatalogView {
                other_repository_count: account
                    .repository_count
                    .saturating_sub(visible.len() as i64)
                    .max(0),
                github_app_write_enabled: app_permissions
                    .iter()
                    .find(|row| row.id == account.id.0)
                    .map(|row| row.github_app_write_enabled),
                account,
                repositories: visible,
            }
        })
        .collect();
    let attachment_account_available = attachment_feedback.is_some_and(|feedback| {
        connection_catalog.iter().any(|connection| {
            connection.account.id == feedback.account_id && !connection.account.revoked
        })
    });
    let attachment_repository_available = attachment_feedback.is_some_and(|feedback| {
        repositories
            .iter()
            .any(|repository| repository.id == feedback.surface_id && repository.can_link)
    });
    let has_linkable_repositories = repositories.iter().any(|repository| repository.can_link);
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
        state,
        "connections.html",
        auth_session,
        context! { repositories, bears, connection_catalog, sync_pending, draft, error,
            attachment_feedback, attachment_account_available, attachment_repository_available, has_linkable_repositories },
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
    let cabinet_reviews = den_service::cabinet::pages::pending_reviews(
        state.sqlx_pool(),
        &den_cabinet::ActorScope::user(den_core::ids::UserId::new(user.id)),
    )
    .await
    .map_err(|error| CustomError::from(den_core::DenError::from(error)))?;
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
        context! { bears, cabinet_reviews, filtered_bear => query.bear },
    )
    .await
}
