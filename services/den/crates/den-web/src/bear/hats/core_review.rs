//! Bear-admin, explicit review of already-curated hat knowledge for the wider
//! Bear-core audience. The source remains in its original hat.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};
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

#[derive(Debug, Deserialize, Serialize)]
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
    render_review(state, auth, bear, HatId::new(hat_uuid), query, None).await
}

async fn render_review(
    state: AppState,
    auth: AuthSession,
    bear: den_service::bears::Bear,
    hat_id: HatId,
    query: ReviewQuery,
    feedback: Option<(StatusCode, String, ReviewForm)>,
) -> Result<Response, CustomError> {
    let (status, error, draft) = match feedback {
        Some((status, error, mut draft)) => {
            draft.acknowledge_bear_and_work_audience = false;
            (status, Some(error), Some(draft))
        }
        None => (StatusCode::OK, None, None),
    };
    let bear_id = BearId::new(bear.id);
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
            draft
                .as_ref()
                .map(|form| form.kind.as_str())
                .unwrap_or(&selected.kind),
        )
        .await?
    } else {
        None
    };
    let mut response = web::render_template(
        &state,
        "bear/manage/hat_core_review.jinja",
        auth,
        context! {
            hat, sources, selected, current_head, error, draft,
            can_manage_bear => true, native_runtime => true,
            ..bear_nav_context(&bear, "hats"),
        },
    )
    .await?;
    *response.status_mut() = status;
    Ok(response)
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
    let hat_id = HatId::new(hat_uuid);
    if !form.acknowledge_bear_and_work_audience {
        return render_review(
            state, auth, bear, hat_id,
            ReviewQuery { source_id: Some(form.source_memory_id) },
            Some((StatusCode::FORBIDDEN, "Review this entry for all Bear members and future autonomous Work and acknowledge that audience before publishing.".into(), form)),
        ).await;
    }
    let result = core_review::promote(
        state.sqlx_pool(),
        &state.memory_stores,
        BearId::new(bear.id),
        UserId::new(session_user(&auth).await?.id),
        CoreReviewDecision {
            source_memory_id: form.source_memory_id,
            hat_id: HatId::new(hat_uuid),
            kind: form.kind.clone(),
            reviewed_content: form.reviewed_content.clone(),
            expected_head: form.expected_head,
            review_notes: form.review_notes.clone(),
            acknowledge_bear_and_work_audience: form.acknowledge_bear_and_work_audience,
        },
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(DenError::ValidationError(message)) => {
            return render_review(
                state,
                auth,
                bear,
                hat_id,
                ReviewQuery {
                    source_id: Some(form.source_memory_id),
                },
                Some((StatusCode::BAD_REQUEST, message, form)),
            )
            .await;
        }
        Err(error) => return Err(error.into()),
    };
    den_runtime::reflection::conductor::enqueue_recall_index_if_enabled(
        state.sqlx_pool(),
        state.config.as_ref(),
        bear.id,
        "reviewed_hat_to_core",
    )
    .await;
    Ok(Redirect::to(&format!(
        "/bear/{}/memory/records/{}",
        bear.slug, result.memory_id,
    ))
    .into_response())
}
