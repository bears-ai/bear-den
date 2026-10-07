//! Resolve exact Work or ordinary run authority before creating durable state.

use bearwire_protocol::session::ExpectedWorkSource;
use den_core::{
    ids::{BearId, UserId},
    ArmatureAvailability, TurnExecutionOrigin,
};
use den_docket::{
    work_runs::{self, WorkExecutionTarget, WorkRunRow, WorkRunState},
    DocketExecutionAttemptState, DocketExecutionBinding, DocketExecutionBindingKind,
    DocketExecutionGate, DocketExecutionHostKind, DocketFocusedExecutionBinding, DocketService,
    DocketWorkBoundaryCheck, PgDocketService,
};
use den_http::{armature_tokens, errors::CustomError};
use den_runtime::turn_ids::ClientSessionId;
use den_service::{
    bears::{
        db as bears_db,
        hats::{self, memory_binding, turn_binding::NativeTurnSource},
    },
    client_sessions,
    conversation::{
        persistence::ConversationRecord,
        viewer::{require_ordinary_tool_source, ConversationViewer},
    },
};
use serde::Deserialize;
use sqlx::{types::time::OffsetDateTime, PgPool};
use uuid::Uuid;

#[path = "run_source_preflight/publication.rs"]
pub(in crate::methods) mod publication;

#[path = "run_source_preflight/transcript.rs"]
mod transcript;
pub(super) use transcript::{require_existing_session, require_live_owned_transcript};

// Parse persisted control data once; runtime authority decisions use the existing enum.
fn parse_work_execution_target(
    target: &str,
    assigned_session: Option<&str>,
) -> Result<WorkExecutionTarget, CustomError> {
    match (target, assigned_session) {
        ("sandbox", None) => Ok(WorkExecutionTarget::Sandbox),
        ("attached_armature", Some(session)) => {
            let session =
                ClientSessionId::new(session.to_owned()).map_err(|_| changed_work_source())?;
            Ok(WorkExecutionTarget::AttachedArmature {
                client_session_id: session.to_string(),
            })
        }
        _ => Err(changed_work_source()),
    }
}

#[derive(Deserialize)]
struct DispatchedToken {
    armature_token_id: Uuid,
    armature_token_user_id: i32,
}

pub(super) fn unproven_active_work_turn() -> CustomError {
    CustomError::Authorization(
        "cannot prove the active turn admitted this exact Work source and fence".into(),
    )
}

fn changed_work_source() -> CustomError {
    CustomError::Authorization("expected checked-out Work source is unavailable or changed".into())
}

/// The expectation is a locator, never authority. All grants come from current Den state.
pub(in crate::methods) async fn require_expected_work_source(
    pool: &PgPool,
    bear: BearId,
    user: UserId,
    session_id: &str,
    expected: ExpectedWorkSource,
) -> Result<(), CustomError> {
    load_expected_work_source(pool, bear, user, session_id, expected).await?;
    Ok(())
}

