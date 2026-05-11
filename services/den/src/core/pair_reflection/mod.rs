use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{core::bears::BearAgentRole, errors::CustomError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairReflectionRunRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub user_id: i32,
    pub acp_session_id: String,
    pub conversation_id: Option<String>,
    pub trigger: String,
    pub status: String,
    pub summary_path: Option<String>,
    pub summary_commit: Option<String>,
    pub considered_message_count: i32,
    pub considered_memory_paths: Vec<String>,
    pub diagnostic: serde_json::Value,
    pub created_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct CreatePairReflectionRun<'a> {
    pub bear_id: Uuid,
    pub user_id: i32,
    pub acp_session_id: &'a str,
    pub conversation_id: Option<&'a str>,
    pub trigger: &'a str,
    pub considered_message_count: i32,
    pub considered_memory_paths: Vec<String>,
    pub diagnostic: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct CompletePairReflectionRun<'a> {
    pub id: Uuid,
    pub status: &'a str,
    pub summary_path: Option<&'a str>,
    pub summary_commit: Option<&'a str>,
    pub diagnostic: serde_json::Value,
}

pub async fn create_run(
    pool: &PgPool,
    params: CreatePairReflectionRun<'_>,
) -> Result<PairReflectionRunRow, CustomError> {
    let row = sqlx::query(
        r#"
        INSERT INTO pair_reflection_runs (
            bear_id, user_id, acp_session_id, conversation_id, trigger,
            status, considered_message_count, considered_memory_paths, diagnostic
        )
        VALUES ($1, $2, $3, $4, $5, 'started', $6, $7, $8)
        RETURNING id, bear_id, user_id, acp_session_id, conversation_id, trigger,
                  status, summary_path, summary_commit, considered_message_count,
                  considered_memory_paths, diagnostic, created_at, completed_at
        "#,
    )
    .bind(params.bear_id)
    .bind(params.user_id)
    .bind(params.acp_session_id)
    .bind(params.conversation_id)
    .bind(params.trigger)
    .bind(params.considered_message_count)
    .bind(params.considered_memory_paths)
    .bind(params.diagnostic)
    .fetch_one(pool)
    .await?;
    Ok(row_from_sql(row))
}

pub async fn complete_run(
    pool: &PgPool,
    params: CompletePairReflectionRun<'_>,
) -> Result<PairReflectionRunRow, CustomError> {
    let row = sqlx::query(
        r#"
        UPDATE pair_reflection_runs
        SET status = $2,
            summary_path = $3,
            summary_commit = $4,
            diagnostic = $5,
            completed_at = NOW()
        WHERE id = $1
        RETURNING id, bear_id, user_id, acp_session_id, conversation_id, trigger,
                  status, summary_path, summary_commit, considered_message_count,
                  considered_memory_paths, diagnostic, created_at, completed_at
        "#,
    )
    .bind(params.id)
    .bind(params.status)
    .bind(params.summary_path)
    .bind(params.summary_commit)
    .bind(params.diagnostic)
    .fetch_one(pool)
    .await?;
    Ok(row_from_sql(row))
}

pub async fn list_recent_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    limit: i64,
) -> Result<Vec<PairReflectionRunRow>, CustomError> {
    let rows = sqlx::query(
        r#"
        SELECT id, bear_id, user_id, acp_session_id, conversation_id, trigger,
               status, summary_path, summary_commit, considered_message_count,
               considered_memory_paths, diagnostic, created_at, completed_at
        FROM pair_reflection_runs
        WHERE bear_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(bear_id)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_from_sql).collect())
}

fn row_from_sql(row: sqlx::postgres::PgRow) -> PairReflectionRunRow {
    PairReflectionRunRow {
        id: row.get("id"),
        bear_id: row.get("bear_id"),
        user_id: row.get("user_id"),
        acp_session_id: row.get("acp_session_id"),
        conversation_id: row.get("conversation_id"),
        trigger: row.get("trigger"),
        status: row.get("status"),
        summary_path: row.get("summary_path"),
        summary_commit: row.get("summary_commit"),
        considered_message_count: row.get("considered_message_count"),
        considered_memory_paths: row.get("considered_memory_paths"),
        diagnostic: row.get("diagnostic"),
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    }
}

pub fn summary_title_for_session(acp_session_id: &str) -> String {
    format!("Pair session summary: {acp_session_id}")
}

pub fn render_pair_summary_markdown(
    acp_session_id: &str,
    conversation_id: Option<&str>,
    trigger: &str,
    message_summaries: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("## Pair reflection summary\n\n");
    out.push_str(&format!("- ACP session: `{acp_session_id}`\n"));
    if let Some(conversation_id) = conversation_id {
        out.push_str(&format!("- Conversation: `{conversation_id}`\n"));
    }
    out.push_str(&format!("- Trigger: `{trigger}`\n"));
    out.push_str("\n### Recent conversation signals\n\n");
    if message_summaries.is_empty() {
        out.push_str("No conversation messages were available for reflection.\n");
    } else {
        for summary in message_summaries.iter().take(20) {
            out.push_str("- ");
            out.push_str(summary.trim());
            out.push('\n');
        }
    }
    out.push_str("\n### Reflection note\n\n");
    out.push_str("This is an initial deterministic pair reflection summary. Future versions should use the pair/reflection model pass to extract durable decisions, lessons, and review requests.\n");
    out
}

pub fn pair_reflection_role() -> BearAgentRole {
    BearAgentRole::Pair
}
