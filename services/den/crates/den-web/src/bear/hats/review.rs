//! An explicit Bear-admin review surface for source-local → hat promotion.
//! Raw notes are never rendered to ordinary members or copied as a default.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::ids::{BearId, HatId, UserId};
use den_memory::hat_promotion;
use den_service::bears::hats::{
    curation::{self, ReviewedHatEntry},
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
        "/bear/{slug}/hats/{hat_id}/review",
        get(review_get).post(review_post),
    )
}

#[derive(Debug, Deserialize)]
struct ReviewQuery {
    #[serde(default)]
    source_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct ReviewForm {
    source_memory_id: Uuid,
    kind: String,
    reviewed_content: String,
    #[serde(default)]
    expected_head: Option<Uuid>,
    review_notes: String,
    #[serde(default)]
    acknowledge_sharing: bool,
    #[serde(default)]
    work_audience_reviewed: bool,
}

#[derive(Debug, Serialize)]
struct CandidateView {
    id: String,
    source: String,
    kind: String,
    preview: String,
    content_text: String,
}

impl From<hat_promotion::ReviewCandidate> for CandidateView {
    fn from(candidate: hat_promotion::ReviewCandidate) -> Self {
        let source = format!("{} {}", candidate.source.kind(), candidate.source.id());
        let preview: String = candidate.content_text.chars().take(110).collect();
        Self {
            id: candidate.memory_id.to_string(),
            source,
            kind: candidate.kind,
            preview,
            content_text: candidate.content_text,
        }
    }
}

#[derive(Debug, Serialize)]
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
    let sources: Vec<CandidateView> = curation::candidates(
        state.sqlx_pool(),
        &state.memory_stores,
        bear_id,
        reviewer,
        hat_id,
        50,
    )
    .await?
    .into_iter()
    .map(CandidateView::from)
    .collect();
    let selected: Option<CandidateView> = if let Some(id) = query.source_id {
        Some(
            curation::candidate(
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
        None
    };
    let current_head = if let Some(selected) = &selected {
        let store = state.memory_stores.store_for_bear(bear.id).await?;
        hat_promotion::current_hat_head(&store, hat_id, &selected.kind)
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
        "bear/manage/hat_review.jinja",
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
    if !form.acknowledge_sharing {
        return Err(CustomError::ValidationError(
            "confirm that the reviewed entry is safe to share with every member wearing this hat"
                .into(),
        ));
    }
    let outcome = curation::promote(
        state.sqlx_pool(),
        &state.memory_stores,
        BearId::new(bear.id),
        UserId::new(session_user(&auth).await?.id),
        ReviewedHatEntry {
            source_memory_id: form.source_memory_id,
            hat_id: HatId::new(hat_uuid),
            kind: form.kind,
            reviewed_content: form.reviewed_content,
            expected_head: form.expected_head,
            review_notes: form.review_notes,
            work_audience_reviewed: form.work_audience_reviewed,
        },
    )
    .await?;
    den_runtime::reflection::conductor::enqueue_recall_index_if_enabled(
        state.sqlx_pool(),
        state.config.as_ref(),
        bear.id,
        "reviewed_source_to_hat",
    )
    .await;
    Ok(Redirect::to(&format!(
        "/bear/{}/memory/records/{}",
        bear.slug, outcome.memory_id
    ))
    .into_response())
}
