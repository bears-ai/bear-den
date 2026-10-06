//! Bounded automatic recovery plus an owner-only upload history/control surface.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use den_cabinet::{ActorScope, ReadRequest};
use den_core::ids::UserId;
use den_service::{
    artifacts::{cleanup as registry, ArtifactLifecycle, ArtifactRef},
    cabinet,
};
use minijinja::context;
use serde::Serialize;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{auth_backend::AuthSession, errors::CustomError, web, AppState};

const BATCH_SIZE: i64 = 10;

pub(crate) fn spawn(state: AppState) {
    if state.media.is_none() {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if run_batch(&state, OffsetDateTime::now_utc(), BATCH_SIZE)
                .await
                .is_err()
            {
                tracing::warn!("Cabinet upload cleanup batch needs retry");
            }
        }
    });
}

pub(crate) async fn run_batch(
    state: &AppState,
    now: OffsetDateTime,
    limit: i64,
) -> Result<usize, CustomError> {
    if state.media.is_none() {
        return Ok(0);
    }
    let tickets = registry::claim_due(state.sqlx_pool(), now, limit).await?;
    let mut removed = 0;
    for ticket in tickets {
        if process(state, &ticket).await.is_ok() {
            removed += 1;
        } else {
            tracing::warn!(artifact_ref = %ticket.reference().as_str(), "Cabinet upload byte cleanup needs retry");
        }
    }
    Ok(removed)
}

async fn process(state: &AppState, ticket: &registry::CleanupTicket) -> Result<(), CustomError> {
    let media = state
        .media
        .as_ref()
        .ok_or_else(|| CustomError::ValidationError("file storage is not configured".into()))?;
    media.remove_retired_artifact(ticket).await?;
    registry::acknowledge(state.sqlx_pool(), ticket, OffsetDateTime::now_utc()).await?;
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cabinet/uploads", get(history))
        .route("/cabinet/uploads/{artifact_ref}/cleanup", post(retry))
}

#[derive(Serialize)]
struct UploadView {
    reference: ArtifactRef,
    title: String,
    bear: String,
    state: &'static str,
    created_at: String,
    eligible_at: Option<String>,
    can_retry: bool,
    page_ref: Option<den_cabinet::CabinetItemRef>,
    page_title: Option<String>,
}

fn timestamp(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_else(|_| value.to_string())
}

async fn history(
    State(state): State<AppState>,
    session: AuthSession,
) -> Result<Response, CustomError> {
    let actor = UserId::new(
        session
            .user
            .as_ref()
            .ok_or_else(|| CustomError::Authentication("login required".into()))?
            .id,
    );
    let records = registry::history(state.sqlx_pool(), actor).await?;
    let now = OffsetDateTime::now_utc();
    let mut uploads = Vec::new();
    for record in records {
        let eligible_at = record
            .expires_at
            .map(|deadline| deadline + registry::WRITE_GRACE);
        let due = eligible_at.is_some_and(|deadline| deadline <= now)
            && !record.retained
            && record.content_removed_at.is_none();
        let status = if record.content_removed_at.is_some() {
            "File removed"
        } else if record.retained {
            "Kept by Cabinet"
        } else if due {
            "Cleanup pending"
        } else {
            match record.lifecycle {
                ArtifactLifecycle::Pending => "Upload not finished",
                ArtifactLifecycle::Finalized => "Waiting for retention or expiry",
                ArtifactLifecycle::Deleted | ArtifactLifecycle::Expired => {
                    "Not published; cleanup waiting"
                }
            }
        };
        let (page_ref, page_title) = if let Some(reference) = record.source.cabinet_ref {
            match cabinet::read(
                state.sqlx_pool(),
                ReadRequest {
                    scope: ActorScope::user(actor),
                    cabinet_ref: reference.clone(),
                    version_ref: None,
                },
            )
            .await
            {
                Ok(page) => (Some(reference), Some(page.item.title)),
                Err(
                    den_cabinet::CabinetError::NotFound | den_cabinet::CabinetError::NotAuthorized,
                ) => (None, None),
                Err(error) => return Err(den_core::DenError::from(error).into()),
            }
        } else {
            (None, None)
        };
        uploads.push(UploadView {
            reference: record.reference,
            title: record.title.unwrap_or_else(|| "Upload".into()),
            bear: record.bear_name,
            state: status,
            created_at: timestamp(record.created_at),
            eligible_at: eligible_at.map(timestamp),
            can_retry: due && state.media.is_some(),
            page_ref,
            page_title,
        });
    }
    let mut response = web::render_template(
        &state,
        "cabinet/uploads.html",
        session,
        context! {
            title => "Your uploads", uploads, storage_enabled => state.media.is_some(),
        },
    )
    .await?;
    super::attachments::protect_response(&mut response);
    Ok(response)
}

async fn retry(
    State(state): State<AppState>,
    session: AuthSession,
    Path(reference): Path<ArtifactRef>,
) -> Result<Response, CustomError> {
    let actor = UserId::new(
        session
            .user
            .as_ref()
            .ok_or_else(|| CustomError::Authentication("login required".into()))?
            .id,
    );
    if state.media.is_none() {
        return Err(CustomError::ValidationError(
            "file storage is not configured".into(),
        ));
    }
    let ticket = registry::claim_owned(
        state.sqlx_pool(),
        OffsetDateTime::now_utc(),
        actor,
        &reference,
    )
    .await?;
    process(&state, &ticket).await?;
    Ok(Redirect::to("/cabinet/uploads").into_response())
}
