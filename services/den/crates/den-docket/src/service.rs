//! `DocketService`: the single public face of the Docket subsystem.
//!
//! Callers (tools, ACP, web) depend on this trait / the concrete
//! `PgDocketService`, never on `docket::db`. This is the seam the crate split
//! promotes to a `den-docket` crate boundary; `TaskDispatcher` (the runtime
//! inversion trait) is added when `den-runtime` is extracted.

use sqlx::PgPool;
use uuid::Uuid;

use den_core::{BearProfile, DenError};

use super::db;
use super::model::{
    task_list_projection_from_docket_job, task_list_projection_from_session_tasks,
    DocketCheckpointDirectiveAcknowledge, DocketCheckpointDirectiveRow, DocketCriterionStateUpdate,
    DocketEntryCreate, DocketEntryListFilter, DocketEntryPromotion, DocketEntryRow,
    DocketExecutionAttemptAuthorize, DocketExecutionAttemptRelease, DocketExecutionAttemptRow,
    DocketExecutionAttemptStart, DocketExecutionGate, DocketExecutionTaskSettlement,
    DocketFocusedAwaitingUserResume, DocketFocusedExecutionAcquire, DocketFocusedExecutionBinding,
    DocketFocusedSliceOutcomeDecision, DocketFocusedSliceOutcomeReport, DocketJobCreate,
    DocketJobExecuteOutcome, DocketJobExecuteRequest, DocketJobListFilter, DocketJobProjection,
    DocketJobRow, DocketJobUpdate, DocketSessionTaskSettlement, DocketTaskCreate,
    DocketTaskListFilter, DocketTaskProjection, DocketTaskRow, DocketTaskUpdate,
    DocketWorkBoundaryCheck, TaskListCheckoutRequest, TaskListCheckoutSource,
    TaskListHandoffOutcome, TaskListHandoffRequest, TaskListProjection, TaskListSyncOutcome,
    TaskListSyncRequest,
};

/// Orchestration API for task and job state. The only public entry point to the
/// subsystem's persistence; never execute task bodies here (ADR-0034 execution
/// invariant) — Docket schedules, gates, and records.
// Native async fn in trait: workspace-internal, only ever consumed via generic
// bounds / the concrete `PgDocketService` (never `dyn`), so auto-trait (Send)
// bounds flow through monomorphization and async-trait boxing is unnecessary.
#[allow(async_fn_in_trait)]
pub trait DocketService: Send + Sync {
    async fn create_job(&self, create: DocketJobCreate) -> Result<DocketJobProjection, DenError>;

    async fn list_jobs(
        &self,
        bear_id: Uuid,
        filter: DocketJobListFilter,
    ) -> Result<Vec<DocketJobRow>, DenError>;

    async fn get_job(
        &self,
        bear_id: Uuid,
        job_id: Uuid,
    ) -> Result<Option<DocketJobProjection>, DenError>;

    async fn update_job(&self, update: DocketJobUpdate) -> Result<DocketJobProjection, DenError>;

    async fn evaluate_criterion(
        &self,
        update: DocketCriterionStateUpdate,
    ) -> Result<DocketJobProjection, DenError>;

    async fn execute_job(
        &self,
        request: DocketJobExecuteRequest,
    ) -> Result<DocketJobExecuteOutcome, DenError>;

    /// Repairs stale scheduler focus and returns the same authoritative control
    /// result as execution. Call this instead of retrying `execute_job` after a
    /// non-retryable `reconcile_execution` outcome.
    async fn reconcile_execution(
        &self,
        request: DocketJobExecuteRequest,
    ) -> Result<DocketJobExecuteOutcome, DenError>;

    /// Cancels the current Docket run in the owning Bear scope. This is
    /// deliberately separate from sandbox work-run cancellation.
    async fn cancel_job_run(
        &self,
        bear_id: Uuid,
        job_id: Uuid,
    ) -> Result<DocketJobProjection, DenError>;

    /// Settles the session's claimed job task and returns successor control.
    /// Ordinary Pair/session tasks must continue to use their own settlement API.
    async fn settle_execution_task(
        &self,
        settlement: DocketExecutionTaskSettlement,
    ) -> Result<DocketJobExecuteOutcome, DenError>;

    /// Atomically acquires or reuses the one live focused-execution attempt for
    /// a stable host-neutral binding.
    async fn acquire_focused_execution(
        &self,
        acquire: DocketFocusedExecutionAcquire,
    ) -> Result<DocketExecutionAttemptRow, DenError>;

