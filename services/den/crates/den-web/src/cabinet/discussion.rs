//! Current page discussion, independent of immutable content revisions.

use super::{actor_label, cabinet_error, item_url, parse_item_ref, require_user_scope};
use crate::{auth_backend::AuthSession, errors::CustomError, web, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::extract::Form;
use den_cabinet::{CabinetError, CabinetItemRef, Lifecycle, ReadRequest};
use den_service::cabinet;
use minijinja::context;
use serde::Deserialize;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/cabinet/{cabinet_ref}/discussion", get(show).post(post))
}

#[derive(Deserialize)]
struct DiscussionForm {
    #[serde(default)]
    content: String,
}

pub(super) fn draft_error(content: &str) -> Option<&'static str> {
    if content.trim().is_empty() {
        Some("Write a message before posting. Your draft has been preserved.")
    } else if content.contains('\0') {
        Some(
            "Remove NUL characters from your message and try again. Your draft has been preserved.",
        )
    } else if content.trim().chars().count() > cabinet::discussion::MAX_DISCUSSION_CHARS {
        Some("Use at most 16,000 characters. Shorten your preserved draft and try again.")
    } else {
        None
    }
}

async fn show(
    State(state): State<AppState>,
    session: AuthSession,
    Path(reference): Path<String>,
) -> Result<Response, CustomError> {
    require_user_scope(&session)?;
    let reference = parse_item_ref(&reference)?;
    render(&state, session, &reference, "", None, StatusCode::OK).await
}

async fn render(
    state: &AppState,
    session: AuthSession,
    reference: &CabinetItemRef,
    draft: &str,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    // Always use current ACL and lifecycle, even when linked from an old revision.
    let page = cabinet::pages::metadata(state.sqlx_pool(), &scope, reference)
        .await
        .map_err(cabinet_error)?;
    let view = cabinet::read(
        state.sqlx_pool(),
        ReadRequest {
            scope: scope.clone(),
            cabinet_ref: reference.clone(),
            version_ref: None,
        },
    )
    .await
    .map_err(cabinet_error)?;
    let entries = cabinet::discussion::list(state.sqlx_pool(), &scope, reference)
        .await
        .map_err(cabinet_error)?;
    let author_labels: Vec<_> = entries
        .iter()
        .map(|entry| actor_label(&entry.authored_by, &scope))
        .collect();
    let mut response = web::render_template(
        state,
        "cabinet/discussion.html",
        session,
        context! {
            title => format!("Discussion: {}", view.item.title),
            item_title => view.item.title,
            cabinet_ref => reference,
            can_post => page.can_write && view.item.lifecycle == Lifecycle::Active,
            archived => view.item.lifecycle == Lifecycle::Archived,
            entries,
            author_labels,
            draft,
            error,
        },
    )
    .await?;
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

async fn post(
    State(state): State<AppState>,
    session: AuthSession,
    Path(reference): Path<String>,
    Form(form): Form<DiscussionForm>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    let reference = parse_item_ref(&reference)?;
    // Author identity is never accepted from the form. Check readability before
    // validation so invalid drafts cannot turn a hidden ref into an existence oracle.
    let page = cabinet::pages::metadata(state.sqlx_pool(), &scope, &reference)
        .await
        .map_err(cabinet_error)?;
    if !page.can_write {
        return render(
            &state, session, &reference, &form.content,
            Some("This page is read only for you. Your draft is preserved; ask a page owner for write access."),
            StatusCode::FORBIDDEN,
        ).await;
    }
    if let Some(error) = draft_error(&form.content) {
        return render(
            &state,
            session,
            &reference,
            &form.content,
            Some(error),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    match cabinet::discussion::post(state.sqlx_pool(), &scope, &reference, &form.content).await {
        Ok(_) => Ok(Redirect::to(&format!("{}/discussion#discussion-entries", item_url(&reference))).into_response()),
        Err(CabinetError::Validation(_) | CabinetError::Policy(_)) => render(
            &state, session, &reference, &form.content,
            Some("The message was not posted. Use a non-empty message of at most 16,000 characters on an active page. Restore an archived page before posting. Your draft is preserved."),
            StatusCode::UNPROCESSABLE_ENTITY,
        ).await,
        Err(CabinetError::NotAuthorized) => render(
            &state, session, &reference, &form.content,
            Some("Your write access changed. The message was not posted and your draft is preserved; ask a page owner for access."),
            StatusCode::FORBIDDEN,
        ).await,
        Err(error) => Err(cabinet_error(error)),
    }
}
