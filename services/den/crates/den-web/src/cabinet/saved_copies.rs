//! Creator-only saved-copy history and explicit retirement; no model-facing tool.
use crate::{auth_backend::AuthSession, errors::CustomError, web, AppState};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::extract::Form;
use axum_login::tower_sessions::Session;
use den_core::{DenError, UserId};
use den_service::artifacts::{
    self,
    snapshot_retirement::{
        self as retirement, HistoryCursor, InventoryFingerprint, RetireCabinetSnapshot,
        SnapshotRetirementPreview,
    },
    ArtifactReader, ArtifactRef,
};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const CONFIRMATION_KEY: &str = "cabinet_snapshot_retirement_confirmation";
#[derive(Serialize, Deserialize)]
struct Confirmation {
    actor: UserId,
    reference: ArtifactRef,
    fingerprint: InventoryFingerprint,
    token: Uuid,
    issued_at: i64,
}
#[derive(Default, Deserialize)]
struct HistoryQuery {
    before: Option<Uuid>,
}
#[derive(Default, Deserialize)]
struct RetirementForm {
    expected: Option<InventoryFingerprint>,
    token: Option<Uuid>,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    acknowledged: bool,
}
fn actor(session: &AuthSession) -> Result<UserId, CustomError> {
    session
        .user
        .as_ref()
        .map(|user| UserId::new(user.id))
        .ok_or_else(|| CustomError::Authentication("Sign in to continue".into()))
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cabinet/saved-copies", get(history))
        .route(
            "/cabinet/saved-copies/{artifact_ref}/retire",
            get(preview).post(retire),
        )
        .route("/cabinet/saved-copies/{artifact_ref}/content", get(content))
}
async fn history(
    State(state): State<AppState>,
    session: AuthSession,
    Query(query): Query<HistoryQuery>,
) -> Result<Response, CustomError> {
    let history = retirement::history(
        state.sqlx_pool(),
        actor(&session)?,
        query.before.map(HistoryCursor),
        None,
        None,
    )
    .await?;
    let mut response = web::render_template(
        &state,
        "cabinet/saved_copies.html",
        session,
        context! { history },
    )
    .await?;
    super::attachments::protect_response(&mut response);
    Ok(response)
}
async fn render(
    state: &AppState,
    auth: AuthSession,
    session: &Session,
    view: SnapshotRetirementPreview,
    reason: String,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, CustomError> {
    let token = Uuid::new_v4();
    session
        .insert(
            CONFIRMATION_KEY,
            Confirmation {
                actor: actor(&auth)?,
                reference: view.reference.clone(),
                fingerprint: view.fingerprint.clone(),
                token,
                issued_at: time::OffsetDateTime::now_utc().unix_timestamp(),
            },
        )
        .await?;
    let blocker = view.blocker.map(|blocker| blocker.explanation());
    let can_retire = view.can_retire();
    let mut response = web::render_template(
        state,
        "cabinet/retire_snapshot.html",
        auth,
        context! { view,token,blocker,can_retire,reason,error },
    )
    .await?;
    *response.status_mut() = status;
    super::attachments::protect_response(&mut response);
    Ok(response)
}
async fn preview(
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
    Path(reference): Path<ArtifactRef>,
) -> Result<Response, CustomError> {
    let view = retirement::preview(state.sqlx_pool(), actor(&auth)?, &reference).await?;
    render(
        &state,
        auth,
        &session,
        view,
        String::new(),
        None,
        StatusCode::OK,
    )
    .await
}
async fn retire(
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
    Path(reference): Path<ArtifactRef>,
    Form(form): Form<RetirementForm>,
) -> Result<Response, CustomError> {
    let actor = actor(&auth)?;
    let confirmation = session.remove::<Confirmation>(CONFIRMATION_KEY).await?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let fresh = confirmation.is_some_and(|value| {
        value.actor == actor
            && value.reference == reference
            && Some(value.token) == form.token
            && Some(&value.fingerprint) == form.expected.as_ref()
            && (0..=900).contains(&(now - value.issued_at))
    });
    if !fresh {
        let view = retirement::preview(state.sqlx_pool(), actor, &reference).await?;
        return render(&state,auth,&session,view,form.reason,Some("Review this fresh preview and confirm again. The earlier confirmation expired, changed, or was already submitted; nothing was changed.".into()),StatusCode::CONFLICT).await;
    }
    let expected = form
        .expected
        .ok_or_else(|| CustomError::ValidationError("Review a fresh preview".into()))?;
    let result = retirement::retire(
        state.sqlx_pool(),
        RetireCabinetSnapshot {
            actor,
            reference: reference.clone(),
            expected,
            reason: form.reason.clone(),
            acknowledged: form.acknowledged,
        },
    )
    .await;
    match result {
        Ok(_) => Ok(Redirect::to(&format!(
            "/cabinet/saved-copies/{}/retire",
            reference.as_str()
        ))
        .into_response()),
        Err(DenError::ValidationError(error)) => {
            let view = retirement::preview(state.sqlx_pool(), actor, &reference).await?;
            render(
                &state,
                auth,
                &session,
                view,
                form.reason,
                Some(error),
                StatusCode::CONFLICT,
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}
async fn content(
    State(state): State<AppState>,
    auth: AuthSession,
    Path(reference): Path<ArtifactRef>,
) -> Result<Response, CustomError> {
    // Creator-only typed snapshot admission is independent of any surviving source-page access.
    let actor = actor(&auth)?;
    let view = retirement::preview(state.sqlx_pool(), actor, &reference).await?;
    if !view.readable {
        return Err(CustomError::NotFound("Saved copy unavailable".into()));
    }
    let payload = artifacts::json_content_for_reader(
        state.sqlx_pool(),
        &reference,
        ArtifactReader::Human(actor),
    )
    .await?;
    let bytes = serde_json::to_vec(&payload)
        .map_err(|_| CustomError::System("Saved copy encoding unavailable".into()))?;
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}.json\"", reference.as_str()),
        )
        .body(Body::from(bytes))
        .map_err(|_| CustomError::System("Saved copy response unavailable".into()))?;
    super::attachments::protect_response(&mut response);
    Ok(response)
}
