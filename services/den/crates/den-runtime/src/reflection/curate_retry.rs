//! Rate-limited, durable recovery for transient Curate synthesis failures. The original
//! run completion and its delayed successor are one Postgres transaction; SQLite
//! remains the only owner of the proposal's pending/published state.

use crate::memory_curate_executor::verified_hat_candidate_is_current;
use den_core::{ids::BearId, DenError};
use den_memory::{get_memory_proposal, MemoryStoreManager};
use den_service::bears::hats;
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

/// Pick up only typed synthesis failures stranded by the older three-attempt
/// policy. Recheck the SQLite proposal, canonical source, and current hat opt-in
/// before queuing; the ordinary executor checks them again before publication.
/// A delayed run makes deployment itself incapable of immediately sending old
/// private notes to a model provider.
pub struct RecoveryProgress {
    pub inspected: usize,
    pub queued: usize,
}

pub async fn recover_exhausted_once(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    offset: i64,
) -> Result<RecoveryProgress, DenError> {
    let candidates = sqlx::query!(
        r#"SELECT DISTINCT r.id, r.bear_id,
                  (outcome.value ->> 'proposal_id')::uuid AS "proposal_id!: Uuid"
           FROM bear_reflection_runs r
           CROSS JOIN LATERAL jsonb_array_elements(
               CASE WHEN jsonb_typeof(r.output_summary -> 'outcomes') = 'array'
                    THEN r.output_summary -> 'outcomes' ELSE '[]'::jsonb END
           ) AS outcome(value)
           WHERE r.lane = 'memory_curate' AND r.status = 'completed'
             AND r.input_summary ->> 'retry_attempt' = '2'
             AND outcome.value ->> 'status' = 'pending'
             AND outcome.value ->> 'retry_reason' = 'synthesis_unavailable'
           ORDER BY r.id, r.bear_id, "proposal_id!: Uuid"
           LIMIT 100 OFFSET $1"#,
        offset.max(0),
    )
    .fetch_all(pool)
    .await?;
    let inspected = candidates.len();
    let mut queued = 0;
    for candidate in candidates {
        let store = stores.store_for_bear(candidate.bear_id).await?;
        let Some(proposal) =
            get_memory_proposal(&store, &candidate.proposal_id.to_string()).await?
        else {
            continue;
        };
        if proposal.status != "pending" {
            continue;
        }
        let Some(verified) = proposal.verified_hat_source else {
            continue;
        };
        let hat = match hats::manage::get_hat(pool, BearId::new(candidate.bear_id), verified.hat_id)
            .await
        {
            Ok(hat) => hat,
            Err(DenError::NotFound(_)) => continue,
            Err(error) => return Err(error),
        };
        if hat.work_enabled
            || !hat.auto_curate_enabled
            || !verified_hat_candidate_is_current(pool, stores, candidate.bear_id, verified).await?
        {
            continue;
        }
        let mut tx = pool.begin().await?;
        // Serialize competing workers on the old run; the NOT EXISTS below
        // prevents repeated sweeps from minting another recovery chain.
        sqlx::query!(
            "SELECT id FROM bear_reflection_runs WHERE id = $1 FOR UPDATE",
            candidate.id
        )
        .fetch_one(&mut *tx)
        .await?;
        let summary = serde_json::json!({
            "proposal_ids": [candidate.proposal_id],
            "retry_attempt": DAILY_ATTEMPT,
            "recovered_from": candidate.id,
        });
        let recovered = sqlx::query!(
            r#"INSERT INTO bear_reflection_runs (
                   bear_id, lane, trigger, status, role_agent_id,
                   conversation_key, conversation_date, input_summary,
                   output_summary, available_at
               )
               SELECT r.bear_id, 'memory_curate', 'verified_hat_recovery', 'queued',
                      r.role_agent_id, r.conversation_key, r.conversation_date,
                      $3, '{}'::jsonb, NOW() + INTERVAL '1 day'
               FROM bear_reflection_runs r
               WHERE r.id = $1 AND r.bear_id = $2
                 AND NOT EXISTS (
                   SELECT 1 FROM bear_reflection_runs newer
                   WHERE newer.bear_id = r.bear_id AND newer.lane = 'memory_curate'
                     AND newer.created_at > r.created_at
                     AND newer.input_summary -> 'proposal_ids' @> jsonb_build_array($4::text)
                 )
               RETURNING id"#,
            candidate.id,
            candidate.bear_id,
            summary,
            candidate.proposal_id.to_string(),
        )
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        queued += usize::from(recovered.is_some());
    }
    Ok(RecoveryProgress { inspected, queued })
}
