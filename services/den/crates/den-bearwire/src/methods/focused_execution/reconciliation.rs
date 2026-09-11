use bearwire_protocol::lifecycle::FocusedExecutionTransitionReason;
use den_docket::{
    DocketExecutionAttemptRelease, DocketExecutionBindingKind, DocketExecutionHostKind,
    DocketService, PgDocketService,
};
use den_http::errors::CustomError;
use den_runtime::turn_runs;
use den_service::DenState;
use serde_json::json;
use uuid::Uuid;

use super::{
    load_focused_execution_snapshot, project_execution_authority_ended,
    FocusedExecutionInvariantViolation, FocusedExecutionLaunchState, FocusedExecutionState,
};

pub(crate) enum StartReconciliation {
    AlreadyRunning,
    Launch { recovered_run_id: Option<String> },
}

pub(crate) async fn reconcile_before_start(
    state: &DenState,
    user_id: i32,
    bear_id: Uuid,
    task_id: Uuid,
    session_id: &str,
) -> Result<StartReconciliation, CustomError> {
    let docket = PgDocketService::from_pool(&state.sqlx_pool);
    let mut recovered_run_id = None;
    if let Some(run) = turn_runs::active_run_for_session(&state.sqlx_pool, session_id)
        .await?
        .filter(|run| run.bear_id == bear_id && run.user_id == user_id)
    {
        let controller_is_live = state
            .turn_cancellations
            .active_for_run(session_id, &run.run_id)
            .is_some();
        if !controller_is_live {
            reconcile_orphaned_run(state, user_id, bear_id, task_id, session_id, &run.run_id)
                .await?;
            recovered_run_id = Some(run.run_id);
        } else if docket
            .get_live_session_task_execution_attempt(bear_id, task_id, session_id, &run.run_id)
            .await?
            .is_some()
        {
            return Ok(StartReconciliation::AlreadyRunning);
        }
    }

    if let Some(attempt) = docket
        .get_live_session_task_execution_attempt_for_session(bear_id, session_id)
        .await?
    {
        if attempt.binding.kind != DocketExecutionBindingKind::ClientSession
            || attempt.host.kind != DocketExecutionHostKind::TurnRun
        {
            return Err(CustomError::ValidationError(
                "live session execution authority has an incompatible binding or host".to_string(),
            ));
        }
        let run_id = attempt.host.run_id.clone();
        let run = turn_runs::get_run(&state.sqlx_pool, &run_id)
            .await?
            .filter(|run| run.bear_id == bear_id && run.user_id == user_id);
        let controller_is_live = state
            .turn_cancellations
            .active_for_run(session_id, &run_id)
            .is_some();
        let run_is_live = run
            .as_ref()
            .map(|run| run.state_value())
            .transpose()?
            .is_some_and(|state| !state.is_terminal());
        if controller_is_live && run_is_live {
            if attempt.task_id == task_id {
                return Ok(StartReconciliation::AlreadyRunning);
            }
        } else {
            release_stale_attempt(
                state,
                user_id,
                bear_id,
                session_id,
                &attempt,
                FocusedExecutionTransitionReason::StaleSessionAuthorityReleased,
                "stale_focused_execution_attempt_terminal_or_missing_run",
            )
            .await?;
        }
    }

    if let Some(attempt) = docket
        .get_live_session_task_execution_attempt_for_task(bear_id, task_id)
        .await?
        .filter(|attempt| {
            attempt.binding.kind != DocketExecutionBindingKind::ClientSession
                || attempt.binding.id != session_id
        })
    {
        if attempt.host.kind != DocketExecutionHostKind::TurnRun {
            return Err(CustomError::ValidationError(
                "session task execution authority has an incompatible host".to_string(),
            ));
        }
        let foreign_run_is_live = turn_runs::get_run(&state.sqlx_pool, &attempt.host.run_id)
            .await?
            .map(|run| run.state_value())
            .transpose()?
            .is_some_and(|state| !state.is_terminal());
        if foreign_run_is_live {
            return Err(CustomError::ValidationError(
                "focused task is already controlled by another live client session".to_string(),
            ));
        }
        release_stale_attempt(
            state,
            user_id,
            bear_id,
            session_id,
            &attempt,
            FocusedExecutionTransitionReason::StaleForeignAuthorityReleased,
            "stale_foreign_focused_execution_attempt_terminal_or_missing_run",
        )
        .await?;
    }

    Ok(StartReconciliation::Launch { recovered_run_id })
}

