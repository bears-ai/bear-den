//! Durable dispatch/claim/lease state for autonomous `work`-stance execution.
//!
//! One `bear_work_runs` row per dispatch attempt of one job run. The dispatch
//! worker (den-runtime) claims rows with a lease; the BearWire `work.*`
//! methods bind the in-sandbox armature's session and record the turn
//! outcome; the worker harvests and finalizes. Tasks remain the execution
//! checkpoints inside that job run (Docket schedules, gates, and records — it
//! never executes task bodies; ADR-0034).

use std::time::Duration as StdDuration;

use serde_json::{json, Value};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::DenError;

use crate::execution_profiles::resolve_execution_profile;
use crate::model::{
    select_dispatch_notebook_context, DocketEntryListFilter, DocketEntryRow,
    DocketExecutionAttemptAuthorize, DocketExecutionAttemptRow, DocketExecutionAttemptStart,
    DocketExecutionBinding, DocketExecutionBindingKind, DocketExecutionDisposition,
    DocketExecutionGate, DocketExecutionHost, DocketExecutionHostKind, DocketExecutionReason,
    DocketFocusedExecutionBinding, DocketTaskDifficulty,
};
use crate::recovery::claim_turn_attempt;
use crate::routing::{route_turn, ExecutionSurface, TurnIntent, TurnSource};
use crate::service::{DocketService, PgDocketService};

pub const ATTACHED_DISCONNECT_TIMEOUT: StdDuration = StdDuration::from_mins(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkExecutionTarget {
    Sandbox,
    AttachedArmature { client_session_id: String },
}

impl WorkExecutionTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::AttachedArmature { .. } => "attached_armature",
        }
    }

    fn client_session_id(&self) -> Option<&str> {
        match self {
            Self::Sandbox => None,
            Self::AttachedArmature { client_session_id } => Some(client_session_id),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkRunState {
    Queued,
    Claimed,
    Provisioning,
    Running,
    Paused,
    Reporting,
    Stalled,
    Succeeded,
    Blocked,
    Failed,
    Cancelled,
    TimedOut,
}

impl WorkRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Provisioning => "provisioning",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Reporting => "reporting",
            Self::Stalled => "stalled",
            Self::Succeeded => "succeeded",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "queued" => Self::Queued,
            "claimed" => Self::Claimed,
            "provisioning" => Self::Provisioning,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "reporting" => Self::Reporting,
            "stalled" => Self::Stalled,
            "succeeded" => Self::Succeeded,
            "blocked" => Self::Blocked,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "timed_out" => Self::TimedOut,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Stalled
                | Self::Succeeded
                | Self::Blocked
                | Self::Failed
                | Self::Cancelled
                | Self::TimedOut
        )
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct WorkRunRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub job_id: Uuid,
    pub job_run_id: Uuid,
    pub executing_task_id: Option<Uuid>,
    pub attempt: i32,
    pub state: String,
    pub runner_id: Option<String>,
    pub lease_expires_at: Option<OffsetDateTime>,
    pub cancel_requested: bool,
    /// Origin that requested cancellation, e.g. `web:user:42` or `tool:pair`.
    pub cancel_requested_by: Option<String>,
    /// Caller-provided reason suitable for a diagnostic surface, never a secret.
    pub cancel_reason: Option<String>,
    pub cancel_requested_at: Option<OffsetDateTime>,
    pub git_ref: Option<String>,
    /// Catalog image name the run was dispatched with (None = provider default).
    pub image_name: Option<String>,
    pub sandbox_server_url: Option<String>,
    pub sandbox_id: Option<String>,
    pub sandbox_type: Option<String>,
    pub sandbox_strength: Option<String>,
    pub work_surface: Option<Value>,
    pub execution_target: String,
    pub attached_client_session_id: Option<String>,
    pub attachment_state: Option<String>,
    pub attachment_warning: Option<String>,
    pub disconnected_at: Option<OffsetDateTime>,
    pub disconnect_deadline_at: Option<OffsetDateTime>,
    pub bearwire_session_id: Option<String>,
    pub result_summary: Option<String>,
    pub result_refs: Option<Value>,
    pub usage: Option<Value>,
    pub error: Option<String>,
    pub queued_at: OffsetDateTime,
    pub started_at: Option<OffsetDateTime>,
    pub finished_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

impl WorkRunRow {
    pub fn state_enum(&self) -> Option<WorkRunState> {
        WorkRunState::parse(&self.state)
    }
}

pub fn effective_work_run_surface(managed_surface_name: Option<&str>) -> Option<String> {
    managed_surface_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Clone, Debug)]
pub struct WorkJobEnqueue {
    pub bear_id: Uuid,
    pub job_id: Uuid,
    pub durable_result: crate::DurableResultKind,
    pub git_ref: Option<String>,
    pub image_name: Option<String>,
    pub requested_by_user_id: Option<i32>,
    pub execution_target: WorkExecutionTarget,
    pub attachment_warning: Option<String>,
}

/// Provenance captured when a user-facing control plane asks a worker to stop.
#[derive(Clone, Debug)]
pub struct WorkRunCancelRequest {
    pub requested_by: String,
    pub reason: String,
}

/// Acknowledges a terminal stalled run without changing its execution outcome.
/// Retrying remains a new work-run attempt; waiting needs no write.
#[derive(Clone, Debug)]
pub struct StalledWorkRunResolution {
    pub resolved_by: String,
    pub reason: String,
}

/// Legacy test fixture input. Production dispatch is job-scoped; tests use a
/// task only to locate its owning job.
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct WorkRunEnqueue {
    pub bear_id: Uuid,
    pub task_id: Uuid,
    pub root_name: Option<String>,
    pub git_ref: Option<String>,
    pub image_name: Option<String>,
    pub requested_by_user_id: Option<i32>,
}

#[cfg(test)]
pub async fn enqueue_work_run(
    pool: &PgPool,
    enqueue: WorkRunEnqueue,
) -> Result<WorkRunRow, DenError> {
    let job_id = sqlx::query_scalar!(
        "SELECT job_id AS \"job_id!\" FROM bear_tasks WHERE id = $1 AND bear_id = $2",
        enqueue.task_id,
        enqueue.bear_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound(format!("Docket task not found: {}", enqueue.task_id)))?;
    enqueue_work_job(
        pool,
        WorkJobEnqueue {
            bear_id: enqueue.bear_id,
            job_id,
            durable_result: crate::DurableResultKind::RepositoryChanges,
            git_ref: enqueue.git_ref,
            image_name: enqueue.image_name,
            requested_by_user_id: enqueue.requested_by_user_id,
            execution_target: WorkExecutionTarget::Sandbox,
            attachment_warning: None,
        },
    )
    .await
    .map(|mut runs| runs.remove(0))
}

