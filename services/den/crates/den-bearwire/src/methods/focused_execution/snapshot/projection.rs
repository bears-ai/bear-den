use den_docket::{
    DocketExecutionAttemptState, DocketExecutionBindingKind, DocketExecutionHost,
    DocketExecutionHostKind, DocketFocusedExecutionBinding,
};
use den_http::errors::CustomError;
use den_runtime::{
    turn_ids::{ClientSessionId, TurnRunId},
    turn_runs::TurnRunState,
};
use den_service::DenState;
use uuid::Uuid;

use super::super::FocusedExecutionLaunchState;
use super::{
    reduce_focused_execution, ControllerDisposition, FocusedExecutionAttempt,
    FocusedExecutionFacts, FocusedExecutionRun, FocusedExecutionSnapshot,
};

pub async fn load_focused_execution_snapshot(
    state: &DenState,
    user_id: i32,
    bear_id: Uuid,
    client_session_id: &str,
    launch_state: FocusedExecutionLaunchState,
) -> Result<FocusedExecutionSnapshot, CustomError> {
    let row = sqlx::query!(
        r#"
        SELECT session.current_task_id AS "task_id?",
               run.run_id AS "run_id?",
               run.state AS "run_state?",
               run.terminal_reason AS "terminal_reason?",
               attempt.id AS "attempt_id?",
               attempt.task_id AS "attempt_task_id?",
               attempt.binding_kind AS "binding_kind?",
               attempt.binding_id AS "binding_id?",
               attempt.host_kind AS "host_kind?",
               attempt.host_run_id AS "host_run_id?",
               attempt.fence_epoch AS "fence_epoch?",
               attempt.state AS "attempt_state?",
               COALESCE((
                   SELECT COUNT(*)
                   FROM turn_obligations obligation
                   WHERE obligation.run_id = run.run_id
                     AND obligation.state IN ('pending', 'running')
               ), 0) AS "open_obligations!"
        FROM client_sessions session
        LEFT JOIN LATERAL (
            SELECT id, task_id, binding_kind, binding_id, host_kind, host_run_id,
                   fence_epoch, state, created_at
            FROM docket_execution_attempts
            WHERE bear_id = session.bear_id
              AND binding_kind = 'client_session'
              AND binding_id = session.client_session_id
              AND (session.current_task_id IS NULL OR task_id = session.current_task_id)
            ORDER BY
                CASE WHEN state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')
                     THEN 0 ELSE 1 END,
                created_at DESC
            LIMIT 1
        ) attempt ON TRUE
        LEFT JOIN LATERAL (
            SELECT run_id, state, terminal_reason, created_at
            FROM turn_runs
            WHERE session_id = session.client_session_id
              AND bear_id = session.bear_id
              AND user_id = session.user_id
              AND (
                  (attempt.host_run_id IS NOT NULL AND run_id = attempt.host_run_id)
                  OR (
                      attempt.host_run_id IS NULL
                      AND session.current_task_id IS NOT NULL
                      AND state IN ('accepted', 'running', 'waiting_for_client', 'continuing')
                  )
              )
            ORDER BY
                CASE WHEN run_id = attempt.host_run_id THEN 0
                     WHEN state IN ('accepted', 'running', 'waiting_for_client', 'continuing') THEN 1
                     ELSE 2 END,
                created_at DESC
            LIMIT 1
        ) run ON TRUE
        WHERE session.user_id = $1
          AND session.bear_id = $2
          AND session.client_session_id = $3
        "#,
        user_id,
        bear_id,
        client_session_id,
    )
    .fetch_optional(&state.sqlx_pool)
    .await?
    .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;

    let session_id = ClientSessionId::new(client_session_id.to_owned())?;
    let run = match (row.run_id, row.run_state) {
        (Some(run_id), Some(run_state)) => Some(FocusedExecutionRun {
            id: TurnRunId::new(run_id)?,
            state: TurnRunState::try_from_storage(&run_state)?,
            terminal_reason: row.terminal_reason,
        }),
        (None, None) => None,
        _ => {
            return Err(CustomError::System(
                "focused execution projection returned a partial run".to_string(),
            ))
        }
    };
    let attempt = match (row.attempt_id, row.attempt_state, row.fence_epoch) {
        (Some(id), Some(attempt_state), Some(fence_epoch)) => Some(FocusedExecutionAttempt {
            id,
            state: DocketExecutionAttemptState::try_from_storage(&attempt_state)?,
            fence_epoch,
        }),
        (None, None, None) => None,
        _ => {
            return Err(CustomError::System(
                "focused execution projection returned a partial attempt".to_string(),
            ))
        }
    };
    let binding = match (row.binding_kind.as_deref(), row.binding_id) {
        (Some("client_session"), Some(id)) => Some(DocketFocusedExecutionBinding {
            kind: DocketExecutionBindingKind::ClientSession,
            id,
        }),
        (Some("work_assignment"), Some(id)) => Some(DocketFocusedExecutionBinding {
            kind: DocketExecutionBindingKind::WorkAssignment,
            id,
        }),
        (None, None) => None,
        _ => {
            return Err(CustomError::System(
                "focused execution projection returned an invalid binding".to_string(),
            ))
        }
    };
    let host = match (row.host_kind.as_deref(), row.host_run_id) {
        (Some("pair"), Some(run_id)) => Some(DocketExecutionHost {
            kind: DocketExecutionHostKind::TurnRun,
            run_id,
        }),
        (Some("work"), Some(run_id)) => Some(DocketExecutionHost {
            kind: DocketExecutionHostKind::WorkRun,
            run_id,
        }),
        (None, None) => None,
        _ => {
            return Err(CustomError::System(
                "focused execution projection returned an invalid host".to_string(),
            ))
        }
    };
    let controller = run
        .as_ref()
        .map_or(ControllerDisposition::NotApplicable, |run| {
            if state
                .turn_cancellations
                .active_for_session(client_session_id)
                .is_some_and(|active| active.run_ids.iter().any(|id| id == run.id.as_str()))
            {
                ControllerDisposition::Live
            } else {
                ControllerDisposition::Missing
            }
        });
    let open_obligations = u32::try_from(row.open_obligations).map_err(|_| {
        CustomError::System("focused execution obligation count overflowed u32".to_string())
    })?;

    Ok(reduce_focused_execution(
        FocusedExecutionFacts {
            session_id,
            task_id: row.task_id,
            binding,
            run,
            attempt,
            attempt_task_id: row.attempt_task_id,
            host,
            controller,
            open_obligations,
        },
        launch_state,
    ))
}
