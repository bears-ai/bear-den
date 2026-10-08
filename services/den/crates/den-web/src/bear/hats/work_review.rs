//! Bear-admin review of all historical hat memory before Work gains access.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use axum_login::tower_sessions::Session;
use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};
use den_service::bears::hats::{
    manage,
    work_review::{self, WorkReviewDecision},
};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::settings::{bear_nav_context, load_session_bear_manage, session_user};
use super::work_review_draft::{WorkReviewDraft, WorkReviewDraftScope};
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

#[derive(Debug, Deserialize, Serialize)]
struct WorkReviewForm {
    expected_sha256: String,
    expected_record_count: i64,
    expected_identity_sha256: String,
    rationale: String,
    #[serde(default)]
    confirm_work_audience: bool,
}

#[derive(Debug, Deserialize)]
struct ReviewPageQuery {
    #[serde(default = "first_review_page")]
    page: u32,
    expected_sha256: Option<String>,
    expected_identity_sha256: Option<String>,
}

fn first_review_page() -> u32 {
    1
}

async fn review_get(
    Path((slug, hat_uuid)): Path<(String, Uuid)>,
    Query(query): Query<ReviewPageQuery>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let hat_id = HatId::new(hat_uuid);
    let scope = WorkReviewDraftScope::new(
        BearId::new(bear.id),
        hat_id,
        UserId::new(session_user(&auth).await?.id),
    );
    let draft = scope.load(&session).await?;
    render_review(state, auth, bear, hat_id, query, None, draft).await
}

async fn render_review(
    state: AppState,
    auth: AuthSession,
    bear: den_service::bears::Bear,
    hat_id: HatId,
    query: ReviewPageQuery,
    error: Option<String>,
    review_draft: Option<WorkReviewDraft>,
) -> Result<Response, CustomError> {
    let bear_id = BearId::new(bear.id);
    let mut error = error;
    let reviewer = UserId::new(session_user(&auth).await?.id);
    let hat = manage::get_hat(state.sqlx_pool(), bear_id, hat_id).await?;
    let mut snapshot = match work_review::snapshot_page_for_admin(
        state.sqlx_pool(),
        &state.memory_stores,
        bear_id,
        hat_id,
        reviewer,
        query.page,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(DenError::ValidationError(message)) => {
            error = Some(format!("{message}. Restart the review at page 1."));
            work_review::snapshot_page_for_admin(
                state.sqlx_pool(),
                &state.memory_stores,
                bear_id,
                hat_id,
                reviewer,
                1,
            )
            .await?
        }
        Err(error) => return Err(error.into()),
    };
    if query.page > 1
        && (snapshot.sha256.is_none()
            || query.expected_sha256.as_deref() != snapshot.sha256.as_deref()
            || query.expected_identity_sha256.as_deref() != Some(snapshot.identity_sha256.as_str()))
    {
        error = Some("Hat memory or identity changed during review. No Work permission was granted; review the new snapshot from page 1.".into());
        snapshot = work_review::snapshot_page_for_admin(
            state.sqlx_pool(),
            &state.memory_stores,
            bear_id,
            hat_id,
            reviewer,
            1,
        )
        .await?;
    }
    let available_hats = den_service::bears::hats::list_hats(state.sqlx_pool(), bear_id).await?;
    let identity_preview = den_service::bears::hats::identity::render_hat_identity_component(
        &bear,
        &hat,
        &available_hats,
    )?;
    let grant_count = manage::allowed_surfaces(state.sqlx_pool(), bear_id, hat_id)
        .await?
        .len();
    let failed = error.is_some();
    let mut response = web::render_template(
        &state,
        "bear/manage/hat_work_review.jinja",
        auth,
        context! {
            hat, snapshot, identity_preview, grant_count, error, review_draft, can_manage_bear => true, native_runtime => true,
            ..bear_nav_context(&bear, "hats"),
        },
    )
    .await?;
    if failed {
        *response.status_mut() = StatusCode::BAD_REQUEST;
    }
    Ok(response)
}

async fn review_post(
    Path((slug, hat_uuid)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
    Form(form): Form<WorkReviewForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let scope = WorkReviewDraftScope::new(
        BearId::new(bear.id),
        HatId::new(hat_uuid),
        UserId::new(session_user(&auth).await?.id),
    );
    let result = if !form.confirm_work_audience {
        Err(DenError::ValidationError(
            "Confirm that you reviewed all hat entries for autonomous Work.".into(),
        ))
    } else {
        work_review::review_and_enable(
            state.sqlx_pool(),
            &state.memory_stores,
            BearId::new(bear.id),
            HatId::new(hat_uuid),
            UserId::new(session_user(&auth).await?.id),
            WorkReviewDecision {
                expected_sha256: form.expected_sha256.clone(),
                expected_record_count: form.expected_record_count,
                expected_identity_sha256: form.expected_identity_sha256.clone(),
                rationale: form.rationale.clone(),
            },
        )
        .await
    };
    let receipt = match result {
        Ok(receipt) => receipt,
        Err(DenError::ValidationError(message)) => {
            scope.save(&session, &form.rationale).await?;
            return render_review(
                state,
                auth,
                bear,
                HatId::new(hat_uuid),
                ReviewPageQuery {
                    page: 1,
                    expected_sha256: None,
                    expected_identity_sha256: None,
                },
                Some(message),
                Some(WorkReviewDraft {
                    rationale: form.rationale,
                }),
            )
            .await;
        }
        Err(error) => return Err(error.into()),
    };
    scope.clear(&session).await?;
    let message = format!(
        "Reviewed {} hat record(s). Work enabled.",
        receipt.record_count
    );
    Ok(Redirect::to(&format!(
        "/bear/{}/hats/{hat_uuid}?message={}",
        bear.slug,
        urlencoding::encode(&message)
    ))
    .into_response())
}
