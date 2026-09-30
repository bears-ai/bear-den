//! Bear-admin, explicit review of already-curated hat knowledge for the wider
//! Bear-core audience. The source remains in its original hat.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::ids::{BearId, HatId, UserId};
use den_service::bears::hats::{
    core_review::{self, CoreReviewDecision},
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
        "/bear/{slug}/hats/{hat_id}/core-review",
        get(review_get).post(review_post),
    )
}

#[derive(Debug, Deserialize)]
struct ReviewQuery {
    source_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct ReviewForm {
    source_memory_id: Uuid,
    kind: String,
    reviewed_content: String,
    expected_head: Option<Uuid>,
    review_notes: String,
    #[serde(default)]
    acknowledge_bear_and_work_audience: bool,
}

#[derive(Serialize)]
struct CandidateView {
    id: String,
    kind: String,
    content_text: String,
    preview: String,
}

impl From<den_memory::reviewed_core::ReviewCandidate> for CandidateView {
    fn from(candidate: den_memory::reviewed_core::ReviewCandidate) -> Self {
        let preview = candidate.content_text.chars().take(110).collect();
        Self {
            id: candidate.memory_id,
            kind: candidate.kind,
            content_text: candidate.content_text,
            preview,
        }
    }
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
    let sources: Vec<CandidateView> = core_review::candidates(
        state.sqlx_pool(),
        &state.memory_stores,
        bear_id,
        reviewer,
        hat_id,
    )
    .await?
    .into_iter()
    .map(CandidateView::from)
    .collect();
    let selected = if let Some(id) = query.source_id {
        Some(
            core_review::candidate(
                state.sqlx_pool(),
                &state.memory_stores,
                bear_id,
                reviewer,
                hat_id,
                id,
            )
            .await?
            .into(),
        )
    } else {
        None::<CandidateView>
    };
    let current_head = if let Some(ref selected) = selected {
        core_review::core_head(
            state.sqlx_pool(),
            &state.memory_stores,
            bear_id,
            reviewer,
            hat_id,
            &selected.kind,
        )
        .await?
    } else {
        None
    };
    web::render_template(
        &state,
        "bear/manage/hat_core_review.jinja",
        auth,
        context! {
            hat, sources, selected, current_head,
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
    let result = core_review::promote(
        state.sqlx_pool(),
        &state.memory_stores,
        BearId::new(bear.id),
        UserId::new(session_user(&auth).await?.id),
        CoreReviewDecision {
            source_memory_id: form.source_memory_id,
            hat_id: HatId::new(hat_uuid),
            kind: form.kind,
            reviewed_content: form.reviewed_content,
            expected_head: form.expected_head,
            review_notes: form.review_notes,
            acknowledge_bear_and_work_audience: form.acknowledge_bear_and_work_audience,
        },
    )
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/memory/records/{}",
        bear.slug, result.memory_id,
    ))
    .into_response())
}
