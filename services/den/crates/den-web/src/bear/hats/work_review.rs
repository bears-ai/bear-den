//! Bear-admin review of all historical hat memory before Work gains access.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::ids::{BearId, HatId, UserId};
use den_service::bears::hats::{
    manage,
    work_review::{self, WorkReviewDecision},
};
use minijinja::context;
use serde::Deserialize;
use uuid::Uuid;

use super::super::settings::{bear_nav_context, load_session_bear_manage, session_user};
use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};

pub(super) fn router() -> Router<AppState> {
    Router::new().route_with_tsr(
        "/bear/{slug}/hats/{hat_id}/work-review",
        get(review_get).post(review_post),
    )
}

#[derive(Debug, Deserialize)]
struct WorkReviewForm {
    expected_sha256: String,
    expected_record_count: i64,
    rationale: String,
    #[serde(default)]
    confirm_work_audience: bool,
}

#[derive(Debug, Deserialize)]
struct ReviewPageQuery {
    #[serde(default = "first_review_page")]
    page: u32,
    expected_sha256: Option<String>,
}

fn first_review_page() -> u32 {
    1
}

async fn review_get(
    Path((slug, hat_uuid)): Path<(String, Uuid)>,
    Query(query): Query<ReviewPageQuery>,
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
    let snapshot = work_review::snapshot_page_for_admin(
        state.sqlx_pool(),
        &state.memory_stores,
        bear_id,
        hat_id,
        reviewer,
        query.page,
    )
    .await?;
    if query.page > 1
        && (snapshot.sha256.is_none()
            || query.expected_sha256.as_deref() != snapshot.sha256.as_deref())
    {
        return Err(CustomError::ValidationError(
            "hat memory changed during review; start again at page 1".into(),
        ));
    }
    web::render_template(
        &state,
        "bear/manage/hat_work_review.jinja",
        auth,
        context! {
            hat, snapshot, can_manage_bear => true, native_runtime => true,
            ..bear_nav_context(&bear, "hats"),
        },
    )
    .await
}

async fn review_post(
    Path((slug, hat_uuid)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<WorkReviewForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    if !form.confirm_work_audience {
        return Err(CustomError::ValidationError(
            "confirm that you reviewed all hat entries for autonomous Work".into(),
        ));
    }
    let receipt = work_review::review_and_enable(
        state.sqlx_pool(),
        &state.memory_stores,
        BearId::new(bear.id),
        HatId::new(hat_uuid),
        UserId::new(session_user(&auth).await?.id),
        WorkReviewDecision {
            expected_sha256: form.expected_sha256,
            expected_record_count: form.expected_record_count,
            rationale: form.rationale,
        },
    )
    .await?;
    let message = format!(
        "Reviewed {} hat record(s) for Work (receipt {}). Work enabled.",
        receipt.record_count, receipt.id
    );
    Ok(Redirect::to(&format!(
        "/bear/{}/hats/{hat_uuid}?message={}",
        bear.slug,
        urlencoding::encode(&message)
    ))
    .into_response())
}
