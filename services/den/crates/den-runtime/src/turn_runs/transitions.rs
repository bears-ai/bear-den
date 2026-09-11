use bearwire_protocol::lifecycle::{
    FocusedExecutionState, FocusedExecutionTransition, FocusedExecutionTransitionReason,
};
use den_core::DenError;
use sqlx::PgConnection;

use super::{TurnRunRow, TurnRunState};

fn focused_state_for_run(state: TurnRunState) -> FocusedExecutionState {
    match state {
        TurnRunState::Accepted => FocusedExecutionState::Starting,
        TurnRunState::Running => FocusedExecutionState::Running,
        TurnRunState::WaitingForClient => FocusedExecutionState::WaitingForClient,
        TurnRunState::Continuing => FocusedExecutionState::Continuing,
        TurnRunState::Completed | TurnRunState::Failed | TurnRunState::Cancelled => {
            FocusedExecutionState::Terminal
        }
    }
}

pub(super) fn transition_reason(
    previous: TurnRunState,
    next: TurnRunState,
) -> FocusedExecutionTransitionReason {
    match (previous, next) {
        (TurnRunState::Accepted, TurnRunState::Running) => {
            FocusedExecutionTransitionReason::AuthorityStarted
        }
        (_, TurnRunState::WaitingForClient) => FocusedExecutionTransitionReason::ClientWaitOpened,
        (TurnRunState::WaitingForClient, TurnRunState::Continuing) => {
            FocusedExecutionTransitionReason::ClientWaitCleared
        }
        (_, TurnRunState::Completed) => FocusedExecutionTransitionReason::RunCompleted,
        (_, TurnRunState::Failed) => FocusedExecutionTransitionReason::RunFailed,
        (_, TurnRunState::Cancelled) => FocusedExecutionTransitionReason::RunCancelled,
        _ => FocusedExecutionTransitionReason::RunStateChanged,
    }
}

pub(super) async fn append_focused_run_transition_on(
    conn: &mut PgConnection,
    run: &TurnRunRow,
    previous: TurnRunState,
    next: TurnRunState,
    open_obligations: u32,
    reason_override: Option<FocusedExecutionTransitionReason>,
) -> Result<(), DenError> {
    let attempt = sqlx::query!(
        r#"
        SELECT id, task_id, fence_epoch
        FROM docket_execution_attempts
        WHERE host_kind = 'pair' AND host_run_id = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        run.run_id,
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(attempt) = attempt else {
        return Ok(());
    };

    crate::bearwire_events::append_focused_execution_transition_on(
        conn,
        run.bear_id,
        run.user_id,
        FocusedExecutionTransition {
            state_version: 0,
            from: None,
            to: focused_state_for_run(next),
            reason: reason_override.unwrap_or_else(|| transition_reason(previous, next)),
            correlation_id: run.run_id.clone(),
            causation_id: None,
            session_id: run.session_id.clone(),
            task_id: Some(attempt.task_id.to_string()),
            run_id: Some(run.run_id.clone()),
            attempt_id: Some(attempt.id.to_string()),
            fence_epoch: Some(attempt.fence_epoch),
            open_obligations,
            task_selection_preserved: true,
        },
    )
    .await?;
    Ok(())
}

pub(super) async fn transition_nonterminal_run_on(
    conn: &mut PgConnection,
    run_id: &str,
    allowed_from: &[TurnRunState],
    next: TurnRunState,
    terminal_reason: Option<&str>,
) -> Result<Option<TurnRunRow>, DenError> {
    debug_assert!(!next.is_terminal());
    let Some(current) = sqlx::query_as!(
        TurnRunRow,
        r#"
        SELECT id, run_id, session_id, bear_id, user_id, state,
               terminal_reason AS "terminal_reason?", created_at, updated_at,
               completed_at AS "completed_at?"
        FROM turn_runs
        WHERE run_id = $1
        FOR UPDATE
        "#,
        run_id,
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let previous = current.state_value()?;
    if !allowed_from.contains(&previous) {
        return Ok(None);
    }
    if previous == next {
        return Ok(Some(current));
    }

    let updated = sqlx::query_as!(
        TurnRunRow,
        r#"
        UPDATE turn_runs
        SET state = $2, terminal_reason = $3, updated_at = NOW()
        WHERE id = $1
        RETURNING id, run_id, session_id, bear_id, user_id, state,
                  terminal_reason AS "terminal_reason?", created_at, updated_at,
                  completed_at AS "completed_at?"
        "#,
        current.id,
        next.as_str(),
        terminal_reason,
    )
    .fetch_one(&mut *conn)
    .await?;
    let open_obligations = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!"
        FROM turn_obligations
        WHERE run_id = $1
          AND state IN ('requested', 'waiting_for_client', 'result_received')
        "#,
        run_id,
    )
    .fetch_one(&mut *conn)
    .await?;
    append_focused_run_transition_on(
        conn,
        &updated,
        previous,
        next,
        u32::try_from(open_obligations).map_err(|_| {
            DenError::System("focused execution obligation count overflowed u32".to_string())
        })?,
        None,
    )
    .await?;
    Ok(Some(updated))
}
