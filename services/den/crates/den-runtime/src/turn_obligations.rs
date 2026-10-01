use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::{
    client_tools::{client_tool_policy_json_for_provider, ClientToolName},
    DenError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnObligationKind {
    ToolResult,
    PermissionDecision,
    HumanInput,
    ResourceBinding,
    HandoffDecision,
}

impl TurnObligationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolResult => "tool_result",
            Self::PermissionDecision => "permission_decision",
            Self::HumanInput => "human_input",
            Self::ResourceBinding => "resource_binding",
            Self::HandoffDecision => "handoff_decision",
        }
    }

    pub fn try_from_storage(value: &str) -> Result<Self, DenError> {
        match value {
            "tool_result" => Ok(Self::ToolResult),
            "permission_decision" => Ok(Self::PermissionDecision),
            "human_input" => Ok(Self::HumanInput),
            "resource_binding" => Ok(Self::ResourceBinding),
            "handoff_decision" => Ok(Self::HandoffDecision),
            other => Err(DenError::ValidationError(format!(
                "unsupported turn obligation kind: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExpectedResponderAction {
    ToolResult,
    PermissionDecision,
    HumanInput,
    ResourceBinding,
    HandoffDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockingReason {
    ToolResult,
    PermissionDecision,
    HumanInput,
    ResourceBinding,
    HandoffDecision,
    Multiple,
}

impl BlockingReason {
    pub fn from_open_obligations(obligations: &[TurnObligationRow]) -> Option<Self> {
        Self::from_expected_actions(
            obligations
                .iter()
                .filter_map(|obligation| obligation.expected_action().ok()),
        )
    }

    pub fn from_expected_actions(
        actions: impl IntoIterator<Item = ExpectedResponderAction>,
    ) -> Option<Self> {
        let mut reason = None;
        for action in actions {
            let next = match action {
                ExpectedResponderAction::ToolResult => Self::ToolResult,
                ExpectedResponderAction::PermissionDecision => Self::PermissionDecision,
                ExpectedResponderAction::HumanInput => Self::HumanInput,
                ExpectedResponderAction::ResourceBinding => Self::ResourceBinding,
                ExpectedResponderAction::HandoffDecision => Self::HandoffDecision,
            };
            if reason.is_some_and(|current| current != next) {
                return Some(Self::Multiple);
            }
            reason = Some(next);
        }
        reason
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolResult => "tool_result",
            Self::PermissionDecision => "permission_decision",
            Self::HumanInput => "human_input",
            Self::ResourceBinding => "resource_binding",
            Self::HandoffDecision => "handoff_decision",
            Self::Multiple => "multiple",
        }
    }
}

impl ExpectedResponderAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolResult => "tool_result",
            Self::PermissionDecision => "permission_decision",
            Self::HumanInput => "human_input",
            Self::ResourceBinding => "resource_binding",
            Self::HandoffDecision => "handoff_decision",
        }
    }

    pub fn try_from_storage(value: &str) -> Result<Self, DenError> {
        match value {
            "tool_result" => Ok(Self::ToolResult),
            "permission_decision" => Ok(Self::PermissionDecision),
            "human_input" => Ok(Self::HumanInput),
            "resource_binding" => Ok(Self::ResourceBinding),
            "handoff_decision" => Ok(Self::HandoffDecision),
            other => Err(DenError::ValidationError(format!(
                "unsupported expected responder action: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnObligationState {
    Requested,
    WaitingForClient,
    ResultReceived,
    Continued,
    Failed,
    Cancelled,
}

impl TurnObligationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::WaitingForClient => "waiting_for_client",
            Self::ResultReceived => "result_received",
            Self::Continued => "continued",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn try_from_storage(value: &str) -> Result<Self, DenError> {
        match value {
            "requested" => Ok(Self::Requested),
            "waiting_for_client" => Ok(Self::WaitingForClient),
            "result_received" => Ok(Self::ResultReceived),
            "continued" => Ok(Self::Continued),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(DenError::ValidationError(format!(
                "unsupported turn obligation state: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnObligationRow {
    pub id: Uuid,
    pub run_id: String,
    pub session_id: String,
    pub kind: String,
    pub expected_responder_action: String,
    pub tool_call_id: Option<String>,
    pub permission_id: Option<String>,
    pub responder_ref_id: Option<String>,
    pub state: String,
    pub turn_step_id: Option<Uuid>,
    pub request_payload: Value,
    pub result_payload: Option<Value>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
    #[serde(skip_serializing)]
    pub lease_attempt_token_hash: Option<String>,
    pub claimed_at: Option<OffsetDateTime>,
    pub lease_expires_at: Option<OffsetDateTime>,
}

impl TurnObligationRow {
    pub fn timeout_ms(&self) -> i64 {
        let tool_name = self
            .request_payload
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let policy = ClientToolName::from_provider_alias(tool_name)
            .map(|_| client_tool_policy_json_for_provider(tool_name))
            .unwrap_or(Value::Null);
        let raw = match self.expected_responder_action.as_str() {
            "permission_decision" => policy
                .get("permission_timeout_ms")
                .and_then(Value::as_i64)
                .unwrap_or(120_000),
            "tool_result" => policy
                .get("total_timeout_ms")
                .or_else(|| policy.get("tool_timeout_ms"))
                .and_then(Value::as_i64)
                .unwrap_or(180_000),
            _ => 120_000,
        };
        raw.clamp(1_000, 600_000)
    }

    pub fn expires_at(&self) -> OffsetDateTime {
        self.lease_expires_at
            .unwrap_or_else(|| self.created_at + time::Duration::milliseconds(self.timeout_ms()))
    }

    pub fn is_claimed(&self) -> bool {
        self.lease_attempt_token_hash.is_some()
    }

    pub fn process_epoch_id(&self) -> Option<Uuid> {
        self.request_payload
            .get("den_process_epoch_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
    }

    pub fn belongs_to_prior_process_epoch(&self, current_process_epoch_id: Uuid) -> bool {
        self.process_epoch_id()
            .is_some_and(|process_epoch_id| process_epoch_id != current_process_epoch_id)
    }

    pub fn timed_out(&self, now: OffsetDateTime) -> bool {
        obligation_is_open(self) && self.expires_at() <= now
    }

    pub fn kind_value(&self) -> Result<TurnObligationKind, DenError> {
        TurnObligationKind::try_from_storage(&self.kind)
    }

    pub fn expected_action(&self) -> Result<ExpectedResponderAction, DenError> {
        ExpectedResponderAction::try_from_storage(&self.expected_responder_action)
    }

    pub fn state_value(&self) -> Result<TurnObligationState, DenError> {
        TurnObligationState::try_from_storage(&self.state)
    }
}

pub async fn create_turn_obligation_for_step(
    pool: &PgPool,
    run_id: &str,
    session_id: &str,
    turn_step_id: Option<Uuid>,
    kind: TurnObligationKind,
    expected_responder_action: ExpectedResponderAction,
    responder_ref_id: &str,
    request_payload: Value,
) -> Result<TurnObligationRow, DenError> {
    let kind = kind.as_str();
    let expected_responder_action = expected_responder_action.as_str();
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        INSERT INTO turn_obligations (
            run_id, session_id, turn_step_id, kind, expected_responder_action,
            responder_ref_id, state, request_payload
        ) VALUES ($1, $2, $3, $4, $5, $6, 'waiting_for_client', $7)
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, responder_ref_id, state, turn_step_id,
                  request_payload, result_payload, created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        run_id,
        session_id,
        turn_step_id,
        kind,
        expected_responder_action,
        responder_ref_id,
        request_payload,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn upsert_tool_result_obligation(
    pool: &PgPool,
    run_id: &str,
    session_id: &str,
    tool_call_id: &str,
    permission_id: Option<&str>,
    request_payload: Value,
) -> Result<TurnObligationRow, DenError> {
    upsert_tool_result_obligation_for_step(
        pool,
        run_id,
        session_id,
        None,
        tool_call_id,
        permission_id,
        request_payload,
    )
    .await
}

pub async fn upsert_tool_result_obligation_for_step(
    pool: &PgPool,
    run_id: &str,
    session_id: &str,
    turn_step_id: Option<Uuid>,
    tool_call_id: &str,
    permission_id: Option<&str>,
    request_payload: Value,
) -> Result<TurnObligationRow, DenError> {
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        INSERT INTO turn_obligations (
            run_id, session_id, turn_step_id, kind, expected_responder_action,
            tool_call_id, permission_id, state, request_payload
        ) VALUES ($1, $2, $3, 'tool_result', 'tool_result', $4, $5, 'waiting_for_client', $6)
        ON CONFLICT (run_id, tool_call_id) WHERE tool_call_id IS NOT NULL
        DO UPDATE SET session_id = EXCLUDED.session_id,
                      turn_step_id = COALESCE(EXCLUDED.turn_step_id, turn_obligations.turn_step_id),
                      expected_responder_action = EXCLUDED.expected_responder_action,
                      permission_id = COALESCE(EXCLUDED.permission_id, turn_obligations.permission_id),
                      state = CASE
                        WHEN turn_obligations.state IN ('result_received','continued','failed','cancelled')
                        THEN turn_obligations.state
                        ELSE EXCLUDED.state
                      END,
                      request_payload = EXCLUDED.request_payload,
                      updated_at = NOW()
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
                  created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        run_id,
        session_id,
        turn_step_id,
        tool_call_id,
        permission_id,
        request_payload,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn upsert_permission_decision_obligation(
    pool: &PgPool,
    run_id: &str,
    session_id: &str,
    permission_id: &str,
    tool_call_id: Option<&str>,
    request_payload: Value,
) -> Result<TurnObligationRow, DenError> {
    upsert_permission_decision_obligation_for_step(
        pool,
        run_id,
        session_id,
        None,
        permission_id,
        tool_call_id,
        request_payload,
    )
    .await
}

pub async fn upsert_permission_decision_obligation_for_step(
    pool: &PgPool,
    run_id: &str,
    session_id: &str,
    turn_step_id: Option<Uuid>,
    permission_id: &str,
    tool_call_id: Option<&str>,
    request_payload: Value,
) -> Result<TurnObligationRow, DenError> {
    if let Some(tool_call_id) = tool_call_id {
        if let Some(row) = sqlx::query_as!(TurnObligationRow,
            r#"
            UPDATE turn_obligations
            SET session_id = $2,
                turn_step_id = COALESCE($6, turn_step_id),
                kind = 'permission_decision',
                expected_responder_action = 'permission_decision',
                permission_id = $4,
                state = CASE
                    WHEN state IN ('result_received','continued','failed','cancelled')
                    THEN state
                    ELSE 'waiting_for_client'
                END,
                request_payload = $5,
                updated_at = NOW()
            WHERE run_id = $1
              AND tool_call_id = $3
              AND (permission_id IS NULL OR permission_id = $4)
            RETURNING id, run_id, session_id, kind, expected_responder_action,
                      tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
                      created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
            "#,
            run_id,
            session_id,
            tool_call_id,
            permission_id,
            request_payload.clone(),
            turn_step_id,
        )
        .fetch_optional(pool)
        .await?
        {
            return Ok(row);
        }
    }

    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        INSERT INTO turn_obligations (
            run_id, session_id, turn_step_id, kind, expected_responder_action,
            tool_call_id, permission_id, state, request_payload
        ) VALUES ($1, $2, $3, 'permission_decision', 'permission_decision', $4, $5, 'waiting_for_client', $6)
        ON CONFLICT (run_id, permission_id) WHERE permission_id IS NOT NULL
        DO UPDATE SET session_id = EXCLUDED.session_id,
                      turn_step_id = COALESCE(EXCLUDED.turn_step_id, turn_obligations.turn_step_id),
                      tool_call_id = COALESCE(EXCLUDED.tool_call_id, turn_obligations.tool_call_id),
                      state = CASE
                        WHEN turn_obligations.state IN ('result_received','continued','failed','cancelled')
                        THEN turn_obligations.state
                        ELSE EXCLUDED.state
                      END,
                      request_payload = EXCLUDED.request_payload,
                      updated_at = NOW()
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
                  created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        run_id,
        session_id,
        turn_step_id,
        tool_call_id,
        permission_id,
        request_payload,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn get_tool_call_obligation(
    pool: &PgPool,
    run_id: &str,
    tool_call_id: &str,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        SELECT id, run_id, session_id, kind, expected_responder_action,
               tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
               created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        FROM turn_obligations
        WHERE run_id = $1 AND tool_call_id = $2
        "#,
        run_id,
        tool_call_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn get_permission_obligation(
    pool: &PgPool,
    run_id: &str,
    permission_id: &str,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        SELECT id, run_id, session_id, kind, expected_responder_action,
               tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
               created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        FROM turn_obligations
        WHERE run_id = $1 AND permission_id = $2
        "#,
        run_id,
        permission_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub const TOOL_LEASE_DURATION_SECONDS: i64 = 30;
pub const TOOL_LEASE_RENEW_AFTER_SECONDS: i64 = 10;

pub fn lease_attempt_token_hash(attempt_token: &str) -> String {
    format!("{:x}", Sha256::digest(attempt_token.as_bytes()))
}

pub async fn claim_tool_execution(
    pool: &PgPool,
    obligation_id: Uuid,
    run_id: &str,
    session_id: &str,
    tool_call_id: &str,
    attempt_token_hash: &str,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(
        TurnObligationRow,
        r#"
        UPDATE turn_obligations
        SET lease_attempt_token_hash = $5,
            claimed_at = NOW(),
            lease_expires_at = NOW() + make_interval(secs => $6),
            updated_at = NOW()
        WHERE id = $1
          AND run_id = $2
          AND session_id = $3
          AND tool_call_id = $4
          AND kind = 'tool_result'
          AND expected_responder_action = 'tool_result'
          AND state = 'waiting_for_client'
          AND lease_attempt_token_hash IS NULL
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, responder_ref_id, state, turn_step_id,
                  request_payload, result_payload, created_at, updated_at, completed_at,
                  lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        obligation_id,
        run_id,
        session_id,
        tool_call_id,
        attempt_token_hash,
        TOOL_LEASE_DURATION_SECONDS as f64,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn renew_tool_execution(
    pool: &PgPool,
    obligation_id: Uuid,
    run_id: &str,
    session_id: &str,
    tool_call_id: &str,
    attempt_token_hash: &str,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(
        TurnObligationRow,
        r#"
        UPDATE turn_obligations
        SET lease_expires_at = NOW() + make_interval(secs => $6),
            updated_at = NOW()
        WHERE id = $1
          AND run_id = $2
          AND session_id = $3
          AND tool_call_id = $4
          AND kind = 'tool_result'
          AND state = 'waiting_for_client'
          AND lease_attempt_token_hash = $5
          AND lease_expires_at > NOW()
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, responder_ref_id, state, turn_step_id,
                  request_payload, result_payload, created_at, updated_at, completed_at,
                  lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        obligation_id,
        run_id,
        session_id,
        tool_call_id,
        attempt_token_hash,
        TOOL_LEASE_DURATION_SECONDS as f64,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn mark_claimed_result_received(
    pool: &PgPool,
    obligation_id: Uuid,
    attempt_token_hash: &str,
    result_payload: Value,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(
        TurnObligationRow,
        r#"
        UPDATE turn_obligations
        SET state = 'result_received',
            result_payload = $3,
            updated_at = NOW()
        WHERE id = $1
          AND state = 'waiting_for_client'
          AND lease_attempt_token_hash = $2
          AND lease_expires_at > NOW()
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, responder_ref_id, state, turn_step_id,
                  request_payload, result_payload, created_at, updated_at, completed_at,
                  lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        obligation_id,
        attempt_token_hash,
        result_payload,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn mark_result_received(
    pool: &PgPool,
    obligation_id: Uuid,
    result_payload: Value,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        UPDATE turn_obligations
        SET state = 'result_received',
            result_payload = $2,
            updated_at = NOW()
        WHERE id = $1
          AND state IN ('requested','waiting_for_client','result_received')
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
                  created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        obligation_id,
        result_payload,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// A winning Den-owned workspace grant has already committed with its
/// permission-result row. Promote its local tool exactly once: duplicate
/// replies cannot reopen an obligation after it becomes a tool-result wait.
pub async fn promote_workspace_grant_permission_to_tool_result(
    pool: &PgPool,
    obligation_id: Uuid,
    result_payload: Value,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        UPDATE turn_obligations
        SET kind = 'tool_result',
            expected_responder_action = 'tool_result',
            state = 'waiting_for_client',
            result_payload = $2,
            request_payload = jsonb_set(
                jsonb_set(request_payload, '{approval_required}', 'false'::jsonb, true),
                '{permission_granted}', 'true'::jsonb, true
            ),
            updated_at = NOW()
        WHERE id = $1
          AND kind = 'permission_decision'
          AND expected_responder_action = 'permission_decision'
          AND state IN ('requested','waiting_for_client','result_received')
          AND tool_call_id IS NOT NULL
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
                  created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        obligation_id,
        result_payload,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn mark_waiting_for_tool_result(
    pool: &PgPool,
    obligation_id: Uuid,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        UPDATE turn_obligations
        SET kind = 'tool_result',
            expected_responder_action = 'tool_result',
            state = 'waiting_for_client',
            request_payload = jsonb_set(
                jsonb_set(request_payload, '{approval_required}', 'false'::jsonb, true),
                '{permission_granted}', 'true'::jsonb,
                true
            ),
            updated_at = NOW()
        WHERE id = $1
          AND state IN ('requested','waiting_for_client','result_received')
          AND tool_call_id IS NOT NULL
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
                  created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        obligation_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn mark_continued(
    pool: &PgPool,
    obligation_id: Uuid,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        UPDATE turn_obligations
        SET state = 'continued',
            completed_at = COALESCE(completed_at, NOW()),
            updated_at = NOW()
        WHERE id = $1
          AND state IN ('result_received','continued')
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
                  created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        obligation_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn mark_failed(
    pool: &PgPool,
    obligation_id: Uuid,
) -> Result<Option<TurnObligationRow>, DenError> {
    let row = sqlx::query_as!(TurnObligationRow,
        r#"
        UPDATE turn_obligations
        SET state = 'failed',
            completed_at = COALESCE(completed_at, NOW()),
            updated_at = NOW()
        WHERE id = $1
          AND state IN ('requested','waiting_for_client','result_received')
        RETURNING id, run_id, session_id, kind, expected_responder_action,
                  tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
                  created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        obligation_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn open_client_obligations_for_step(
    pool: &PgPool,
    turn_step_id: Uuid,
) -> Result<Vec<TurnObligationRow>, DenError> {
    let rows = sqlx::query_as!(TurnObligationRow,
        r#"
        SELECT id, run_id, session_id, kind, expected_responder_action,
               tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
               created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        FROM turn_obligations
        WHERE turn_step_id = $1
          AND state IN ('requested','waiting_for_client')
        ORDER BY created_at ASC, id ASC
        "#,
        turn_step_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn open_client_obligations_for_run(
    pool: &PgPool,
    run_id: &str,
) -> Result<Vec<TurnObligationRow>, DenError> {
    let rows = sqlx::query_as!(TurnObligationRow,
        r#"
        SELECT id, run_id, session_id, kind, expected_responder_action,
               tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
               created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        FROM turn_obligations
        WHERE run_id = $1
          AND state IN ('requested','waiting_for_client')
        ORDER BY created_at ASC, id ASC
        "#,
        run_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn open_client_obligations_for_session(
    pool: &PgPool,
    session_id: &str,
) -> Result<Vec<TurnObligationRow>, DenError> {
    let rows = sqlx::query_as!(TurnObligationRow,
        r#"
        SELECT id, run_id, session_id, kind, expected_responder_action,
               tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload,
               created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        FROM turn_obligations
        WHERE session_id = $1
          AND state IN ('requested','waiting_for_client')
        ORDER BY created_at ASC, id ASC
        "#,
        session_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn open_client_obligations(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<TurnObligationRow>, DenError> {
    let limit = limit.clamp(1, 10_000);
    let rows = sqlx::query_as!(TurnObligationRow,
        r#"
        SELECT id, run_id, session_id, kind, expected_responder_action,
               tool_call_id, permission_id, responder_ref_id, state, turn_step_id,
               request_payload, result_payload, created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        FROM turn_obligations
        WHERE state IN ('requested','waiting_for_client')
        ORDER BY created_at ASC, id ASC
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn expire_open_client_obligations_for_session(
    pool: &PgPool,
    session_id: &str,
) -> Result<Vec<TurnObligationRow>, DenError> {
    let open = open_client_obligations_for_session(pool, session_id).await?;
    expire_open_client_obligations_from_rows(pool, open).await
}

pub async fn client_obligations_requiring_reconciliation(
    pool: &PgPool,
    current_process_epoch_id: Uuid,
    limit: i64,
) -> Result<Vec<TurnObligationRow>, DenError> {
    let open = open_client_obligations(pool, limit).await?;
    let now = OffsetDateTime::now_utc();
    Ok(open
        .into_iter()
        .filter(|obligation| {
            obligation.timed_out(now)
                || obligation.belongs_to_prior_process_epoch(current_process_epoch_id)
        })
        .collect())
}

pub async fn expire_open_client_obligations(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<TurnObligationRow>, DenError> {
    let open = open_client_obligations(pool, limit).await?;
    expire_open_client_obligations_from_rows(pool, open).await
}

async fn expire_open_client_obligations_from_rows(
    _pool: &PgPool,
    open: Vec<TurnObligationRow>,
) -> Result<Vec<TurnObligationRow>, DenError> {
    let now = OffsetDateTime::now_utc();
    // Do not settle an obligation here. Its caller terminalizes the run in one
    // transaction, which settles every open obligation. Mutating it first can
    // leave an active run without a sweepable obligation if terminalization
    // transiently fails.
    Ok(open
        .into_iter()
        .filter(|obligation| obligation.timed_out(now))
        .collect())
}

pub fn obligation_accepts_responder_action(
    obligation: &TurnObligationRow,
    action: ExpectedResponderAction,
) -> bool {
    obligation
        .expected_action()
        .map(|expected| expected == action)
        .unwrap_or(false)
}

pub fn obligation_is_open(obligation: &TurnObligationRow) -> bool {
    matches!(
        obligation.state_value(),
        Ok(TurnObligationState::Requested)
            | Ok(TurnObligationState::WaitingForClient)
            | Ok(TurnObligationState::ResultReceived)
    )
}

#[cfg(test)]
mod blocking_reason_tests {
    use super::{BlockingReason, ExpectedResponderAction};

    #[test]
    fn blocking_reason_is_derived_from_expected_actions() {
        assert_eq!(
            BlockingReason::from_expected_actions([ExpectedResponderAction::ToolResult]),
            Some(BlockingReason::ToolResult)
        );
        assert_eq!(
            BlockingReason::from_expected_actions([
                ExpectedResponderAction::ToolResult,
                ExpectedResponderAction::PermissionDecision,
            ]),
            Some(BlockingReason::Multiple)
        );
        assert_eq!(
            BlockingReason::from_expected_actions(std::iter::empty()),
            None
        );
    }
}
