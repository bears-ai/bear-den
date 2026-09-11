//! `den-docket` — Den's control-plane subsystem for work management (ADR-0034).
//!
//! Public face is [`DocketService`] / [`PgDocketService`]; `db` is crate-internal.
//! Docket jobs/tasks use the ADR-0034 relational Postgres tables. Historical
//! task-list tables may exist in old migrations/data, but this crate should not
//! keep active read/write shims for them.
//! This crate is a service-layer leaf: it depends only on `den-core`. The
//! relational jobs/tasks realization is tracked in
//! `docs/roadmap/DOCKET_IMPLEMENTATION_PLAN.md`; the crate split itself in
//! `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md`.

pub mod cursors;
mod db;
pub mod diagnostics;
pub mod dispatch_preflight;
mod dispatcher;
#[cfg(test)]
mod execution_attempt_tests;
#[cfg(test)]
mod execution_control_tests;
pub mod execution_profiles;
#[cfg(test)]
mod integration_tests;
pub mod model;
#[cfg(test)]
mod primary_output_tests;
pub mod recovery;
pub mod routing;
pub mod service;
pub mod supervisor;
pub mod work_runs;
#[cfg(test)]
mod work_runs_tests;

pub use cursors::{clear_cursor, get_cursor, set_cursor, DocketCursor};
pub use diagnostics::{
    run_diagnostics, DiagnosticAttachment, DiagnosticAttention, DiagnosticEvent, DiagnosticOutcome,
    DiagnosticRollup, DiagnosticTask, NormalizedFailure, RunDiagnostics,
};
pub use dispatch_preflight::{
    preflight_dispatch, CheckoutRelationship, DispatchBlocker, DispatchPreflight,
    DurableResultKind, PublicationRoute,
};
pub use dispatcher::TaskDispatcher;
pub use execution_profiles::{
    resolve_execution_profile, ExecutionProfile, ProfileProvenance, ResolvedExecutionProfile,
};
pub use model::{
    docket_job_status_report, docket_task_status_from_task_list_item_status,
    normalize_task_list_item_ids, render_task_list_prompt_context, role_can_read_task_list,
    role_can_request_task_list_handoff, role_can_update_task_list,
    select_dispatch_notebook_context, task_list_item_from_update_item,
    task_list_projection_from_docket_job, task_list_projection_from_local,
    task_list_projection_from_session_tasks,
    task_list_projection_from_session_tasks_with_current_task, validate_docket_job_create,
    validate_docket_task_create, validate_task_list_items, validate_task_list_update,
    DocketCheckpointDirectiveAcknowledge, DocketCheckpointDirectiveRow,
    DocketCheckpointDirectiveState, DocketCommitPolicy, DocketCountByStatus,
    DocketCriteriaCountByStatus, DocketCriterionKind, DocketCriterionStateRow,
    DocketCriterionStateUpdate, DocketCriterionStatus, DocketEffortHint, DocketEntryCreate,
    DocketEntryKind, DocketEntryListFilter, DocketEntryPromotion, DocketEntryRow, DocketEntryScope,
    DocketExecutionAttemptAuthorize, DocketExecutionAttemptRelease, DocketExecutionAttemptRow,
    DocketExecutionAttemptStart, DocketExecutionAttemptState, DocketExecutionBinding,
    DocketExecutionBindingKind, DocketExecutionControl, DocketExecutionControlReference,
    DocketExecutionControlResult, DocketExecutionControlState, DocketExecutionDisposition,
    DocketExecutionGate, DocketExecutionHost, DocketExecutionHostKind, DocketExecutionNextAction,
    DocketExecutionReason, DocketExecutionTaskSettlement, DocketFocusedExecutionAcquire,
    DocketFocusedExecutionBinding, DocketJobCreate, DocketJobCriterionInput, DocketJobCriterionRow,
    DocketJobExecuteOutcome, DocketJobExecuteRequest, DocketJobListFilter,
    DocketJobOverlapResolution, DocketJobProjection, DocketJobRow, DocketJobRunRow,
    DocketJobStatus, DocketJobStatusReport, DocketJobSurfaceAssignmentInput, DocketJobUpdate,
    DocketOutcomeDisposition, DocketPairAwaitingUserQuestion, DocketPairAwaitingUserResume,
    DocketPairBoundedOutcome, DocketPairBoundedOutcomeDecision, DocketPairBoundedOutcomeReport,
    DocketPairContinuationDecision, DocketRunState, DocketRunTrigger,
    DocketSchedulerObservationDeliveryState, DocketSchedulerObservationDisposition,
    DocketSchedulerObservationEnqueue, DocketSchedulerObservationRow, DocketSessionTaskSettlement,
    DocketTaskCreate, DocketTaskDefinitionPatch, DocketTaskDifficulty, DocketTaskInput,
    DocketTaskKind, DocketTaskListFilter, DocketTaskPlacement, DocketTaskProjection, DocketTaskRow,
    DocketTaskRunStateRow, DocketTaskRunStateUpdate, DocketTaskScope, DocketTaskStatus,
    DocketTaskUpdate, DocketValidationError, DocketWorkBoundaryCheck, DocketWorkBoundarySignal,
    MutationPolicy, ResultRollupPolicy, RoutingStrategy, TaskListCheckoutRequest,
    TaskListCheckoutSource, TaskListHandoffOutcome, TaskListHandoffRequest, TaskListItem,
    TaskListItemStatus, TaskListLocalProjection, TaskListProjection, TaskListSourceRef,
    TaskListSyncOutcome, TaskListSyncRequest, TaskListSyncState, TaskListUpdate,
    TaskListUpdateItem, TaskListValidationError, TaskListVisibility,
    DISPATCH_NOTEBOOK_CONTEXT_MAX_CHARS, DISPATCH_NOTEBOOK_CONTEXT_MAX_ENTRIES,
};
pub use recovery::{
    apply_supervisor_disposition, claim_turn_attempt, decide_escalation, disposition_for,
    escalation_for_attempt, parent_rollup_context, persist_attention_outbox, persist_result_rollup,
    record_turn_activity, terminalize_stale_attempts, terminalize_turn_attempt, AttemptOutcome,
    EscalationDecision, ResultRollup, RetryDisposition, SupervisorDisposition, TurnAttempt,
    MAX_TURN_ATTEMPTS,
};
pub use routing::{
    route_turn, ConversationStrategy, ExecutionSurface, RoutingDecision, TurnIntent, TurnSource,
};
pub use service::{DocketService, PgDocketService};
pub use supervisor::set_work_run_paused;
pub use work_runs::{
    request_work_run_cancel_with_provenance, resolve_stalled_work_run, StalledWorkRunResolution,
    WorkRunCancelRequest,
};
