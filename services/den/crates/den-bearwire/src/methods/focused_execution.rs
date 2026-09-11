pub use bearwire_protocol::lifecycle::RunLaunchState as FocusedExecutionLaunchState;
use bearwire_protocol::lifecycle::{FocusedExecutionTransition, FocusedExecutionTransitionReason};
use den_core::{BearCapability, CapabilitySet};
use den_docket::{
    DocketExecutionAttemptStart, DocketExecutionBindingKind, DocketExecutionHost,
    DocketExecutionHostKind, DocketFocusedExecutionAcquire, DocketFocusedExecutionBinding,
    DocketService, PgDocketService,
};
use den_http::errors::CustomError;
use den_runtime::{
    bearwire_events,
    turn_ids::{ToolCallId, TurnRunId},
};
use den_service::{bears::Bear, client_sessions, DenState};

use uuid::Uuid;

use super::session::start_session_task_execution;

mod snapshot;

pub(crate) use snapshot::load_focused_execution_snapshot;
pub use snapshot::{
    ControllerDisposition, FocusedExecutionAttempt, FocusedExecutionInvariantViolation,
    FocusedExecutionObligations, FocusedExecutionRun, FocusedExecutionSnapshot,
    FocusedExecutionState, FocusedExecutionTask,
};

/// Start or reconcile the selected Docket task against a session execution owner.
///
/// This is the Den-side command boundary. Docket RPCs, `/focus`, ACP, and other clients should
/// not reproduce its attach/select/start/reduce sequence.
pub async fn start_or_reconcile_session_task_execution(
    state: &DenState,
    user_id: i32,
    bear: Bear,
    client_session_id: &str,
    task_id: Uuid,
    capabilities: &CapabilitySet,
) -> Result<FocusedExecutionSnapshot, CustomError> {
    capabilities.require(BearCapability::ExecuteFocusedTask)?;
    with_execution_lock(state, bear.id, client_session_id, || async {
        start_or_reconcile_locked(state, user_id, bear, client_session_id, task_id).await
    })
    .await
}

/// Starts focused execution for the session's already-selected task. This keeps the
/// session focus RPC on the same serialized command boundary as Docket `/focus`.
pub async fn start_selected_session_task_execution(
    state: &DenState,
    user_id: i32,
    bear: Bear,
    client_session_id: &str,
    capabilities: &CapabilitySet,
) -> Result<FocusedExecutionSnapshot, CustomError> {
    capabilities.require(BearCapability::ExecuteFocusedTask)?;
    let bear_id = bear.id;
    tracing::info!(
        event = "session_task_focus_requested",
        bear_id = %bear_id,
        user_id,
        client_session_id,
        "received session task focus request"
    );
    let result = with_execution_lock(state, bear_id, client_session_id, || async {
        let session = client_sessions::find_for_user_bear_session_id(
            &state.sqlx_pool,
            user_id,
            bear.id,
            client_session_id,
        )
        .await?
        .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;
        let task_id = session.current_task_id.ok_or_else(|| {
            CustomError::ValidationError(
                "no current session task is selected for this session".to_string(),
            )
        })?;
        start_or_reconcile_locked(state, user_id, bear, client_session_id, task_id).await
    })
    .await;
    match &result {
        Ok(execution) => tracing::info!(
            event = "session_task_focus_established",
            bear_id = %bear_id,
            user_id,
            client_session_id,
            task_id = ?execution.task_id(),
            run_id = ?execution.run_id(),
            attempt_id = ?execution.attempt_id(),
            launch_state = execution.launch_state.as_str(),
            fence_epoch = ?execution.fence_epoch(),
            "session task focus established execution control"
        ),
        Err(error) => tracing::warn!(
            event = "session_task_focus_failed",
            bear_id = %bear_id,
            user_id,
            client_session_id,
            error = %error,
            "session task focus did not establish execution control"
        ),
    }
    result
}

