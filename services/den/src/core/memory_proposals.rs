use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{core::bears::BearAgentRole, errors::CustomError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProposalRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub source_role: String,
    pub source_agent_id: Option<String>,
    pub source_paths: Vec<String>,
    pub source_refs: serde_json::Value,
    pub proposal_type: String,
    pub suggested_action: String,
    pub target_ref: Option<String>,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    pub proposed_content: Option<String>,
    pub proposed_patch: Option<String>,
    pub refs: serde_json::Value,
    pub sensitivity: String,
    pub requires_human: bool,
    pub status: String,
    pub reviewer_role: Option<String>,
    pub reviewer_agent_id: Option<String>,
    pub review_notes: Option<String>,
    pub decision_summary: Option<String>,
    pub result_path: Option<String>,
    pub result_commit: Option<String>,
    pub created_at: OffsetDateTime,
    pub reviewed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct CreateMemoryProposal<'a> {
    pub bear_id: Uuid,
    pub source_role: BearAgentRole,
    pub source_agent_id: Option<String>,
    pub source_paths: Vec<String>,
    pub source_refs: serde_json::Value,
    pub suggested_action: &'a str,
    pub target_ref: Option<&'a str>,
    pub title: &'a str,
    pub summary: &'a str,
    pub rationale: &'a str,
    pub proposed_content: Option<&'a str>,
    pub proposed_patch: Option<&'a str>,
    pub refs: serde_json::Value,
    pub sensitivity: &'a str,
    pub requires_human: bool,
}

pub async fn create(
    pool: &PgPool,
    params: CreateMemoryProposal<'_>,
) -> Result<MemoryProposalRow, CustomError> {
    let row = sqlx::query(
        r#"
        INSERT INTO bear_memory_proposals (
            bear_id, source_role, source_agent_id, source_paths, source_refs,
            proposal_type, suggested_action, target_ref, title, summary, rationale,
            proposed_content, proposed_patch, refs, sensitivity, requires_human, status
        )
        VALUES (
            $1, $2, $3, $4, $5, 'memory_review', $6, $7, $8, $9, $10,
            $11, $12, $13, $14, $15, 'pending'
        )
        RETURNING id, bear_id, source_role, source_agent_id, source_paths, source_refs,
                  proposal_type, suggested_action, target_ref, title, summary, rationale,
                  proposed_content, proposed_patch, refs, sensitivity, requires_human, status,
                  reviewer_role, reviewer_agent_id, review_notes, decision_summary,
                  result_path, result_commit, created_at, reviewed_at
        "#,
    )
    .bind(params.bear_id)
    .bind(params.source_role.as_str())
    .bind(params.source_agent_id)
    .bind(params.source_paths)
    .bind(params.source_refs)
    .bind(params.suggested_action)
    .bind(params.target_ref)
    .bind(params.title)
    .bind(params.summary)
    .bind(params.rationale)
    .bind(params.proposed_content)
    .bind(params.proposed_patch)
    .bind(params.refs)
    .bind(params.sensitivity)
    .bind(params.requires_human)
    .fetch_one(pool)
    .await?;
    Ok(row_from_sql(row))
}

pub async fn list_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<MemoryProposalRow>, CustomError> {
    let rows = sqlx::query(
        r#"
        SELECT id, bear_id, source_role, source_agent_id, source_paths, source_refs,
               proposal_type, suggested_action, target_ref, title, summary, rationale,
               proposed_content, proposed_patch, refs, sensitivity, requires_human, status,
               reviewer_role, reviewer_agent_id, review_notes, decision_summary,
               result_path, result_commit, created_at, reviewed_at
        FROM bear_memory_proposals
        WHERE bear_id = $1
          AND ($2::text IS NULL OR status = $2)
        ORDER BY created_at DESC
        LIMIT $3
        "#,
    )
    .bind(bear_id)
    .bind(status)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_from_sql).collect())
}

pub async fn resolve_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    proposal_id: Uuid,
    reviewer_role: BearAgentRole,
    reviewer_agent_id: Option<&str>,
    status: &str,
    review_notes: Option<&str>,
    decision_summary: Option<&str>,
) -> Result<MemoryProposalRow, CustomError> {
    let row = sqlx::query(
        r#"
        UPDATE bear_memory_proposals
        SET status = $3,
            reviewer_role = $4,
            reviewer_agent_id = $5,
            review_notes = $6,
            decision_summary = $7,
            reviewed_at = NOW()
        WHERE bear_id = $1 AND id = $2
        RETURNING id, bear_id, source_role, source_agent_id, source_paths, source_refs,
                  proposal_type, suggested_action, target_ref, title, summary, rationale,
                  proposed_content, proposed_patch, refs, sensitivity, requires_human, status,
                  reviewer_role, reviewer_agent_id, review_notes, decision_summary,
                  result_path, result_commit, created_at, reviewed_at
        "#,
    )
    .bind(bear_id)
    .bind(proposal_id)
    .bind(status)
    .bind(reviewer_role.as_str())
    .bind(reviewer_agent_id)
    .bind(review_notes)
    .bind(decision_summary)
    .fetch_one(pool)
    .await?;
    Ok(row_from_sql(row))
}

pub async fn get_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    proposal_id: Uuid,
) -> Result<Option<MemoryProposalRow>, CustomError> {
    let row = sqlx::query(
        r#"
        SELECT id, bear_id, source_role, source_agent_id, source_paths, source_refs,
               proposal_type, suggested_action, target_ref, title, summary, rationale,
               proposed_content, proposed_patch, refs, sensitivity, requires_human, status,
               reviewer_role, reviewer_agent_id, review_notes, decision_summary,
               result_path, result_commit, created_at, reviewed_at
        FROM bear_memory_proposals
        WHERE bear_id = $1 AND id = $2
        "#,
    )
    .bind(bear_id)
    .bind(proposal_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(row_from_sql))
}

fn row_from_sql(row: sqlx::postgres::PgRow) -> MemoryProposalRow {
    MemoryProposalRow {
        id: row.get("id"),
        bear_id: row.get("bear_id"),
        source_role: row.get("source_role"),
        source_agent_id: row.get("source_agent_id"),
        source_paths: row.get("source_paths"),
        source_refs: row.get("source_refs"),
        proposal_type: row.get("proposal_type"),
        suggested_action: row.get("suggested_action"),
        target_ref: row.get("target_ref"),
        title: row.get("title"),
        summary: row.get("summary"),
        rationale: row.get("rationale"),
        proposed_content: row.get("proposed_content"),
        proposed_patch: row.get("proposed_patch"),
        refs: row.get("refs"),
        sensitivity: row.get("sensitivity"),
        requires_human: row.get("requires_human"),
        status: row.get("status"),
        reviewer_role: row.get("reviewer_role"),
        reviewer_agent_id: row.get("reviewer_agent_id"),
        review_notes: row.get("review_notes"),
        decision_summary: row.get("decision_summary"),
        result_path: row.get("result_path"),
        result_commit: row.get("result_commit"),
        created_at: row.get("created_at"),
        reviewed_at: row.get("reviewed_at"),
    }
}
