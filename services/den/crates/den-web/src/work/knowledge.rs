//! Human Job↔Cabinet navigation and immutable document evidence.

use axum::{
    body::Body,
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use den_cabinet::{ActorScope, CabinetError, CabinetItemRef, CabinetVersionRef, ReadRequest};
use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_docket::missions::{self, JobReference};
use den_service::{
    artifacts::{
        self, ArtifactAccessLevel, ArtifactReader, ArtifactRef, ArtifactStorageKind,
        DocketArtifactTargetKind,
    },
    cabinet,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{auth_backend::AuthSession, errors::CustomError, AppState};

#[derive(Debug, Serialize)]
pub(super) struct MissionView {
    pub reference: Option<CabinetItemRef>,
    pub title: Option<String>,
    pub version: Option<CabinetVersionRef>,
    pub revision: i64,
    pub can_edit: bool,
    pub evidence: Vec<EvidenceView>,
    pub saved_copies: Vec<artifacts::snapshot_retirement::SnapshotSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct EvidenceView {
    pub reference: ArtifactRef,
    pub title: String,
}

#[derive(Deserialize)]
struct LinkForm {
    #[serde(default)]
    cabinet_ref: String,
    revision: i64,
}

#[derive(Deserialize)]
struct SnapshotForm {
    revision: i64,
    version: CabinetVersionRef,
}

fn error(value: CabinetError) -> CustomError {
    CustomError::from(DenError::from(value))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs/{job_ref}/mission", post(link))
        .route("/jobs/{job_ref}/mission/snapshot", post(snapshot))
        .route(
            "/jobs/{job_ref}/evidence/{artifact_ref}/content",
            get(evidence_content),
        )
}

pub(super) async fn view(
    state: &AppState,
    bear: BearId,
    job: Uuid,
    user: UserId,
) -> Result<MissionView, CustomError> {
    let annotation =
        missions::get_for_viewer(state.sqlx_pool(), bear, JobReference(job), user).await?;
    let mut visible = MissionView {
        reference: None,
        title: None,
        version: None,
        revision: annotation.revision,
        can_edit: annotation.can_edit,
        evidence: Vec::new(),
        saved_copies: Vec::new(),
    };
    if let Some(reference) = annotation.cabinet_ref {
        match cabinet::read(
            state.sqlx_pool(),
            ReadRequest {
                scope: ActorScope::user(user),
                cabinet_ref: reference.clone(),
                version_ref: None,
            },
        )
        .await
        {
            Ok(page) => {
                visible.reference = Some(reference);
                visible.title = Some(page.item.title);
                visible.version = page.item.current_version;
            }
            Err(CabinetError::NotFound | CabinetError::NotAuthorized) => {}
            Err(failure) => return Err(error(failure)),
        }
    }
    for link in artifacts::list_docket_artifact_links(
        state.sqlx_pool(),
        bear.as_uuid(),
        DocketArtifactTargetKind::Job,
        job,
    )
    .await?
    {
        let reference = ArtifactRef::parse(&link.artifact_ref)?;
        let metadata = match artifacts::authorize_for_reader(
            state.sqlx_pool(),
            &reference,
            ArtifactReader::Human(user),
            ArtifactAccessLevel::Content,
        )
        .await
        {
            Ok(metadata) => metadata,
            Err(DenError::NotFound(_) | DenError::Authorization(_)) => continue,
            Err(failure) => return Err(failure.into()),
        };
        if metadata.kind == "cabinet_document_snapshot" {
            continue;
        }
        if metadata.storage_kind == ArtifactStorageKind::DbText {
            visible.evidence.push(EvidenceView {
                reference,
                title: metadata.title.unwrap_or_else(|| "Saved document".into()),
            });
        }
    }
    visible.saved_copies = artifacts::snapshot_retirement::history(
        state.sqlx_pool(),
        user,
        None,
        Some(bear),
        Some(job),
    )
    .await?
    .copies;
    Ok(visible)
}

async fn evidence_content(
    State(state): State<AppState>,
    session: AuthSession,
    Path((bear_slug, job_ref, reference)): Path<(String, String, ArtifactRef)>,
) -> Result<Response, CustomError> {
    let bear = super::bear_context(&state, &session, &bear_slug).await?;
    let job = super::resolve_job_prefix(state.sqlx_pool(), &bear, &job_ref).await?;
    let actor = UserId::new(super::require_user(&session)?);
    missions::get_for_viewer(
        state.sqlx_pool(),
        BearId::new(bear.id),
        JobReference(job),
        actor,
    )
    .await?;
    let links = artifacts::list_docket_artifact_links(
        state.sqlx_pool(),
        bear.id,
        DocketArtifactTargetKind::Job,
        job,
    )
    .await?;
    if !links
        .iter()
        .any(|link| link.artifact_ref == reference.as_str())
    {
        return Err(CustomError::NotFound("Job evidence unavailable".into()));
    }
    let value = artifacts::json_content_for_reader(
        state.sqlx_pool(),
        &reference,
        ArtifactReader::Human(actor),
    )
    .await?;
    missions::get_for_viewer(
        state.sqlx_pool(),
        BearId::new(bear.id),
        JobReference(job),
        actor,
    )
    .await?;
    artifacts::authorize_for_reader(
        state.sqlx_pool(),
        &reference,
        ArtifactReader::Human(actor),
        ArtifactAccessLevel::Content,
    )
    .await?;
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| CustomError::System("encode document evidence".into()))?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}.json\"", reference.as_str()),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(bytes))
        .map_err(|_| CustomError::System("document evidence response failed".into()))
}

