//! Server-rendered Cabinet page policy, topology and review mutations.

use super::{cabinet_error, item_url, parse_item_ref, require_user_scope};
use crate::{auth_backend::AuthSession, errors::CustomError, AppState};
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use den_cabinet::{CabinetPolicy, ReviewDecision, ReviewRequest};
use den_service::cabinet::pages;
use serde::Deserialize;

#[derive(Deserialize)]
struct PolicyForm {
    #[serde(default)]
    people: String,
    #[serde(default)]
    bears: String,
    #[serde(default)]
    reviewers: String,
    #[serde(default)]
    bears_may_write: bool,
    #[serde(default)]
    review_required: bool,
    #[serde(default)]
    confirm_audience: bool,
}
#[derive(Deserialize)]
struct MoveForm {
    #[serde(default)]
    parent: String,
    position: i32,
    #[serde(default)]
    confirm_audience: bool,
}
#[derive(Deserialize)]
struct ReviewForm {
    version: den_cabinet::CabinetVersionRef,
    decision: ReviewDecision,
    rationale: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cabinet/{reference}/policy", post(policy))
        .route("/cabinet/{reference}/organize", post(organize))
        .route("/cabinet/{reference}/move", get(move_preview))
        .route("/cabinet/{reference}/review", post(review))
}
fn names(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}
async fn policy(
    State(state): State<AppState>,
    session: AuthSession,
    Path(reference): Path<String>,
    Form(form): Form<PolicyForm>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    let reference = parse_item_ref(&reference)?;
    if !form.confirm_audience {
        return Err(CustomError::ValidationError(
            "confirm the page audience change".into(),
        ));
    }
    pages::configure_named(
        state.sqlx_pool(),
        &scope,
        &reference,
        CabinetPolicy {
            bears_may_write: form.bears_may_write,
            review_required: form.review_required,
            allowed_kinds: None,
        },
        &names(&form.people),
        &names(&form.bears),
        &names(&form.reviewers),
    )
    .await
    .map_err(cabinet_error)?;
    Ok(Redirect::to(&item_url(&reference)).into_response())
}
#[derive(Deserialize)]
struct MovePreview {
    #[serde(default)]
    parent: String,
    position: i32,
}

async fn move_preview(
    State(state): State<AppState>,
    session: AuthSession,
    Path(reference): Path<String>,
    Query(query): Query<MovePreview>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    let reference = parse_item_ref(&reference)?;
    let page = pages::metadata(state.sqlx_pool(), &scope, &reference)
        .await
        .map_err(cabinet_error)?;
    if !page.can_write {
        return Err(CustomError::Authorization("page is read only".into()));
    }
    let parent = if query.parent.is_empty() {
        None
    } else {
        Some(parse_item_ref(&query.parent)?)
    };
    if let Some(parent) = &parent {
        if !pages::metadata(state.sqlx_pool(), &scope, parent)
            .await
            .map_err(cabinet_error)?
            .can_write
        {
            return Err(CustomError::Authorization(
                "destination is read only".into(),
            ));
        }
    }
    if parent.as_ref() == Some(&reference) || query.position < 0 {
        return Err(CustomError::ValidationError(
            "Choose a different destination and a nonnegative position.".into(),
        ));
    }
    let viewer = session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".into()))?
        .id;
    let preview = sqlx::query!(
        r#"SELECT i.title AS page_title, p.title AS "destination_title?",
            EXISTS (SELECT 1 FROM cabinet_ancestors(p.id) a WHERE a.id = i.id) AS "cycle!",
            CASE WHEN p.id IS NULL THEN false ELSE
                (SELECT count(*) FROM cabinet_ancestors(p.id)) +
                (WITH RECURSIVE subtree AS (
                    SELECT id, 1 AS depth FROM cabinet_items WHERE id = i.id
                    UNION ALL SELECT child.id, subtree.depth + 1 FROM cabinet_items child
                    JOIN subtree ON child.parent_item_id = subtree.id WHERE subtree.depth < 33
                ) SELECT COALESCE(max(depth), 1) FROM subtree) > 32
            END AS "too_deep!"
        FROM cabinet_items i LEFT JOIN cabinet_items p ON p.cabinet_ref = $2
        WHERE i.cabinet_ref = $1 AND cabinet_can_access(i.id, $3, NULL, true)
            AND ($2::text IS NULL OR cabinet_can_access(p.id, $3, NULL, true))"#,
        reference.as_str(),
        parent.as_ref().map(|parent| parent.as_str()),
        viewer,
    )
    .fetch_optional(state.sqlx_pool())
    .await
    .map_err(den_core::DenError::from)?
    .ok_or_else(|| CustomError::NotFound("page or destination unavailable".into()))?;
    if preview.cycle || preview.too_deep {
        return Err(CustomError::ValidationError("Choose a destination outside this page's subtree and within the page depth limit. No page was moved.".into()));
    }
    let current_audience = super::audience::for_page(state.sqlx_pool(), &scope, &reference).await?;
    let target_audience =
        super::audience::after_move(state.sqlx_pool(), &scope, &reference, parent.as_ref()).await?;
    crate::web::render_template(&state, "cabinet/move.html", session, minijinja::context! {
        title => "Review page move", cabinet_ref => reference, parent, position => query.position,
        page_title => preview.page_title, destination_title => preview.destination_title,
        current_audience, target_audience,
    }).await
}

async fn organize(
    State(state): State<AppState>,
    session: AuthSession,
    Path(reference): Path<String>,
    Form(form): Form<MoveForm>,
) -> Result<Response, CustomError> {
    let scope = require_user_scope(&session)?;
    let reference = parse_item_ref(&reference)?;
    let parent = if form.parent.is_empty() {
        None
    } else {
        Some(parse_item_ref(&form.parent)?)
    };
    pages::organize(
        state.sqlx_pool(),
        &scope,
        &reference,
        parent.as_ref(),
        form.position,
        form.confirm_audience,
    )
    .await
    .map_err(cabinet_error)?;
    Ok(Redirect::to(&item_url(&reference)).into_response())
}
async fn review(
    State(state): State<AppState>,
    session: AuthSession,
    Path(reference): Path<String>,
    Form(form): Form<ReviewForm>,
) -> Result<Response, CustomError> {
    let reference = parse_item_ref(&reference)?;
    pages::review(
        state.sqlx_pool(),
        ReviewRequest {
            scope: require_user_scope(&session)?,
            cabinet_ref: reference.clone(),
            version_ref: form.version,
            decision: form.decision,
            rationale: form.rationale,
        },
    )
    .await
    .map_err(cabinet_error)?;
    Ok(Redirect::to(&item_url(&reference)).into_response())
}
