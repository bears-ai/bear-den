use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::CustomError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertAcpSession {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub codepool_session_id: String,
    pub conversation_id: String,
    pub resolved_conversation_id: Option<String>,
    pub client: String,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpSessionRow {
    pub id: Uuid,
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub codepool_session_id: String,
    pub conversation_id: String,
    pub resolved_conversation_id: Option<String>,
    pub client: String,
}

pub async fn upsert_session(pool: &PgPool, session: UpsertAcpSession) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        INSERT INTO acp_sessions (
            user_id, bear_id, bear_slug, acp_session_id, codepool_session_id,
            conversation_id, resolved_conversation_id, client, cwd
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (user_id, bear_id, acp_session_id) DO UPDATE
        SET bear_slug = EXCLUDED.bear_slug,
            codepool_session_id = EXCLUDED.codepool_session_id,
            conversation_id = EXCLUDED.conversation_id,
            resolved_conversation_id = EXCLUDED.resolved_conversation_id,
            client = EXCLUDED.client,
            cwd = EXCLUDED.cwd,
            updated_at = NOW()
        "#,
    )
    .bind(session.user_id)
    .bind(session.bear_id)
    .bind(session.bear_slug)
    .bind(session.acp_session_id)
    .bind(session.codepool_session_id)
    .bind(session.conversation_id)
    .bind(session.resolved_conversation_id)
    .bind(session.client)
    .bind(session.cwd)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_resolved(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    resolved_conversation_id: &str,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        UPDATE acp_sessions
        SET resolved_conversation_id = $4, updated_at = NOW()
        WHERE user_id = $1 AND bear_id = $2 AND acp_session_id = $3
        "#,
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(acp_session_id)
    .bind(resolved_conversation_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_for_user_bear_session(
    pool: &PgPool,
    user_id: i32,
    bear_slug: &str,
    acp_session_id: &str,
) -> Result<Option<AcpSessionRow>, CustomError> {
    let row = sqlx::query(
        r#"
        SELECT id, user_id, bear_id, bear_slug, acp_session_id, codepool_session_id,
               conversation_id, resolved_conversation_id, client
        FROM acp_sessions
        WHERE user_id = $1 AND bear_slug = $2 AND acp_session_id = $3
        "#,
    )
    .bind(user_id)
    .bind(bear_slug)
    .bind(acp_session_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| AcpSessionRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        bear_id: row.get("bear_id"),
        bear_slug: row.get("bear_slug"),
        acp_session_id: row.get("acp_session_id"),
        codepool_session_id: row.get("codepool_session_id"),
        conversation_id: row.get("conversation_id"),
        resolved_conversation_id: row.get("resolved_conversation_id"),
        client: row.get("client"),
    }))
}

pub async fn mark_closed(pool: &PgPool, id: Uuid) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        UPDATE acp_sessions
        SET closed_at = NOW(), updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_archived(pool: &PgPool, id: Uuid) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        UPDATE acp_sessions
        SET archived_at = NOW(), updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}