async fn release_stale_attempt(
    state: &DenState,
    user_id: i32,
    bear_id: Uuid,
    session_id: &str,
    attempt: &den_docket::DocketExecutionAttemptRow,
    transition_reason: FocusedExecutionTransitionReason,
    storage_reason: &str,
) -> Result<(), CustomError> {
    PgDocketService::from_pool(&state.sqlx_pool)
        .release_execution_attempt(DocketExecutionAttemptRelease {
            attempt_id: attempt.id,
            fence_epoch: attempt.fence_epoch,
            recovery_key: Uuid::new_v4(),
            recovery_reason: storage_reason.to_string(),
        })
        .await?;
    project_execution_authority_ended(
        state,
        user_id,
        bear_id,
        session_id,
        attempt.task_id,
        &attempt.host.run_id,
        attempt.id,
        attempt.fence_epoch,
        transition_reason,
    )
    .await;
    Ok(())
}

async fn reconcile_orphaned_run(
    state: &DenState,
    user_id: i32,
    bear_id: Uuid,
    task_id: Uuid,
    session_id: &str,
    run_id: &str,
) -> Result<(), CustomError> {
    let snapshot = load_focused_execution_snapshot(
        state,
        user_id,
        bear_id,
        session_id,
        FocusedExecutionLaunchState::AlreadyRunning,
    )
    .await?;
    if snapshot.task_id() != Some(task_id)
        || snapshot
            .run_id()
            .map(den_runtime::turn_ids::TurnRunId::as_str)
            != Some(run_id)
    {
        return Err(CustomError::ValidationError(
            "orphan recovery no longer matches the selected task and host run".to_string(),
        ));
    }
    if !matches!(
        snapshot.state,
        FocusedExecutionState::Terminal
            | FocusedExecutionState::Inconsistent {
                violation: FocusedExecutionInvariantViolation::RunningWithoutController
                    | FocusedExecutionInvariantViolation::ActiveRunWithoutAttempt,
            }
    ) {
        return Err(CustomError::ValidationError(format!(
            "focused execution is not recoverable as an orphan: {:?}",
            snapshot.state
        )));
    }
    let attempt = snapshot.attempt.filter(|attempt| attempt.state.is_live());
    turn_runs::fail_run_with_transition_reason(
        &state.sqlx_pool,
        session_id,
        run_id,
        bear_id,
        user_id,
        "orphaned_execution_controller",
        json!({
            "run_id": run_id,
            "message": "Focused execution host stopped before reaching a terminal boundary.",
            "reason": "orphaned_execution_controller",
            "recovery": "replacement_pending",
            "task_id": task_id,
            "task_selection_preserved": true,
        }),
        FocusedExecutionTransitionReason::OrphanedControllerReconciled,
    )
    .await?
    .ok_or_else(|| {
        CustomError::ValidationError(format!(
            "execution run {run_id} changed while orphan recovery was in progress; retry focus"
        ))
    })?;
    den_runtime::native_runtime::remove_native_client_run(session_id, run_id);
    if let Some(attempt) = attempt {
        PgDocketService::from_pool(&state.sqlx_pool)
            .release_execution_attempt(DocketExecutionAttemptRelease {
                attempt_id: attempt.id,
                fence_epoch: attempt.fence_epoch,
                recovery_key: Uuid::new_v4(),
                recovery_reason: "orphaned_execution_controller".to_string(),
            })
            .await?;
    }
    Ok(())
}