/// Queue one job-scoped work run. The job must have at least one runnable
/// work task; task state is deliberately not encoded on the work-run row.
pub async fn enqueue_work_job(
    pool: &PgPool,
    enqueue: WorkJobEnqueue,
) -> Result<Vec<WorkRunRow>, DenError> {
    let mut tx = pool.begin().await?;
    let job = sqlx::query!(
        "SELECT a.work_surface_id AS \"work_surface_id?\", s.name AS \"surface_name?\", a.mutation_policy AS \"mutation_policy?\", j.current_run_id AS \"current_run_id?\", j.lifecycle_intent AS \"lifecycle_intent?\",
                    j.commit_policy AS \"commit_policy?\", j.work_branch AS \"work_branch?\",
                    EXISTS (
                        SELECT 1 FROM work_surface_bears wsb
                        WHERE wsb.surface_id = a.work_surface_id AND wsb.bear_id = j.bear_id
                    ) AS \"surface_assigned!\"
             FROM bear_jobs j
             LEFT JOIN LATERAL (
                 SELECT a.work_surface_id, a.mutation_policy
                 FROM job_work_surface_assignments a
                 JOIN work_surfaces s ON s.id = a.work_surface_id
                 WHERE a.job_id = j.id AND s.kind = 'git_workspace'
                 ORDER BY a.created_at
                 LIMIT 1
             ) a ON true
             LEFT JOIN work_surfaces s ON s.id = a.work_surface_id
             WHERE j.id = $1 AND j.bear_id = $2 FOR UPDATE OF j", enqueue.job_id, enqueue.bear_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(job) = job else {
        return Err(DenError::NotFound(format!(
            "Docket job not found: {}",
            enqueue.job_id
        )));
    };
    if job.work_surface_id.is_none()
        || job
            .surface_name
            .as_deref()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err(DenError::ValidationError(
            "work_surface_required: this work job lacks a valid managed work-surface binding; select or rebind a surface before dispatch".into(),
        ));
    }
    if job.mutation_policy.as_deref() == Some("forbidden") {
        return Err(DenError::ValidationError(format!(
            "managed work surface '{}' forbids mutation for this job",
            job.surface_name.as_deref().unwrap_or("unknown")
        )));
    }
    if !job.surface_assigned {
        return Err(DenError::ValidationError(format!(
            "managed work surface '{}' is not assigned to this Bear",
            job.surface_name.as_deref().unwrap_or("unknown")
        )));
    }
    if job.lifecycle_intent.is_some() {
        return Err(DenError::ValidationError(
            "job is not dispatchable; cancelled or archived work jobs cannot start work runs"
                .into(),
        ));
    }

    let commit_policy = match job.commit_policy.as_deref() {
        Some("none") => Some(crate::DocketCommitPolicy::None),
        Some("per_task") => Some(crate::DocketCommitPolicy::PerTask),
        Some("per_job") => Some(crate::DocketCommitPolicy::PerJob),
        _ => None,
    };
    let preflight = crate::preflight_dispatch(
        &enqueue.execution_target,
        enqueue.durable_result,
        commit_policy,
        job.work_branch.as_deref(),
    );
    if !preflight.dispatchable {
        return Err(DenError::ValidationError(
            "repository_changes_without_publication: sandbox runs use isolated ephemeral checkouts; set commit_policy to per_task or per_job, or use the attached worktree"
                .into(),
        ));
    }

    let provider_surface = effective_work_run_surface(job.surface_name.as_deref());
    if provider_surface.is_none() {
        return Err(DenError::ValidationError(
            "work_surface_required: this work job lacks a usable managed work-surface binding"
                .into(),
        ));
    }
    let runnable = sqlx::query_scalar!(
        "SELECT EXISTS (
             SELECT 1 FROM bear_tasks t
             LEFT JOIN bear_task_run_state s ON s.task_id = t.id AND s.run_id = $2
             WHERE t.job_id = $1
               AND COALESCE(s.status, 'pending') IN ('pending', 'blocked')
         )",
        enqueue.job_id,
        job.current_run_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if !runnable.unwrap_or(false) {
        return Err(DenError::ValidationError(
            "job has no runnable work tasks to dispatch".into(),
        ));
    }

    let job_run_id = match job.current_run_id {
        Some(run_id) => run_id,
        None => {
            let run_id: Uuid = sqlx::query_scalar!(
                "INSERT INTO bear_job_runs (job_id, trigger, state) VALUES ($1, 'event', 'running') RETURNING id", enqueue.job_id)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE bear_jobs SET current_run_id = $2, updated_at = now() WHERE id = $1",
                enqueue.job_id,
                run_id
            )
            .execute(&mut *tx)
            .await?;
            run_id
        }
    };
    let attempt: i32 = sqlx::query_scalar!(
        "SELECT COALESCE(MAX(attempt), 0) + 1 AS \"attempt!\" FROM bear_work_runs WHERE job_id = $1",
        enqueue.job_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let execution_target = enqueue.execution_target.as_str();
    let attached_client_session_id = enqueue.execution_target.client_session_id();
    let attachment_state = attached_client_session_id.map(|_| "attached");
    let run = sqlx::query_as!(WorkRunRow,        "INSERT INTO bear_work_runs (bear_id, job_id, job_run_id, attempt, git_ref, image_name,
                                     execution_target, attached_client_session_id, attachment_state,
                                     attachment_warning)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", enqueue.bear_id, enqueue.job_id, job_run_id, attempt, enqueue.git_ref, enqueue.image_name, execution_target, attached_client_session_id, attachment_state, enqueue.attachment_warning)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| match err {
        sqlx::Error::Database(db)
            if db.constraint() == Some("idx_bear_work_runs_one_active_per_job") =>
        {
            DenError::ValidationError("job already has an active work run".into())
        }
        other => other.into(),
    })?;
    tx.commit().await?;
    Ok(vec![run])
}

/// Claim the next dispatchable run with a lease (`FOR UPDATE SKIP LOCKED`).
/// Picks up fresh `queued` runs and takes over non-terminal runs whose lease
/// expired (worker crash); the state of a taken-over run is preserved so the
/// new owner can reconcile rather than restart blindly.
///
/// **Runs serialize per job**: a queued run is only claimable when its job
/// has no other in-flight run, so a multi-task job drains one run at a time
/// in queue order — sequential tasks build on the job's work branch instead
/// of racing it (concurrent publishes to one branch are guaranteed
/// non-fast-forward failures). Expired-lease takeovers are exempt: the
/// in-flight run being taken over *is* the job's active run.
pub async fn claim_next_work_run(
    pool: &PgPool,
    runner_id: &str,
    lease: std::time::Duration,
) -> Result<Option<WorkRunRow>, DenError> {
    // The in-flight-sibling gate reads committed state, so two workers
    // claiming simultaneously can each see the other's queued sibling as
    // claimable. The post-claim recheck resolves that race with a
    // deterministic older-run-wins rule; one bounce is enough because the
    // retry's fresh snapshot sees the winner in flight.
    for _ in 0..3 {
        let Some(run) = claim_next_work_run_once(pool, runner_id, lease).await? else {
            return Ok(None);
        };
        // Only freshly claimed queued runs (no sandbox yet) are subject to
        // the recheck; releasing a provisioned takeover would orphan its
        // sandbox.
        let fresh_claim = run.state == "claimed" && run.sandbox_id.is_none();
        if fresh_claim && has_older_inflight_sibling(pool, &run).await? {
            release_work_run_claim(pool, run.id, runner_id).await?;
            continue;
        }
        return Ok(Some(run));
    }
    Ok(None)
}

async fn claim_next_work_run_once(
    pool: &PgPool,
    runner_id: &str,
    lease: std::time::Duration,
) -> Result<Option<WorkRunRow>, DenError> {
    let lease_secs = i64::try_from(lease.as_secs()).unwrap_or(i64::MAX);
    let row = sqlx::query_as!(WorkRunRow,        "WITH candidate AS (
             SELECT id FROM bear_work_runs r
             WHERE (
                     r.state = 'queued'
                     AND r.execution_target = 'sandbox'
                     AND EXISTS (
                         SELECT 1 FROM bear_jobs j
                         WHERE j.id = r.job_id AND COALESCE(j.lifecycle_intent, '') NOT IN ('cancelled', 'archived')
                     )
                     AND NOT EXISTS (
                         SELECT 1 FROM bear_work_runs sibling
                         WHERE sibling.job_id = r.job_id
                           AND sibling.id <> r.id
                           AND sibling.state IN ('claimed', 'provisioning', 'running', 'reporting')
                     )
                   )
                OR (r.state IN ('claimed', 'provisioning', 'running', 'reporting')
                    AND r.lease_expires_at IS NOT NULL AND r.lease_expires_at < now())
             ORDER BY r.queued_at ASC
             LIMIT 1
             FOR UPDATE SKIP LOCKED
         )
         UPDATE bear_work_runs r
         SET state = CASE WHEN r.state = 'queued' THEN 'claimed' ELSE r.state END,
             runner_id = $1,
             lease_expires_at = now() + make_interval(secs => $2),
             updated_at = now()
         FROM candidate
         WHERE r.id = candidate.id
         RETURNING r.id, r.bear_id, r.job_id, r.job_run_id, r.executing_task_id, r.attempt, r.state, r.runner_id, r.lease_expires_at, r.cancel_requested, r.cancel_requested_by, r.cancel_reason, r.cancel_requested_at, r.git_ref, r.image_name, r.sandbox_server_url, r.sandbox_id, r.sandbox_type, r.sandbox_strength, r.work_surface, r.execution_target, r.attached_client_session_id, r.attachment_state, r.attachment_warning, r.disconnected_at, r.disconnect_deadline_at, r.bearwire_session_id, r.result_summary, r.result_refs, r.usage, r.error, r.queued_at, r.started_at, r.finished_at, r.updated_at", runner_id, lease_secs as f64)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Whether the run's job has another in-flight run that was queued earlier
/// (ties broken by id). Used by the claim recheck: when two workers race two
/// queued runs of one job into flight, the younger one yields.
async fn has_older_inflight_sibling(pool: &PgPool, run: &WorkRunRow) -> Result<bool, DenError> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS (
             SELECT 1 FROM bear_work_runs sibling
             WHERE sibling.job_id = $1
               AND sibling.id <> $2
               AND sibling.state IN ('claimed', 'provisioning', 'running', 'reporting')
               AND (sibling.queued_at < $3
                    OR (sibling.queued_at = $3 AND sibling.id < $2))
         ) AS \"exists!\"",
        run.job_id,
        run.id,
        run.queued_at
    )
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

