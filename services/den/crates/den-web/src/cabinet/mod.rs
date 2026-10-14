// ROUTES: When modifying routes in this file, update /src/ROUTES.md.
//! Cabinet's session-authenticated, ACL-filtered page UI.
//!
//! Content edits publish immutable versions through the shared service facade;
//! discussions are separate append-only records and never publish a revision.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::extract::Form;
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};
use den_cabinet::{
    Actor, ActorScope, CabinetError, CabinetItemRef, CabinetSourceRef, CabinetVersionRef,
    CreateItemRequest, HistoryRequest, ItemKind, Lifecycle, LinkSourceRequest, NewSourceLink,
    ReadRequest, ReviewState, SearchFilters, SearchRequest, SourceKind, SourceRole,
    UnlinkSourceRequest, UpdateItemRequest,
};
use den_core::ids::UserId;
use den_service::cabinet as cabinet_service;
mod attachments;
mod audience;
pub(crate) mod cleanup;
mod discussion;
mod pages;
mod previews;
mod saved_copies;
mod uploads;

#[cfg(test)]
mod tests;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(pages::router())
        .merge(attachments::router())
        .merge(cleanup::router())
        .merge(discussion::router())
        .merge(saved_copies::router())
        .merge(previews::router())
        .merge(uploads::router())
        .route("/cabinet", get(index))
        .route("/cabinet/new", get(new_form).post(create))
        .route("/cabinet/{cabinet_ref}", get(item))
        .route("/cabinet/{cabinet_ref}/edit", get(edit_form).post(update))
        .route("/cabinet/{cabinet_ref}/history", get(history))
        .route(
            "/cabinet/{cabinet_ref}/archive",
            axum::routing::post(archive),
        )
        .route(
            "/cabinet/{cabinet_ref}/restore",
            axum::routing::post(restore),
        )
        .route("/cabinet/{cabinet_ref}/delete", axum::routing::post(delete))
        .route(
            "/cabinet/{cabinet_ref}/sources",
            axum::routing::post(add_source),
        )
        .route(
            "/cabinet/{cabinet_ref}/sources/{source_ref}/remove",
            axum::routing::post(remove_source),
        )
}

fn require_user_scope(auth_session: &AuthSession) -> Result<ActorScope, CustomError> {
    auth_session
        .user
        .as_ref()
        .map(|user| ActorScope::user(UserId(user.id)))
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))
}

fn cabinet_error(error: CabinetError) -> CustomError {
    CustomError::from(den_core::DenError::from(error))
}

fn parse_item_ref(value: &str) -> Result<CabinetItemRef, CustomError> {
    CabinetItemRef::parse(value).map_err(|_| CustomError::NotFound("no such item".to_string()))
}

