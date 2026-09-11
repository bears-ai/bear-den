use std::{collections::BTreeMap, time::Duration};

use den_core::{client_tools::ClientToolName, DenError};
use den_runtime::{turn_obligations, turn_runs};
use den_service::DenState;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::methods::run::{persist_run_failed, RunFailureReason};

const DEFAULT_EXPIRY_BATCH_LIMIT: i64 = 1_000;

fn is_command_tool_result(obligation: &turn_obligations::TurnObligationRow) -> bool {
    if obligation.expected_responder_action != "tool_result" {
        return false;
    }
    let Some(tool_name) = obligation
        .request_payload
        .get("tool_name")
        .and_then(Value::as_str)
    else {
        return false;
    };
    matches!(
        ClientToolName::from_provider_alias(tool_name),
        Some(
            ClientToolName::RunCommand
                | ClientToolName::ProcessRun
                | ClientToolName::TerminalRunCommand
        )
    )
}

pub async fn run_client_obligation_expiry_loop(
    state: DenState,
    token: CancellationToken,
    interval: Duration,
) -> Result<(), DenError> {
    tracing::info!(
        interval_ms = interval.as_millis(),
        "BearWire client-obligation expiry loop enabled"
    );
    loop {
        tokio::select! {
            () = token.cancelled() => {
                tracing::info!("BearWire client-obligation expiry loop stopped");
                return Ok(());
            }
            () = tokio::time::sleep(interval) => {
                if let Err(err) = expire_client_obligations_once(&state, DEFAULT_EXPIRY_BATCH_LIMIT).await {
                    tracing::warn!(error = %err, "BearWire client-obligation expiry sweep failed");
                }
            }
        }
    }
}

pub async fn expire_client_obligations_once(
    state: &DenState,
    limit: i64,
) -> Result<usize, DenError> {
    let pool = &state.sqlx_pool;
    let affected = turn_obligations::client_obligations_requiring_reconciliation(
        pool,
        state.process_epoch_id,
        limit,
    )
    .await?;
    if affected.is_empty() {
        return Ok(0);
    }

    let mut by_run = BTreeMap::<String, (Vec<Value>, bool, bool)>::new();
    for obligation in affected {
        let command_outcome_unknown =
            obligation.is_claimed() && is_command_tool_result(&obligation);
        let interrupted_by_restart =
            obligation.belongs_to_prior_process_epoch(state.process_epoch_id);
        let entry = by_run
            .entry(obligation.run_id.clone())
            .or_insert_with(|| (Vec::new(), false, false));
        entry.0.push(json!({
            "obligation_id": obligation.id,
            "kind": obligation.kind,
            "expected_responder_action": obligation.expected_responder_action,
            "tool_call_id": obligation.tool_call_id,
            "tool_name": obligation.request_payload.get("tool_name"),
            "permission_id": obligation.permission_id,
            "timeout_ms": obligation.timeout_ms(),
            "created_at": obligation.created_at,
            "expires_at": obligation.expires_at(),
            "claimed": obligation.is_claimed(),
            "claimed_at": obligation.claimed_at,
            "lease_expires_at": obligation.lease_expires_at,
            "process_epoch_id": obligation.process_epoch_id(),
        }));
        entry.1 |= command_outcome_unknown;
        entry.2 |= interrupted_by_restart;
    }

    let affected_run_count = by_run.len();
    for (run_id, (affected_obligations, command_outcome_unknown, interrupted_by_restart)) in by_run
    {
        let Some(run) = turn_runs::get_run(pool, &run_id).await? else {
            tracing::warn!(
                run_id,
                affected_obligation_count = affected_obligations.len(),
                "reconciled BearWire client obligations reference a missing run"
            );
            continue;
        };
        let (reason, message, recovery) = if command_outcome_unknown {
            (
                RunFailureReason::CommandOutcomeUnknown,
                "Connection failure: Builder Bear lost contact with the BearWire service or connected work surface before it could confirm whether the command completed. To avoid duplicate changes, the command was not retried automatically.".to_string(),
                Some(json!({
                    "status": "outcome_unknown",
                    "automatic_retry_allowed": false,
                    "next_action": "run_state",
                    "next_action_params": { "run_id": run_id },
                })),
            )
        } else if interrupted_by_restart {
            (
                RunFailureReason::ServerRestartInterrupted,
                "Den restarted while waiting for the connected work surface to complete a local step. The process-local continuation could not be resumed automatically.".to_string(),
                None,
            )
        } else {
            let timed_out_permissions = affected_obligations
                .iter()
                .filter(|obligation| {
                    obligation
                        .get("expected_responder_action")
                        .and_then(Value::as_str)
                        == Some("permission_decision")
                })
                .filter_map(|obligation| obligation.get("permission_id").and_then(Value::as_str))
                .collect::<Vec<_>>();
            let timed_out_tool_results = affected_obligations
                .iter()
                .filter(|obligation| {
                    obligation
                        .get("expected_responder_action")
                        .and_then(Value::as_str)
                        == Some("tool_result")
                })
                .filter_map(|obligation| obligation.get("tool_name").and_then(Value::as_str))
                .collect::<Vec<_>>();
            let detail = match timed_out_permissions.as_slice() {
                [permission_id] => {
                    format!("Permission request {permission_id} expired without a decision.")
                }
                [] if !timed_out_tool_results.is_empty() => format!(
                    "The work surface did not return the requested result for {} before the local-step wait expired.",
                    timed_out_tool_results.join(", ")
                ),
                [] => "The connection was interrupted, or the required work-surface response did not arrive before the step timed out.".to_string(),
                permission_ids => format!(
                    "Permission requests {} expired without decisions.",
                    permission_ids.join(", ")
                ),
            };
            let reason = if timed_out_permissions.is_empty() {
                RunFailureReason::ClientObligationTimeout
            } else {
                RunFailureReason::PermissionDecisionExpired
            };
            let message = if timed_out_permissions.is_empty() {
                format!("{detail} Reconnect the work surface and send another message to retry.")
            } else {
                format!("{detail} No edit was performed. Reconnect the work surface and send another message to request approval again.")
            };
            (reason, message, None)
        };
        // The durable failure alone does not wake an ACP continuation already
        // blocked on this client response.
        state
            .turn_cancellations
            .cancel_run(&run.session_id, &run.run_id);
        state.tool_turns.cancel_active_turn(&run.session_id);
        persist_run_failed(
            pool,
            &run.session_id,
            &run.run_id,
            run.bear_id,
            run.user_id,
            reason,
            message,
            Some(json!({
                "affected_obligations": affected_obligations,
                "expired_obligations": if interrupted_by_restart { Value::Null } else { json!(affected_obligations) },
                "source": if interrupted_by_restart {
                    "bearwire_client_obligation_restart_reconciliation"
                } else {
                    "bearwire_client_obligation_expiry_loop"
                },
                "current_process_epoch_id": state.process_epoch_id,
                "recovery": if interrupted_by_restart {
                    json!({
                        "status": "interrupted",
                        "retryable": true,
                        "automatic_retry_allowed": false,
                        "next_action": "send_message",
                    })
                } else {
                    recovery.unwrap_or(Value::Null)
                },
            })),
        )
        .await;
    }

    Ok(affected_run_count)
}
