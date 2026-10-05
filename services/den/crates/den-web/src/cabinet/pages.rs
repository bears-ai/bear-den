//! Server-rendered Cabinet page policy, topology and review mutations.

use super::{cabinet_error, item_url, parse_item_ref, require_user_scope};
use crate::{auth_backend::AuthSession, errors::CustomError, AppState};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::post,
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
