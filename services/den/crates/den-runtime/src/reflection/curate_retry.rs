//! Rate-limited, durable recovery for transient Curate synthesis failures. The original
//! run completion and its delayed successor are one Postgres transaction; SQLite
//! remains the only owner of the proposal's pending/published state.

use den_core::DenError;
use serde::Deserialize;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::conductor::ReflectionRunRow;
use crate::memory::curate_executor::{CurateProposalOutcome, CurateRetryReason};

#[cfg(test)]
#[path = "curate_retry/tests.rs"]
mod tests;

const FIRST_RETRY_DELAY: Duration = Duration::minutes(1);
const SECOND_RETRY_DELAY: Duration = Duration::minutes(10);
const THIRD_RETRY_DELAY: Duration = Duration::hours(1);
const FOURTH_RETRY_DELAY: Duration = Duration::hours(6);
const DAILY_RETRY_DELAY: Duration = Duration::days(1);
const DAILY_ATTEMPT: u8 = 5;

#[derive(Deserialize)]
struct RetryInput {
    #[serde(default)]
    retry_attempt: u8,
}

struct RetryPlan {
    attempt: u8,
    delay: Duration,
    proposal_ids: Vec<Uuid>,
}

fn retry_plan(
    input_summary: &serde_json::Value,
    outcomes: &[CurateProposalOutcome],
) -> Option<RetryPlan> {
    let attempt = serde_json::from_value::<RetryInput>(input_summary.clone())
        .ok()?
        .retry_attempt;
    let (next_attempt, delay) = match attempt {
        0 => (1, FIRST_RETRY_DELAY),
        1 => (2, SECOND_RETRY_DELAY),
        2 => (3, THIRD_RETRY_DELAY),
        3 => (4, FOURTH_RETRY_DELAY),
        _ => (DAILY_ATTEMPT, DAILY_RETRY_DELAY),
    };
    let proposal_ids = outcomes
        .iter()
        .filter(|outcome| {
            outcome.status == "pending"
                && outcome.retry_reason == Some(CurateRetryReason::SynthesisUnavailable)
        })
        .map(|outcome| outcome.proposal_id)
        .collect::<Vec<_>>();
    (!proposal_ids.is_empty()).then_some(RetryPlan {
        attempt: next_attempt,
        delay,
        proposal_ids,
    })
}

pub struct CompletedCurateRun {
    pub completed: ReflectionRunRow,
    pub retry_run: Option<ReflectionRunRow>,
}

pub async fn complete_and_schedule(
    pool: &PgPool,
    run: &ReflectionRunRow,
    output_summary: serde_json::Value,
    outcomes: &[CurateProposalOutcome],
) -> Result<CompletedCurateRun, DenError> {
    let plan = retry_plan(&run.input_summary, outcomes);
    let mut tx = pool.begin().await?;
    let completed = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'completed',
            output_summary = $3,
            error = NULL,
            completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'memory_curate'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        run.bear_id,
        run.id,
        output_summary,
    )
    .fetch_one(&mut *tx)
    .await?;
    let retry_run = if let Some(plan) = plan {
        let summary = serde_json::json!({
            "proposal_ids": plan.proposal_ids,
            "retry_attempt": plan.attempt,
        });
        let available_at = OffsetDateTime::now_utc() + plan.delay;
        Some(
            sqlx::query_as!(
                ReflectionRunRow,
                r"
            INSERT INTO bear_reflection_runs (
                bear_id, lane, trigger, status, role_agent_id,
                conversation_id, conversation_key, conversation_date,
                input_summary, output_summary, available_at
            ) VALUES ($1, 'memory_curate', 'verified_hat_retry', 'queued', $2,
                      NULL, $3, $4, $5, '{}'::jsonb, $6)
            RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                      conversation_id, conversation_key, conversation_date,
                      input_summary, output_summary, error,
                      started_at, completed_at, created_at
            ",
                run.bear_id,
                run.role_agent_id.as_deref(),
                run.conversation_key.as_deref(),
                run.conversation_date,
                summary,
                available_at,
            )
            .fetch_one(&mut *tx)
            .await?,
        )
    } else {
        None
    };
    tx.commit().await?;
    Ok(CompletedCurateRun {
        completed,
        retry_run,
    })
}
