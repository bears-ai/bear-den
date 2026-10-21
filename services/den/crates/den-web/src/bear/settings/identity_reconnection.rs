//! Human-only portable identity review; never part of the model tool surface.
mod authority;
mod export;
mod presentation;
mod session_review;
mod views;

pub(super) use export::record_origins;

use super::{load_session_bear_manage, session_user};
use crate::{auth_backend::AuthSession, errors::CustomError, web::AppState};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use axum_login::tower_sessions::Session;
use den_core::ids::{BearId, UserId};
use den_memory::identity_reconnection as core;
use presentation::ReviewedMetadata;
use serde::Deserialize;
use session_review::{
    PendingReview, PrepareToken, ReviewNonce, ReviewScope, PREPARE_KEY, REVIEW_KEY,
};

const RECENT_RECEIPT_LIMIT: u32 = 100;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bear/{slug}/portability/identities", get(list))
        .route_with_tsr("/bear/{slug}/portability/identities/prepare", post(prepare))
        .route_with_tsr(
            "/bear/{slug}/portability/identities/review/{nonce}",
            get(review),
        )
        .route_with_tsr("/bear/{slug}/portability/identities/confirm", post(confirm))
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Reconnect,
    Revoke,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareForm {
    prepare_token: ReviewNonce,
    action: Action,
    entity_id: String,
    destination_user_id: Option<UserId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmForm {
    nonce: ReviewNonce,
    confirmation_token: ReviewNonce,
    #[serde(default)]
    confirm_audience: bool,
    #[serde(default)]
    confirm_replacement: bool,
}

async fn list(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let actor = UserId::new(session_user(&auth).await?.id);
    let bear_id = BearId::new(bear.id);
    let mut locked = authority::lock(state.sqlx_pool(), bear_id, actor, None).await?;
    let store = state.memory_stores()?.store_for_bear(bear.id).await?;
    let identities = core::list_identities(&store).await?;
    // Imported history is not authority: even a history read failure must leave
    // current identities and their preparation token available for management.
    let history = core::receipt_history(&store, RECENT_RECEIPT_LIMIT)
        .await
        .ok();
    let accounts = locked.eligible_accounts().await?;
    let token = PrepareToken::new(ReviewScope::new(
        bear_id,
        actor,
        session_review::session_id(&session)?,
    ));
    session.insert(PREPARE_KEY, &token).await?;
    drop(locked);
    views::list(
        &state,
        auth,
        &bear,
        identities,
        history,
        accounts,
        token.nonce,
    )
    .await
}

async fn prepare(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
    Form(form): Form<PrepareForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let actor = UserId::new(session_user(&auth).await?.id);
    let nonce = match prepare_review(&state, BearId::new(bear.id), actor, &session, form).await {
        Ok(nonce) => nonce,
        Err(error) => return views::validation_failure(&state, auth, &bear.slug, error).await,
    };
    Ok(Redirect::to(&format!(
        "/bear/{}/portability/identities/review/{}",
        bear.slug, nonce.0
    ))
    .into_response())
}

async fn prepare_review(
    state: &AppState,
    bear_id: BearId,
    actor: UserId,
    session: &Session,
    form: PrepareForm,
) -> Result<ReviewNonce, CustomError> {
    let session_id = session_review::session_id(session)?;
    let token = session
        .get::<PrepareToken>(PREPARE_KEY)
        .await?
        .ok_or_else(session_review::unavailable)?;
    token.authorize(form.prepare_token, bear_id, actor, &session_id)?;
    if form.entity_id.is_empty() || form.entity_id.len() > 128 {
        return Err(CustomError::ValidationError(
            "Choose a person from this Bear's identity list.".into(),
        ));
    }
    let destination = match form.action {
        Action::Reconnect => Some(form.destination_user_id.unwrap_or(actor)),
        Action::Revoke if form.destination_user_id.is_none() => None,
        Action::Revoke => {
            return Err(CustomError::ValidationError(
                "Revocation does not select a new destination account.".into(),
            ))
        }
    };
    let locked = authority::lock(state.sqlx_pool(), bear_id, actor, destination).await?;
    let store = state
        .memory_stores()?
        .store_for_bear(bear_id.as_uuid())
        .await?;
    // Bracket core review generation with metadata snapshots. Epochs rule out an
    // ABA change, so the eventual page cannot mix a core snapshot with newer labels.
    let identities = core::list_identities(&store).await?;
    let metadata = ReviewedMetadata::capture(
        locked.actor.clone(),
        locked.destination.clone(),
        &form.entity_id,
        &identities,
    )?;
    let expected = match destination {
        Some(destination) => {
            core::review_reconnection(&store, actor, &form.entity_id, destination).await?
        }
        None => core::review_revocation(&store, actor, &form.entity_id).await?,
    };
    metadata.authorize(&locked, &expected, &core::list_identities(&store).await?)?;
    let pending = PendingReview::new(
        ReviewScope::new(bear_id, actor, session_id),
        expected,
        metadata,
    );
    // No expected versions, actor, binding, or core review are accepted from the browser.
    pending.authorize(
        pending.nonce,
        bear_id,
        actor,
        &session_review::session_id(session)?,
    )?;
    if pending.expected.entity_id() != form.entity_id
        || pending.expected.destination_user_id() != destination
    {
        return Err(CustomError::System(
            "Identity review did not match the selected operation.".into(),
        ));
    }
    session.insert(REVIEW_KEY, &pending).await?;
    drop(locked);
    Ok(pending.nonce)
}

async fn pending(
    session: &Session,
    nonce: ReviewNonce,
    bear_id: BearId,
    actor: UserId,
) -> Result<PendingReview, CustomError> {
    let pending = session
        .get::<PendingReview>(REVIEW_KEY)
        .await?
        .ok_or_else(session_review::unavailable)?;
    pending.authorize(nonce, bear_id, actor, &session_review::session_id(session)?)?;
    Ok(pending)
}

async fn review(
    Path((slug, nonce)): Path<(String, ReviewNonce)>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let actor = UserId::new(session_user(&auth).await?.id);
    let pending = match pending(&session, nonce, BearId::new(bear.id), actor).await {
        Ok(pending) => pending,
        Err(error) => return views::validation_failure(&state, auth, &bear.slug, error).await,
    };
    views::review(&state, auth, &bear, &pending, None).await
}

async fn confirm(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
    Form(form): Form<ConfirmForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let actor = UserId::new(session_user(&auth).await?.id);
    let pending = match pending(&session, form.nonce, BearId::new(bear.id), actor).await {
        Ok(pending) => pending,
        Err(error) => return views::validation_failure(&state, auth, &bear.slug, error).await,
    };
    let consent = match pending.consent(
        form.confirmation_token,
        form.confirm_audience,
        form.confirm_replacement,
    ) {
        Ok(consent) => consent,
        Err(CustomError::ValidationError(error)) => {
            return views::review(&state, auth, &bear, &pending, Some(error)).await
        }
        Err(error) => return Err(error),
    };

    // Dropping an HTTP request must not release Postgres authority locks while a
    // SQLite commit continues. The core's durable review UUID handles replay races;
    // session remove/get is intentionally not used as an exactly-once mechanism.
    let committed = tokio::spawn(commit_locked(
        state.clone(), pending, actor, session_review::session_id(&session)?, consent,
    ))
    .await
    .map_err(|_| CustomError::System("Identity confirmation outcome could not be read. Refresh the identity list before attempting another review.".into()))?;
    let committed = match committed {
        Ok(committed) => committed,
        Err(error) => return views::validation_failure(&state, auth, &bear.slug, error).await,
    };
    views::receipt(&state, auth, &bear, &committed).await
}

struct CommittedDecision {
    receipt: core::IdentityReconnectionReceipt,
    actor: authority::AccountLabel,
    destination: Option<authority::AccountLabel>,
}

async fn commit_locked(
    state: AppState,
    pending: PendingReview,
    actor: UserId,
    session_id: String,
    consent: core::ReplacementConsent,
) -> Result<CommittedDecision, CustomError> {
    let locked = authority::lock(
        state.sqlx_pool(),
        pending.expected.bear_id(),
        actor,
        pending.expected.destination_user_id(),
    )
    .await?;
    let store = state
        .memory_stores()?
        .store_for_bear(pending.expected.bear_id().as_uuid())
        .await?;
    pending.authorize(
        pending.nonce,
        pending.expected.bear_id(),
        actor,
        &session_id,
    )?;
    pending.metadata.authorize(
        &locked,
        &pending.expected,
        &core::list_identities(&store).await?,
    )?;
    let receipt = core::commit_review(&store, &pending.expected, consent).await?;
    let committed = CommittedDecision {
        receipt,
        actor: locked.actor.clone(),
        destination: locked.destination.clone(),
    };
    drop(locked);
    Ok(committed)
}

#[cfg(test)]
mod tests;
