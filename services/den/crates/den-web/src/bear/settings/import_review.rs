//! Review-before-create edge. The client submits only a nonce and consent, never
//! a file path, a manifest, or authority to restore destination grants.
use super::import_outcome::{CreationFailure, CreationStage};
use super::import_staging::{PendingReview, ReviewNonce, StagedFile, VerifiedReview, REVIEW_KEY};
use super::{
    portable_models, pretty_json, session_user, BearBundleManifest, BEAR_BUNDLE_MAX_UPLOAD_BYTES,
};
use crate::{
    auth_backend::AuthSession,
    bear::member::email_verify_redirect,
    errors::CustomError,
    web::{self, AppState},
};
use axum::{
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use axum_login::tower_sessions::Session;
use den_core::ids::UserId;
use minijinja::context;
use serde::Deserialize;
use uuid::Uuid;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr(
            "/bears/import",
            post(upload).layer(axum::extract::DefaultBodyLimit::max(
                BEAR_BUNDLE_MAX_UPLOAD_BYTES + 64 * 1024,
            )),
        )
        .route_with_tsr("/bears/import/{nonce}", get(review))
        .route_with_tsr("/bears/import/{nonce}/confirm", post(confirm))
        .route_with_tsr("/bears/import/{nonce}/cancel", post(cancel))
}

fn session_id(session: &Session) -> Result<String, CustomError> {
    session
        .id()
        .map(|id| id.to_string())
        .ok_or_else(|| CustomError::Authentication("Sign in again before importing.".into()))
}

async fn pending(
    session: &Session,
    actor: UserId,
    nonce: ReviewNonce,
) -> Result<PendingReview, CustomError> {
    let pending = session
        .get::<PendingReview>(REVIEW_KEY)
        .await
        .map_err(|error| CustomError::Session(format!("Read import review: {error}")))?
        .ok_or_else(|| {
            CustomError::ValidationError(
                "No import review exists in this session. Upload the bundle again.".into(),
            )
        })?;
    pending.authorize(actor, &session_id(session)?, nonce)?;
    Ok(pending)
}

async fn upload(
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
    mut multipart: Multipart,
) -> Result<Response, CustomError> {
    let actor = UserId::new(session_user(&auth_session).await?.id);
    if let Some(redirect) = email_verify_redirect(state.sqlx_pool(), actor.get()).await? {
        return Ok(redirect.into_response());
    }
    let id = session_id(&session)?;
    let (staged, mut file) = StagedFile::reserve(&state.config)?;
    let mut size = 0;
    let mut found = false;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| CustomError::ValidationError(format!("Invalid .bear upload: {error}")))?
    {
        if field.name() != Some("bundle") {
            continue;
        }
        if found {
            return Err(CustomError::ValidationError(
                "Select exactly one bundle.".into(),
            ));
        }
        found = true;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| CustomError::ValidationError(format!("Read .bear upload: {error}")))?
        {
            super::import_staging::write_chunk(&mut file, &mut size, &chunk)?;
        }
    }
    if !found || size == 0 {
        return Err(CustomError::ValidationError(
            "Please select a .bear bundle.".into(),
        ));
    }
    file.sync_all()?;
    drop(file);
    let ((mut staged, hash), permit) = super::import_jobs::run(move || {
        let (_, hash) = staged.preview()?;
        Ok((staged, hash))
    })
    .await?;
    drop(permit);
    // A replacement upload retires this session's older review without touching
    // any other actor's pending files. Concurrent uploads are still bounded.
    if let Some(old) = session
        .get::<PendingReview>(REVIEW_KEY)
        .await
        .map_err(|error| CustomError::Session(error.to_string()))?
    {
        if old.authorize(actor, &id, old.nonce).is_ok() {
            let _ = old.discard(&state.config);
        }
    }
    let pending = staged.finish(actor, id, hash)?;
    if let Err(error) = session.insert(REVIEW_KEY, &pending).await {
        return Err(CustomError::Session(format!("Save import review: {error}")));
    }
    staged.retain_pending();
    Ok(Redirect::to(&format!("/bears/import/{}", pending.nonce.0)).into_response())
}

async fn compatibility_errors(state: &AppState, manifest: &BearBundleManifest) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(configurations) = manifest.model_configurations.as_deref() {
        for configuration in configurations {
            if let Err(error) =
                den_service::bears::model_configurations::validate_model_configuration(
                    state.sqlx_pool(),
                    &configuration.model_handle,
                    configuration.thinking_effort,
                )
                .await
            {
                errors.push(format!(
                    "{} ({}): {error}",
                    configuration.name,
                    configuration.model_handle,
                    error = safe_catalog_error(error.into())
                ));
            }
        }
    } else if let Err(error) = portable_models::validate_catalog(
        state.sqlx_pool(),
        None,
        manifest.bear.default_model.as_deref(),
    )
    .await
    {
        errors.push(safe_catalog_error(error));
    }
    errors
}

