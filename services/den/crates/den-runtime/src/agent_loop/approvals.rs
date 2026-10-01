use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use den_core::DenError;

#[derive(Debug, Clone)]
pub struct NativeApprovalRow {
    pub approval_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments_json: Value,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeApprovalDecision {
    Approve,
    Deny,
}

impl NativeApprovalDecision {
    const fn status(self) -> &'static str {
        match self {
            Self::Approve => "approved",
            Self::Deny => "denied",
        }
    }
}

pub async fn create_native_approval(
    pool: &PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    client_session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    arguments: &Value,
) -> Result<String, DenError> {
    let approval_id = Uuid::new_v4().to_string();
    sqlx::query!(
        r"
        INSERT INTO runtime_approvals (
            approval_id, bear_id, conversation_id, client_session_id,
            tool_call_id, tool_name, arguments_json
        ) VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb)
        ",
        approval_id,
        bear_id,
        conversation_id,
        client_session_id,
        tool_call_id,
        tool_name,
        arguments.clone()
    )
    .execute(pool)
    .await
    .map_err(|e| DenError::System(format!("create runtime approval failed: {e}")))?;
    Ok(approval_id)
}

pub async fn decide_native_approval(
    pool: &PgPool,
    approval_id: &str,
    decision: NativeApprovalDecision,
    reason: Option<&str>,
) -> Result<(), DenError> {
    let status = decision.status();
    let updated = sqlx::query!(
        r"
        UPDATE runtime_approvals
        SET status = $2, decision_reason = $3, decided_at = NOW()
        WHERE approval_id = $1 AND consumed_at IS NULL
        ",
        approval_id,
        status,
        reason
    )
    .execute(pool)
    .await
    .map_err(|e| DenError::System(format!("decide runtime approval failed: {e}")))?;
    if updated.rows_affected() != 1 {
        return Err(DenError::Authorization(
            "runtime approval is missing or already consumed".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_decision_status_matches_persisted_values() {
        assert_eq!(NativeApprovalDecision::Approve.status(), "approved");
        assert_eq!(NativeApprovalDecision::Deny.status(), "denied");
    }
}
