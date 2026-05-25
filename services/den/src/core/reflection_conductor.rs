use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::errors::CustomError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionRunRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub lane: String,
    pub trigger: String,
    pub status: String,
    pub role_agent_id: Option<String>,
    pub conversation_id: Option<String>,
    pub conversation_key: Option<String>,
    pub conversation_date: Option<Date>,
    pub input_summary: serde_json::Value,
    pub output_summary: serde_json::Value,
    pub error: Option<String>,
    pub started_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct CreateReflectionRun<'a> {
    pub bear_id: Uuid,
    pub lane: &'a str,
    pub trigger: &'a str,
    pub status: &'a str,
    pub role_agent_id: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub conversation_key: Option<&'a str>,
    pub conversation_date: Option<Date>,
    pub input_summary: serde_json::Value,
    pub output_summary: serde_json::Value,
    pub error: Option<&'a str>,
}

pub async fn create_run(
    pool: &PgPool,
    params: CreateReflectionRun<'_>,
) -> Result<ReflectionRunRow, CustomError> {
    let row = sqlx::query(
        r#"
        INSERT INTO bear_reflection_runs (
            bear_id, lane, trigger, status, role_agent_id,
            conversation_id, conversation_key, conversation_date,
            input_summary, output_summary, error
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        "#,
    )
    .bind(params.bear_id)
    .bind(params.lane)
    .bind(params.trigger)
    .bind(params.status)
    .bind(params.role_agent_id)
    .bind(params.conversation_id)
    .bind(params.conversation_key)
    .bind(params.conversation_date)
    .bind(params.input_summary)
    .bind(params.output_summary)
    .bind(params.error)
    .fetch_one(pool)
    .await?;
    Ok(row_from_sql(row))
}

pub struct ProposalEnqueueParams<'a> {
    pub bear_id: Uuid,
    pub role_agent_id: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub conversation_key: Option<&'a str>,
    pub conversation_date: Option<Date>,
    pub trigger: &'a str,
    pub proposal_ids: Vec<Uuid>,
}

pub async fn enqueue_memory_curate_for_proposals(
    pool: &PgPool,
    params: ProposalEnqueueParams<'_>,
) -> Result<ReflectionRunRow, CustomError> {
    let proposal_id_values: Vec<serde_json::Value> = params
        .proposal_ids
        .into_iter()
        .map(|id| serde_json::Value::String(id.to_string()))
        .collect();
    create_run(
        pool,
        CreateReflectionRun {
            bear_id: params.bear_id,
            lane: "memory_curate",
            trigger: params.trigger,
            status: "queued",
            role_agent_id: params.role_agent_id,
            conversation_id: params.conversation_id,
            conversation_key: params.conversation_key,
            conversation_date: params.conversation_date,
            input_summary: serde_json::json!({ "proposal_ids": proposal_id_values }),
            output_summary: serde_json::json!({}),
            error: None,
        },
    )
    .await
}

fn row_from_sql(row: sqlx::postgres::PgRow) -> ReflectionRunRow {
    ReflectionRunRow {
        id: row.get("id"),
        bear_id: row.get("bear_id"),
        lane: row.get("lane"),
        trigger: row.get("trigger"),
        status: row.get("status"),
        role_agent_id: row.get("role_agent_id"),
        conversation_id: row.get("conversation_id"),
        conversation_key: row.get("conversation_key"),
        conversation_date: row.get("conversation_date"),
        input_summary: row.get("input_summary"),
        output_summary: row.get("output_summary"),
        error: row.get("error"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        created_at: row.get("created_at"),
    }
}
