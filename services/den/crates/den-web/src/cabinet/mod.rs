// ROUTES: When modifying routes in this file, update /src/ROUTES.md.
//! Cabinet wiki UI: Den's shared, human-editable knowledge layer (Phase 1).
//!
//! Every logged-in user can browse, create, and edit items; each edit
//! publishes an immutable version through the same `den_service::cabinet`
//! facade the model tools use. Stale-base edits re-render the edit form with
//! the conflict, never merging silently.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::extract::Form;
use minijinja::context;
use serde::Deserialize;

use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};
use den_cabinet::{
    ActorScope, CabinetError, CabinetItemRef, CabinetSourceRef, CabinetVersionRef,
    CreateItemRequest, HistoryRequest, ItemKind, Lifecycle, LinkSourceRequest, NewSourceLink,
    ReadRequest, SearchFilters, SearchRequest, SourceKind, SourceRole, UnlinkSourceRequest,
    UpdateItemRequest,
};
use den_core::ids::UserId;
use den_service::cabinet as cabinet_service;
mod attachments;
mod audience;
pub(crate) mod cleanup;
mod pages;
mod previews;
mod saved_copies;
mod uploads;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(pages::router())
        .merge(attachments::router())
        .merge(cleanup::router())
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

#[derive(Debug, Default, Deserialize)]
struct IndexQuery {
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
    let roots_only = query.q.as_deref().is_none_or(|q| q.trim().is_empty());
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
    let items = if roots_only {
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
    web::render_template(
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
    .await
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
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&auth_session)?;
    let audience = audience::for_page(state.sqlx_pool(), &scope, cabinet_ref).await?;
    web::render_template(
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
            error => error,
        },
    )
    .await
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
    let base_version = CabinetVersionRef::parse(&form.base_version)
        .map_err(|error| CustomError::ValidationError(error.to_string()))?;
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
            )
            .await
        }
        Err(error) => Err(cabinet_error(error)),
    }
}

async fn history(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(cabinet_ref): Path<String>,
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
    let versions = cabinet_service::history(
        state.sqlx_pool(),
        HistoryRequest {
            scope,
            cabinet_ref: cabinet_ref.clone(),
        },
    )
    .await
    .map_err(cabinet_error)?;
    let versions: Vec<serde_json::Value> = versions
        .into_iter()
        .map(|version| {
            serde_json::json!({
                "version_ref": version.version_ref.as_str(),
                "revision": version.revision,
                "authored_by": version.authored_by,
                "authored_at": version.authored_at,
            })
        })
        .collect();
    web::render_template(
        &state,
        "cabinet/history.html",
        auth_session,
        context! {
            title => format!("History: {}", item_view.item.title),
            cabinet_ref => cabinet_ref.as_str(),
            item_title => item_view.item.title,
            versions => versions,
        },
    )
    .await
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
