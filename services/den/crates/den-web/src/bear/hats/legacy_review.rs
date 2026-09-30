//! Bear-admin inventory of unattributed profile-local records. A human writes
//! new shareable content; no imported record is assigned an invented owner.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::ids::{BearId, HatId, UserId};
use den_service::bears::hats::{
    legacy_review::{self, LegacyReviewDecision},
    manage,
};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::settings::{bear_nav_context, load_session_bear_manage, session_user};
use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};

pub(super) fn router() -> Router<AppState> {
    Router::new().route_with_tsr(
        "/bear/{slug}/hats/{hat_id}/legacy-review",
        get(review_get).post(review_post),
    )
}

#[derive(Debug, Deserialize)]
struct ReviewQuery {
    source_id: Option<String>,
    before: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ReviewForm {
    source_memory_id: String,
    kind: String,
    reviewed_content: String,
    expected_head: Option<Uuid>,
    review_notes: String,
    #[serde(default)]
    acknowledge_unverified_source_and_members: bool,
    #[serde(default)]
    work_audience_reviewed: bool,
}

#[derive(Serialize)]
struct HeadView {
    id: String,
    content_text: String,
}

async fn review_get(
    Path((slug, hat_uuid)): Path<(String, Uuid)>,
    Query(query): Query<ReviewQuery>,
    State(state): State<AppState>,
    auth: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let bear_id = BearId::new(bear.id);
    let hat_id = HatId::new(hat_uuid);
    let reviewer = UserId::new(session_user(&auth).await?.id);
    let hat = manage::get_hat(state.sqlx_pool(), bear_id, hat_id).await?;
    let page = legacy_review::inventory_page(
        state.sqlx_pool(),
        &state.memory_stores,
        bear_id,
        reviewer,
        hat_id,
        query.before,
    )
    .await?;
    let selected = if let Some(ref source_id) = query.source_id {
        Some(
            legacy_review::candidate(
                state.sqlx_pool(),
                &state.memory_stores,
                bear_id,
                reviewer,
                hat_id,
                source_id,
            )
            .await?,
        )
    } else {
        None
    };
    let current_head = if let Some(ref selected) = selected {
        let store = state.memory_stores.store_for_bear(bear.id).await?;
        den_memory::hat_promotion::current_hat_head(&store, hat_id, &selected.kind)
            .await?
            .map(|head| HeadView {
                id: head.memory_id.to_string(),
                content_text: head.content_text,
            })
    } else {
        None
    };
    web::render_template(
        &state,
        "bear/manage/hat_legacy_review.jinja",
        auth,
        context! {
            hat, page, selected, current_head,
            can_manage_bear => true, native_runtime => true,
            ..bear_nav_context(&bear, "hats"),
        },
    )
    .await
}

async fn review_post(
    Path((slug, hat_uuid)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<ReviewForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let result = legacy_review::reauthor(
        state.sqlx_pool(),
        &state.memory_stores,
        BearId::new(bear.id),
        UserId::new(session_user(&auth).await?.id),
        LegacyReviewDecision {
            source_memory_id: form.source_memory_id,
            target_hat: HatId::new(hat_uuid),
            kind: form.kind,
            reviewed_content: form.reviewed_content,
            expected_head: form.expected_head,
            review_notes: form.review_notes,
            acknowledge_unverified_source_and_members: form
                .acknowledge_unverified_source_and_members,
            work_audience_reviewed: form.work_audience_reviewed,
        },
    )
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/memory/records/{}",
        bear.slug, result.memory_id,
    ))
    .into_response())
}