pub(super) async fn load_expected_work_source(
    pool: &PgPool,
    bear: BearId,
    user: UserId,
    session_id: &str,
    expected: ExpectedWorkSource,
) -> Result<WorkRunRow, CustomError> {
    if expected.fence_epoch <= 0 {
        return Err(changed_work_source());
    }
    super::super::session::require_exclusive_client_session_id(
        pool,
        &ClientSessionId::new(session_id.to_owned())?,
        user,
        bear,
    )
    .await?;
    let run = work_runs::get_live_work_run_by_session(pool, session_id)
        .await?
        .ok_or_else(changed_work_source)?;
    if run.id != expected.work_run_id
        || run.bear_id != bear.as_uuid()
        || run.bearwire_session_id.as_deref() != Some(session_id)
        || run.cancel_requested
        || !matches!(
            run.state_enum(),
            Some(
                WorkRunState::Claimed
                    | WorkRunState::Provisioning
                    | WorkRunState::Running
                    | WorkRunState::Reporting
            )
        )
        || run
            .attached_client_session_id
            .as_deref()
            .is_some_and(|assigned| assigned != session_id)
    {
        return Err(changed_work_source());
    }
    let docket = PgDocketService::from_pool(pool);
    let job = docket
        .get_job(bear.as_uuid(), run.job_id)
        .await?
        .ok_or_else(changed_work_source)?;
    // Match native Work admission: membership/admin visibility is not authority
    // to infer as a different Job creator.
    if job.job.created_by_user_id != user.get()
        || job.job.current_run_id != Some(run.job_run_id)
        || job.job.lifecycle_intent.is_some()
        || !bears_db::user_may_use_bear(pool, user.get(), bear.as_uuid()).await?
    {
        return Err(changed_work_source());
    }
    let target = parse_work_execution_target(
        &run.execution_target,
        run.attached_client_session_id.as_deref(),
    )?;
    match target {
        WorkExecutionTarget::Sandbox => {
            // Dispatch persists the token and its actor before launching the sandbox.
            let token: DispatchedToken =
                serde_json::from_value(run.result_refs.clone().ok_or_else(changed_work_source)?)
                    .map_err(|_| changed_work_source())?;
            if token.armature_token_user_id != user.get() {
                return Err(changed_work_source());
            }
            let active = armature_tokens::list_for_user(pool, user.get())
                .await?
                .into_iter()
                .any(|current| {
                    current.id == token.armature_token_id
                        && current.bear_id == bear.as_uuid()
                        && current.revoked_at.is_none()
                        && current
                            .expires_at
                            .is_none_or(|expires| expires > OffsetDateTime::now_utc())
                        && armature_tokens::scopes_contains(
                            &current.scopes,
                            armature_tokens::armature_chat_scope(),
                        )
                });
            if !active {
                return Err(changed_work_source());
            }
        }
        WorkExecutionTarget::AttachedArmature { client_session_id } => {
            if client_session_id != session_id
                || client_sessions::find_for_user_bear_session_id(
                    pool,
                    user.get(),
                    bear.as_uuid(),
                    session_id,
                )
                .await?
                .is_none()
            {
                return Err(changed_work_source());
            }
        }
    }
    memory_binding::for_work_run(pool, bear, run.id).await?;
    let attempt = docket
        .get_live_focused_execution(
            bear.as_uuid(),
            DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::WorkAssignment,
                id: run.id.to_string(),
            },
        )
        .await?
        .ok_or_else(changed_work_source)?;
    if attempt.id != expected.execution_attempt_id
        || attempt.bear_id != bear.as_uuid()
        || attempt.fence_epoch != expected.fence_epoch
        || attempt.state != DocketExecutionAttemptState::Running
        || attempt.binding.kind != DocketExecutionBindingKind::WorkAssignment
        || Uuid::parse_str(&attempt.binding.id).ok() != Some(run.id)
        || attempt.host.kind != DocketExecutionHostKind::WorkRun
        || Uuid::parse_str(&attempt.host.run_id).ok() != Some(run.id)
        || attempt.authorization_key != run.id
        || run.executing_task_id != Some(attempt.task_id)
    {
        return Err(changed_work_source());
    }
    let gate = docket
        .check_work_boundary(DocketWorkBoundaryCheck {
            bear_id: bear.as_uuid(),
            attempt_id: expected.execution_attempt_id,
            fence_epoch: expected.fence_epoch,
            boundary_key: Uuid::new_v4(),
            signal: None,
        })
        .await?;
    if !matches!(gate, DocketExecutionGate::Allowed {
        task_id,
        binding: DocketExecutionBinding::WorkRun { work_run_id, job_run_id },
    } if task_id == attempt.task_id && work_run_id == run.id && job_run_id == run.job_run_id)
    {
        return Err(changed_work_source());
    }
    Ok(run)
}

