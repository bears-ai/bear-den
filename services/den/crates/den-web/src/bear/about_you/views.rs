use axum::{
    http::{header, StatusCode},
    response::Response,
};
use den_service::{
    bears::Bear,
    personal_memory::{
        CandidateReviewDecision, CandidateState, PersonalFactState, PersonalMemoryInspection,
    },
};
use minijinja::context;
use serde::Serialize;
use uuid::Uuid;

use super::session_state::{ActionNonce, Intent, Registry, Scope};
use crate::{
    auth_backend::AuthSession, bear::settings::bear_nav_context, errors::CustomError, AppState,
};

pub(super) async fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}

#[derive(Serialize)]
struct CurrentView {
    state: &'static str,
    name: Option<String>,
    retract: Option<ActionNonce>,
}

#[derive(Serialize)]
struct ReviewView {
    token: ActionNonce,
    label: &'static str,
    kind: &'static str,
}

#[derive(Serialize)]
struct CandidateView {
    id: String,
    name: String,
    state: &'static str,
    approve: Option<ActionNonce>,
    reviews: Vec<ReviewView>,
}

fn candidate_state(state: CandidateState) -> &'static str {
    match state {
        CandidateState::Pending => "Awaiting your approval",
        CandidateState::Deferred => "Deferred",
        CandidateState::Rejected => "Rejected",
        CandidateState::Expired => "Closed as expired",
        CandidateState::Withdrawn => "Withdrawn",
        CandidateState::Quarantined => "Quarantined; a fresh request is needed before approval",
        CandidateState::Published => {
            "Previously approved; see the current name above for active status"
        }
    }
}

pub(super) async fn page(
    state: &AppState,
    auth: AuthSession,
    bear: &Bear,
    can_manage_bear: bool,
    inspection: PersonalMemoryInspection,
    scope: Scope,
    registry: &Registry,
) -> Result<Response, CustomError> {
    let mut actions = Vec::new();
    let mut token = |intent| {
        let nonce = ActionNonce::new();
        actions.push((nonce, intent));
        nonce
    };
    let expected_head = inspection.current.as_ref().map(|head| head.fact_id);
    let capture_token = token(Intent::Capture(!inspection.capture_enabled));
    let request_token = inspection.capture_enabled.then(|| {
        token(Intent::Request {
            request_id: Uuid::new_v4(),
        })
    });
    let current = inspection.current.map(|head| CurrentView {
        state: match head.state {
            PersonalFactState::Active => "Active",
            PersonalFactState::Retracted => "Retracted",
            PersonalFactState::Suppressed => "Unavailable for use",
        },
        name: head
            .preferred_name
            .filter(|_| head.state == PersonalFactState::Active)
            .map(|name| name.as_str().to_owned()),
        retract: (head.state != PersonalFactState::Retracted).then(|| {
            token(Intent::Retract {
                expected_head: head.fact_id,
            })
        }),
    });
    let candidates: Vec<_> = inspection
        .candidates
        .into_iter()
        .take(100)
        .map(|candidate| {
            let id = candidate.id();
            let candidate_state_value = candidate.state();
            let mut reviews = Vec::new();
            let mut review = |decision, label, kind| {
                reviews.push(ReviewView {
                    token: token(Intent::Review {
                        candidate_id: id,
                        decision,
                        expected_review: candidate.latest_review_id(),
                    }),
                    label,
                    kind,
                });
            };
            if matches!(
                candidate_state_value,
                CandidateState::Pending | CandidateState::Deferred | CandidateState::Quarantined
            ) {
                review(CandidateReviewDecision::Reject, "Reject request", "reject");
                review(
                    CandidateReviewDecision::Expire,
                    "Close as expired",
                    "expire",
                );
            }
            if inspection.capture_enabled {
                match candidate_state_value {
                    CandidateState::Pending => {
                        review(CandidateReviewDecision::Defer, "Defer request", "defer");
                    }
                    CandidateState::Deferred => {
                        review(CandidateReviewDecision::Resume, "Resume review", "resume");
                    }
                    _ => {}
                }
            }
            CandidateView {
                id: id.to_string(),
                name: candidate.preferred_name().as_str().to_owned(),
                state: candidate_state(candidate_state_value),
                approve: (inspection.capture_enabled
                    && matches!(
                        candidate_state_value,
                        CandidateState::Pending | CandidateState::Deferred
                    ))
                .then(|| {
                    token(Intent::Publish {
                        candidate_id: id,
                        expected_head,
                    })
                }),
                reviews,
            }
        })
        .collect();
    if let Err(message) = registry.register(scope, inspection.owner, actions) {
        return failure(
            state,
            auth,
            bear,
            can_manage_bear,
            StatusCode::SERVICE_UNAVAILABLE,
            message,
        )
        .await;
    }
    crate::render_template(state, "bear/memory/about_you.html", auth, context! {
        can_manage_bear, manage_title => "About you", capture_enabled => inspection.capture_enabled,
        capture_token, request_token, current, candidates,
        ..bear_nav_context(bear, "memory"),
    }).await
}

pub(super) async fn failure(
    state: &AppState,
    auth: AuthSession,
    bear: &Bear,
    can_manage_bear: bool,
    status: StatusCode,
    message: &str,
) -> Result<Response, CustomError> {
    // No reinspection or retained values are rendered on a rejected action.
    let mut response = crate::render_template(
        state,
        "bear/memory/about_you.html",
        auth,
        context! {
            can_manage_bear, manage_title => "About you", error => message,
            ..bear_nav_context(bear, "memory"),
        },
    )
    .await?;
    *response.status_mut() = status;
    Ok(response)
}
