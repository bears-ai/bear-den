use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;

use den_core::DenError;

use den_core::tools::constants::DEN_WEB_FETCH;

use crate::{turn_obligations, turn_runs, turn_steps};

#[derive(Debug, Deserialize)]
struct LocalToolRequestPayload {
    #[serde(default = "default_local_tool_name")]
    tool_name: String,
    #[serde(default)]
    arguments: Value,
}

fn default_local_tool_name() -> String {
    "local_tool".to_string()
}

fn local_tool_request_payload(value: &Value) -> Result<LocalToolRequestPayload, DenError> {
    serde_json::from_value(value.clone()).map_err(|err| {
        DenError::ValidationError(format!("invalid local tool request payload: {err}"))
    })
}

fn obligation_is_den_web_fetch(obligation_payload: &Value) -> bool {
    let tool_name = obligation_payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    den_core::tools::descriptor::builtin_den_tool_descriptor_for_provider_name(tool_name)
        .map(|descriptor| descriptor.name == DEN_WEB_FETCH)
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub enum ToolResultCoordinatorOutcome {
    WaitingForMoreClientResults {
        run: Option<turn_runs::TurnRunRow>,
        open_obligations: Vec<turn_obligations::TurnObligationRow>,
        result: Option<turn_runs::TurnObligationResultRow>,
    },
    ContinueModel {
        run: Option<turn_runs::TurnRunRow>,
        result: Option<turn_runs::TurnObligationResultRow>,
    },
    DuplicateIdentical {
        result: turn_runs::TurnObligationResultRow,
        run_state: String,
    },
    DuplicateConflict {
        existing_hash: String,
    },
    IgnoredLateResult {
        run_state: String,
        obligation_state: String,
    },
}

#[derive(Debug, Clone)]
pub enum PermissionResultCoordinatorOutcome {
    DispatchLocalTool {
        run: Option<turn_runs::TurnRunRow>,
        tool_obligation: Box<turn_obligations::TurnObligationRow>,
        tool_call_id: String,
        tool_name: String,
        args: Value,
        result: Option<turn_runs::TurnObligationResultRow>,
    },
    ContinueModel {
        run: Option<turn_runs::TurnRunRow>,
        result: Option<turn_runs::TurnObligationResultRow>,
    },
    DuplicateIdentical {
        result: turn_runs::TurnObligationResultRow,
        run_state: String,
    },
    DuplicateConflict {
        existing_hash: String,
    },
    IgnoredLateResult {
        run_state: String,
        obligation_state: String,
    },
}

fn validate_result_turn_step(
    obligation: &turn_obligations::TurnObligationRow,
    result_turn_step_id: Option<uuid::Uuid>,
) -> Result<(), DenError> {
    if result_turn_step_id.is_some() && result_turn_step_id != obligation.turn_step_id {
        return Err(DenError::ValidationError(format!(
            "turn_step_id mismatch for obligation {}: expected {:?}, got {:?}",
            obligation.id, obligation.turn_step_id, result_turn_step_id
        )));
    }
    Ok(())
}

fn run_is_terminal(run: &turn_runs::TurnRunRow) -> bool {
    matches!(run.state.as_str(), "completed" | "failed" | "cancelled")
}

enum ExistingClientResultOutcome {
    DuplicateIdentical {
        result: turn_runs::TurnObligationResultRow,
        run_state: String,
    },
    DuplicateConflict {
        existing_hash: String,
    },
    IgnoredLateResult {
        run_state: String,
        obligation_state: String,
    },
}

async fn existing_client_result_or_late(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    obligation_kind: &str,
    obligation_id: &str,
    result_payload: &Value,
) -> Result<Option<ExistingClientResultOutcome>, DenError> {
    if let Some(record) = turn_runs::existing_client_result_for_payload(
        pool,
        &run.run_id,
        obligation_kind,
        obligation_id,
        result_payload,
    )
    .await?
    {
        return Ok(Some(match record {
            turn_runs::TurnObligationResultRecord::DuplicateIdentical { row } => {
                ExistingClientResultOutcome::DuplicateIdentical {
                    result: row,
                    run_state: run.state.clone(),
                }
            }
            turn_runs::TurnObligationResultRecord::DuplicateConflict { existing_hash } => {
                ExistingClientResultOutcome::DuplicateConflict { existing_hash }
            }
            turn_runs::TurnObligationResultRecord::Inserted { .. } => unreachable!(),
        }));
    }
    if !run_is_terminal(run) && turn_obligations::obligation_is_open(obligation) {
        return Ok(None);
    }
    Ok(Some(ExistingClientResultOutcome::IgnoredLateResult {
        run_state: run.state.clone(),
        obligation_state: obligation.state.clone(),
    }))
}

async fn existing_tool_result_or_late(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    obligation_kind: &str,
    obligation_id: &str,
    result_payload: &Value,
) -> Result<Option<ToolResultCoordinatorOutcome>, DenError> {
    Ok(existing_client_result_or_late(
        pool,
        run,
        obligation,
        obligation_kind,
        obligation_id,
        result_payload,
    )
    .await?
    .map(|outcome| match outcome {
        ExistingClientResultOutcome::DuplicateIdentical { result, run_state } => {
            ToolResultCoordinatorOutcome::DuplicateIdentical { result, run_state }
        }
        ExistingClientResultOutcome::DuplicateConflict { existing_hash } => {
            ToolResultCoordinatorOutcome::DuplicateConflict { existing_hash }
        }
        ExistingClientResultOutcome::IgnoredLateResult {
            run_state,
            obligation_state,
        } => ToolResultCoordinatorOutcome::IgnoredLateResult {
            run_state,
            obligation_state,
        },
    }))
}

async fn existing_permission_result_or_late(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    obligation_kind: &str,
    obligation_id: &str,
    result_payload: &Value,
) -> Result<Option<PermissionResultCoordinatorOutcome>, DenError> {
    Ok(existing_client_result_or_late(
        pool,
        run,
        obligation,
        obligation_kind,
        obligation_id,
        result_payload,
    )
    .await?
    .map(|outcome| match outcome {
        ExistingClientResultOutcome::DuplicateIdentical { result, run_state } => {
            PermissionResultCoordinatorOutcome::DuplicateIdentical { result, run_state }
        }
        ExistingClientResultOutcome::DuplicateConflict { existing_hash } => {
            PermissionResultCoordinatorOutcome::DuplicateConflict { existing_hash }
        }
        ExistingClientResultOutcome::IgnoredLateResult {
            run_state,
            obligation_state,
        } => PermissionResultCoordinatorOutcome::IgnoredLateResult {
            run_state,
            obligation_state,
        },
    }))
}

fn attach_tool_result(
    outcome: ToolResultCoordinatorOutcome,
    result: turn_runs::TurnObligationResultRow,
) -> ToolResultCoordinatorOutcome {
    match outcome {
        ToolResultCoordinatorOutcome::WaitingForMoreClientResults {
            run,
            open_obligations,
            ..
        } => ToolResultCoordinatorOutcome::WaitingForMoreClientResults {
            run,
            open_obligations,
            result: Some(result),
        },
        ToolResultCoordinatorOutcome::ContinueModel { run, .. } => {
            ToolResultCoordinatorOutcome::ContinueModel {
                run,
                result: Some(result),
            }
        }
        other => other,
    }
}

fn attach_permission_result(
    outcome: PermissionResultCoordinatorOutcome,
    result: turn_runs::TurnObligationResultRow,
) -> PermissionResultCoordinatorOutcome {
    match outcome {
        PermissionResultCoordinatorOutcome::DispatchLocalTool {
            run,
            tool_obligation,
            tool_call_id,
            tool_name,
            args,
            ..
        } => PermissionResultCoordinatorOutcome::DispatchLocalTool {
            run,
            tool_obligation,
            tool_call_id,
            tool_name,
            args,
            result: Some(result),
        },
        PermissionResultCoordinatorOutcome::ContinueModel { run, .. } => {
            PermissionResultCoordinatorOutcome::ContinueModel {
                run,
                result: Some(result),
            }
        }
        other => other,
    }
}

pub async fn record_and_settle_tool_result(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    attempt_token_hash: &str,
    obligation_kind: &str,
    obligation_id: &str,
    result_payload: Value,
) -> Result<ToolResultCoordinatorOutcome, DenError> {
    record_and_settle_tool_result_for_step(
        pool,
        run,
        obligation,
        obligation.turn_step_id,
        attempt_token_hash,
        obligation_kind,
        obligation_id,
        result_payload,
    )
    .await
}

pub async fn record_and_settle_tool_result_for_step(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    result_turn_step_id: Option<uuid::Uuid>,
    attempt_token_hash: &str,
    obligation_kind: &str,
    obligation_id: &str,
    result_payload: Value,
) -> Result<ToolResultCoordinatorOutcome, DenError> {
    validate_result_turn_step(obligation, result_turn_step_id)?;
    if let Some(outcome) = existing_tool_result_or_late(
        pool,
        run,
        obligation,
        obligation_kind,
        obligation_id,
        &result_payload,
    )
    .await?
    {
        return Ok(outcome);
    }
    match turn_runs::record_claimed_tool_result_for_step(
        pool,
        run.run_id.as_str(),
        obligation.turn_step_id,
        obligation.id,
        obligation_id,
        attempt_token_hash,
        result_payload.clone(),
    )
    .await?
    {
        turn_runs::ClaimedToolResultRecord::ClaimRejected => {
            Ok(ToolResultCoordinatorOutcome::IgnoredLateResult {
                run_state: run.state.clone(),
                obligation_state: obligation.state.clone(),
            })
        }
        turn_runs::ClaimedToolResultRecord::Recorded(
            turn_runs::TurnObligationResultRecord::Inserted { row },
        ) => {
            let outcome = settle_recorded_tool_result(pool, run, obligation).await?;
            Ok(attach_tool_result(outcome, row))
        }
        turn_runs::ClaimedToolResultRecord::Recorded(
            turn_runs::TurnObligationResultRecord::DuplicateIdentical { row },
        ) => Ok(ToolResultCoordinatorOutcome::DuplicateIdentical {
            result: row,
            run_state: run.state.clone(),
        }),
        turn_runs::ClaimedToolResultRecord::Recorded(
            turn_runs::TurnObligationResultRecord::DuplicateConflict { existing_hash },
        ) => Ok(ToolResultCoordinatorOutcome::DuplicateConflict { existing_hash }),
    }
}

/// Claim a local-tool permission and its Den-owned workspace grant together.
/// Only an inserted result can create policy; duplicate identical replies may
/// complete an interrupted transition, but cannot insert another grant.
pub async fn record_and_settle_workspace_hat_permission_result(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    permission_id: &str,
    result_payload: Value,
    grant: &turn_runs::WorkspacePermissionGrant,
) -> Result<PermissionResultCoordinatorOutcome, DenError> {
    let recorded = turn_runs::record_permission_result_with_workspace_grant(
        pool,
        &run.run_id,
        &run.session_id,
        obligation.id,
        permission_id,
        obligation.turn_step_id,
        result_payload.clone(),
        grant,
    )
    .await?;
    let row = match recorded {
        Some(
            turn_runs::TurnObligationResultRecord::Inserted { row }
            | turn_runs::TurnObligationResultRecord::DuplicateIdentical { row },
        ) => row,
        Some(turn_runs::TurnObligationResultRecord::DuplicateConflict { existing_hash }) => {
            return Ok(PermissionResultCoordinatorOutcome::DuplicateConflict { existing_hash });
        }
        None => {
            return Ok(PermissionResultCoordinatorOutcome::IgnoredLateResult {
                run_state: run.state.clone(),
                obligation_state: obligation.state.clone(),
            });
        }
    };
    let Some(tool_obligation) =
        turn_obligations::promote_workspace_grant_permission_to_tool_result(
            pool,
            obligation.id,
            result_payload,
        )
        .await?
    else {
        return Ok(PermissionResultCoordinatorOutcome::DuplicateIdentical {
            result: row,
            run_state: run.state.clone(),
        });
    };
    let tool_call_id = obligation.tool_call_id.clone().ok_or_else(|| {
        DenError::ValidationError("workspace-granted permission has no tool call".into())
    })?;
    let transitioned = turn_runs::transition_run(
        pool,
        &run.run_id,
        turn_runs::TurnRunState::WaitingForClient,
        None,
    )
    .await?;
    let payload = local_tool_request_payload(&obligation.request_payload)?;
    Ok(PermissionResultCoordinatorOutcome::DispatchLocalTool {
        run: transitioned,
        tool_obligation: Box::new(tool_obligation),
        tool_call_id,
        tool_name: payload.tool_name,
        args: payload.arguments,
        result: Some(row),
    })
}

pub async fn record_and_settle_permission_result(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    normalized_decision: &str,
    obligation_kind: &str,
    obligation_id: &str,
    result_payload: Value,
) -> Result<PermissionResultCoordinatorOutcome, DenError> {
    record_and_settle_permission_result_for_step(
        pool,
        run,
        obligation,
        obligation.turn_step_id,
        normalized_decision,
        obligation_kind,
        obligation_id,
        result_payload,
    )
    .await
}

pub async fn record_and_settle_permission_result_for_step(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    result_turn_step_id: Option<uuid::Uuid>,
    normalized_decision: &str,
    obligation_kind: &str,
    obligation_id: &str,
    result_payload: Value,
) -> Result<PermissionResultCoordinatorOutcome, DenError> {
    validate_result_turn_step(obligation, result_turn_step_id)?;
    if let Some(outcome) = existing_permission_result_or_late(
        pool,
        run,
        obligation,
        obligation_kind,
        obligation_id,
        &result_payload,
    )
    .await?
    {
        return Ok(outcome);
    }
    match turn_runs::record_client_result_for_step(
        pool,
        run.run_id.as_str(),
        obligation.turn_step_id,
        obligation_kind,
        obligation_id,
        result_payload.clone(),
    )
    .await?
    {
        turn_runs::TurnObligationResultRecord::Inserted { row } => {
            let outcome = settle_permission_result(
                pool,
                run,
                obligation,
                normalized_decision,
                result_payload,
            )
            .await?;
            Ok(attach_permission_result(outcome, row))
        }
        turn_runs::TurnObligationResultRecord::DuplicateIdentical { row } => {
            Ok(PermissionResultCoordinatorOutcome::DuplicateIdentical {
                result: row,
                run_state: run.state.clone(),
            })
        }
        turn_runs::TurnObligationResultRecord::DuplicateConflict { existing_hash } => {
            Ok(PermissionResultCoordinatorOutcome::DuplicateConflict { existing_hash })
        }
    }
}

pub async fn settle_tool_result(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    attempt_token_hash: &str,
    result_payload: Value,
) -> Result<ToolResultCoordinatorOutcome, DenError> {
    let Some(_received_obligation) = turn_obligations::mark_claimed_result_received(
        pool,
        obligation.id,
        attempt_token_hash,
        result_payload,
    )
    .await?
    else {
        return Ok(ToolResultCoordinatorOutcome::IgnoredLateResult {
            run_state: run.state.clone(),
            obligation_state: obligation.state.clone(),
        });
    };

    settle_recorded_tool_result(pool, run, obligation).await
}

async fn settle_recorded_tool_result(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
) -> Result<ToolResultCoordinatorOutcome, DenError> {
    let open_obligations = if let Some(step_id) = obligation.turn_step_id {
        turn_obligations::open_client_obligations_for_step(pool, step_id).await?
    } else {
        turn_obligations::open_client_obligations_for_run(pool, &run.run_id).await?
    };
    if !open_obligations.is_empty() {
        let transitioned = turn_runs::transition_run(
            pool,
            &run.run_id,
            turn_runs::TurnRunState::WaitingForClient,
            None,
        )
        .await?;
        return Ok(ToolResultCoordinatorOutcome::WaitingForMoreClientResults {
            run: transitioned,
            open_obligations,
            result: None,
        });
    }

    let transitioned = continue_after_client_result(pool, run, obligation).await?;
    Ok(ToolResultCoordinatorOutcome::ContinueModel {
        run: transitioned,
        result: None,
    })
}

pub async fn settle_permission_result(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
    normalized_decision: &str,
    result_payload: Value,
) -> Result<PermissionResultCoordinatorOutcome, DenError> {
    let Some(_received_obligation) =
        turn_obligations::mark_result_received(pool, obligation.id, result_payload).await?
    else {
        return Ok(PermissionResultCoordinatorOutcome::IgnoredLateResult {
            run_state: run.state.clone(),
            obligation_state: obligation.state.clone(),
        });
    };

    if normalized_decision == "granted" && !obligation_is_den_web_fetch(&obligation.request_payload)
    {
        return dispatch_local_tool_after_grant(pool, run, obligation).await;
    }

    let transitioned = continue_after_client_result(pool, run, obligation).await?;
    Ok(PermissionResultCoordinatorOutcome::ContinueModel {
        run: transitioned,
        result: None,
    })
}

async fn continue_after_client_result(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
) -> Result<Option<turn_runs::TurnRunRow>, DenError> {
    let _ = turn_obligations::mark_continued(pool, obligation.id).await?;
    if let Some(step_id) = obligation.turn_step_id {
        let _ = turn_steps::transition_step(pool, step_id, turn_steps::TurnStepState::Continued)
            .await?;
    }

    // A client result can race another result that completes the same step.
    // Only the caller that atomically claims this transition may start a model
    // continuation; a lost claim is an idempotent acknowledgement.
    turn_runs::claim_run_continuation(pool, &run.run_id, None).await
}

async fn dispatch_local_tool_after_grant(
    pool: &PgPool,
    run: &turn_runs::TurnRunRow,
    obligation: &turn_obligations::TurnObligationRow,
) -> Result<PermissionResultCoordinatorOutcome, DenError> {
    let Some(tool_call_id) = obligation.tool_call_id.clone() else {
        return Err(DenError::ValidationError(
            "granted armature-local permission obligation missing tool_call_id".to_string(),
        ));
    };
    let Some(tool_obligation) =
        turn_obligations::mark_waiting_for_tool_result(pool, obligation.id).await?
    else {
        return Ok(PermissionResultCoordinatorOutcome::IgnoredLateResult {
            run_state: run.state.clone(),
            obligation_state: obligation.state.clone(),
        });
    };
    let transitioned = turn_runs::transition_run(
        pool,
        &run.run_id,
        turn_runs::TurnRunState::WaitingForClient,
        None,
    )
    .await?;
    let payload = local_tool_request_payload(&obligation.request_payload)?;
    Ok(PermissionResultCoordinatorOutcome::DispatchLocalTool {
        run: transitioned,
        tool_obligation: Box::new(tool_obligation),
        tool_call_id,
        tool_name: payload.tool_name,
        args: payload.arguments,
        result: None,
    })
}
