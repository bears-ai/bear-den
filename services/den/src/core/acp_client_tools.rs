use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::CustomError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewAcpClientToolCall {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub codepool_session_id: String,
    pub conversation_id: String,
    pub request_id: Uuid,
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub descriptor: Value,
    pub timeout_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpClientToolCallRow {
    pub id: Uuid,
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub codepool_session_id: String,
    pub conversation_id: String,
    pub request_id: Uuid,
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub descriptor: Value,
    pub status: String,
}

pub async fn persist_sent_call(
    pool: &PgPool,
    call: NewAcpClientToolCall,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        INSERT INTO acp_client_tool_calls (
            user_id, bear_id, bear_slug, acp_session_id, codepool_session_id,
            conversation_id, request_id, call_id, tool_name, arguments,
            descriptor, status, sent_at, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'sent_to_client', NOW(), NOW() + ($12 * interval '1 millisecond'))
        ON CONFLICT (request_id, call_id) DO UPDATE
        SET status = 'sent_to_client',
            sent_at = COALESCE(acp_client_tool_calls.sent_at, NOW()),
            arguments = EXCLUDED.arguments,
            descriptor = EXCLUDED.descriptor,
            expires_at = EXCLUDED.expires_at
        "#,
    )
    .bind(call.user_id)
    .bind(call.bear_id)
    .bind(call.bear_slug)
    .bind(call.acp_session_id)
    .bind(call.codepool_session_id)
    .bind(call.conversation_id)
    .bind(call.request_id)
    .bind(call.call_id)
    .bind(call.tool_name)
    .bind(call.arguments)
    .bind(call.descriptor)
    .bind(call.timeout_ms)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_active_call_for_result(
    pool: &PgPool,
    user_id: i32,
    bear_slug: &str,
    acp_session_id: &str,
    request_id: Uuid,
    call_id: &str,
) -> Result<Option<AcpClientToolCallRow>, CustomError> {
    let row = sqlx::query(
        r#"
        SELECT id, user_id, bear_id, bear_slug, acp_session_id, codepool_session_id,
               conversation_id, request_id, call_id, tool_name, arguments, descriptor, status
        FROM acp_client_tool_calls
        WHERE user_id = $1
          AND bear_slug = $2
          AND acp_session_id = $3
          AND request_id = $4
          AND call_id = $5
          AND expires_at > NOW()
          AND status IN ('sent_to_client', 'approved', 'pending')
        "#,
    )
    .bind(user_id)
    .bind(bear_slug)
    .bind(acp_session_id)
    .bind(request_id)
    .bind(call_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| AcpClientToolCallRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        bear_id: row.get("bear_id"),
        bear_slug: row.get("bear_slug"),
        acp_session_id: row.get("acp_session_id"),
        codepool_session_id: row.get("codepool_session_id"),
        conversation_id: row.get("conversation_id"),
        request_id: row.get("request_id"),
        call_id: row.get("call_id"),
        tool_name: row.get("tool_name"),
        arguments: row.get("arguments"),
        descriptor: row.get("descriptor"),
        status: row.get("status"),
    }))
}

pub async fn mark_result_received(
    pool: &PgPool,
    id: Uuid,
    status: &str,
    result: Option<Value>,
    error: Option<Value>,
    client_observation: Option<Value>,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        UPDATE acp_client_tool_calls
        SET status = $2,
            result = $3,
            error = $4,
            client_observation = $5,
            result_received_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(status)
    .bind(result)
    .bind(error)
    .bind(client_observation)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_forwarded(pool: &PgPool, id: Uuid) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        UPDATE acp_client_tool_calls
        SET status = 'forwarded_to_codepool', forwarded_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}