    /// Loads the live authority for a stable host-neutral binding.
    async fn get_live_focused_execution(
        &self,
        bear_id: Uuid,
        binding: DocketFocusedExecutionBinding,
    ) -> Result<Option<DocketExecutionAttemptRow>, DenError>;

    async fn authorize_execution_attempt(
        &self,
        authorize: DocketExecutionAttemptAuthorize,
    ) -> Result<DocketExecutionAttemptRow, DenError>;

    async fn start_execution_attempt(
        &self,
        start: DocketExecutionAttemptStart,
    ) -> Result<DocketExecutionAttemptRow, DenError>;

    /// Loads the exact live session-task authority already bound to a run. Repeated
    /// focus/start calls must reuse this row rather than minting new authority.
    async fn get_live_session_task_execution_attempt(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        session_id: &str,
        turn_run_id: &str,
    ) -> Result<Option<DocketExecutionAttemptRow>, DenError>;

    async fn get_live_session_task_execution_attempt_for_session(
        &self,
        bear_id: Uuid,
        session_id: &str,
    ) -> Result<Option<DocketExecutionAttemptRow>, DenError>;

    /// Loads any live session-task authority for this task, regardless of session.
    /// Callers must verify a foreign host is truly orphaned before releasing it.
    async fn get_live_session_task_execution_attempt_for_task(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
    ) -> Result<Option<DocketExecutionAttemptRow>, DenError>;

    /// Revalidates exact canonical Work authority at a safe boundary. A
    /// pending checkpoint directive denies a new runtime window.
    async fn check_work_boundary(
        &self,
        check: DocketWorkBoundaryCheck,
    ) -> Result<DocketExecutionGate, DenError>;

    /// Reconciles a lost owner by terminally releasing the exact live attempt.
    /// `recovery_key` makes repeated recovery delivery idempotent.
    async fn release_execution_attempt(
        &self,
        release: DocketExecutionAttemptRelease,
    ) -> Result<DocketExecutionAttemptRow, DenError>;

    /// Records a fenced focused slice outcome and returns Docket's canonical next action.
    async fn report_focused_slice_outcome(
        &self,
        report: DocketFocusedSliceOutcomeReport,
    ) -> Result<DocketFocusedSliceOutcomeDecision, DenError>;

    /// Records an authenticated response to the exact pending focused-session
    /// question and makes the attempt startable again. Callers must authenticate
    /// the user/session before invoking this Docket transition.
    async fn resume_focused_awaiting_user(
        &self,
        resume: DocketFocusedAwaitingUserResume,
    ) -> Result<DocketExecutionAttemptRow, DenError>;

    async fn acknowledge_checkpoint_directive(
        &self,
        acknowledge: DocketCheckpointDirectiveAcknowledge,
    ) -> Result<DocketCheckpointDirectiveRow, DenError>;

    async fn list_session_tasks(
        &self,
        bear_id: Uuid,
        session_id: Uuid,
    ) -> Result<Vec<DocketTaskProjection>, DenError>;

    async fn attach_task_to_session(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), DenError>;

    async fn create_task(&self, create: DocketTaskCreate) -> Result<DocketTaskRow, DenError>;

    async fn list_tasks(
        &self,
        bear_id: Uuid,
        filter: DocketTaskListFilter,
    ) -> Result<Vec<DocketTaskProjection>, DenError>;

    async fn update_task(&self, update: DocketTaskUpdate)
        -> Result<DocketTaskProjection, DenError>;

    async fn settle_session_task(
        &self,
        settlement: DocketSessionTaskSettlement,
    ) -> Result<DocketTaskProjection, DenError>;

    async fn append_entry(&self, create: DocketEntryCreate) -> Result<DocketEntryRow, DenError>;

    async fn promote_entry(
        &self,
        promotion: DocketEntryPromotion,
    ) -> Result<DocketEntryRow, DenError>;

    async fn list_entries(
        &self,
        bear_id: Uuid,
        filter: DocketEntryListFilter,
    ) -> Result<Vec<DocketEntryRow>, DenError>;

    async fn checkout_task_list(
        &self,
        bear_id: Uuid,
        viewer_role: BearProfile,
        user_id: i32,
        request: TaskListCheckoutRequest,
    ) -> Result<Option<TaskListProjection>, DenError>;

    async fn sync_task_list(
        &self,
        request: TaskListSyncRequest,
    ) -> Result<TaskListSyncOutcome, DenError>;

    async fn request_task_list_handoff(
        &self,
        request: TaskListHandoffRequest,
    ) -> Result<TaskListHandoffOutcome, DenError>;
}

