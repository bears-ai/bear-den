use super::{authority, session_review::PendingReview, CommittedDecision, ReviewNonce};
use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};
use axum::{
    http::{header, StatusCode},
    response::Response,
};
use den_memory::identity_reconnection::{IdentityReceiptHistory, UserIdentitySummary};
use den_service::bears::Bear;
use minijinja::context;

fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}

pub(super) async fn list(
    state: &AppState,
    auth: AuthSession,
    bear: &Bear,
    identities: Vec<UserIdentitySummary>,
    history: Option<IdentityReceiptHistory>,
    accounts: Vec<authority::AccountLabel>,
    prepare_token: ReviewNonce,
) -> Result<Response, CustomError> {
    Ok(private(
        web::render_template(
            state,
            "bear/manage/identity_reconnection.html",
            auth,
            context! {
                identities, history, accounts, prepare_token => prepare_token.0,
                receipt_limit => super::RECENT_RECEIPT_LIMIT,
                can_manage_bear => true, manage_title => "Imported identities",
                ..super::super::bear_nav_context(bear, "portability"),
            },
        )
        .await?,
    ))
}

/// Only this scoped validation boundary offers recovery wording. Never disclose the cause.
pub(super) async fn validation_failure(
    state: &AppState,
    auth: AuthSession,
    slug: &str,
    error: CustomError,
) -> Result<Response, CustomError> {
    match error {
        CustomError::ValidationError(_) => {
            let mut response = web::render_template(
                state,
                "bear/manage/identity_reconnection_unavailable.html",
                auth,
                context! { bear_slug => slug },
            )
            .await?;
            *response.status_mut() = StatusCode::BAD_REQUEST;
            Ok(private(response))
        }
        other => Err(other),
    }
}

struct ReviewDisplay {
    identity: UserIdentitySummary,
    actor: authority::AccountLabel,
    destination: Option<authority::AccountLabel>,
}

async fn inspect_review(
    state: &AppState,
    bear: &Bear,
    pending: &PendingReview,
) -> Result<ReviewDisplay, CustomError> {
    let expected = &pending.expected;
    let locked = authority::lock(
        state.sqlx_pool(),
        expected.bear_id(),
        expected.actor_user_id(),
        expected.destination_user_id(),
    )
    .await?;
    let store = state.memory_stores()?.store_for_bear(bear.id).await?;
    let identities = den_memory::identity_reconnection::list_identities(&store).await?;
    pending.metadata.authorize(&locked, expected, &identities)?;
    let identity = identities
        .into_iter()
        .find(|identity| identity.entity_id == expected.entity_id())
        .ok_or_else(super::session_review::unavailable)?;
    let display = ReviewDisplay {
        identity,
        actor: locked.actor.clone(),
        destination: locked.destination.clone(),
    };
    drop(locked);
    Ok(display)
}

pub(super) async fn review(
    state: &AppState,
    auth: AuthSession,
    bear: &Bear,
    pending: &PendingReview,
    error: Option<String>,
) -> Result<Response, CustomError> {
    let ReviewDisplay {
        identity,
        actor,
        destination,
    } = match inspect_review(state, bear, pending).await {
        Ok(display) => display,
        Err(error) => return validation_failure(state, auth, &bear.slug, error).await,
    };
    let expected = &pending.expected;
    let failed = error.is_some();
    let mut response = web::render_template(
        state,
        "bear/manage/identity_reconnection_review.html",
        auth,
        context! {
            identity, actor, destination, error,
            reconnecting => expected.destination_user_id().is_some(),
            displaced_entity_id => expected.displaced_entity_id(),
            nonce => pending.nonce.0,
            confirmation_token => pending.confirmation_token.0,
            review_id => expected.id(), expires_at_unix => expected.expires_at_unix(),
            can_manage_bear => true, manage_title => "Review identity mapping",
            ..super::super::bear_nav_context(bear, "portability"),
        },
    )
    .await?;
    if failed {
        *response.status_mut() = StatusCode::BAD_REQUEST;
    }
    Ok(private(response))
}

pub(super) async fn receipt(
    state: &AppState,
    auth: AuthSession,
    bear: &Bear,
    committed: &CommittedDecision,
) -> Result<Response, CustomError> {
    let receipt = &committed.receipt;
    let outcome = if receipt.destination_user_id.is_some() {
        if receipt.displaced_entity_id.is_some() {
            "Identity reconnected. The destination account's previous identity is parked with all records and relations preserved. No facts were lost or combined; that identity can be reconnected later."
        } else {
            "Identity reconnected to the reviewed account. Other imported people remain separate; only eligible memory for this person can be recalled."
        }
    } else {
        "Mapping revoked. The identity is parked and its personal audience stays closed until a new authorized reconnection. All records and relations are preserved."
    };
    Ok(private(
        web::render_template(
            state,
            "bear/manage/identity_reconnection_receipt.html",
            auth,
            context! {
                receipt, actor => committed.actor, destination => committed.destination, outcome,
                can_manage_bear => true, manage_title => "Identity mapping recorded",
                ..super::super::bear_nav_context(bear, "portability"),
            },
        )
        .await?,
    ))
}