pub(super) struct AdmittedRunSource {
    pub conversation: ConversationRecord,
    pub origin: TurnExecutionOrigin,
    pub turn_source: NativeTurnSource,
}

pub(super) async fn admit(
    pool: &PgPool,
    viewer: &ConversationViewer,
    bear: BearId,
    user: UserId,
    session_id: &str,
    external_id: &str,
    expected: Option<ExpectedWorkSource>,
) -> Result<AdmittedRunSource, CustomError> {
    let session = client_sessions::find_for_user_bear_session_id(
        pool,
        user.get(),
        bear.as_uuid(),
        session_id,
    )
    .await?;
    if let Some(session) = session.as_ref() {
        require_existing_session(pool, viewer, bear, user, session, Some(external_id)).await?;
    }
    let external_id = session
        .as_ref()
        .and_then(|session| session.resolved_conversation_id.as_deref())
        .unwrap_or(external_id);
    let work = match expected {
        Some(expected) => {
            Some(load_expected_work_source(pool, bear, user, session_id, expected).await?)
        }
        None => work_runs::get_live_work_run_by_session(pool, session_id).await?,
    };
    // Job/hat authority never grants ownership of an existing transcript.
    let existing_source = require_live_owned_transcript(pool, viewer, bear, external_id).await?;
    let initial_hat = if let Some(run) = &work {
        if run.bear_id != bear.as_uuid() || run.cancel_requested {
            return Err(CustomError::Authorization(
                "Work run is not active for this Bear".into(),
            ));
        }
        memory_binding::for_work_run(pool, bear, run.id).await?;
        None
    } else if existing_source.is_some() {
        require_ordinary_tool_source(pool, bear, user, external_id).await?;
        None
    } else {
        let hat = hats::ide_default_hat(pool, bear)
            .await?
            .ok_or_else(memory_binding::missing_binding)?;
        hats::manage::get_hat(pool, bear, hat).await?;
        Some(hat)
    };
    if let Some(expected) = expected {
        require_expected_work_source(pool, bear, user, session_id, expected).await?;
    }
    let conversation = match existing_source {
        Some(conversation) => conversation,
        None => {
            let authority = match (&work, initial_hat) {
                (Some(run), _) => publication::NewSourceAuthority::WorkRun(run.id),
                (None, Some(hat)) => publication::NewSourceAuthority::Ordinary(hat),
                (None, None) => return Err(memory_binding::missing_binding().into()),
            };
            let winner = publication::materialize_run_source(
                pool,
                publication::NewRunSource {
                    bear,
                    user,
                    session_id,
                    selection: external_id,
                    authority,
                    initial_mode: None,
                },
            )
            .await?;
            require_live_owned_transcript(pool, viewer, bear, &winner)
                .await?
                .ok_or_else(|| {
                    CustomError::Authorization("admitted transcript disappeared".into())
                })?
        }
    };
    let durable_id = conversation
        .external_conversation_id
        .as_deref()
        .ok_or_else(|| {
            CustomError::Authorization("admitted source has no external conversation ID".into())
        })?;
    let (origin, turn_source) = if let Some(run) = work {
        memory_binding::for_work_run(pool, bear, run.id).await?;
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            NativeTurnSource::WorkRun(run.id),
        )
    } else {
        require_ordinary_tool_source(pool, bear, user, durable_id).await?;
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            NativeTurnSource::Conversation(conversation.id),
        )
    };
    if let Some(expected) = expected {
        require_expected_work_source(pool, bear, user, session_id, expected).await?;
    }
    Ok(AdmittedRunSource {
        conversation,
        origin,
        turn_source,
    })
}

#[cfg(test)]
#[path = "run_source_preflight/expected_work_source_tests.rs"]
mod expected_work_source_tests;
#[cfg(test)]
#[path = "run_source_preflight/helper_tests.rs"]
mod helpers;
#[cfg(test)]
#[path = "run_source_preflight/startup_authority_tests.rs"]
mod startup_authority_tests;