/// Postgres-backed `DocketService`. Holds the shared Den pool (cheap to clone —
/// it is an `Arc` internally).
#[derive(Debug, Clone)]
pub struct PgDocketService {
    pub(crate) pool: PgPool,
}

impl PgDocketService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Construct from a borrowed pool (clones the inner `Arc`).
    pub fn from_pool(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }
}

impl DocketService for PgDocketService {
    async fn create_job(&self, create: DocketJobCreate) -> Result<DocketJobProjection, DenError> {
        db::create_job(&self.pool, create).await
    }

    async fn list_jobs(
        &self,
        bear_id: Uuid,
        filter: DocketJobListFilter,
    ) -> Result<Vec<DocketJobRow>, DenError> {
        db::list_jobs(&self.pool, bear_id, filter).await
    }

    async fn get_job(
        &self,
        bear_id: Uuid,
        job_id: Uuid,
    ) -> Result<Option<DocketJobProjection>, DenError> {
        db::get_job(&self.pool, bear_id, job_id).await
    }

    async fn update_job(&self, update: DocketJobUpdate) -> Result<DocketJobProjection, DenError> {
        db::update_job(&self.pool, update).await
    }

    async fn evaluate_criterion(
        &self,
        update: DocketCriterionStateUpdate,
    ) -> Result<DocketJobProjection, DenError> {
        db::evaluate_criterion(&self.pool, update).await
    }

    async fn execute_job(
        &self,
        request: DocketJobExecuteRequest,
    ) -> Result<DocketJobExecuteOutcome, DenError> {
        db::execute_job(&self.pool, request).await
    }

    async fn reconcile_execution(
        &self,
        request: DocketJobExecuteRequest,
    ) -> Result<DocketJobExecuteOutcome, DenError> {
        db::reconcile_execution(&self.pool, request).await
    }

    async fn cancel_job_run(
        &self,
        bear_id: Uuid,
        job_id: Uuid,
    ) -> Result<DocketJobProjection, DenError> {
        db::cancel_job_run(&self.pool, bear_id, job_id).await
    }

    async fn settle_execution_task(
        &self,
        settlement: DocketExecutionTaskSettlement,
    ) -> Result<DocketJobExecuteOutcome, DenError> {
        db::settle_execution_task(&self.pool, settlement).await
    }

    async fn acquire_focused_execution(
        &self,
        acquire: DocketFocusedExecutionAcquire,
    ) -> Result<DocketExecutionAttemptRow, DenError> {
        db::acquire_focused_execution(&self.pool, acquire).await
    }

    async fn get_live_focused_execution(
        &self,
        bear_id: Uuid,
        binding: DocketFocusedExecutionBinding,
    ) -> Result<Option<DocketExecutionAttemptRow>, DenError> {
        db::get_live_focused_execution(&self.pool, bear_id, binding).await
    }

    async fn authorize_execution_attempt(
        &self,
        authorize: DocketExecutionAttemptAuthorize,
    ) -> Result<DocketExecutionAttemptRow, DenError> {
        db::authorize_execution_attempt(&self.pool, authorize).await
    }

    async fn start_execution_attempt(
        &self,
        start: DocketExecutionAttemptStart,
    ) -> Result<DocketExecutionAttemptRow, DenError> {
        db::start_execution_attempt(&self.pool, start).await
    }

    async fn get_live_session_task_execution_attempt(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        session_id: &str,
        turn_run_id: &str,
    ) -> Result<Option<DocketExecutionAttemptRow>, DenError> {
        db::get_live_session_task_execution_attempt(
            &self.pool,
            bear_id,
            task_id,
            session_id,
            turn_run_id,
        )
        .await
    }

    async fn get_live_session_task_execution_attempt_for_session(
        &self,
        bear_id: Uuid,
        session_id: &str,
    ) -> Result<Option<DocketExecutionAttemptRow>, DenError> {
        db::get_live_session_task_execution_attempt_for_session(&self.pool, bear_id, session_id)
            .await
    }

    async fn get_live_session_task_execution_attempt_for_task(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
    ) -> Result<Option<DocketExecutionAttemptRow>, DenError> {
        db::get_live_session_task_execution_attempt_for_task(&self.pool, bear_id, task_id).await
    }

    async fn check_work_boundary(
        &self,
        check: DocketWorkBoundaryCheck,
    ) -> Result<DocketExecutionGate, DenError> {
        db::check_work_boundary(&self.pool, check).await
    }