/// Promote the active turn run that invoked `focus_current_task` into the
/// selected task's execution owner. The current controller remains authoritative;
/// no replacement run or synthetic turn is created.
pub async fn acquire_selected_task_for_run(
    state: &DenState,
    user_id: i32,
    bear: Bear,
    client_session_id: &str,
    origin_run_id: &TurnRunId,
    tool_call_id: &ToolCallId,
    capabilities: &CapabilitySet,
) -> Result<FocusedExecutionSnapshot, CustomError> {
    capabilities.require(BearCapability::ExecuteFocusedTask)?;
    if !bear.work_enabled {
        return Err(CustomError::ValidationError(
            "focused task controls are disabled".to_string(),
        ));
    }
    tracing::info!(
        event = "session_task_model_focus_requested",
        bear_id = %bear.id,
        user_id,
        client_session_id,
        origin_run_id = %origin_run_id,
        tool_call_id = %tool_call_id,
        "promoting active turn run into focused execution"
    );
    let result = with_execution_lock(state, bear.id, client_session_id, || async {
        let session = client_sessions::find_for_user_bear_session_id(
            &state.sqlx_pool,
            user_id,
            bear.id,
            client_session_id,
        )
        .await?
        .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;
        let task_id = session.current_task_id.ok_or_else(|| {
            CustomError::ValidationError(
                "no current session task is selected for this session".to_string(),
            )
        })?;
        let run =
            den_runtime::turn_runs::active_run_for_session(&state.sqlx_pool, client_session_id)
                .await?
                .filter(|run| {
                    run.run_id == origin_run_id.as_str()
                        && run.bear_id == bear.id
                        && run.user_id == user_id
                })
                .ok_or_else(|| {
                    CustomError::ValidationError(
                        "focus origin is not the active turn run for this session".to_string(),
                    )
                })?;
        let run_state = den_runtime::turn_runs::TurnRunState::try_from_storage(&run.state)?;
        if !matches!(
            run_state,
            den_runtime::turn_runs::TurnRunState::Running
                | den_runtime::turn_runs::TurnRunState::WaitingForClient
                | den_runtime::turn_runs::TurnRunState::Continuing
        ) {
            return Err(CustomError::ValidationError(format!(
                "focus origin run is not executable from state {}",
                run.state
            )));
        }
        let controller_is_live = state
            .turn_cancellations
            .active_for_run(client_session_id, origin_run_id.as_str())
            .is_some();
        if !controller_is_live {
            return Err(CustomError::ValidationError(
                "focus origin run has no live controller in this Den process".to_string(),
            ));
        }

        activate_session_task(
            state,
            user_id,
            bear.id,
            client_session_id,
            session.id,
            task_id,
        )
        .await?;

        let docket = PgDocketService::from_pool(&state.sqlx_pool);
        if let Some(existing) = docket
            .get_live_session_task_execution_attempt_for_session(bear.id, client_session_id)
            .await?
        {
            if existing.host.run_id != origin_run_id.as_str() {
                return Err(CustomError::ValidationError(format!(
                    "session execution authority belongs to run {}; refusing implicit transfer to {}",
                    existing.host.run_id, origin_run_id
                )));
            }
        }
        let acquisition_key = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("den:focused-execution:{origin_run_id}:{tool_call_id}").as_bytes(),
        );
        let attempt = docket
            .acquire_focused_execution(DocketFocusedExecutionAcquire {
                bear_id: bear.id,
                task_id,
                binding: DocketFocusedExecutionBinding {
                    kind: DocketExecutionBindingKind::ClientSession,
                    id: client_session_id.to_string(),
                },
                host: DocketExecutionHost {
                    kind: DocketExecutionHostKind::TurnRun,
                    run_id: origin_run_id.to_string(),
                },
                acquisition_key,
            })
            .await?;
        let attempt_was_running = attempt.state == den_docket::DocketExecutionAttemptState::Running;
        let attempt = docket
            .start_execution_attempt(DocketExecutionAttemptStart {
                attempt_id: attempt.id,
                fence_epoch: attempt.fence_epoch,
            })
            .await?;
        let execution = load_focused_execution_snapshot(
            state,
            user_id,
            bear.id,
            client_session_id,
            FocusedExecutionLaunchState::AlreadyRunning,
        )
        .await?;
        execution.require_active_authority()?;
        if execution.task_id() != Some(task_id)
            || execution.run_id() != Some(origin_run_id)
            || execution.attempt_id() != Some(attempt.id)
        {
            return Err(CustomError::System(
                "same-run task focus projection did not preserve acquired authority".to_string(),
            ));
        }
        if !attempt_was_running {
            project_execution_transition(
                state,
                user_id,
                bear.id,
                &execution,
                FocusedExecutionTransitionReason::FocusAcquired,
                origin_run_id.as_str(),
                Some(tool_call_id.as_str()),
            )
            .await;
        }
        Ok(execution)
    })
    .await;
    match &result {
        Ok(execution) => tracing::info!(
            event = "session_task_model_focus_established",
            bear_id = %bear.id,
            user_id,
            client_session_id,
            origin_run_id = %origin_run_id,
            tool_call_id = %tool_call_id,
            attempt_id = ?execution.attempt_id(),
            fence_epoch = ?execution.fence_epoch(),
            "active turn run now owns focused execution"
        ),
        Err(error) => tracing::warn!(
            event = "session_task_model_focus_rejected",
            bear_id = %bear.id,
            user_id,
            client_session_id,
            origin_run_id = %origin_run_id,
            tool_call_id = %tool_call_id,
            error = %error,
            "active turn run was not promoted"
        ),
    }
    result
}