/// Return a freshly claimed (never provisioned) run to the queue.
async fn release_work_run_claim(
    pool: &PgPool,
    run_id: Uuid,
    runner_id: &str,
) -> Result<(), DenError> {
    sqlx::query!(
        "UPDATE bear_work_runs
         SET state = 'queued', runner_id = NULL, lease_expires_at = NULL, updated_at = now()
         WHERE id = $1 AND runner_id = $2 AND state = 'claimed' AND sandbox_id IS NULL",
        run_id,
        runner_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Queue placement of a `queued` run within its job (runs serialize per
/// job): 1-based position in the job's queue and the in-flight run it is
/// waiting behind, when there is one. Derived at read time — never stored.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct WorkRunQueueInfo {
    pub run_id: Uuid,
    pub position: i64,
    pub waiting_on_run_id: Option<Uuid>,
}

/// Queue info for the `queued` runs among `run_ids` (non-queued ids are
/// simply absent from the result). One query for the whole batch, so list
/// views can annotate cheaply.
pub async fn queued_run_positions(
    pool: &PgPool,
    run_ids: &[Uuid],
) -> Result<Vec<WorkRunQueueInfo>, DenError> {
    if run_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as!(
        WorkRunQueueInfo,
        "SELECT r.id AS run_id,
                (SELECT count(*) FROM bear_work_runs q
                 WHERE q.job_id = r.job_id AND q.state = 'queued'
                   AND (q.queued_at < r.queued_at
                        OR (q.queued_at = r.queued_at AND q.id <= r.id))) AS \"position!\",
                (SELECT s.id FROM bear_work_runs s
                 WHERE s.job_id = r.job_id
                   AND s.state IN ('claimed', 'provisioning', 'running', 'reporting')
                 ORDER BY s.queued_at ASC, s.id ASC
                 LIMIT 1) AS waiting_on_run_id
         FROM bear_work_runs r
         WHERE r.id = ANY($1) AND r.state = 'queued'",
        run_ids
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// A job-scoped work run that needs attention. Tasks remain visible in the
/// task list; the run is deliberately not attributed to one task.
#[derive(Clone, Debug, serde::Serialize, sqlx::FromRow)]
pub struct AttentionWorkRun {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub job_goal: String,
    pub state: String,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub finished_at: Option<OffsetDateTime>,
}

/// Latest-attempt job runs in attention states. A newer queued/active attempt
/// supersedes an older failure for the same job.
pub async fn attention_work_runs(
    pool: &PgPool,
    bear_id: Uuid,
    job_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<AttentionWorkRun>, DenError> {
    let rows = sqlx::query_as!(
        AttentionWorkRun,
        "SELECT latest.id AS run_id, latest.job_id, j.goal AS job_goal,
                latest.state, latest.result_summary, latest.error, latest.finished_at
         FROM (
             SELECT DISTINCT ON (job_id)
                    id, job_id, state, result_summary, error, finished_at
             FROM bear_work_runs
             WHERE bear_id = $1 AND ($2::uuid IS NULL OR job_id = $2)
             ORDER BY job_id, queued_at DESC, id DESC
         ) latest
         JOIN bear_jobs j ON j.id = latest.job_id
         WHERE latest.state IN ('stalled', 'blocked', 'failed', 'timed_out')
         ORDER BY latest.finished_at DESC NULLS LAST
         LIMIT $3",
        bear_id,
        job_id,
        limit.clamp(1, 100)
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Jobs whose tasks are all done (or cancelled) in the current run but whose
/// job status has not been closed out — "done but unjudged", awaiting
/// criteria review / completion.
pub async fn jobs_awaiting_completion(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<crate::model::DocketJobRow>, DenError> {
    let rows = sqlx::query_as!(crate::model::DocketJobRow,
        "SELECT j.id, j.bear_id, j.created_by_user_id, j.created_by_role, j.goal,
                (SELECT a.work_surface_id
                 FROM job_work_surface_assignments a
                 JOIN work_surfaces s ON s.id = a.work_surface_id
                 WHERE a.job_id = j.id AND s.kind = 'git_workspace' AND a.mutation_policy <> 'forbidden'
                 ORDER BY a.created_at LIMIT 1) AS work_surface_id,
                j.commit_policy, j.work_branch,
                COALESCE(j.lifecycle_intent, 'draft') AS \"status!\", j.lifecycle_intent, j.visibility,
                j.source_conversation_id, j.objective_kind, j.current_run_id, j.supersedes_job_id,
                j.created_at, j.updated_at
         FROM bear_jobs j
         WHERE j.bear_id = $1
           AND j.lifecycle_intent IS NULL
           AND EXISTS (SELECT 1 FROM bear_tasks t WHERE t.job_id = j.id)
           AND NOT EXISTS (
               SELECT 1 FROM bear_tasks t
               LEFT JOIN bear_task_run_state s
                 ON s.task_id = t.id AND s.run_id = j.current_run_id
               WHERE t.job_id = j.id
                 AND COALESCE(s.status, 'pending') NOT IN ('done', 'cancelled')
           )
         ORDER BY j.updated_at DESC", bear_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Extend the lease on a run this worker owns. Returns false when the run is
/// no longer owned by `runner_id` (lease was reclaimed) — the worker must
/// drop it.
pub async fn heartbeat_work_run(
    pool: &PgPool,
    run_id: Uuid,
    runner_id: &str,
    lease: std::time::Duration,
) -> Result<bool, DenError> {
    let lease_secs = i64::try_from(lease.as_secs()).unwrap_or(i64::MAX);
    let result = sqlx::query!(
        "UPDATE bear_work_runs
         SET lease_expires_at = now() + make_interval(secs => $3), updated_at = now()
         WHERE id = $1 AND runner_id = $2
           AND state IN ('claimed', 'provisioning', 'running', 'reporting')",
        run_id,
        runner_id,
        lease_secs as f64
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[derive(Clone, Debug)]
pub struct WorkRunProvisioned {
    pub sandbox_server_url: String,
    pub sandbox_id: String,
    pub sandbox_type: String,
    pub sandbox_strength: String,
    pub work_surface: Value,
    pub rust_dependency_preparation: Option<Value>,
}

/// Record sandbox placement and transition claimed → running.
pub async fn record_work_run_provisioned(
    pool: &PgPool,
    run_id: Uuid,
    provisioned: &WorkRunProvisioned,
) -> Result<WorkRunRow, DenError> {
    let rust_dependency_preparation = provisioned
        .rust_dependency_preparation
        .as_ref()
        .unwrap_or(&Value::Null);
    let row = sqlx::query_as!(WorkRunRow,
        "UPDATE bear_work_runs
         SET state = 'running',
             sandbox_server_url = $2, sandbox_id = $3, sandbox_type = $4,
             sandbox_strength = $5, work_surface = $6,
             result_refs = CASE WHEN $8 THEN
                 COALESCE(result_refs, '{}'::jsonb)
                 || jsonb_build_object('rust_dependency_preparation', $7::jsonb)
             ELSE result_refs END,
             started_at = COALESCE(started_at, now()), updated_at = now()
         WHERE id = $1 AND state IN ('claimed', 'provisioning')
         RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", run_id, &provisioned.sandbox_server_url, &provisioned.sandbox_id, &provisioned.sandbox_type, &provisioned.sandbox_strength, &provisioned.work_surface, rust_dependency_preparation, provisioned.rust_dependency_preparation.is_some())
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        DenError::ValidationError(format!("work run {run_id} is not in a provisionable state"))
    })?;
    Ok(row)
}

/// Bind the in-sandbox armature's BearWire session to its work run
/// (from `work.checkout`). Fails if the run belongs to a different bear or is
/// not live.
pub async fn bind_work_run_session(
    pool: &PgPool,
    run_id: Uuid,
    bear_id: Uuid,
    session_id: &str,
) -> Result<WorkRunRow, DenError> {
    let row = sqlx::query_as!(WorkRunRow,        "UPDATE bear_work_runs
         SET bearwire_session_id = $3, updated_at = now()
         WHERE id = $1 AND bear_id = $2
           AND state IN ('claimed', 'provisioning', 'running')
         RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", run_id, bear_id, session_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        DenError::NotFound(format!(
            "no live work run {run_id} for this bear (wrong id, wrong bear, or already finished)"
        ))
    })?;
    Ok(row)
}

/// Record the Den-side turn outcome for the session bound to a work run and
/// move it to `reporting` (the dispatch worker harvests from there). Returns
/// `None` when the session is not bound to a live run.
pub async fn record_work_run_turn_outcome(
    pool: &PgPool,
    session_id: &str,
    outcome: &Value,
) -> Result<Option<WorkRunRow>, DenError> {
    let row = sqlx::query_as!(WorkRunRow,        "UPDATE bear_work_runs
         SET state = 'reporting',
             result_refs = COALESCE(result_refs, '{}'::jsonb) || jsonb_build_object('turn_outcome', $2::jsonb),
             updated_at = now()
         WHERE bearwire_session_id = $1 AND state IN ('provisioning', 'running')
         RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", session_id, outcome)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Store the armature's advisory `work.report` summary.
pub async fn record_work_run_report(
    pool: &PgPool,
    run_id: Uuid,
    bear_id: Uuid,
    status_hint: &str,
    summary: &str,
) -> Result<(), DenError> {
    sqlx::query!(
        "UPDATE bear_work_runs
         SET result_refs = COALESCE(result_refs, '{}'::jsonb)
                 || jsonb_build_object('armature_report',
                        jsonb_build_object('status_hint', $3::text, 'summary', $4::text)),
             updated_at = now()
         WHERE id = $1 AND bear_id = $2",
        run_id,
        bear_id,
        status_hint,
        summary
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Store the latest hosted Cargo dependency-preparation result for a work run.
/// This is durable run evidence for the web UI; it deliberately excludes any
/// provider credentials or unbounded helper output.
pub async fn record_work_run_dependency_preparation(
    pool: &PgPool,
    run_id: Uuid,
    bear_id: Uuid,
    result: &Value,
) -> Result<(), DenError> {
    sqlx::query!(
        "UPDATE bear_work_runs
         SET result_refs = COALESCE(result_refs, '{}'::jsonb)
                 || jsonb_build_object('rust_dependency_preparation', $3::jsonb),
             updated_at = now()
         WHERE id = $1 AND bear_id = $2",
        run_id,
        bear_id,
        result
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct WorkRunFinalize {
    pub result_summary: Option<String>,
    /// Merged into (not replacing) existing result_refs.
    pub result_refs: Option<Value>,
    pub usage: Option<Value>,
    pub error: Option<String>,
}

/// The one user-facing outcome for a work run. Turn and Armature reports are
/// evidence only: neither may claim that work completed. A completed worker
/// process is only eligible for a completed work outcome; durable task state
/// and structured validation evidence decide the outcome.
pub fn canonical_work_run_state(
    state: WorkRunState,
    refs: &Value,
    task_statuses: &[String],
) -> WorkRunState {
    if !matches!(state, WorkRunState::Succeeded) {
        return state;
    }
    if refs.pointer("/cargo_failure/code").and_then(Value::as_str)
        == Some("cargo_offline_cache_miss")
        || task_statuses.iter().any(|status| status == "blocked")
        || task_statuses.iter().any(|status| status == "pending")
    {
        WorkRunState::Blocked
    } else {
        state
    }
}

pub fn canonical_work_run_outcome(
    state: WorkRunState,
    refs: &Value,
    task_statuses: &[String],
) -> Value {
    if matches!(state, WorkRunState::Blocked)
        && refs.pointer("/cargo_failure/code").and_then(Value::as_str)
            == Some("cargo_offline_cache_miss")
    {
        let package = refs
            .pointer("/cargo_failure/required_package")
            .and_then(Value::as_str)
            .map(|package| format!(" `{package}` could not be resolved."))
            .unwrap_or_default();
        return json!({
            "status": "blocked",
            "code": "cargo_offline_cache_miss",
            "summary": format!("Rust dependencies are unavailable in the offline cache.{package}"),
            "next_action": "Prepare Rust dependencies with the hosted dependency tool, then retry Cargo.",
            "evidence_refs": ["cargo_failure", "turn_outcome", "armature_report"],
        });
    }

    if matches!(state, WorkRunState::Blocked) {
        let blocked = task_statuses
            .iter()
            .filter(|status| *status == "blocked")
            .count();
        if blocked > 0 {
            return json!({
                "status": "blocked",
                "code": "task_blocked",
                "summary": format!("Work is blocked: {blocked} task(s) are blocked."),
                "evidence_refs": ["task_run_states", "turn_outcome", "armature_report"],
            });
        }
        let unfinished = task_statuses
            .iter()
            .filter(|status| *status == "pending")
            .count();
        if unfinished > 0 {
            let missing_terminal_status =
                refs.pointer("/turn_outcome/kind").and_then(Value::as_str) == Some("completed");
            let (code, summary) = if missing_terminal_status {
                (
                    "task_status_not_recorded",
                    format!(
                        "Sandbox turn completed without recording a terminal task status for {unfinished} task(s)."
                    ),
                )
            } else {
                (
                    "work_incomplete",
                    format!("Work is incomplete: {unfinished} task(s) remain unfinished."),
                )
            };
            return json!({
                "status": "incomplete",
                "code": code,
                "summary": summary,
                "evidence_refs": ["task_run_states", "turn_outcome", "armature_report"],
            });
        }
    }

    let (status, code, summary) = match state {
        WorkRunState::Succeeded => ("completed", "completed", "Work completed."),
        WorkRunState::Blocked => ("blocked", "work_blocked", "Work is blocked."),
        WorkRunState::Stalled => (
            "stalled",
            "work_stalled",
            "Work stalled awaiting operator action.",
        ),
        WorkRunState::Failed => ("failed", "work_failed", "Work failed."),
        WorkRunState::TimedOut => ("timed_out", "work_timed_out", "Work timed out."),
        WorkRunState::Cancelled => ("cancelled", "work_cancelled", "Work was cancelled."),
        _ => ("incomplete", "work_incomplete", "Work is incomplete."),
    };
    json!({
        "status": status,
        "code": code,
        "summary": summary,
        "evidence_refs": ["turn_outcome", "armature_report"],
    })
}

/// Terminal transition; clears the lease, stamps finished_at, and appends the
/// matching task audit event.
pub async fn finalize_work_run(
    pool: &PgPool,
    run_id: Uuid,
    state: WorkRunState,
    finalize: WorkRunFinalize,
) -> Result<WorkRunRow, DenError> {
    if !state.is_terminal() {
        return Err(DenError::ValidationError(format!(
            "finalize_work_run requires a terminal state, got {}",
            state.as_str()
        )));
    }
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as!(WorkRunRow,        "UPDATE bear_work_runs
         SET state = $2,
             result_summary = COALESCE($3, result_summary),
             result_refs = COALESCE(result_refs, '{}'::jsonb) || COALESCE($4::jsonb, '{}'::jsonb),
             usage = COALESCE($5::jsonb, usage),
             error = COALESCE($6, error),
             runner_id = NULL,
             lease_expires_at = NULL,
             finished_at = COALESCE(finished_at, now()),
             updated_at = now()
         WHERE id = $1 AND state NOT IN ('succeeded', 'stalled', 'blocked', 'failed', 'cancelled', 'timed_out')
         RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", run_id, state.as_str(), finalize.result_summary.as_deref(), finalize.result_refs.as_ref(), finalize.usage.as_ref(), finalize.error.as_deref())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        DenError::ValidationError(format!("work run {run_id} is already finalized or unknown"))
    })?;
    let task_statuses = sqlx::query_scalar!(
        "SELECT COALESCE(s.status, 'pending') AS \"status!\"
         FROM bear_tasks t
         LEFT JOIN bear_task_run_state s ON s.task_id = t.id AND s.run_id = $2
         WHERE t.job_id = $1",
        row.job_id,
        row.job_run_id
    )
    .fetch_all(&mut *tx)
    .await?;
    let result_refs = row.result_refs.as_ref().unwrap_or(&Value::Null);
    let mut canonical_state = canonical_work_run_state(state, result_refs, &task_statuses);
    if result_refs
        .pointer("/turn_outcome/detail/category")
        .and_then(Value::as_str)
        == Some("continuation_watchdog_timeout")
    {
        canonical_state = WorkRunState::Stalled;
    }
    let mut canonical_outcome =
        canonical_work_run_outcome(canonical_state, result_refs, &task_statuses);
    if !matches!(canonical_state, WorkRunState::Succeeded) {
        if let Some(summary) = finalize.result_summary.as_deref() {
            canonical_outcome["summary"] = Value::String(summary.to_string());
        }
    }
    if result_refs
        .pointer("/turn_outcome/detail/category")
        .and_then(Value::as_str)
        == Some("continuation_watchdog_timeout")
    {
        let affected_task = sqlx::query!(
            "SELECT t.id AS \"id!\", t.title AS \"title!\", COALESCE(s.status, 'pending') AS \"status!\"
             FROM bear_tasks t
             LEFT JOIN bear_task_run_state s ON s.task_id = t.id AND s.run_id = $2
             WHERE t.id = $1",
            row.executing_task_id,
            row.job_run_id
        )
        .fetch_optional(&mut *tx)
        .await?;
        let detail = result_refs.pointer("/turn_outcome/detail");
        canonical_outcome = json!({
            "status": "stalled",
            "code": "continuation_watchdog_timeout",
            "summary": "The model continuation stopped responding before this work run could finish.",
            "next_action": "Wait for evidence, retry the work run, cancel it, or resolve it as failed.",
            "affected_task": affected_task.map(|task| json!({
                "id": task.id,
                "title": task.title,
                "status": task.status,
            })),
            "forensics": detail.and_then(|detail| detail.get("forensics")).cloned(),
            "evidence_refs": ["turn_outcome", "task_run_states"],
        });
    }
    let row = sqlx::query_as!(WorkRunRow,        "UPDATE bear_work_runs
         SET state = $2,
             result_summary = $3,
             result_refs = COALESCE(result_refs, '{}'::jsonb)
                 || jsonb_build_object('outcome', $4::jsonb)
         WHERE id = $1
         RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", run_id, canonical_state.as_str(), canonical_outcome["summary"].as_str(), &canonical_outcome)
    .fetch_one(&mut *tx)
    .await?;

    // A terminal work run cannot retain task-level execution authority: a
    // retry gets a new work-run id and therefore a new authorization key.
    // Release this run's exact live attempt before committing so the retry
    // cannot collide with docket_execution_attempts_live_task_idx.
    if let Some(attempt) = sqlx::query!(
        "UPDATE docket_execution_attempts
         SET state = 'released', released_at = NOW(), updated_at = NOW()
         WHERE work_run_id = $1
           AND state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')
         RETURNING id, fence_epoch",
        row.id,
    )
    .fetch_optional(&mut *tx)
    .await?
    {
        sqlx::query!(
            "INSERT INTO docket_execution_attempt_recoveries
                 (execution_attempt_id, fence_epoch, recovery_key, recovery_reason)
             VALUES ($1, $2, $3, 'work run finalized')",
            attempt.id,
            attempt.fence_epoch,
            Uuid::new_v4(),
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        "UPDATE docket_turn_claims
         SET state = 'settled', updated_at = NOW()
         WHERE work_run_id = $1 AND state = 'executing'",
        row.id,
    )
    .execute(&mut *tx)
    .await?;

    // A work run is execution telemetry, not task or job authority. Task and
    // criterion state are reconciled by their own transactional updates; a
    // terminal (including stale) work run must not complete or block the job.
    tx.commit().await?;
    Ok(row)
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    #[test]
    fn cargo_cache_miss_blocks_even_when_turn_completed() {
        let refs = json!({
            "cargo_failure": {
                "code": "cargo_offline_cache_miss",
                "required_package": "serde"
            },
            "turn_outcome": { "kind": "completed" },
            "armature_report": { "status_hint": "completed" }
        });
        let task_statuses = vec!["pending".to_string()];
        let state = canonical_work_run_state(WorkRunState::Succeeded, &refs, &task_statuses);
        let outcome = canonical_work_run_outcome(state, &refs, &task_statuses);
        assert_eq!(outcome["status"], "blocked");
        assert_eq!(outcome["code"], "cargo_offline_cache_miss");
        assert!(outcome["summary"].as_str().unwrap().contains("serde"));
        assert!(outcome["next_action"]
            .as_str()
            .unwrap()
            .contains("Prepare Rust dependencies"));
    }

    #[test]
    fn completed_turn_without_terminal_task_status_is_explicit() {
        let refs = json!({ "turn_outcome": { "kind": "completed" } });
        let task_statuses = vec!["pending".to_string()];
        let state = canonical_work_run_state(WorkRunState::Succeeded, &refs, &task_statuses);
        let outcome = canonical_work_run_outcome(state, &refs, &task_statuses);
        assert_eq!(state, WorkRunState::Blocked);
        assert_eq!(outcome["status"], "incomplete");
        assert_eq!(outcome["code"], "task_status_not_recorded");
        assert!(outcome["summary"]
            .as_str()
            .unwrap()
            .contains("without recording a terminal task status"));
    }

    #[test]
    fn unfinished_tasks_prevent_completed_outcome() {
        let refs = json!({ "turn_outcome": { "kind": "failed" } });
        let task_statuses = vec!["done".to_string(), "pending".to_string()];
        let state = canonical_work_run_state(WorkRunState::Succeeded, &refs, &task_statuses);
        let outcome = canonical_work_run_outcome(state, &refs, &task_statuses);
        assert_eq!(state, WorkRunState::Blocked);
        assert_eq!(outcome["status"], "incomplete");
        assert_eq!(outcome["code"], "work_incomplete");
    }

    #[test]
    fn terminal_worker_failures_remain_authoritative() {
        let refs = json!({
            "cargo_failure": { "code": "cargo_offline_cache_miss" },
            "turn_outcome": { "kind": "completed" },
        });
        let task_statuses = vec!["pending".to_string()];
        let state = canonical_work_run_state(WorkRunState::TimedOut, &refs, &task_statuses);
        let outcome = canonical_work_run_outcome(state, &refs, &task_statuses);
        assert_eq!(state, WorkRunState::TimedOut);
        assert_eq!(outcome["status"], "timed_out");
        assert_eq!(outcome["code"], "work_timed_out");
    }

    #[test]
    fn blocked_task_beats_unfinished_task_count() {
        let refs = json!({ "turn_outcome": { "kind": "completed" } });
        let task_statuses = vec!["blocked".to_string(), "pending".to_string()];
        let state = canonical_work_run_state(WorkRunState::Succeeded, &refs, &task_statuses);
        let outcome = canonical_work_run_outcome(state, &refs, &task_statuses);
        assert_eq!(state, WorkRunState::Blocked);
        assert_eq!(outcome["status"], "blocked");
        assert_eq!(outcome["code"], "task_blocked");
    }
}

/// Ask the owning worker to cancel; teardown happens asynchronously. Returns
/// false when the run is already terminal.
pub async fn request_work_run_cancel(
    pool: &PgPool,
    run_id: Uuid,
    bear_id: Uuid,
) -> Result<bool, DenError> {
    request_work_run_cancel_with_provenance(
        pool,
        run_id,
        bear_id,
        &WorkRunCancelRequest {
            requested_by: "system".into(),
            reason: "cancellation requested without caller provenance".into(),
        },
    )
    .await
}

/// Ask the owning worker to cancel with caller provenance; teardown happens
/// asynchronously. Returns false when the run is already terminal.
pub async fn request_work_run_cancel_with_provenance(
    pool: &PgPool,
    run_id: Uuid,
    bear_id: Uuid,
    request: &WorkRunCancelRequest,
) -> Result<bool, DenError> {
    let result = sqlx::query!(
        "UPDATE bear_work_runs
         SET cancel_requested = TRUE,
             cancel_requested_by = COALESCE(cancel_requested_by, $3),
             cancel_reason = COALESCE(cancel_reason, $4),
             cancel_requested_at = COALESCE(cancel_requested_at, now()),
             updated_at = now()
         WHERE id = $1 AND bear_id = $2
           AND state IN ('queued', 'claimed', 'provisioning', 'running', 'paused', 'reporting')",
        run_id,
        bear_id,
        &request.requested_by,
        &request.reason
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Record a human/operator resolution of a stalled run while preserving its
/// `stalled` outcome and original diagnostic. This is deliberately not a
/// cancellation: the worker already stopped, and retry creates a new attempt.
pub async fn resolve_stalled_work_run(
    pool: &PgPool,
    run_id: Uuid,
    bear_id: Uuid,
    resolution: &StalledWorkRunResolution,
) -> Result<bool, DenError> {
    let result = sqlx::query!(
        "UPDATE bear_work_runs
         SET result_refs = COALESCE(result_refs, '{}'::jsonb) || jsonb_build_object(
                 'stalled_resolution', jsonb_build_object(
                     'resolved_by', $3::text,
                     'reason', $4::text,
                     'resolved_at', now()
                 )
             ),
             updated_at = now()
         WHERE id = $1 AND bear_id = $2 AND state = 'stalled'
           AND NOT (COALESCE(result_refs, '{}'::jsonb) ? 'stalled_resolution')",
        run_id,
        bear_id,
        &resolution.resolved_by,
        &resolution.reason
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The latest work run bound to a BearWire session, including terminal runs.
pub async fn get_work_run_by_session(
    pool: &PgPool,
    session_id: &str,
) -> Result<Option<WorkRunRow>, DenError> {
    let row = sqlx::query_as!(WorkRunRow,        "SELECT id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at FROM bear_work_runs
         WHERE bearwire_session_id = $1
         ORDER BY updated_at DESC
         LIMIT 1", session_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// The live work run bound to a BearWire session, if any. This is the stance
/// signal for `run.start`: a session bound via `work.checkout` runs in the
/// Work stance; everything else stays Pair.
pub async fn get_live_work_run_by_session(
    pool: &PgPool,
    session_id: &str,
) -> Result<Option<WorkRunRow>, DenError> {
    let row = sqlx::query_as!(WorkRunRow,        "SELECT id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at FROM bear_work_runs
         WHERE bearwire_session_id = $1
           AND state IN ('claimed', 'provisioning', 'running', 'reporting')
         ORDER BY updated_at DESC
         LIMIT 1", session_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Project the native BearWire permission obligation onto an attached work
/// run. The obligation remains the authority; this state is diagnostic only
/// and cannot grant permission.
pub async fn mark_attached_work_run_permission_required(
    pool: &PgPool,
    session_id: &str,
) -> Result<bool, DenError> {
    // ponytail: runtime SQL until Phase 4 migration metadata can be prepared against Postgres;
    // upgrade to query! when cargo-sqlx and a migrated database are available.
    let result = sqlx::query!(
        "UPDATE bear_work_runs
         SET attachment_state = 'permission_required', updated_at = now()
         WHERE attached_client_session_id = $1
           AND execution_target = 'attached_armature'
           AND state IN ('queued', 'claimed', 'provisioning', 'running', 'paused', 'reporting')
           AND attachment_state IN ('attached', 'permission_required')",
        session_id
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Clear the diagnostic permission state after the authoritative obligation
/// accepts a current client decision. Late and conflicting decisions never
/// reach this function.
pub async fn settle_attached_work_run_permission(
    pool: &PgPool,
    session_id: &str,
) -> Result<bool, DenError> {
    // ponytail: runtime SQL until Phase 4 migration metadata can be prepared against Postgres;
    // upgrade to query! when cargo-sqlx and a migrated database are available.
    let result = sqlx::query!(
        "UPDATE bear_work_runs
         SET attachment_state = 'attached', updated_at = now()
         WHERE attached_client_session_id = $1
           AND execution_target = 'attached_armature'
           AND state IN ('queued', 'claimed', 'provisioning', 'running', 'paused', 'reporting')
           AND attachment_state = 'permission_required'",
        session_id
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn disconnect_attached_work_run(
    pool: &PgPool,
    session_id: &str,
    timeout: StdDuration,
) -> Result<Option<WorkRunRow>, DenError> {
    let deadline = OffsetDateTime::now_utc()
        + time::Duration::try_from(timeout)
            .map_err(|_| DenError::ValidationError("disconnect timeout is too large".into()))?;
    let row = sqlx::query_as!(WorkRunRow,        "UPDATE bear_work_runs
         SET state = 'paused', attachment_state = 'disconnected',
             disconnected_at = COALESCE(disconnected_at, now()),
             disconnect_deadline_at = COALESCE(disconnect_deadline_at, $2),
             runner_id = NULL, lease_expires_at = NULL, updated_at = now()
         WHERE attached_client_session_id = $1
           AND execution_target = 'attached_armature'
           AND state IN ('queued', 'claimed', 'provisioning', 'running', 'paused', 'reporting')
           AND attachment_state <> 'timed_out'
         RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", session_id, deadline)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn reconnect_attached_work_run(
    pool: &PgPool,
    session_id: &str,
) -> Result<Option<WorkRunRow>, DenError> {
    let row = sqlx::query_as!(WorkRunRow,        "UPDATE bear_work_runs
         SET attachment_state = 'attached', disconnected_at = NULL,
             disconnect_deadline_at = NULL, updated_at = now()
         WHERE attached_client_session_id = $1
           AND execution_target = 'attached_armature'
           AND state = 'paused' AND attachment_state = 'disconnected'
           AND disconnect_deadline_at > now()
         RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", session_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn timeout_disconnected_work_runs(pool: &PgPool) -> Result<Vec<WorkRunRow>, DenError> {
    let rows = sqlx::query_as!(WorkRunRow,        "UPDATE bear_work_runs
         SET state = 'timed_out', attachment_state = 'timed_out',
             result_summary = 'Attached armature disconnected and did not reconnect before the deadline.',
             result_refs = COALESCE(result_refs, '{}'::jsonb) || jsonb_build_object(
                 'outcome', jsonb_build_object(
                     'status', 'timed_out',
                     'code', 'armature_disconnect_timeout',
                     'summary', 'Attached armature disconnected and did not reconnect before the deadline.',
                     'next_action', 'Reconnect the armature and recover this run.'
                 ),
                 'attachment', jsonb_build_object(
                     'disconnected_at', disconnected_at,
                     'deadline_at', disconnect_deadline_at
                 )
             ),
             error = 'armature_disconnect_timeout', finished_at = COALESCE(finished_at, now()),
             runner_id = NULL, lease_expires_at = NULL, updated_at = now()
         WHERE execution_target = 'attached_armature'
           AND state = 'paused' AND attachment_state = 'disconnected'
           AND disconnect_deadline_at <= now()
         RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at")
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

fn is_attached_recovery_source(
    execution_target: &str,
    state: &str,
    result_refs: Option<&Value>,
) -> bool {
    execution_target == "attached_armature"
        && state == "timed_out"
        && result_refs
            .and_then(|refs| refs.pointer("/outcome/code"))
            .and_then(Value::as_str)
            == Some("armature_disconnect_timeout")
}

pub async fn recover_attached_work_run(
    pool: &PgPool,
    source_run_id: Uuid,
    bear_id: Uuid,
) -> Result<WorkRunRow, DenError> {
    let mut tx = pool.begin().await?;
    let source = sqlx::query_as!(WorkRunRow,        "SELECT id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at FROM bear_work_runs
         WHERE id = $1 AND bear_id = $2 FOR UPDATE", source_run_id, bear_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DenError::NotFound(format!("work run not found: {source_run_id}")))?;
    if !is_attached_recovery_source(
        &source.execution_target,
        &source.state,
        source.result_refs.as_ref(),
    ) {
        return Err(DenError::ValidationError(
            "work run is not eligible for attached-armature recovery".into(),
        ));
    }
    let session_id = source
        .attached_client_session_id
        .as_deref()
        .ok_or_else(|| {
            DenError::ValidationError("recovery source has no attached session".into())
        })?;
    // ponytail: runtime SQL until Phase 4 migration metadata can be prepared against Postgres;
    // upgrade to query_scalar! when cargo-sqlx and a migrated database are available.
    let attempt = sqlx::query_scalar!(
        "SELECT COALESCE(MAX(attempt), 0) + 1 AS \"attempt!\" FROM bear_work_runs WHERE job_id = $1",
        source.job_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let source_work_run_id = source.id.to_string();
    let recovered = sqlx::query_as!(WorkRunRow,        "INSERT INTO bear_work_runs (
             bear_id, job_id, job_run_id, attempt, git_ref, image_name,
             execution_target, attached_client_session_id, attachment_state,
             attachment_warning, result_refs
         ) VALUES (
             $1, $2, $3, $4, $5, $6,
             'attached_armature', $7, 'attached', $8,
             jsonb_build_object('recovery', jsonb_build_object(
                 'source_work_run_id', $9::text,
                 'source_outcome', COALESCE($10::jsonb, '{}'::jsonb)
             ))
         ) RETURNING id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at", source.bear_id, source.job_id, source.job_run_id, attempt, source.git_ref.as_deref(), source.image_name.as_deref(), session_id, source.attachment_warning.as_deref(), &source_work_run_id, source.result_refs.as_ref())
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| match err {
        sqlx::Error::Database(db)
            if db.constraint() == Some("idx_bear_work_runs_one_active_per_job") =>
        {
            DenError::ValidationError(
                "recovery already started or the job has another active work run".into(),
            )
        }
        other => other.into(),
    })?;
    tx.commit().await?;
    Ok(recovered)
}

#[derive(Clone, Debug)]
pub struct WorkRunCheckout {
    pub run: WorkRunRow,
    pub gate: DocketExecutionGate,
    pub execution_attempt: Option<DocketExecutionAttemptRow>,
    pub prompt_context: Option<WorkPromptContext>,
    pub task_title: Option<String>,
}

/// Docket's authoritative Work-mode task decision. Checkout must consume this
/// result instead of turning an absent runnable leaf into an executor-local
/// error or synthesizing a competing scheduler decision.
enum WorkExecutionAuthorization {
    Allowed {
        task: (
            Uuid,
            String,
            String,
            sqlx::types::Json<Vec<String>>,
            Option<String>,
        ),
        gate: DocketExecutionGate,
    },
    Rejected(DocketExecutionGate),
}

async fn record_work_execution_rejection(
    pool: &PgPool,
    work_run_id: Uuid,
    reason: &DocketExecutionReason,
) -> Result<DocketExecutionDisposition, DenError> {
    // ponytail: two equivalent rejections are enough to surface an operator
    // intervention. Make this policy-configurable only if production evidence
    // shows the fixed threshold is inadequate.
    const INTERVENTION_THRESHOLD: i32 = 2;
    let occurrences = sqlx::query_scalar::<_, i32>(
        "INSERT INTO bear_execution_rejection_observations (
             work_run_id, reason, occurrences, last_observed_at
         ) VALUES ($1, $2, 1, NOW())
         ON CONFLICT (work_run_id, reason) DO UPDATE
         SET occurrences = bear_execution_rejection_observations.occurrences + 1,
             last_observed_at = NOW()
         RETURNING occurrences",
    )
    .bind(work_run_id)
    .bind(
        serde_json::to_value(reason)
            .expect("execution rejection reason serializes")
            .as_str()
            .expect("execution rejection reason is a string"),
    )
    .fetch_one(pool)
    .await?;
    Ok(if occurrences >= INTERVENTION_THRESHOLD {
        if crate::db::require_checkpoint_directive_for_work_run(pool, work_run_id)
            .await?
            .is_some()
        {
            DocketExecutionDisposition::RequireCheckpoint
        } else {
            DocketExecutionDisposition::RequireIntervention
        }
    } else {
        reason.disposition()
    })
}

async fn clear_work_execution_rejections(pool: &PgPool, work_run_id: Uuid) -> Result<(), DenError> {
    sqlx::query("DELETE FROM bear_execution_rejection_observations WHERE work_run_id = $1")
        .bind(work_run_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn authorize_work_execution(
    pool: &PgPool,
    run: &WorkRunRow,
) -> Result<WorkExecutionAuthorization, DenError> {
    if crate::db::pending_checkpoint_directive_for_work_run(pool, run.id)
        .await?
        .is_some()
    {
        let reason = DocketExecutionReason::CheckpointRequired;
        return Ok(WorkExecutionAuthorization::Rejected(
            DocketExecutionGate::Rejected {
                disposition: DocketExecutionDisposition::RequireCheckpoint,
                reason,
            },
        ));
    }
    let active_task = sqlx::query!(
        "SELECT t.id AS \"id!\", t.title AS \"title!\", t.body AS \"body!\", t.completion_criteria AS \"completion_criteria!\", t.difficulty AS \"difficulty?\", rs.status AS \"run_status?\"
         FROM bear_tasks t
         LEFT JOIN bear_task_run_state rs ON rs.task_id = t.id AND rs.run_id = $3
         WHERE t.id = $1 AND t.job_id = $2",
        run.executing_task_id,
        run.job_id,
        run.job_run_id,
    )
    .fetch_optional(pool)
    .await?;
    let task = match active_task {
        Some(task)
            if matches!(
                task.run_status.as_deref(),
                Some("done" | "blocked" | "cancelled")
            ) =>
        {
            let reason = DocketExecutionReason::ActiveTaskIsStale;
            let disposition = record_work_execution_rejection(pool, run.id, &reason).await?;
            return Ok(WorkExecutionAuthorization::Rejected(
                DocketExecutionGate::Rejected {
                    disposition,
                    reason,
                },
            ));
        }
        Some(task) => (
            task.id,
            task.title,
            task.body,
            sqlx::types::Json(serde_json::from_value(task.completion_criteria).map_err(
                |error| {
                    DenError::ValidationError(format!("invalid task completion criteria: {error}"))
                },
            )?),
            task.difficulty,
        ),
        None => match crate::db::select_next_execution_task(pool, run.bear_id, run.job_id).await? {
            Some(task) => (
                task.id,
                task.title,
                task.body,
                task.completion_criteria,
                task.difficulty,
            ),
            None => {
                let reason = DocketExecutionReason::NoActionableTask;
                let disposition = record_work_execution_rejection(pool, run.id, &reason).await?;
                return Ok(WorkExecutionAuthorization::Rejected(
                    DocketExecutionGate::Rejected {
                        disposition,
                        reason,
                    },
                ));
            }
        },
    };
    clear_work_execution_rejections(pool, run.id).await?;
    let gate = DocketExecutionGate::Allowed {
        task_id: task.0,
        binding: DocketExecutionBinding::WorkRun {
            work_run_id: run.id,
            job_run_id: run.job_run_id,
        },
    };
    Ok(WorkExecutionAuthorization::Allowed { task, gate })
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct WorkPromptContext {
    /// The durable sandbox authority. A Work run is always assigned to one Job.
    pub job_id: Uuid,
    pub run_id: Uuid,
    pub goal: String,
    /// The task currently selected for progress within `job_id`; it is not the
    /// Work run's sandbox assignment.
    pub current_task_id: Uuid,
    pub tasks: Vec<WorkPromptTask>,
    pub commit_policy: Option<String>,
    pub notebook_entries: Vec<DocketEntryRow>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct WorkPromptTask {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub completion_criteria: Vec<String>,
}

/// The armature side of `work.checkout`: bind the session to its run, open
/// the Docket execution session whose job/task focus satisfies the Work-stance
/// gate, and build the non-interactive prompt from the durable task definition.
pub async fn checkout_work_run_for_session(
    pool: &PgPool,
    run_id: Uuid,
    bear_id: Uuid,
    session_id: &str,
) -> Result<WorkRunCheckout, DenError> {
    let run = bind_work_run_session(pool, run_id, bear_id, session_id).await?;

    // A pending directive blocks a new authorization epoch. It is checked at
    // checkout—not per workspace mutation—so a fresh Allowed decision remains
    // the only way to resume after checkpoint acknowledgement.
    if crate::db::pending_checkpoint_directive_for_work_run(pool, run.id)
        .await?
        .is_some()
    {
        sqlx::query!(
            "UPDATE bear_work_runs
             SET bearwire_session_id = NULL, updated_at = NOW()
             WHERE id = $1 AND bearwire_session_id = $2",
            run.id,
            session_id,
        )
        .execute(pool)
        .await?;
        return Ok(WorkRunCheckout {
            run,
            gate: DocketExecutionGate::Rejected {
                reason: DocketExecutionReason::CheckpointRequired,
                disposition: DocketExecutionDisposition::RequireCheckpoint,
            },
            execution_attempt: None,
            prompt_context: None,
            task_title: None,
        });
    }

    let authorization = authorize_work_execution(pool, &run).await?;
    let (task, gate) = match authorization {
        WorkExecutionAuthorization::Allowed { task, gate } => (task, gate),
        WorkExecutionAuthorization::Rejected(gate) => {
            // The initial bind makes the live-run check atomic with checkout. A
            // rejected gate must not leave a session claiming runnable work.
            sqlx::query!(
                "UPDATE bear_work_runs
                 SET bearwire_session_id = NULL, updated_at = NOW()
                 WHERE id = $1 AND bearwire_session_id = $2",
                run.id,
                session_id,
            )
            .execute(pool)
            .await?;
            return Ok(WorkRunCheckout {
                run,
                gate,
                execution_attempt: None,
                prompt_context: None,
                task_title: None,
            });
        }
    };
    // A checkout records the selected task on the work run. Task state is
    // durable outcome only; active execution is derived from this live run.
    sqlx::query!(
        "UPDATE bear_work_runs SET executing_task_id = $2, updated_at = NOW() WHERE id = $1",
        run.id,
        task.0
    )
    .execute(pool)
    .await?;

    let service = PgDocketService::from_pool(pool);
    let attempt = service
        .authorize_execution_attempt(DocketExecutionAttemptAuthorize {
            bear_id,
            task_id: task.0,
            binding: DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::WorkAssignment,
                id: run.id.to_string(),
            },
            host: DocketExecutionHost {
                kind: DocketExecutionHostKind::WorkRun,
                run_id: run.id.to_string(),
            },
            // A work run is one durable dispatch attempt. Re-checkout must
            // replay authorization rather than create a competing authority.
            authorization_key: run.id,
        })
        .await?;
    let attempt = service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: attempt.id,
            fence_epoch: attempt.fence_epoch,
        })
        .await?;

    let difficulty = task.4.as_deref().and_then(parse_task_difficulty);
    let resolved_profile = resolve_execution_profile(difficulty);
    let persisted_profile = resolved_profile.persisted_value();
    let routing = route_turn(
        pool,
        TurnIntent {
            // One work run is one durable dispatch turn. Re-checkout reuses it.
            idempotency_key: run.id,
            bear_id,
            job_id: run.job_id,
            run_id: run.job_run_id,
            task_id: task.0,
            source: TurnSource::Dispatch,
            originating_conversation_id: None,
            parent_conversation_id: None,
            surface: if run.execution_target == "attached_armature" {
                ExecutionSurface::Armature
            } else {
                ExecutionSurface::Sandbox
            },
            resolved_profile: Some(persisted_profile),
            attempt: run.attempt,
        },
    )
    .await?;
    claim_turn_attempt(
        pool,
        routing.id,
        Some(run.id),
        run.attempt,
        resolved_profile,
    )
    .await?;
    let tasks = vec![(task.0, task.1, task.2, task.3)];
    let job = sqlx::query!(
        "SELECT goal AS \"goal!\", commit_policy AS \"commit_policy?\" FROM bear_jobs WHERE id = $1",
        run.job_id
    )
    .fetch_one(pool)
    .await?;
    let goal = job.goal;
    let commit_policy = job.commit_policy;
    let task_title = format!(
        "{} work task{}",
        tasks.len(),
        if tasks.len() == 1 { "" } else { "s" }
    );
    let service = PgDocketService::from_pool(pool);
    let notebook_context = service
        .list_entries(
            bear_id,
            DocketEntryListFilter {
                job_id: Some(run.job_id),
                task_id: None,
                limit: 500,
            },
        )
        .await?;
    let notebook_context = select_dispatch_notebook_context(&notebook_context);
    Ok(WorkRunCheckout {
        gate,
        execution_attempt: Some(attempt),
        prompt_context: Some(WorkPromptContext {
            job_id: run.job_id,
            run_id: run.job_run_id,
            goal,
            current_task_id: task.0,
            tasks: tasks
                .into_iter()
                .map(|(id, title, body, criteria)| WorkPromptTask {
                    id,
                    title,
                    body,
                    completion_criteria: criteria.0,
                })
                .collect(),
            commit_policy,
            notebook_entries: notebook_context,
        }),
        run,
        task_title: Some(task_title),
    })
}

fn parse_task_difficulty(raw: &str) -> Option<DocketTaskDifficulty> {
    Some(match raw {
        "trivial" => DocketTaskDifficulty::Trivial,
        "moderate" => DocketTaskDifficulty::Moderate,
        "hard" => DocketTaskDifficulty::Hard,
        "unknown" => DocketTaskDifficulty::Unknown,
        _ => return None,
    })
}

pub async fn get_work_run(pool: &PgPool, run_id: Uuid) -> Result<Option<WorkRunRow>, DenError> {
    let row = sqlx::query_as!(WorkRunRow,        "SELECT id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at FROM bear_work_runs WHERE id = $1", run_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

#[derive(Clone, Debug, Default)]
pub struct WorkRunListFilter {
    pub bear_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub state: Option<String>,
    pub limit: i64,
}

pub async fn list_work_runs(
    pool: &PgPool,
    filter: WorkRunListFilter,
) -> Result<Vec<WorkRunRow>, DenError> {
    let limit = if filter.limit <= 0 {
        50
    } else {
        filter.limit.min(200)
    };
    let rows = sqlx::query_as!(WorkRunRow,        "SELECT id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at FROM bear_work_runs
         WHERE ($1::uuid IS NULL OR bear_id = $1)
           AND ($2::uuid IS NULL OR job_id = $2)
           AND ($3::text IS NULL OR state = $3)
         ORDER BY queued_at DESC
         LIMIT $4", filter.bear_id, filter.job_id, filter.state.as_deref(), limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Runs currently owned by a worker whose lease is still live, for the
/// monitor step of the dispatch loop.
pub async fn list_owned_work_runs(
    pool: &PgPool,
    runner_id: &str,
) -> Result<Vec<WorkRunRow>, DenError> {
    let rows = sqlx::query_as!(WorkRunRow,        "SELECT id, bear_id, job_id, job_run_id, executing_task_id, attempt, state, runner_id, lease_expires_at, cancel_requested, cancel_requested_by, cancel_reason, cancel_requested_at, git_ref, image_name, sandbox_server_url, sandbox_id, sandbox_type, sandbox_strength, work_surface, execution_target, attached_client_session_id, attachment_state, attachment_warning, disconnected_at, disconnect_deadline_at, bearwire_session_id, result_summary, result_refs, usage, error, queued_at, started_at, finished_at, updated_at FROM bear_work_runs
         WHERE runner_id = $1
           AND state IN ('claimed', 'provisioning', 'running', 'reporting')
         ORDER BY queued_at ASC", runner_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Merge extra keys into a run's `result_refs` (e.g. the minted armature
/// token id, recorded so a restarted worker can still revoke it).
pub async fn merge_work_run_result_refs(
    pool: &PgPool,
    run_id: Uuid,
    refs: &Value,
) -> Result<(), DenError> {
    sqlx::query!(
        "UPDATE bear_work_runs
         SET result_refs = COALESCE(result_refs, '{}'::jsonb) || $2::jsonb, updated_at = now()
         WHERE id = $1",
        run_id,
        refs
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// The run-scoped Docket status of a task (`bear_task_run_state`), used by
/// the harvest step: a work run succeeded only if the model marked the task
/// done in-turn. `None` = no run state recorded (treated as pending).
pub async fn get_task_run_status(
    pool: &PgPool,
    job_run_id: Uuid,
    task_id: Uuid,
) -> Result<Option<String>, DenError> {
    sqlx::query_scalar!(
        "SELECT status AS \"status!\" FROM bear_task_run_state WHERE run_id = $1 AND task_id = $2",
        job_run_id,
        task_id
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Run-scoped states for every unfinished task in a job. A job-scoped work run
/// owns the sandbox lifecycle; these remain the individual completion
/// checkpoints reported by the agent.
pub async fn get_job_work_task_run_statuses(
    pool: &PgPool,
    job_id: Uuid,
    job_run_id: Uuid,
) -> Result<Vec<(Uuid, String)>, DenError> {
    let rows = sqlx::query!(
        "SELECT t.id AS \"id!\", COALESCE(s.status, 'pending') AS \"status!\"
         FROM bear_tasks t
         LEFT JOIN bear_task_run_state s ON s.task_id = t.id AND s.run_id = $2
         WHERE t.job_id = $1
         ORDER BY t.sibling_order, t.created_at",
        job_id,
        job_run_id
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| (row.id, row.status)).collect())
}

/// Bears that have jobs with unfinished tasks, for the optional auto-enqueue
/// sweep (`WORK_DISPATCH_AUTO`).
pub async fn list_bears_with_work_tasks(pool: &PgPool) -> Result<Vec<Uuid>, DenError> {
    sqlx::query_scalar!(
        "SELECT DISTINCT bear_id AS \"bear_id!\" FROM bear_jobs WHERE lifecycle_intent IS NULL"
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Everything the dispatch worker needs to provision a claimed run.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct WorkRunDispatchContext {
    pub bear_slug: String,
    pub bear_name: String,
    pub created_by_user_id: i32,
    pub job_goal: String,
    pub work_surface_name: Option<String>,
    pub commit_policy: Option<String>,
    pub work_branch: Option<String>,
    pub allow_default_ref: bool,
    /// Validated child summaries for this job run. Raw child transcripts and tool traces
    /// are intentionally not projected into the dispatch context.
    pub child_result_rollups: serde_json::Value,
}

impl WorkRunDispatchContext {
    /// Whether the job's commit policy publishes successful runs to the
    /// upstream work branch (`per_task` / `per_job`).
    pub fn publishes(&self) -> bool {
        matches!(self.commit_policy.as_deref(), Some("per_task" | "per_job"))
    }
}

pub async fn get_work_run_dispatch_context(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<WorkRunDispatchContext, DenError> {
    sqlx::query_as!(WorkRunDispatchContext,
        "SELECT b.slug AS bear_slug, b.name AS bear_name, j.created_by_user_id, j.goal AS job_goal, s.name AS work_surface_name,
                j.commit_policy, j.work_branch,
                COALESCE(j.work_branch = g.default_ref, FALSE) AS \"allow_default_ref!\",
                COALESCE((
                    SELECT jsonb_agg(
                        jsonb_build_object('summary', rr.summary, 'evidence_refs', rr.evidence_refs)
                        ORDER BY rr.created_at, rr.task_id
                    )
                    FROM docket_result_rollups rr
                    WHERE rr.run_id = r.job_run_id
                      AND rr.parent_task_id = r.executing_task_id
                ), '[]'::jsonb) AS child_result_rollups
         FROM bear_work_runs r
         JOIN bears b ON b.id = r.bear_id
         JOIN bear_jobs j ON j.id = r.job_id
         LEFT JOIN LATERAL (
             SELECT a.work_surface_id
             FROM job_work_surface_assignments a
             JOIN work_surfaces candidate ON candidate.id = a.work_surface_id
             WHERE a.job_id = j.id
               AND candidate.kind = 'git_workspace'
               AND a.mutation_policy <> 'forbidden'
             ORDER BY a.created_at
             LIMIT 1
         ) assignment ON true
         LEFT JOIN work_surfaces s ON s.id = assignment.work_surface_id
         LEFT JOIN git_work_surface_details g ON g.id = s.id
         WHERE r.id = $1", run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound(format!("work run not found: {run_id}")))
}

/// The branch a job's work runs publish to, setting the generated default
/// (`den/job-<short-id>`) on first use. Callers may have set an explicit
/// branch at job creation; this never overwrites one.
pub async fn ensure_job_work_branch(pool: &PgPool, job_id: Uuid) -> Result<String, DenError> {
    let generated = format!("den/job-{}", &job_id.simple().to_string()[..8]);
    let branch = sqlx::query_scalar!(
        "UPDATE bear_jobs
         SET work_branch = COALESCE(work_branch, $2), updated_at = now()
         WHERE id = $1
         RETURNING work_branch AS \"work_branch!\"",
        job_id,
        &generated
    )
    .fetch_one(pool)
    .await?;
    Ok(branch)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{effective_work_run_surface, is_attached_recovery_source};

    #[test]
    fn attached_recovery_requires_the_canonical_timeout_outcome() {
        let timeout = json!({"outcome": {"code": "armature_disconnect_timeout"}});
        assert!(is_attached_recovery_source(
            "attached_armature",
            "timed_out",
            Some(&timeout)
        ));
        assert!(!is_attached_recovery_source(
            "sandbox",
            "timed_out",
            Some(&timeout)
        ));
        assert!(!is_attached_recovery_source(
            "attached_armature",
            "failed",
            Some(&timeout)
        ));
        assert!(!is_attached_recovery_source(
            "attached_armature",
            "timed_out",
            Some(&json!({"outcome": {"code": "activity_timeout"}}))
        ));
    }

    #[test]
    fn effective_work_run_surface_trims_the_managed_surface_name() {
        assert_eq!(
            effective_work_run_surface(Some(" managed-surface ")),
            Some("managed-surface".to_string())
        );
        assert_eq!(effective_work_run_surface(Some("   ")), None);
        assert_eq!(effective_work_run_surface(None), None);
    }
}
