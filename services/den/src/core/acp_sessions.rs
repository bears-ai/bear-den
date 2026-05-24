use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgRow, PgPool, Row as SqlxRow};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::errors::CustomError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertAcpSession {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub runtime_session_id: String,
    pub conversation_id: String,
    pub resolved_conversation_id: Option<String>,
    pub client: String,
    pub cwd: Option<String>,
    #[serde(default)]
    pub current_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpSessionRow {
    pub id: Uuid,
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub runtime_session_id: String,
    pub conversation_id: String,
    pub resolved_conversation_id: Option<String>,
    pub client: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_environment: Option<serde_json::Value>,
    pub current_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_title_updated_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_title_synced_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub async fn upsert_session(pool: &PgPool, session: UpsertAcpSession) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        INSERT INTO acp_sessions (
            user_id, bear_id, bear_slug, acp_session_id, runtime_session_id,
            conversation_id, resolved_conversation_id, client, cwd, current_mode
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, 'ask'))
        ON CONFLICT (user_id, bear_id, acp_session_id) DO UPDATE
        SET bear_slug = EXCLUDED.bear_slug,
            runtime_session_id = EXCLUDED.runtime_session_id,
            conversation_id = EXCLUDED.conversation_id,
            resolved_conversation_id = EXCLUDED.resolved_conversation_id,
            client = EXCLUDED.client,
            cwd = EXCLUDED.cwd,
            current_mode = COALESCE(acp_sessions.current_mode, EXCLUDED.current_mode),
            updated_at = NOW()
        "#,
    )
    .bind(session.user_id)
    .bind(session.bear_id)
    .bind(session.bear_slug)
    .bind(session.acp_session_id)
    .bind(session.runtime_session_id)
    .bind(session.conversation_id)
    .bind(session.resolved_conversation_id)
    .bind(session.client)
    .bind(session.cwd)
    .bind(session.current_mode)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_current_mode(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    mode: &str,
) -> Result<(), CustomError> {
    if !matches!(mode, "ask" | "plan" | "write") {
        return Err(CustomError::ValidationError(
            "ACP session mode must be one of ask, plan, write".to_string(),
        ));
    }
    sqlx::query(
        r#"
        UPDATE acp_sessions
        SET current_mode = $4, updated_at = NOW()
        WHERE user_id = $1 AND bear_id = $2 AND acp_session_id = $3
        "#,
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(acp_session_id)
    .bind(mode)
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

fn acp_session_row_from_sql(row: &PgRow) -> AcpSessionRow {
    AcpSessionRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        bear_id: row.get("bear_id"),
        bear_slug: row.get("bear_slug"),
        acp_session_id: row.get("acp_session_id"),
        runtime_session_id: row.get("runtime_session_id"),
        conversation_id: row.get("conversation_id"),
        resolved_conversation_id: row.get("resolved_conversation_id"),
        client: row.get("client"),
        cwd: row.get("cwd"),
        adapter_environment: row.get("adapter_environment"),
        current_mode: row.get("current_mode"),
        conversation_title: row.get("conversation_title"),
        conversation_title_updated_at: row.get("conversation_title_updated_at"),
        conversation_title_synced_at: row.get("conversation_title_synced_at"),
        closed_at: row.get("closed_at"),
        archived_at: row.get("archived_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

pub async fn find_for_user_bear_session(
    pool: &PgPool,
    user_id: i32,
    bear_slug: &str,
    acp_session_id: &str,
) -> Result<Option<AcpSessionRow>, CustomError> {
    let row = sqlx::query(
        r#"
        SELECT id, user_id, bear_id, bear_slug, acp_session_id, runtime_session_id,
               conversation_id, resolved_conversation_id, client, cwd, adapter_environment, current_mode,
               conversation_title, conversation_title_updated_at, conversation_title_synced_at,
               closed_at, archived_at, created_at, updated_at
        FROM acp_sessions
        WHERE user_id = $1 AND bear_slug = $2 AND acp_session_id = $3
        "#,
    )
    .bind(user_id)
    .bind(bear_slug)
    .bind(acp_session_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.as_ref().map(acp_session_row_from_sql))
}

/// Lists persisted ACP sessions for a user on a bear, newest activity first.
pub struct SessionListParams<'a> {
    pub user_id: i32,
    pub bear_slug: &'a str,
    pub include_closed: bool,
    pub cwd_filter: Option<&'a str>,
    pub limit: i64,
    pub cursor_updated_at: Option<OffsetDateTime>,
    pub cursor_id: Option<Uuid>,
}

pub async fn list_for_user_bear(
    pool: &PgPool,
    params: SessionListParams<'_>,
) -> Result<Vec<AcpSessionRow>, CustomError> {
    let limit = params.limit.clamp(1, 100);
    let cwd_filter = params.cwd_filter.map(str::trim).filter(|s| !s.is_empty());
    let rows = sqlx::query(
        r#"
        SELECT id, user_id, bear_id, bear_slug, acp_session_id, runtime_session_id,
               conversation_id, resolved_conversation_id, client, cwd, adapter_environment, current_mode,
               conversation_title, conversation_title_updated_at, conversation_title_synced_at,
               closed_at, archived_at, created_at, updated_at
        FROM acp_sessions
        WHERE user_id = $1 AND bear_slug = $2
          AND ($3 OR closed_at IS NULL)
          AND ($4::text IS NULL OR cwd IS NOT DISTINCT FROM $4)
          AND (
            $6::timestamptz IS NULL
            OR updated_at < $6
            OR (updated_at = $6 AND id < $7)
          )
        ORDER BY updated_at DESC, id DESC
        LIMIT $5
        "#,
    )
    .bind(params.user_id)
    .bind(params.bear_slug)
    .bind(params.include_closed)
    .bind(cwd_filter)
    .bind(limit)
    .bind(params.cursor_updated_at)
    .bind(params.cursor_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(acp_session_row_from_sql).collect())
}

pub async fn update_adapter_environment(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    adapter_environment: &serde_json::Value,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        UPDATE acp_sessions
        SET adapter_environment = $4,
            updated_at = NOW()
        WHERE user_id = $1 AND bear_id = $2 AND acp_session_id = $3
        "#,
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(acp_session_id)
    .bind(adapter_environment)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_client_conversation_title(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    title: Option<&str>,
) -> Result<(), CustomError> {
    let normalized = title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(120).collect::<String>());
    sqlx::query(
        r#"
        UPDATE acp_sessions
        SET conversation_title = $4,
            conversation_title_updated_at = CASE
                WHEN $4::text IS NULL THEN conversation_title_updated_at
                WHEN conversation_title IS DISTINCT FROM $4 THEN NOW()
                ELSE conversation_title_updated_at
            END,
            conversation_title_synced_at = CASE
                WHEN $4::text IS NULL THEN conversation_title_synced_at
                WHEN conversation_title IS DISTINCT FROM $4 THEN NULL
                ELSE conversation_title_synced_at
            END,
            updated_at = NOW()
        WHERE user_id = $1 AND bear_id = $2 AND acp_session_id = $3
        "#,
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(acp_session_id)
    .bind(normalized)
    .execute(pool)
    .await?;
    Ok(())
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

pub async fn set_title_for_bear_conversation(
    pool: &PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    title: &str,
) -> Result<u64, CustomError> {
    let result = sqlx::query(
        r#"
        UPDATE acp_sessions
        SET conversation_title = $3,
            conversation_title_updated_at = NOW(),
            conversation_title_synced_at = NULL,
            updated_at = NOW()
        WHERE bear_id = $1
          AND (conversation_id = $2 OR resolved_conversation_id = $2)
        "#,
    )
    .bind(bear_id)
    .bind(conversation_id)
    .bind(title)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn mark_title_synced(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        UPDATE acp_sessions
        SET conversation_title_synced_at = NOW()
        WHERE user_id = $1 AND bear_id = $2 AND acp_session_id = $3
        "#,
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(acp_session_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn resolved_conversation_ids_for_bear(
    pool: &PgPool,
    bear_slug: &str,
) -> Result<Vec<String>, CustomError> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT resolved_conversation_id
        FROM acp_sessions
        WHERE bear_slug = $1
          AND resolved_conversation_id IS NOT NULL
          AND resolved_conversation_id LIKE 'conv-%'
        "#,
    )
    .bind(bear_slug)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("resolved_conversation_id"))
        .collect())
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