async fn link(
    State(state): State<AppState>,
    session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
    Form(form): Form<LinkForm>,
) -> Result<Response, CustomError> {
    let bear = super::bear_context(&state, &session, &bear_slug).await?;
    let job = super::resolve_job_prefix(state.sqlx_pool(), &bear, &job_ref).await?;
    let actor = UserId::new(super::require_user(&session)?);
    let page = if form.cabinet_ref.trim().is_empty() {
        None
    } else {
        Some(
            CabinetItemRef::parse(form.cabinet_ref.trim())
                .map_err(|_| CustomError::ValidationError("choose a Cabinet page".into()))?,
        )
    };
    let mut tx = state.sqlx_pool().begin().await.map_err(DenError::from)?;
    cabinet::write_fence(&mut tx).await.map_err(error)?;
    missions::authorize_edit(&mut tx, BearId::new(bear.id), JobReference(job), actor).await?;
    if let Some(reference) = &page {
        cabinet::read(
            state.sqlx_pool(),
            ReadRequest {
                scope: ActorScope::user(actor),
                cabinet_ref: reference.clone(),
                version_ref: None,
            },
        )
        .await
        .map_err(error)?;
    }
    missions::set_in_tx(
        &mut tx,
        BearId::new(bear.id),
        JobReference(job),
        actor,
        page.as_ref(),
        form.revision,
    )
    .await?;
    tx.commit().await.map_err(DenError::from)?;
    Ok(Redirect::to(&format!(
        "/bear/{}/jobs/{}",
        bear.slug,
        super::route_id(job)
    ))
    .into_response())
}

async fn snapshot(
    State(state): State<AppState>,
    session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
    Form(form): Form<SnapshotForm>,
) -> Result<Response, CustomError> {
    let bear = super::bear_context(&state, &session, &bear_slug).await?;
    let job = super::resolve_job_prefix(state.sqlx_pool(), &bear, &job_ref).await?;
    let actor = UserId::new(super::require_user(&session)?);
    let mut tx = state.sqlx_pool().begin().await.map_err(DenError::from)?;
    artifacts::snapshot_retirement::lock_owner(&mut tx, actor, BearId::new(bear.id)).await?;
    cabinet::write_fence(&mut tx).await.map_err(error)?;
    missions::authorize_edit(&mut tx, BearId::new(bear.id), JobReference(job), actor).await?;
    let annotation = missions::get_for_viewer(
        state.sqlx_pool(),
        BearId::new(bear.id),
        JobReference(job),
        actor,
    )
    .await?;
    if annotation.revision != form.revision {
        return Err(CustomError::ValidationError(
            "Job page link changed; reload before capture".into(),
        ));
    }
    let page = annotation
        .cabinet_ref
        .ok_or_else(|| CustomError::NotFound("no accessible Mission page".into()))?;
    let artifact = cabinet::snapshots::capture_in_tx(
        &mut tx,
        state.sqlx_pool(),
        &ActorScope::user(actor),
        BearId::new(bear.id),
        &page,
        &form.version,
    )
    .await
    .map_err(error)?;
    artifacts::attach_docket_artifact_in_tx(
        &mut tx,
        artifacts::AttachDocketArtifactInput {
            artifact_ref: artifact.as_str().into(),
            bear_id: bear.id,
            target_kind: DocketArtifactTargetKind::Job,
            target_id: job,
            role: artifacts::DocketArtifactRole::Source,
            metadata: serde_json::json!({}),
            created_by_user_id: Some(actor.get()),
        },
    )
    .await?;
    tx.commit().await.map_err(DenError::from)?;
    Ok(Redirect::to(&format!(
        "/bear/{}/jobs/{}",
        bear.slug,
        super::route_id(job)
    ))
    .into_response())
}