async fn render_review(
    state: &AppState,
    auth_session: AuthSession,
    pending: &PendingReview,
    manifest: &BearBundleManifest,
    error: Option<String>,
    consumed: bool,
    creation_failure: Option<&CreationFailure>,
) -> Result<Response, CustomError> {
    let compatibility_errors = if creation_failure.is_none() {
        compatibility_errors(state, manifest).await
    } else {
        Vec::new()
    };
    let status = if creation_failure.is_some() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else if error.is_some() || !compatibility_errors.is_empty() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    let can_confirm = !consumed && compatibility_errors.is_empty();
    let response = web::render_template(state, "bear/manage/import_review.html", auth_session, context! {
        manifest, nonce => pending.nonce.0.to_string(), compatibility_errors, error,
        can_confirm, consumed, creation_failure,
        compatibility_checked => creation_failure.is_none(),
        creation_stage => creation_failure.map(|failure| failure.stage().label()),
        context_profile => manifest.prompts.context_profile.as_ref().map(|value| pretty_json(value.clone())),
        legacy_profiles => pretty_json(manifest.profiles.clone()),
    }).await?;
    let mut response = (status, response).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

async fn review(
    Path(nonce): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let actor = UserId::new(session_user(&auth_session).await?.id);
    if let Some(redirect) = email_verify_redirect(state.sqlx_pool(), actor.get()).await? {
        return Ok(redirect.into_response());
    }
    let pending = pending(&session, actor, ReviewNonce(nonce)).await?;
    let (read, manifest) = inspect(&pending, &state).await?;
    let response =
        render_review(&state, auth_session, &pending, &manifest, None, false, None).await?;
    // The response has rendered successfully from verified bytes. Record only
    // this nonce's receipt; a late GET A must never replace session pointer B.
    read.mark_displayed()?;
    Ok(response)
}

#[derive(Deserialize, Default)]
struct ConfirmForm {
    #[serde(default)]
    confirm_imported_knowledge: bool,
}

async fn confirm(
    Path(nonce): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
    Form(form): Form<ConfirmForm>,
) -> Result<Response, CustomError> {
    let actor = UserId::new(session_user(&auth_session).await?.id);
    if let Some(redirect) = email_verify_redirect(state.sqlx_pool(), actor.get()).await? {
        return Ok(redirect.into_response());
    }
    let pending = pending(&session, actor, ReviewNonce(nonce)).await?;
    if !form.confirm_imported_knowledge {
        let (_read, manifest) = inspect(&pending, &state).await?;
        return render_review(&state, auth_session, &pending, &manifest,
            Some("Nothing was imported. Acknowledge the identity, private-memory boundary and knowledge audience before creating a Bear.".into()), false, None).await;
    }
    // Claim precedes all creation. Only the winner can read these reviewed bytes
    // and create; session-store last-write-wins cannot authorize a second import.
    let claimed = pending.claim(&state.config)?;
    let reviewed = pending.clone();
    let ((claimed, manifest, memory), permit) = super::import_jobs::run(move || {
        let (manifest, memory) = reviewed.read_claimed(&claimed)?;
        Ok((claimed, manifest, memory))
    })
    .await?;
    if let Err(error) = portable_models::validate_catalog(
        state.sqlx_pool(),
        manifest.model_configurations.as_deref(),
        manifest.bear.default_model.as_deref(),
    )
    .await
    {
        return render_review(&state, auth_session, &pending, &manifest,
            Some(format!("Nothing was imported. Destination compatibility changed or is unavailable: {}. Upload again after repairing the catalog; no model was remapped.", safe_catalog_error(error))), true, None).await;
    }
    let displayed_manifest = manifest.clone();
    let creation_state = state.clone();
    // Request cancellation must not abandon a partially created Bear or release
    // the active claim/job permit while setup and compensation are still running.
    let creation = tokio::spawn(async move {
        let result = super::import_creation::create(&creation_state, actor, manifest, memory).await;
        drop(claimed);
        drop(permit);
        result
    })
    .await;
    let slug = match creation {
        Ok(Ok(slug)) => slug,
        result => {
            let failure = match result {
                Ok(Err(failure)) => failure,
                _ => CreationFailure::unconfirmed(CreationStage::Creation),
            };
            return render_review(
                &state,
                auth_session,
                &pending,
                &displayed_manifest,
                None,
                true,
                Some(&failure),
            )
            .await;
        }
    };
    Ok(Redirect::to(&format!("/bear/{slug}/overview?message={}", urlencoding::encode(
        "Bear imported as a new Bear. Review all imported knowledge and procedures before granting members or enabling Work; no live grants were restored."
    ))).into_response())
}

async fn cancel(
    Path(nonce): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let actor = UserId::new(session_user(&auth_session).await?.id);
    if let Some(redirect) = email_verify_redirect(state.sqlx_pool(), actor.get()).await? {
        return Ok(redirect.into_response());
    }
    let pending = pending(&session, actor, ReviewNonce(nonce)).await?;
    pending.discard(&state.config)?;
    Ok(Redirect::to("/").into_response())
}

async fn inspect(
    pending: &PendingReview,
    state: &AppState,
) -> Result<(VerifiedReview, BearBundleManifest), CustomError> {
    let read = pending.open_review(&state.config)?;
    let ((read, manifest), permit) = super::import_jobs::run(move || read.verify()).await?;
    drop(permit);
    Ok((read, manifest))
}

fn safe_catalog_error(error: CustomError) -> String {
    match error {
        CustomError::ValidationError(message) | CustomError::NotFound(message) => message,
        _ => "The destination model catalog is unavailable or could not validate this choice. Ask a Den operator to repair it, then review again.".into(),
    }
}