fn item_url(cabinet_ref: &CabinetItemRef) -> String {
    format!("/cabinet/{}", cabinet_ref.as_str())
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum IndexView {
    #[default]
    Browse,
    Search,
    Pages,
    Reviews,
}

#[derive(Debug, Default, Deserialize)]
struct IndexQuery {
    #[serde(default)]
    view: IndexView,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    lifecycle: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

async fn index(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(query): Query<IndexQuery>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let archived = query.lifecycle.as_deref() == Some("archived");
    let review_items = cabinet_service::pages::pending_reviews(state.sqlx_pool(), &scope)
        .await
        .map_err(cabinet_error)?;
    let roots_only =
        query.view == IndexView::Browse && query.q.as_deref().is_none_or(|q| q.trim().is_empty());
    let request = SearchRequest {
        scope: scope.clone(),
        query: query.q.clone().unwrap_or_default(),
        filters: SearchFilters {
            lifecycle: Some(if archived {
                Lifecycle::Archived
            } else {
                Lifecycle::Active
            }),
            ..SearchFilters::default()
        },
    };
    let items = if query.view == IndexView::Reviews {
        Ok(Vec::new())
    } else if roots_only {
        cabinet_service::search_roots(state.sqlx_pool(), request).await
    } else {
        cabinet_service::search(state.sqlx_pool(), request).await
    }
    .map_err(cabinet_error)?;
    let items: Vec<serde_json::Value> = items
        .into_iter()
        .map(|item| {
            serde_json::json!({
                "cabinet_ref": item.cabinet_ref.as_str(),
                "title": item.title,
                "updated_at": item.updated_at,
            })
        })
        .collect();
    web::render_template(
        &state,
        "cabinet/index.html",
        auth_session,
        context! {
            title => "Cabinet",
            items => items,
            review_items,
            roots_only,
            index_view => query.view,
            q => query.q,
            archived => archived,
            message => query.message,
        },
    )
    .await
}

#[derive(Default, Deserialize)]
struct NewQuery {
    parent: Option<String>,
}

async fn new_form(
    Query(query): Query<NewQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let audience = if let Some(parent) = query.parent.as_deref() {
        audience::for_page(state.sqlx_pool(), &scope, &parse_item_ref(parent)?).await?
    } else {
        audience::Audience::OpenWiki
    };
    if let Some(parent) = query.parent.as_deref() {
        if !cabinet_service::pages::metadata(state.sqlx_pool(), &scope, &parse_item_ref(parent)?)
            .await
            .map_err(cabinet_error)?
            .can_write
        {
            return Err(CustomError::Authorization(
                "destination is read only".into(),
            ));
        }
    }
    web::render_template(
        &state,
        "cabinet/new.html",
        auth_session,
        context! { title => "New Cabinet page", parent => query.parent, audience },
    )
    .await
}

#[derive(Debug, Deserialize)]
struct CreateForm {
    title: String,
    content: String,
    #[serde(default)]
    parent: String,
}

async fn create(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<CreateForm>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let parent = if form.parent.is_empty() {
        None
    } else {
        Some(parse_item_ref(&form.parent)?)
    };
    let request = CreateItemRequest {
        scope,
        kind: ItemKind::Document,
        title: form.title,
        content: form.content,
        collection_ref: None,
        mission_ref: None,
        source_links: Vec::new(),
    };
    let view = if let Some(parent) = parent {
        cabinet_service::create_child(state.sqlx_pool(), request, &parent).await
    } else {
        cabinet_service::create_item(state.sqlx_pool(), request).await
    }
    .map_err(cabinet_error)?;
    Ok(Redirect::to(&item_url(&view.item.cabinet_ref)).into_response())
}

#[derive(Debug, Default, Deserialize)]
struct ItemQuery {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

async fn item(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
    Query(query): Query<ItemQuery>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    let version_ref = query
        .version
        .as_deref()
        .map(CabinetVersionRef::parse)
        .transpose()
        .map_err(|_| CustomError::NotFound("no such version".to_string()))?;
    let view = cabinet_service::read(
        state.sqlx_pool(),
        ReadRequest {
            scope: scope.clone(),
            cabinet_ref: cabinet_ref.clone(),
            version_ref: version_ref.clone(),
        },
    )
    .await
    .map_err(cabinet_error)?;
    let page = cabinet_service::pages::metadata(state.sqlx_pool(), &scope, &cabinet_ref)
        .await
        .map_err(cabinet_error)?;
    let audience = audience::for_page(state.sqlx_pool(), &scope, &cabinet_ref).await?;
    let attachments = cabinet_service::attachments::list(state.sqlx_pool(), &scope, &cabinet_ref)
        .await
        .map_err(cabinet_error)?;
    let upload_bears = if page.can_write && state.media.is_some() {
        let user = auth_session
            .user
            .as_ref()
            .ok_or_else(|| CustomError::Authentication("login required".into()))?;
        den_service::bears::db::list_bears_for_user(state.sqlx_pool(), user.id).await?
    } else {
        Vec::new()
    };
    let children = cabinet_service::pages::children(state.sqlx_pool(), &scope, &cabinet_ref)
        .await
        .map_err(cabinet_error)?;
    let (people_names, bear_names, reviewer_names) =
        cabinet_service::pages::member_names(state.sqlx_pool(), &page)
            .await
            .map_err(cabinet_error)?;
    let destinations = cabinet_service::search(
        state.sqlx_pool(),
        SearchRequest {
            scope: scope.clone(),
            query: String::new(),
            filters: SearchFilters::default(),
        },
    )
    .await
    .map_err(cabinet_error)?;
    let is_current = view.item.current_version.as_ref() == Some(view.version.version_ref());
    let sources: Vec<serde_json::Value> = view
        .sources
        .iter()
        .map(|source| {
            serde_json::json!({
                "ref": source.source_ref.as_str(),
                "kind": source.source_kind,
                "locator": source.locator,
                "role": source.role,
            })
        })
        .collect();
    let mut response = web::render_template(
        &state,
        "cabinet/item.html",
        auth_session,
        context! {
            title => view.item.title,
            cabinet_ref => cabinet_ref.as_str(),
            item_title => view.item.title,
            content => view.version.content(),
            revision => view.version.revision(),
            version_ref => view.version.version_ref().as_str(),
            is_current => is_current,
            lifecycle => view.item.lifecycle,
            authored_by => view.version.authored_by(),
            author_label => actor_label(view.version.authored_by(), &scope),
            authored_at => view.version.authored_at(),
            sources => sources,
            page,
            audience,
            children,
            attachments,
            byte_storage_enabled => state.media.is_some(),
            upload_bears,
            people_names,
            bear_names,
            reviewer_names,
            destinations,
            review => view.version.review(),
            proposed_title => view.proposed_title,
            has_published => view.item.current_version.is_some(),
            message => query.message,
        },
    )
    .await?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Debug, Default, Deserialize)]
struct EditQuery {
    #[serde(default)]
    error: Option<String>,
}

async fn edit_form(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
    Query(query): Query<EditQuery>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    let view = cabinet_service::read(
        state.sqlx_pool(),
        ReadRequest {
            scope,
            cabinet_ref,
            version_ref: None,
        },
    )
    .await
    .map_err(cabinet_error)?;
    render_edit_form(
        &state,
        auth_session,
        &view.item.cabinet_ref,
        &view.item.title,
        view.version.content(),
        view.version.version_ref().as_str(),
        query.error.as_deref(),
        StatusCode::OK,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn render_edit_form(
    state: &AppState,
    auth_session: AuthSession,
    cabinet_ref: &CabinetItemRef,
    title: &str,
    content: &str,
    base_version: &str,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let page = cabinet_service::pages::metadata(state.sqlx_pool(), &scope, cabinet_ref)
        .await
        .map_err(cabinet_error)?;
    let current = cabinet_service::read(
        state.sqlx_pool(),
        ReadRequest {
            scope: scope.clone(),
            cabinet_ref: cabinet_ref.clone(),
            version_ref: None,
        },
    )
    .await
    .map_err(cabinet_error)?;
    let audience = audience::for_page(state.sqlx_pool(), &scope, cabinet_ref).await?;
    let mut response = web::render_template(
        state,
        "cabinet/edit.html",
        auth_session,
        context! {
            title => format!("Edit: {title}"),
            cabinet_ref => cabinet_ref.as_str(),
            item_title => title,
            content => content,
            base_version => base_version,
            audience,
            can_publish => page.can_write && current.item.lifecycle == Lifecycle::Active,
            archived => current.item.lifecycle == Lifecycle::Archived,
            error => error,
        },
    )
    .await?;
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct EditForm {
    title: String,
    content: String,
    base_version: String,
}

async fn update(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
    Form(form): Form<EditForm>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    let base_version = match CabinetVersionRef::parse(&form.base_version) {
        Ok(version) => version,
        Err(_) => {
            return render_edit_form(
                &state, auth_session, &cabinet_ref, &form.title, &form.content,
                &form.base_version,
                Some("The base revision is invalid. Your draft is preserved; reopen the current page before publishing."),
                StatusCode::UNPROCESSABLE_ENTITY,
            ).await;
        }
    };
    let result = cabinet_service::update_item(
        state.sqlx_pool(),
        UpdateItemRequest {
            scope,
            cabinet_ref: cabinet_ref.clone(),
            content: form.content.clone(),
            base_version,
            title: Some(form.title.clone()),
        },
    )
    .await;
    match result {
        Ok(view) => Ok(Redirect::to(&format!(
            "{}?message={}",
            item_url(&view.item.cabinet_ref),
            urlencoding::encode("Published a new revision.")
        ))
        .into_response()),
        Err(CabinetError::Conflict { current_version }) => {
            // Someone published first: hand the editor back their draft with
            // the new base so they can reconcile explicitly.
            render_edit_form(
                &state,
                auth_session,
                &cabinet_ref,
                &form.title,
                &form.content,
                current_version.as_str(),
                Some(
                    "Someone else published a newer revision while you were editing. \
                     Your draft is preserved below; review the latest revision before saving.",
                ),
                StatusCode::CONFLICT,
            )
            .await
        }
        Err(CabinetError::Validation(_) | CabinetError::Policy(_)) => {
            render_edit_form(
                &state, auth_session, &cabinet_ref, &form.title, &form.content,
                &form.base_version,
                Some("The revision was not published. Check the title and content, and restore an archived page before publishing. Your draft is preserved."),
                StatusCode::UNPROCESSABLE_ENTITY,
            ).await
        }
        Err(CabinetError::NotAuthorized) => {
            render_edit_form(
                &state, auth_session, &cabinet_ref, &form.title, &form.content,
                &form.base_version,
                Some("Your write access changed. The revision was not published and your draft is preserved; ask a page owner for access."),
                StatusCode::FORBIDDEN,
            ).await
        }
        Err(error) => Err(cabinet_error(error)),
    }
}

fn actor_label(author: &ActorScope, viewer: &ActorScope) -> String {
    match &author.actor {
        Actor::User { .. } if author.actor == viewer.actor => "You".into(),
        Actor::User { user_id } => format!("Person (user {user_id})"),
        Actor::Bear { bear_id, stance } => format!("Bear {bear_id} ({stance})"),
    }
}

fn published(review: ReviewState) -> bool {
    matches!(review, ReviewState::None | ReviewState::Approved)
}

#[derive(Default, Deserialize)]
struct HistoryQuery {
    before: Option<CabinetVersionRef>,
}

#[derive(Serialize)]
struct VersionComparison {
    before_ref: CabinetVersionRef,
    before_revision: u32,
    before_content: String,
    current_ref: CabinetVersionRef,
    current_revision: u32,
    current_content: String,
    unchanged: bool,
}

async fn history(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    let item_view = cabinet_service::read(
        state.sqlx_pool(),
        ReadRequest {
            scope: scope.clone(),
            cabinet_ref: cabinet_ref.clone(),
            version_ref: None,
        },
    )
    .await
    .map_err(cabinet_error)?;
    let page = cabinet_service::pages::metadata(state.sqlx_pool(), &scope, &cabinet_ref)
        .await
        .map_err(cabinet_error)?;
    let versions = cabinet_service::history(
        state.sqlx_pool(),
        HistoryRequest {
            scope: scope.clone(),
            cabinet_ref: cabinet_ref.clone(),
        },
    )
    .await
    .map_err(cabinet_error)?;
    // History metadata must not advertise private review versions to ordinary readers.
    let versions: Vec<_> = versions
        .into_iter()
        .filter(|version| {
            published(version.review) || page.can_review || version.authored_by.actor == scope.actor
        })
        .collect();
    let mut comparison = None;
    let mut comparison_notice = None;
    if let Some(before_ref) = query.before {
        let before_summary = versions
            .iter()
            .find(|version| version.version_ref == before_ref)
            .ok_or_else(|| CustomError::NotFound("no such version".into()))?;
        if !published(before_summary.review) || !published(item_view.version.review()) {
            comparison_notice = Some("Only published revisions can be compared here. Open a pending or rejected version explicitly to review it; it is not included in this comparison.");
        } else {
            let before = cabinet_service::read(
                state.sqlx_pool(),
                ReadRequest {
                    scope: scope.clone(),
                    cabinet_ref: cabinet_ref.clone(),
                    version_ref: Some(before_ref),
                },
            )
            .await
            .map_err(cabinet_error)?;
            // Re-check the actual read, not just the earlier history metadata.
            if published(before.version.review()) {
                comparison = Some(VersionComparison {
                    before_ref: before.version.version_ref().clone(),
                    before_revision: before.version.revision(),
                    before_content: before.version.content().to_owned(),
                    current_ref: item_view.version.version_ref().clone(),
                    current_revision: item_view.version.revision(),
                    current_content: item_view.version.content().to_owned(),
                    unchanged: before.version.content() == item_view.version.content(),
                });
            }
        }
    }
    let current_ref = item_view.item.current_version.as_ref();
    let versions: Vec<serde_json::Value> = versions.into_iter().map(|version| {
        serde_json::json!({
            "version_ref": version.version_ref,
            "revision": version.revision,
            "author_label": actor_label(&version.authored_by, &scope),
            "authored_at": version.authored_at.format(&time::format_description::well_known::Rfc3339)
                            .unwrap_or_else(|_| version.authored_at.to_string()),
            "review": version.review,
            "is_current": current_ref == Some(&version.version_ref),
            "can_compare": published(version.review) && current_ref.is_some()
                && current_ref != Some(&version.version_ref),
        })
    }).collect();
    let mut response = web::render_template(
        &state,
        "cabinet/history.html",
        auth_session,
        context! {
            title => format!("History: {}", item_view.item.title),
            cabinet_ref => cabinet_ref.as_str(),
            item_title => item_view.item.title,
            versions => versions,
            comparison,
            comparison_notice,
        },
    )
    .await?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn archive(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    cabinet_service::archive_item(state.sqlx_pool(), &scope, &cabinet_ref)
        .await
        .map_err(cabinet_error)?;
    Ok(Redirect::to(&format!(
        "/cabinet?message={}",
        urlencoding::encode("Item archived.")
    ))
    .into_response())
}

async fn restore(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    cabinet_service::restore_item(state.sqlx_pool(), &scope, &cabinet_ref)
        .await
        .map_err(cabinet_error)?;
    Ok(Redirect::to(&format!(
        "{}?message={}",
        item_url(&cabinet_ref),
        urlencoding::encode("Item restored.")
    ))
    .into_response())
}

/// Tombstone an item. Deletion is a person-only operation (see
/// `den_service::cabinet::delete_item`); Bears may only archive.
async fn delete(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    cabinet_service::delete_item(state.sqlx_pool(), &scope, &cabinet_ref)
        .await
        .map_err(cabinet_error)?;
    Ok(Redirect::to(&format!(
        "/cabinet?message={}",
        urlencoding::encode("Item deleted. Its revisions are retained for existing citations.")
    ))
    .into_response())
}

#[derive(Debug, Deserialize)]
struct AddSourceForm {
    source_kind: SourceKind,
    locator: String,
    role: SourceRole,
}

async fn add_source(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
    Form(form): Form<AddSourceForm>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    let url = item_url(&cabinet_ref);
    cabinet_service::link_source(
        state.sqlx_pool(),
        LinkSourceRequest {
            scope,
            cabinet_ref,
            link: NewSourceLink {
                source_kind: form.source_kind,
                locator: form.locator.trim().to_string(),
                role: form.role,
            },
        },
    )
    .await
    .map_err(cabinet_error)?;
    Ok(Redirect::to(&format!(
        "{url}?message={}",
        urlencoding::encode("Source linked.")
    ))
    .into_response())
}

async fn remove_source(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((cabinet_ref, source_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let cabinet_ref = parse_item_ref(&cabinet_ref)?;
    let url = item_url(&cabinet_ref);
    let source_ref = CabinetSourceRef::parse(&source_ref)
        .map_err(|error| CustomError::ValidationError(error.to_string()))?;
    cabinet_service::unlink_source(
        state.sqlx_pool(),
        UnlinkSourceRequest {
            scope,
            cabinet_ref,
            source_ref,
        },
    )
    .await
    .map_err(cabinet_error)?;
    Ok(Redirect::to(&format!(
        "{url}?message={}",
        urlencoding::encode("Source link removed.")
    ))
    .into_response())
}