async fn with_execution_lock<T, F, Fut>(
    state: &DenState,
    bear_id: Uuid,
    client_session_id: &str,
    operation: F,
) -> Result<T, CustomError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, CustomError>>,
{
    let lock_key = format!("focused-execution:{bear_id}:{client_session_id}");
    let mut lock_transaction = state.sqlx_pool.begin().await?;
    sqlx::query!(
        r#"SELECT pg_advisory_xact_lock(hashtextextended($1, 0)) AS "locked!""#,
        lock_key
    )
    .fetch_one(&mut *lock_transaction)
    .await?;
    // ponytail: PostgreSQL advisory locking is global per database; if command volume
    // becomes material, replace it with a persisted command queue/lease.
    let result = operation().await;
    let release_result = lock_transaction.commit().await;
    match (result, release_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(CustomError::System(format!(
            "release focused session execution command lock failed: {err}"
        ))),
    }
}

async fn start_or_reconcile_locked(
    state: &DenState,
    user_id: i32,
    bear: Bear,
    client_session_id: &str,
    task_id: Uuid,
) -> Result<FocusedExecutionSnapshot, CustomError> {
    let mut existing = load_focused_execution_snapshot(
        state,
        user_id,
        bear.id,
        client_session_id,
        FocusedExecutionLaunchState::AlreadyRunning,
    )
    .await?;
    if existing.task_id() == Some(task_id) && existing.state.has_active_authority() {
        match existing.controller {
            ControllerDisposition::Claimed => {
                existing.launch_state = FocusedExecutionLaunchState::Claimed;
                return Ok(existing);
            }
            ControllerDisposition::Live => return Ok(existing),
            ControllerDisposition::NotApplicable
            | ControllerDisposition::Queued
            | ControllerDisposition::Missing
            | ControllerDisposition::Recovering => {}
        }
    }
    let session = client_sessions::find_for_user_bear_session_id(
        &state.sqlx_pool,
        user_id,
        bear.id,
        client_session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;

    activate_session_task(
        state,
        user_id,
        bear.id,
        client_session_id,
        session.id,
        task_id,
    )
    .await?;
    tracing::info!(
        event = "session_task_focus_task_activated",
        bear_id = %bear.id,
        user_id,
        client_session_id,
        task_id = %task_id,
        "persisted session task attachment for focus"
    );

    let launch_state =
        start_session_task_execution(state, user_id, bear.clone(), client_session_id).await?;
    let execution =
        load_focused_execution_snapshot(state, user_id, bear.id, client_session_id, launch_state)
            .await?;
    tracing::info!(
        event = "session_task_focus_start_resolved",
        bear_id = %bear.id,
        user_id,
        client_session_id,
        task_id = ?execution.task_id(),
        run_id = ?execution.run_id(),
        attempt_id = ?execution.attempt_id(),
        state = ?execution.state,
        launch_state = execution.launch_state.as_str(),
        fence_epoch = ?execution.fence_epoch(),
        "resolved session task execution start"
    );
    execution.require_active_authority()?;

    Ok(execution)
}

pub(crate) async fn project_execution_transition(
    state: &DenState,
    user_id: i32,
    bear_id: Uuid,
    execution: &FocusedExecutionSnapshot,
    reason: FocusedExecutionTransitionReason,
    correlation_id: &str,
    causation_id: Option<&str>,
) {
    let transition = FocusedExecutionTransition {
        state_version: 0,
        from: None,
        to: execution.state,
        reason,
        correlation_id: correlation_id.to_string(),
        causation_id: causation_id.map(str::to_string),
        session_id: execution.session_id.to_string(),
        task_id: execution.task.as_ref().map(|task| task.id.to_string()),
        run_id: execution.run.as_ref().map(|run| run.id.to_string()),
        attempt_id: execution
            .attempt
            .as_ref()
            .map(|attempt| attempt.id.to_string()),
        fence_epoch: execution
            .attempt
            .as_ref()
            .map(|attempt| attempt.fence_epoch),
        open_obligations: execution
            .obligations
            .map(|obligations| obligations.open)
            .unwrap_or(0),
        task_selection_preserved: true,
    };
    if let Err(err) = bearwire_events::append_focused_execution_transition(
        &state.sqlx_pool,
        bear_id,
        user_id,
        transition,
    )
    .await
    {
        tracing::warn!(
            error = %err,
            session_id = %execution.session_id,
            run_id = ?execution.run_id(),
            attempt_id = ?execution.attempt_id(),
            "failed to persist focused execution diagnostic transition"
        );
    }
}

pub(crate) async fn project_execution_authority_ended(
    state: &DenState,
    user_id: i32,
    bear_id: Uuid,
    session_id: &str,
    task_id: Uuid,
    run_id: &str,
    attempt_id: Uuid,
    fence_epoch: i64,
    reason: FocusedExecutionTransitionReason,
) {
    if let Err(err) = bearwire_events::append_focused_execution_transition(
        &state.sqlx_pool,
        bear_id,
        user_id,
        FocusedExecutionTransition {
            state_version: 0,
            from: None,
            to: FocusedExecutionState::Terminal,
            reason,
            correlation_id: run_id.to_string(),
            causation_id: None,
            session_id: session_id.to_string(),
            task_id: Some(task_id.to_string()),
            run_id: Some(run_id.to_string()),
            attempt_id: Some(attempt_id.to_string()),
            fence_epoch: Some(fence_epoch),
            open_obligations: 0,
            task_selection_preserved: true,
        },
    )
    .await
    {
        tracing::warn!(
            error = %err,
            session_id,
            run_id,
            attempt_id = %attempt_id,
            "failed to persist focused execution authority end"
        );
    }
}

/// Commits the durable part of `/focus` in one transaction. Starting the loop
/// remains outside it: startup is reconciled idempotently after interruption,
/// while an attachment without its selected task is not a valid focus state.
async fn activate_session_task(
    state: &DenState,
    user_id: i32,
    bear_id: Uuid,
    client_session_id: &str,
    session_id: Uuid,
    task_id: Uuid,
) -> Result<(), CustomError> {
    let mut tx = state.sqlx_pool.begin().await?;
    let session_exists = sqlx::query(
        "SELECT 1 FROM client_sessions WHERE id = $1 AND user_id = $2 AND bear_id = $3 AND client_session_id = $4 FOR UPDATE",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(bear_id)
    .bind(client_session_id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    if !session_exists {
        return Err(CustomError::NotFound(
            "client session not found".to_string(),
        ));
    }

    let attached = sqlx::query(
        r#"
        INSERT INTO bear_pair_task_attachments (task_id, session_id)
        SELECT id, $3 FROM bear_tasks
        WHERE id = $2 AND bear_id = $1 AND settled_by_entry_id IS NULL
          AND (job_id IS NOT NULL OR EXISTS (
            SELECT 1 FROM bear_pair_task_attachments existing
            WHERE existing.task_id = bear_tasks.id
              AND existing.session_id = $3 AND existing.released_at IS NULL
          ))
        ON CONFLICT (task_id) DO UPDATE
        SET session_id = EXCLUDED.session_id, attached_at = NOW(), released_at = NULL
        WHERE bear_pair_task_attachments.released_at IS NOT NULL
           OR bear_pair_task_attachments.session_id = EXCLUDED.session_id
        "#,
    )
    .bind(bear_id)
    .bind(task_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;
    if attached.rows_affected() == 0 {
        return Err(CustomError::ValidationError(
            "task is not an unclaimed durable task available to this client session".to_string(),
        ));
    }

    sqlx::query(
        "UPDATE client_sessions SET current_task_id = $2, updated_at = NOW() WHERE id = $1",
    )
    .bind(session_id)
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::debug!(%bear_id, %session_id, %task_id, "activated session task for focused execution");
    Ok(())
}