    async fn release_execution_attempt(
        &self,
        release: DocketExecutionAttemptRelease,
    ) -> Result<DocketExecutionAttemptRow, DenError> {
        db::release_execution_attempt(&self.pool, release).await
    }

    async fn report_focused_slice_outcome(
        &self,
        report: DocketFocusedSliceOutcomeReport,
    ) -> Result<DocketFocusedSliceOutcomeDecision, DenError> {
        db::report_focused_slice_outcome(&self.pool, report).await
    }

    async fn resume_focused_awaiting_user(
        &self,
        resume: DocketFocusedAwaitingUserResume,
    ) -> Result<DocketExecutionAttemptRow, DenError> {
        db::resume_focused_awaiting_user(&self.pool, resume).await
    }

    async fn acknowledge_checkpoint_directive(
        &self,
        acknowledge: DocketCheckpointDirectiveAcknowledge,
    ) -> Result<DocketCheckpointDirectiveRow, DenError> {
        db::acknowledge_checkpoint_directive(&self.pool, acknowledge).await
    }

    async fn list_session_tasks(
        &self,
        bear_id: Uuid,
        session_id: Uuid,
    ) -> Result<Vec<DocketTaskProjection>, DenError> {
        db::list_session_tasks(&self.pool, bear_id, session_id).await
    }

    async fn attach_task_to_session(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), DenError> {
        db::attach_task_to_session(&self.pool, bear_id, task_id, session_id).await
    }

    async fn create_task(&self, create: DocketTaskCreate) -> Result<DocketTaskRow, DenError> {
        db::create_task(&self.pool, create).await
    }

    async fn list_tasks(
        &self,
        bear_id: Uuid,
        filter: DocketTaskListFilter,
    ) -> Result<Vec<DocketTaskProjection>, DenError> {
        db::list_tasks(&self.pool, bear_id, filter).await
    }

    async fn update_task(
        &self,
        update: DocketTaskUpdate,
    ) -> Result<DocketTaskProjection, DenError> {
        db::update_task(&self.pool, update).await
    }

    async fn settle_session_task(
        &self,
        settlement: DocketSessionTaskSettlement,
    ) -> Result<DocketTaskProjection, DenError> {
        db::settle_session_task(&self.pool, settlement).await
    }

    async fn append_entry(&self, create: DocketEntryCreate) -> Result<DocketEntryRow, DenError> {
        db::append_entry(&self.pool, create).await
    }

    async fn promote_entry(
        &self,
        promotion: DocketEntryPromotion,
    ) -> Result<DocketEntryRow, DenError> {
        db::promote_entry(&self.pool, promotion).await
    }

    async fn list_entries(
        &self,
        bear_id: Uuid,
        filter: DocketEntryListFilter,
    ) -> Result<Vec<DocketEntryRow>, DenError> {
        db::list_entries(&self.pool, bear_id, filter).await
    }

    async fn checkout_task_list(
        &self,
        bear_id: Uuid,
        viewer_role: BearProfile,
        _user_id: i32,
        request: TaskListCheckoutRequest,
    ) -> Result<Option<TaskListProjection>, DenError> {
        match request.source {
            TaskListCheckoutSource::DocketJob {
                job_id,
                parent_task_id,
            } => {
                if let Some(session_id) = request.session_anchor_id {
                    db::attach_job_tasks_to_session(&self.pool, bear_id, job_id, session_id)
                        .await?;
                    let tasks = self.list_session_tasks(bear_id, session_id).await?;
                    Ok(task_list_projection_from_session_tasks(
                        bear_id,
                        viewer_role,
                        "",
                        session_id,
                        None,
                        &tasks,
                    ))
                } else {
                    Ok(self
                        .get_job(bear_id, job_id)
                        .await?
                        .map(|job| task_list_projection_from_docket_job(&job, parent_task_id)))
                }
            }
            TaskListCheckoutSource::LocalProjection(task_list) => Ok(Some(*task_list)),
        }
    }

    async fn sync_task_list(
        &self,
        request: TaskListSyncRequest,
    ) -> Result<TaskListSyncOutcome, DenError> {
        db::sync_task_list(&self.pool, request).await
    }

    async fn request_task_list_handoff(
        &self,
        request: TaskListHandoffRequest,
    ) -> Result<TaskListHandoffOutcome, DenError> {
        Ok(TaskListHandoffOutcome::review_required(
            &request,
            "Task-list handoff seam is present; durable Docket promotion awaits relational Docket backing.",
        ))
    }
}
