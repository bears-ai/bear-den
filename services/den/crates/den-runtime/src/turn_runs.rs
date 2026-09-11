use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::DenError;

use crate::turn_ids::{ClientSessionId, TurnRunId};
use crate::turn_obligations::TurnObligationState;
use bearwire_protocol::lifecycle::FocusedExecutionTransitionReason;

mod transitions;

use transitions::{append_focused_run_transition_on, transition_nonterminal_run_on};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnRunState {
    Accepted,
    Running,
    WaitingForClient,
    Continuing,
    Completed,
    Failed,
    Cancelled,
}

impl TurnRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Running => "running",
            Self::WaitingForClient => "waiting_for_client",
            Self::Continuing => "continuing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn try_from_storage(value: &str) -> Result<Self, DenError> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "running" => Ok(Self::Running),
            "waiting_for_client" => Ok(Self::WaitingForClient),
            "continuing" => Ok(Self::Continuing),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(DenError::ValidationError(format!(
                "unsupported turn run state: {other}"
            ))),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn allows_open_obligation(self) -> bool {
        !self.is_terminal()
    }

    fn terminal_obligation_state(self) -> Option<TurnObligationState> {
        match self {
            Self::Completed => Some(TurnObligationState::Continued),
            Self::Failed => Some(TurnObligationState::Failed),
            Self::Cancelled => Some(TurnObligationState::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnRunRow {
    pub id: Uuid,
    pub run_id: String,
    pub session_id: String,
    pub bear_id: Uuid,
    pub user_id: i32,
    pub state: String,
    pub terminal_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

impl TurnRunRow {
    pub fn state_value(&self) -> Result<TurnRunState, DenError> {
        TurnRunState::try_from_storage(&self.state)
    }
}

pub async fn create_run(
    pool: &PgPool,
    run_id: &str,
    session_id: &str,
    bear_id: Uuid,
    user_id: i32,
) -> Result<TurnRunRow, DenError> {
    let run_id = TurnRunId::new(run_id.to_owned())?;
    let session_id = ClientSessionId::new(session_id.to_owned())?;
    create_run_with_ids(pool, &run_id, &session_id, bear_id, user_id).await
}

pub async fn create_run_with_ids(
    pool: &PgPool,
    run_id: &TurnRunId,
    session_id: &ClientSessionId,
    bear_id: Uuid,
    user_id: i32,
) -> Result<TurnRunRow, DenError> {
    let row = sqlx::query_as!(
        TurnRunRow,
        r#"
        INSERT INTO turn_runs (run_id, session_id, bear_id, user_id, state)
        VALUES ($1, $2, $3, $4, 'accepted')
        RETURNING id, run_id, session_id, bear_id, user_id, state,
                  terminal_reason AS "terminal_reason?", created_at, updated_at,
                  completed_at AS "completed_at?"
        "#,
        run_id.as_str(),
        session_id.as_str(),
        bear_id,
        user_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

#[derive(Debug, Clone, Serialize)]
pub enum CreateOrAttachRun {
    Created(TurnRunRow),
    Attached(TurnRunRow),
}

/// Serializes non-superseding starts for one client session. The advisory lock
/// closes the gap between checking for a live run and creating its replacement.
pub async fn create_or_attach_active_run_with_ids(
    pool: &PgPool,
    run_id: &TurnRunId,
    session_id: &ClientSessionId,
    bear_id: Uuid,
    user_id: i32,
) -> Result<CreateOrAttachRun, DenError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(session_id.as_str())
        .execute(&mut *transaction)
        .await?;

    if let Some(row) = sqlx::query_as!(
        TurnRunRow,
        r#"
        SELECT id, run_id, session_id, bear_id, user_id, state,
               terminal_reason AS "terminal_reason?", created_at, updated_at,
               completed_at AS "completed_at?"
        FROM turn_runs
        WHERE session_id = $1
          AND state IN ('accepted', 'running', 'waiting_for_client', 'continuing')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        session_id.as_str(),
    )
    .fetch_optional(&mut *transaction)
    .await?
    {
        transaction.commit().await?;
        return Ok(CreateOrAttachRun::Attached(row));
    }

    let row = sqlx::query_as!(
        TurnRunRow,
        r#"
        INSERT INTO turn_runs (run_id, session_id, bear_id, user_id, state)
        VALUES ($1, $2, $3, $4, 'accepted')
        RETURNING id, run_id, session_id, bear_id, user_id, state,
                  terminal_reason AS "terminal_reason?", created_at, updated_at,
                  completed_at AS "completed_at?"
        "#,
        run_id.as_str(),
        session_id.as_str(),
        bear_id,
        user_id,
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(CreateOrAttachRun::Created(row))
}

pub async fn get_run(pool: &PgPool, run_id: &str) -> Result<Option<TurnRunRow>, DenError> {
    let row = sqlx::query_as!(
        TurnRunRow,
        r#"
        SELECT id, run_id, session_id, bear_id, user_id, state,
               terminal_reason AS "terminal_reason?", created_at, updated_at,
               completed_at AS "completed_at?"
        FROM turn_runs
        WHERE run_id = $1
        "#,
        run_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn list_recent_failed_runs(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<TurnRunRow>, DenError> {
    let limit = limit.clamp(1, 100);
    let rows = sqlx::query_as!(
        TurnRunRow,
        r#"
        SELECT id, run_id, session_id, bear_id, user_id, state,
               terminal_reason AS "terminal_reason?", created_at, updated_at,
               completed_at AS "completed_at?"
        FROM turn_runs
        WHERE state = 'failed'
        ORDER BY completed_at DESC NULLS LAST, updated_at DESC
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn active_run_for_session(
    pool: &PgPool,
    session_id: &str,
) -> Result<Option<TurnRunRow>, DenError> {
    let row = sqlx::query_as!(
        TurnRunRow,
        r#"
        SELECT id, run_id, session_id, bear_id, user_id, state,
               terminal_reason AS "terminal_reason?", created_at, updated_at,
               completed_at AS "completed_at?"
        FROM turn_runs
        WHERE session_id = $1
          AND state IN ('accepted', 'running', 'waiting_for_client', 'continuing')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        session_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn supersede_active_run_for_session(
    pool: &PgPool,
    session_id: &str,
    bear_id: Uuid,
    user_id: i32,
    reason: &str,
) -> Result<Option<TurnRunRow>, DenError> {
    let row = sqlx::query_as!(
        TurnRunRow,
        r#"
        UPDATE turn_runs
        SET state = 'failed', terminal_reason = $4, completed_at = NOW(), updated_at = NOW()
        WHERE id = (
            SELECT id
            FROM turn_runs
            WHERE session_id = $1 AND bear_id = $2 AND user_id = $3
              AND state IN ('accepted', 'running', 'waiting_for_client', 'continuing')
            ORDER BY created_at DESC
            LIMIT 1
        )
        RETURNING id, run_id, session_id, bear_id, user_id, state,
                  terminal_reason AS "terminal_reason?", created_at, updated_at,
                  completed_at AS "completed_at?"
        "#,
        session_id,
        bear_id,
        user_id,
        reason,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnObligationResultRow {
    pub id: Uuid,
    pub run_id: String,
    pub obligation_kind: String,
    pub obligation_id: String,
    pub result_hash: String,
    pub payload_json: serde_json::Value,
    pub turn_step_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TurnObligationResultRecord {
    Inserted { row: TurnObligationResultRow },
    DuplicateIdentical { row: TurnObligationResultRow },
    DuplicateConflict { existing_hash: String },
}

fn result_hash(payload: &serde_json::Value) -> Result<String, DenError> {
    let bytes = serde_json::to_vec(payload).map_err(|err| {
        DenError::System(format!("serialize BearWire client result failed: {err}"))
    })?;
    let digest = Sha256::digest(&bytes);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest))
}

pub async fn existing_client_result_for_payload(
    pool: &PgPool,
    run_id: &str,
    obligation_kind: &str,
    obligation_id: &str,
    payload_json: &serde_json::Value,
) -> Result<Option<TurnObligationResultRecord>, DenError> {
    let Some(row) = sqlx::query_as!(
        TurnObligationResultRow,
        r#"
        SELECT id, run_id, obligation_kind, obligation_id, result_hash,
               payload_json AS "payload_json: serde_json::Value",
               turn_step_id AS "turn_step_id?", created_at
        FROM turn_obligation_results
        WHERE run_id = $1 AND obligation_kind = $2 AND obligation_id = $3
        "#,
        run_id,
        obligation_kind,
        obligation_id,
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    if row.result_hash == result_hash(payload_json)? {
        Ok(Some(TurnObligationResultRecord::DuplicateIdentical { row }))
    } else {
        Ok(Some(TurnObligationResultRecord::DuplicateConflict {
            existing_hash: row.result_hash,
        }))
    }
}

pub async fn record_client_result(
    pool: &PgPool,
    run_id: &str,
    obligation_kind: &str,
    obligation_id: &str,
    payload_json: serde_json::Value,
) -> Result<TurnObligationResultRecord, DenError> {
    record_client_result_for_step(
        pool,
        run_id,
        None,
        obligation_kind,
        obligation_id,
        payload_json,
    )
    .await
}

pub enum ClaimedToolResultRecord {
    ClaimRejected,
    Recorded(TurnObligationResultRecord),
}

pub async fn record_claimed_tool_result_for_step(
    pool: &PgPool,
    run_id: &str,
    turn_step_id: Option<Uuid>,
    obligation_id: Uuid,
    tool_call_id: &str,
    attempt_token_hash: &str,
    payload_json: serde_json::Value,
) -> Result<ClaimedToolResultRecord, DenError> {
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query_scalar!(
        r#"
        UPDATE turn_obligations
        SET state = 'result_received', result_payload = $4, updated_at = NOW()
        WHERE id = $1
          AND run_id = $2
          AND tool_call_id = $3
          AND kind = 'tool_result'
          AND state = 'waiting_for_client'
          AND lease_attempt_token_hash = $5
          AND lease_expires_at > NOW()
        RETURNING id
        "#,
        obligation_id,
        run_id,
        tool_call_id,
        &payload_json,
        attempt_token_hash,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if claimed.is_none() {
        tx.rollback().await?;
        return Ok(ClaimedToolResultRecord::ClaimRejected);
    }

    let hash = result_hash(&payload_json)?;
    let inserted = sqlx::query_as!(
        TurnObligationResultRow,
        r#"
        INSERT INTO turn_obligation_results (
            run_id, turn_step_id, obligation_kind, obligation_id, result_hash, payload_json
        ) VALUES ($1, $2, 'tool', $3, $4, $5)
        ON CONFLICT (run_id, obligation_kind, obligation_id) DO NOTHING
        RETURNING id, run_id, obligation_kind, obligation_id, result_hash,
                  payload_json AS "payload_json: serde_json::Value",
                  turn_step_id AS "turn_step_id?", created_at
        "#,
        run_id,
        turn_step_id,
        tool_call_id,
        &hash,
        &payload_json,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let record = if let Some(row) = inserted {
        TurnObligationResultRecord::Inserted { row }
    } else {
        let row = sqlx::query_as!(
            TurnObligationResultRow,
            r#"
            SELECT id, run_id, obligation_kind, obligation_id, result_hash,
                   payload_json AS "payload_json: serde_json::Value",
                   turn_step_id AS "turn_step_id?", created_at
            FROM turn_obligation_results
            WHERE run_id = $1 AND obligation_kind = 'tool' AND obligation_id = $2
            "#,
            run_id,
            tool_call_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if row.result_hash == hash {
            TurnObligationResultRecord::DuplicateIdentical { row }
        } else {
            TurnObligationResultRecord::DuplicateConflict {
                existing_hash: row.result_hash,
            }
        }
    };
    tx.commit().await?;
    Ok(ClaimedToolResultRecord::Recorded(record))
}

pub async fn record_client_result_for_step(
    pool: &PgPool,
    run_id: &str,
    turn_step_id: Option<Uuid>,
    obligation_kind: &str,
    obligation_id: &str,
    payload_json: serde_json::Value,
) -> Result<TurnObligationResultRecord, DenError> {
    let hash = result_hash(&payload_json)?;
    let inserted = sqlx::query_as!(
        TurnObligationResultRow,
        r#"
        INSERT INTO turn_obligation_results (
            run_id, turn_step_id, obligation_kind, obligation_id, result_hash, payload_json
        ) VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (run_id, obligation_kind, obligation_id) DO NOTHING
        RETURNING id, run_id, obligation_kind, obligation_id, result_hash,
                  payload_json AS "payload_json: serde_json::Value",
                  turn_step_id AS "turn_step_id?", created_at
        "#,
        run_id,
        turn_step_id,
        obligation_kind,
        obligation_id,
        &hash,
        &payload_json,
    )
    .fetch_optional(pool)
    .await?;
    if let Some(row) = inserted {
        return Ok(TurnObligationResultRecord::Inserted { row });
    }

    let row = sqlx::query_as!(
        TurnObligationResultRow,
        r#"
        SELECT id, run_id, obligation_kind, obligation_id, result_hash,
               payload_json AS "payload_json: serde_json::Value",
               turn_step_id AS "turn_step_id?", created_at
        FROM turn_obligation_results
        WHERE run_id = $1 AND obligation_kind = $2 AND obligation_id = $3
        "#,
        run_id,
        obligation_kind,
        obligation_id,
    )
    .fetch_one(pool)
    .await?;
    if row.result_hash == hash {
        Ok(TurnObligationResultRecord::DuplicateIdentical { row })
    } else {
        Ok(TurnObligationResultRecord::DuplicateConflict {
            existing_hash: row.result_hash,
        })
    }
}

pub async fn client_result_count_for_run_kind(
    pool: &PgPool,
    run_id: &str,
    obligation_kind: &str,
) -> Result<i64, DenError> {
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!"
        FROM turn_obligation_results
        WHERE run_id = $1 AND obligation_kind = $2
        "#,
        run_id,
        obligation_kind,
    )
    .fetch_one(pool)
    .await?;
    Ok(count)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishRunResult {
    pub settled_obligations: u64,
    pub settled_steps: u64,
    pub event_sequence: i64,
}

pub async fn complete_run(
    pool: &PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: Uuid,
    user_id: i32,
    terminal_reason: Option<&str>,
    data: Value,
) -> Result<Option<FinishRunResult>, DenError> {
    finish_run(
        pool,
        session_id,
        run_id,
        bear_id,
        user_id,
        TurnRunState::Completed,
        FocusedExecutionTransitionReason::RunCompleted,
        "run.completed",
        terminal_reason,
        data,
    )
    .await
}

pub async fn fail_run(
    pool: &PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: Uuid,
    user_id: i32,
    terminal_reason: &str,
    data: Value,
) -> Result<Option<FinishRunResult>, DenError> {
    finish_run(
        pool,
        session_id,
        run_id,
        bear_id,
        user_id,
        TurnRunState::Failed,
        FocusedExecutionTransitionReason::RunFailed,
        "run.failed",
        Some(terminal_reason),
        data,
    )
    .await
}

pub async fn cancel_run(
    pool: &PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: Uuid,
    user_id: i32,
    terminal_reason: &str,
    data: Value,
) -> Result<Option<FinishRunResult>, DenError> {
    cancel_run_with_transition_reason(
        pool,
        session_id,
        run_id,
        bear_id,
        user_id,
        terminal_reason,
        data,
        FocusedExecutionTransitionReason::RunCancelled,
    )
    .await
}

pub async fn cancel_run_with_transition_reason(
    pool: &PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: Uuid,
    user_id: i32,
    terminal_reason: &str,
    data: Value,
    transition_reason: FocusedExecutionTransitionReason,
) -> Result<Option<FinishRunResult>, DenError> {
    finish_run(
        pool,
        session_id,
        run_id,
        bear_id,
        user_id,
        TurnRunState::Cancelled,
        transition_reason,
        "run.cancelled",
        Some(terminal_reason),
        data,
    )
    .await
}

async fn finish_run(
    pool: &PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: Uuid,
    user_id: i32,
    state: TurnRunState,
    transition_reason: FocusedExecutionTransitionReason,
    event_type: &'static str,
    terminal_reason: Option<&str>,
    data: Value,
) -> Result<Option<FinishRunResult>, DenError> {
    let obligation_state = state
        .terminal_obligation_state()
        .expect("typed terminal method supplies a terminal state");
    let mut event = bearwire_protocol::wire::BearWireEvent::ephemeral(event_type, data);
    event.bear_id = Some(bear_id.to_string());
    event.human_id = Some(user_id.to_string());
    event.session_id = Some(session_id.to_string());
    event.run_id = Some(run_id.to_string());
    let settlement_state = obligation_state.as_str();
    let mut tx = pool.begin().await?;
    let Some(run) = sqlx::query_as!(
        TurnRunRow,
        r#"
        SELECT id, run_id, session_id, bear_id, user_id, state,
               terminal_reason AS "terminal_reason?", created_at, updated_at,
               completed_at AS "completed_at?"
        FROM turn_runs
        WHERE run_id = $1 AND session_id = $2
        FOR UPDATE
        "#,
        run_id,
        session_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.rollback().await?;
        return Ok(None);
    };
    let previous_state = run.state_value()?;
    if previous_state.is_terminal() {
        tx.rollback().await?;
        return Ok(None);
    }
    let claimed = sqlx::query!(
        r#"
        UPDATE turn_runs
        SET state = $2,
            terminal_reason = $3,
            updated_at = NOW(),
            completed_at = COALESCE(completed_at, NOW())
        WHERE run_id = $1
          AND session_id = $4
          AND state NOT IN ('completed','failed','cancelled')
        "#,
        run_id,
        state.as_str(),
        terminal_reason,
        session_id,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        tx.rollback().await?;
        return Ok(None);
    }
    let settled_obligations = sqlx::query!(
        r#"
        UPDATE turn_obligations
        SET state = $2,
            completed_at = COALESCE(completed_at, NOW()),
            updated_at = NOW()
        WHERE run_id = $1
          AND state IN ('requested','waiting_for_client','result_received')
        "#,
        run_id,
        settlement_state,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let settled_steps = sqlx::query!(
        r#"
        UPDATE turn_steps
        SET state = $2,
            closed_at = COALESCE(closed_at, NOW())
        WHERE run_id = $1
          AND state IN ('streaming_model','waiting_for_client','ready_to_continue')
        "#,
        run_id,
        settlement_state,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    append_focused_run_transition_on(
        &mut tx,
        &run,
        previous_state,
        state,
        0,
        Some(transition_reason),
    )
    .await?;
    event.data["settled_obligations"] = serde_json::json!(settled_obligations);
    event.data["settled_steps"] = serde_json::json!(settled_steps);
    let terminal_event = crate::bearwire_events::append_bearwire_event_on(
        &mut tx,
        session_id,
        Some(bear_id),
        Some(user_id),
        event,
    )
    .await?;
    tx.commit().await?;
    Ok(Some(FinishRunResult {
        settled_obligations,
        settled_steps,
        event_sequence: terminal_event.sequence_no,
    }))
}

pub const TECHNICAL_BUDGET_RECOVERY_SNAPSHOT_VERSION: u64 = 1;

/// Sanitized, versioned inputs needed to restart a Pair turn after a
/// technical budget boundary survives process loss. The caller must not place
/// credentials, provider continuation tokens, or live stream handles here.
///
/// `start_request` intentionally remains JSON at this persistence boundary:
/// BearWire owns the concrete `TurnStartRequest` type and is responsible for
/// validating and reconstructing it before a fresh stream starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechnicalBudgetRecoverySnapshot {
    pub version: u64,
    pub session_id: String,
    pub bear_id: Uuid,
    pub user_id: i32,
    pub selected_task_id: Option<Uuid>,
    pub start_request: Value,
}

impl TechnicalBudgetRecoverySnapshot {
    pub fn new(
        session_id: String,
        bear_id: Uuid,
        user_id: i32,
        selected_task_id: Option<Uuid>,
        start_request: Value,
    ) -> Self {
        Self {
            version: TECHNICAL_BUDGET_RECOVERY_SNAPSHOT_VERSION,
            session_id,
            bear_id,
            user_id,
            selected_task_id,
            start_request,
        }
    }
}

pub enum TechnicalBudgetContinuationClaim {
    Claimed(TurnRunRow),
    /// A sibling continuation already owns the successor slice for this run.
    AlreadyClaimed,
    /// The run is terminal or missing, so it cannot be continued.
    RunStateConflict {
        actual_state: Option<String>,
    },
}

pub struct TurnRunRecoverySnapshotRow {
    pub run_id: String,
    pub reason: String,
    pub snapshot: Value,
    pub recovery_lease_id: Option<Uuid>,
    pub recovery_lease_expires_at: Option<OffsetDateTime>,
    pub recovered_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Stores the replay inputs atomically with the only technical-budget claim
/// that may survive process loss. `snapshot` must be sanitized by the caller:
/// never include credentials, stream handles, or provider continuation tokens.
pub async fn claim_technical_budget_continuation(
    pool: &PgPool,
    run_id: &str,
    reason: &str,
    snapshot: &Value,
) -> Result<TechnicalBudgetContinuationClaim, DenError> {
    let mut tx = pool.begin().await?;
    let row = transition_nonterminal_run_on(
        &mut tx,
        run_id,
        &[TurnRunState::Running],
        TurnRunState::Continuing,
        Some(reason),
    )
    .await?;
    if row.is_some() {
        sqlx::query!(
            r#"
            INSERT INTO turn_run_recovery_snapshots (run_id, reason, snapshot)
            VALUES ($1, $2, $3)
            "#,
            run_id,
            reason,
            snapshot,
        )
        .execute(&mut *tx)
        .await?;
    }
    let outcome = match row {
        Some(row) => TechnicalBudgetContinuationClaim::Claimed(row),
        None => {
            let actual_state =
                sqlx::query_scalar::<_, String>("SELECT state FROM turn_runs WHERE run_id = $1")
                    .bind(run_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            match actual_state.as_deref() {
                Some("continuing") => TechnicalBudgetContinuationClaim::AlreadyClaimed,
                _ => TechnicalBudgetContinuationClaim::RunStateConflict { actual_state },
            }
        }
    };
    tx.commit().await?;
    Ok(outcome)
}

/// Reads an unconsumed technical-budget recovery snapshot so the caller can
/// authenticate and validate its ownership before attempting a lease.
pub async fn technical_budget_recovery_snapshot(
    pool: &PgPool,
    run_id: &str,
) -> Result<Option<TurnRunRecoverySnapshotRow>, DenError> {
    let row = sqlx::query_as!(
        TurnRunRecoverySnapshotRow,
        r#"
        SELECT snapshots.run_id, snapshots.reason, snapshots.snapshot,
               snapshots.recovery_lease_id AS "recovery_lease_id?",
               snapshots.recovery_lease_expires_at AS "recovery_lease_expires_at?",
               snapshots.recovered_at AS "recovered_at?", snapshots.created_at,
               snapshots.updated_at
        FROM turn_run_recovery_snapshots AS snapshots
        JOIN turn_runs AS runs ON runs.run_id = snapshots.run_id
        WHERE snapshots.run_id = $1
          AND runs.state = 'continuing'
          AND snapshots.recovered_at IS NULL
        "#,
        run_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Atomically leases an eligible, stranded technical-budget continuation.
/// Callers must still authenticate and revalidate current-task authority
/// before rebuilding execution from the returned snapshot.
pub async fn lease_technical_budget_recovery(
    pool: &PgPool,
    run_id: &str,
    lease_id: Uuid,
) -> Result<Option<TurnRunRecoverySnapshotRow>, DenError> {
    let row = sqlx::query_as!(
        TurnRunRecoverySnapshotRow,
        r#"
        UPDATE turn_run_recovery_snapshots AS snapshots
        SET recovery_lease_id = $2,
            recovery_lease_expires_at = NOW() + INTERVAL '5 minutes',
            updated_at = NOW()
        FROM turn_runs AS runs
        WHERE snapshots.run_id = runs.run_id
          AND snapshots.run_id = $1
          AND runs.state = 'continuing'
          AND snapshots.recovered_at IS NULL
          AND (snapshots.recovery_lease_expires_at IS NULL
               OR snapshots.recovery_lease_expires_at < NOW())
        RETURNING snapshots.run_id, snapshots.reason, snapshots.snapshot,
                  snapshots.recovery_lease_id AS "recovery_lease_id?",
                  snapshots.recovery_lease_expires_at AS "recovery_lease_expires_at?",
                  snapshots.recovered_at AS "recovered_at?", snapshots.created_at,
                  snapshots.updated_at
        "#,
        run_id,
        lease_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Releases a recovery lease when rebuilding its replacement run fails. The
/// snapshot remains eligible for a later authenticated retry.
pub async fn release_technical_budget_recovery(
    pool: &PgPool,
    run_id: &str,
    lease_id: Uuid,
) -> Result<bool, DenError> {
    let affected = sqlx::query!(
        r#"
        UPDATE turn_run_recovery_snapshots
        SET recovery_lease_id = NULL,
            recovery_lease_expires_at = NULL,
            updated_at = NOW()
        WHERE run_id = $1
          AND recovery_lease_id = $2
          AND recovered_at IS NULL
        "#,
        run_id,
        lease_id,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// Marks a leased technical-budget snapshot consumed after its replacement
/// turn has been accepted. The lease id prevents a stale recovery worker from
/// consuming another worker's claim.
pub async fn complete_technical_budget_recovery(
    pool: &PgPool,
    run_id: &str,
    lease_id: Uuid,
) -> Result<bool, DenError> {
    let affected = sqlx::query!(
        r#"
        UPDATE turn_run_recovery_snapshots
        SET recovered_at = NOW(),
            recovery_lease_id = NULL,
            recovery_lease_expires_at = NULL,
            updated_at = NOW()
        WHERE run_id = $1
          AND recovery_lease_id = $2
          AND recovered_at IS NULL
        "#,
        run_id,
        lease_id,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

pub async fn claim_run_continuation(
    pool: &PgPool,
    run_id: &str,
    terminal_reason: Option<&str>,
) -> Result<Option<TurnRunRow>, DenError> {
    let mut tx = pool.begin().await?;
    let row = transition_nonterminal_run_on(
        &mut tx,
        run_id,
        &[
            TurnRunState::Accepted,
            TurnRunState::Running,
            TurnRunState::WaitingForClient,
        ],
        TurnRunState::Continuing,
        terminal_reason,
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

/// Starts work after a successful continuation claim.
///
/// This deliberately requires `continuing`: it consumes the one-shot continuation
/// claim, so a duplicate budget boundary cannot start another successor slice. A
/// technical-budget snapshot is only for process-loss recovery; consume it in the
/// same transaction when the in-process successor successfully starts.
pub async fn begin_claimed_run_continuation(
    pool: &PgPool,
    run_id: &str,
) -> Result<Option<TurnRunRow>, DenError> {
    let mut tx = pool.begin().await?;
    let row = transition_nonterminal_run_on(
        &mut tx,
        run_id,
        &[TurnRunState::Continuing],
        TurnRunState::Running,
        None,
    )
    .await?;
    if row.is_some() {
        sqlx::query!(
            "DELETE FROM turn_run_recovery_snapshots WHERE run_id = $1",
            run_id,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(row)
}

/// Releases a continuation claim when durable setup fails before the successor
/// step begins. The caller will surface the failure, so `running` accurately
/// means the normal run-failure path may settle it rather than advertising a
/// resumable successor that was never started.
pub async fn release_claimed_run_continuation(
    pool: &PgPool,
    run_id: &str,
) -> Result<Option<TurnRunRow>, DenError> {
    let mut tx = pool.begin().await?;
    let row = transition_nonterminal_run_on(
        &mut tx,
        run_id,
        &[TurnRunState::Continuing],
        TurnRunState::Running,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn transition_run(
    pool: &PgPool,
    run_id: &str,
    state: TurnRunState,
    terminal_reason: Option<&str>,
) -> Result<Option<TurnRunRow>, DenError> {
    if state.is_terminal() {
        return Err(DenError::ValidationError(format!(
            "terminal run state {} must use its typed terminal method",
            state.as_str()
        )));
    }
    let mut tx = pool.begin().await?;
    let row = transition_nonterminal_run_on(
        &mut tx,
        run_id,
        &[
            TurnRunState::Accepted,
            TurnRunState::Running,
            TurnRunState::WaitingForClient,
            TurnRunState::Continuing,
        ],
        state,
        terminal_reason,
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use den_core::DenError;

    use super::{transitions::transition_reason, TurnRunState};
    use crate::turn_ids::{ClientSessionId, TurnRunId};
    use bearwire_protocol::lifecycle::FocusedExecutionTransitionReason;

    #[test]
    fn terminal_run_with_open_obligation_is_invalid() {
        for terminal in [
            TurnRunState::Completed,
            TurnRunState::Failed,
            TurnRunState::Cancelled,
        ] {
            assert_eq!(
                terminal.terminal_obligation_state().is_some(),
                terminal.is_terminal()
            );
            assert!(!terminal.allows_open_obligation());
        }

        assert!(TurnRunState::WaitingForClient.allows_open_obligation());
        assert_eq!(
            transition_reason(TurnRunState::Running, TurnRunState::WaitingForClient),
            FocusedExecutionTransitionReason::ClientWaitOpened
        );
        assert_eq!(
            transition_reason(TurnRunState::WaitingForClient, TurnRunState::Continuing),
            FocusedExecutionTransitionReason::ClientWaitCleared
        );
        assert_eq!(
            transition_reason(TurnRunState::Continuing, TurnRunState::Completed),
            FocusedExecutionTransitionReason::RunCompleted
        );
    }

    #[test]
    fn turn_run_boundary_ids_reject_blank_strings() {
        assert!(matches!(
            TurnRunId::new("  "),
            Err(DenError::ValidationError(message)) if message == "TurnRunId must not be empty"
        ));
        assert!(matches!(
            ClientSessionId::new(""),
            Err(DenError::ValidationError(message)) if message == "ClientSessionId must not be empty"
        ));

        let run_id = TurnRunId::new("run_abc").expect("valid run id");
        let session_id = ClientSessionId::new("session_xyz").expect("valid session id");
        assert_eq!(run_id.as_str(), "run_abc");
        assert_eq!(session_id.as_str(), "session_xyz");
    }
}
