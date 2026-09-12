use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use den_core::tools::constants::{
    DEN_DOCKET_ENTRY_APPEND, DEN_DOCKET_ENTRY_LIST, DEN_DOCKET_ENTRY_PROMOTE, DEN_JOB_ARCHIVE,
    DEN_JOB_CANCEL, DEN_JOB_CANCEL_RUN, DEN_JOB_CREATE, DEN_JOB_EVALUATE_CRITERION,
    DEN_JOB_EXECUTE, DEN_JOB_FIND, DEN_JOB_GET, DEN_JOB_LIST, DEN_JOB_RECONCILE,
    DEN_JOB_SETTLE_TASK, DEN_JOB_UPDATE, DEN_RUNTIME_DIAGNOSTICS_LIST, DEN_TASK_CREATE,
    DEN_TASK_FIND, DEN_TASK_LIST, DEN_TASK_LISTS_GET_STATUS, DEN_TASK_LISTS_LIST,
    DEN_TASK_LISTS_UPDATE, DEN_TASK_LIST_CHECKOUT, DEN_TASK_LIST_SYNC, DEN_TASK_SELECT,
    DEN_TASK_UPDATE, DEN_TASK_UPDATE_CURRENT_STATUS, DEN_WORK_CATALOG, DEN_WORK_DISPATCH,
    DEN_WORK_RUN_CANCEL, DEN_WORK_RUN_FIND, DEN_WORK_RUN_GET, DEN_WORK_RUN_LIST,
    DEN_WORK_RUN_RESOLVE_STALLED, DEN_WORK_SURFACE_CONFIRM,
};
use den_docket::{
    self as docket, docket_job_status_report, DocketCommitPolicy, DocketCriterionStateUpdate,
    DocketCriterionStatus, DocketEffortHint, DocketEntryCreate, DocketEntryKind,
    DocketEntryListFilter, DocketEntryPromotion, DocketEntryScope, DocketExecutionNextAction,
    DocketExecutionTaskSettlement, DocketJobCreate, DocketJobCriterionInput,
    DocketJobExecuteRequest, DocketJobListFilter, DocketJobOverlapResolution, DocketJobProjection,
    DocketJobStatus, DocketJobStatusReport, DocketJobSurfaceAssignmentInput, DocketJobUpdate,
    DocketService, DocketSessionTaskSettlement, DocketTaskCreate, DocketTaskDefinitionPatch,
    DocketTaskDifficulty, DocketTaskInput, DocketTaskKind, DocketTaskListFilter,
    DocketTaskPlacement, DocketTaskRunStateUpdate, DocketTaskScope, DocketTaskStatus,
    DocketTaskUpdate, DocketValidationError, MutationPolicy, PgDocketService,
    TaskListCheckoutRequest, TaskListCheckoutSource, TaskListProjection, TaskListSyncRequest,
    TaskListVisibility,
};

use crate::{
    config::Config,
    core::tools::{
        session::DenToolInvocationContext, support::clean_optional,
        work_surface::persist_recognized_session_surface,
    },
    errors::{CustomError, DenError},
};
use den_memory::{tools as sqlite_memory, MemoryStoreManager};
use den_runtime::plan_mode;
use den_runtime::runtime_exception_events::{
    self, RuntimeExceptionEventFilter, RuntimeExceptionSeverity,
};
use den_runtime::{
    agent_loop::{LedgerEvidenceRef, LoopControlDecisionKind, LoopControlLedgerInput},
    current_task::select_session_current_task,
};
use den_service::bears::db::get_bear;
use den_service::{
    bears::BearProfile, client_sessions, conversation::persistence as conversation_persistence,
};

const FOCUSED_CONVERSATION_TITLE_MAX_CHARS: usize = 120;

fn effective_tool_policy(
    role: BearProfile,
    context: &DenToolInvocationContext,
) -> den_core::EffectivePolicy {
    den_core::EffectivePolicy::compile(
        role,
        den_core::Governance::Interactive,
        if context.client_session_id.is_some() {
            den_core::ArmatureAvailability::Connected
        } else {
            den_core::ArmatureAvailability::Absent
        },
    )
}

/// Chat-facing identity for a Docket resource. `id` remains the only value
/// callers may pass back to a tool; `display` is deliberately presentation
/// only.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct DocketEntityPresentation {
    id: Uuid,
    display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    href: Option<String>,
}

fn docket_entity_presentation(
    kind: &str,
    id: Uuid,
    ids: &[Uuid],
    href: Option<String>,
) -> DocketEntityPresentation {
    // Short handles are presentation only: tool inputs retain canonical UUIDs.
    // Canonical URLs use eight hexadecimal characters; extend only when this
    // response contains a collision.
    let id_text = id.simple().to_string();
    let prefix_len = (8..=id_text.len())
        .find(|&len| {
            ids.iter()
                .filter(|other| **other != id)
                .all(|other| !other.simple().to_string().starts_with(&id_text[..len]))
        })
        .unwrap_or(id_text.len());
    DocketEntityPresentation {
        id,
        display: format!("{kind} {}", &id_text[..prefix_len]),
        href,
    }
}

fn docket_entity_presentations(
    kind: &str,
    ids: impl IntoIterator<Item = Uuid>,
    href_for: impl Fn(Uuid) -> Option<String>,
) -> Vec<DocketEntityPresentation> {
    let ids: Vec<Uuid> = ids.into_iter().collect();
    ids.iter()
        .copied()
        .map(|id| docket_entity_presentation(kind, id, &ids, href_for(id)))
        .collect()
}

async fn docket_web_base(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
) -> Result<String, CustomError> {
    let bear = get_bear(pool, bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound(format!("bear not found: {bear_id}")))?;
    Ok(format!(
        "{}/bear/{}/jobs",
        config.web_server_url.trim_end_matches('/'),
        bear.slug
    ))
}

fn docket_resource_href(base: &str, segment: &str, id: Uuid) -> String {
    let compact_id = id.simple().to_string();
    format!("{base}/{segment}/{}", &compact_id[..16])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OrientedTaskCreatePolicy {
    root_task_id: Uuid,
    max_children: usize,
    max_depth_below_oriented_task: usize,
}

pub(crate) fn is_workflow_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        DEN_TASK_LISTS_LIST
            | DEN_TASK_LISTS_GET_STATUS
            | DEN_TASK_LISTS_UPDATE
            | DEN_JOB_CREATE
            | DEN_JOB_LIST
            | DEN_JOB_GET
            | DEN_JOB_FIND
            | DEN_JOB_UPDATE
            | DEN_JOB_CANCEL
            | DEN_JOB_CANCEL_RUN
            | DEN_JOB_ARCHIVE
            | DEN_JOB_EXECUTE
            | DEN_JOB_RECONCILE
            | DEN_JOB_SETTLE_TASK
            | DEN_JOB_EVALUATE_CRITERION
            | DEN_TASK_CREATE
            | DEN_TASK_LIST
            | DEN_TASK_FIND
            | DEN_TASK_UPDATE
            | DEN_TASK_UPDATE_CURRENT_STATUS
            | DEN_RUNTIME_DIAGNOSTICS_LIST
            | DEN_TASK_SELECT
            | DEN_DOCKET_ENTRY_APPEND
            | DEN_DOCKET_ENTRY_PROMOTE
            | DEN_DOCKET_ENTRY_LIST
            | DEN_TASK_LIST_SYNC
            | DEN_TASK_LIST_CHECKOUT
            | DEN_WORK_DISPATCH
            | DEN_WORK_RUN_LIST
            | DEN_WORK_RUN_GET
            | DEN_WORK_RUN_FIND
            | DEN_WORK_RUN_CANCEL
            | DEN_WORK_RUN_RESOLVE_STALLED
            | DEN_WORK_CATALOG
            | DEN_WORK_SURFACE_CONFIRM
    )
}

#[cfg(test)]
mod workflow_tool_tests {
    use den_core::tools::constants::{
        DEN_JOB_ARCHIVE, DEN_JOB_CANCEL, DEN_JOB_CANCEL_RUN, DEN_JOB_RECONCILE,
        DEN_JOB_SETTLE_TASK, DEN_RUNTIME_DIAGNOSTICS_LIST,
    };

    #[test]
    fn docket_lifecycle_and_execution_controls_are_workflow_tools() {
        assert!(super::is_workflow_tool(DEN_JOB_CANCEL));
        assert!(super::is_workflow_tool(DEN_JOB_CANCEL_RUN));
        assert!(super::is_workflow_tool(DEN_JOB_ARCHIVE));
        assert!(super::is_workflow_tool(DEN_JOB_RECONCILE));
        assert!(super::is_workflow_tool(DEN_JOB_SETTLE_TASK));
        assert!(super::is_workflow_tool(DEN_RUNTIME_DIAGNOSTICS_LIST));
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct TaskListListArguments {
    #[serde(default)]
    pub(crate) include_completed: bool,
    #[serde(default)]
    pub(crate) include_plan_mode: Option<bool>,
    #[serde(default)]
    pub(crate) include_artifacts: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobOverlapResolution {
    #[default]
    Reject,
    Independent,
    Supersede,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketJobSurfaceAssignmentArguments {
    pub(crate) work_surface_id: Uuid,
    #[serde(default)]
    pub(crate) mutation_policy: MutationPolicy,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketJobCreateArguments {
    pub(crate) goal: String,
    #[serde(default)]
    pub(crate) work_surface_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) work_surface_assignments: Vec<DocketJobSurfaceAssignmentArguments>,
    #[serde(default)]
    pub(crate) commit_policy: Option<DocketCommitPolicy>,
    #[serde(default)]
    pub(crate) work_branch: Option<String>,
    #[serde(default = "default_job_visibility")]
    pub(crate) visibility: TaskListVisibility,
    #[serde(default)]
    pub(crate) supersedes_job_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) overlap_resolution: JobOverlapResolution,
    #[serde(default)]
    pub(crate) criteria: Vec<DocketJobCriterionInput>,
    #[serde(default)]
    pub(crate) tasks: Vec<DocketTaskInput>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ConfirmWorkSurfaceArguments {
    pub(crate) work_surface_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketJobListArguments {
    #[serde(default, rename = "status")]
    pub(crate) statuses: Option<Vec<DocketJobStatus>>,
    #[serde(default)]
    pub(crate) include_cancelled: bool,
    #[serde(default)]
    pub(crate) limit: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketJobGetArguments {
    pub(crate) job_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketJobFindArguments {
    pub(crate) job_ref: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketJobUpdateArguments {
    pub(crate) job_id: Uuid,
    #[serde(default)]
    pub(crate) goal: Option<String>,
    #[serde(default)]
    pub(crate) work_surface_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) commit_policy: Option<DocketCommitPolicy>,
    #[serde(default)]
    pub(crate) clear_commit_policy: bool,
    #[serde(default)]
    pub(crate) visibility: Option<TaskListVisibility>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketJobLifecycleArguments {
    pub(crate) job_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketJobExecuteArguments {
    pub(crate) job_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketCriterionEvaluateArguments {
    pub(crate) job_id: Uuid,
    pub(crate) run_id: Uuid,
    pub(crate) criterion_id: Uuid,
    pub(crate) status: DocketCriterionStatus,
    #[serde(default)]
    pub(crate) evidence: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketTaskCreateArguments {
    #[serde(default)]
    pub(crate) job_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) parent_task_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) sibling_order: i32,
    #[serde(default)]
    pub(crate) placement: Option<DocketTaskPlacement>,
    #[serde(default = "default_task_kind")]
    pub(crate) kind: DocketTaskKind,
    #[serde(default = "default_task_scope")]
    pub(crate) scope: DocketTaskScope,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) completion_criteria: Vec<String>,
    #[serde(default)]
    pub(crate) difficulty: Option<DocketTaskDifficulty>,
    #[serde(default)]
    pub(crate) effort_hint: Option<DocketEffortHint>,
    #[serde(default)]
    pub(crate) created_in_run_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketTaskFindArguments {
    pub(crate) task_ref: String,
    #[serde(default)]
    pub(crate) job_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketTaskSelectArguments {
    #[serde(default)]
    pub(crate) task_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketTaskListArguments {
    #[serde(default)]
    pub(crate) job_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) parent_task_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) include_descendants: bool,
    #[serde(default)]
    pub(crate) limit: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketTaskUpdateArguments {
    pub(crate) task_id: Uuid,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) body: Option<String>,
    #[serde(default)]
    pub(crate) completion_criteria: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) parent_task_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) clear_parent_task_id: bool,
    #[serde(default)]
    pub(crate) sibling_order: Option<i32>,
    #[serde(default)]
    pub(crate) kind: Option<DocketTaskKind>,
    #[serde(default)]
    pub(crate) scope: Option<DocketTaskScope>,
    #[serde(default)]
    pub(crate) difficulty: Option<DocketTaskDifficulty>,
    #[serde(default)]
    pub(crate) effort_hint: Option<DocketEffortHint>,
    #[serde(default)]
    pub(crate) run_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) status: Option<DocketTaskStatus>,
    #[serde(default)]
    pub(crate) result_refs: Option<Value>,
    #[serde(default)]
    pub(crate) result_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketCurrentTaskStatusArguments {
    #[serde(default)]
    pub(crate) job_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) run_id: Option<Uuid>,
    pub(crate) task_id: Uuid,
    pub(crate) status: DocketTaskStatus,
    #[serde(default)]
    pub(crate) outcome_disposition: Option<den_docket::DocketOutcomeDisposition>,
    #[serde(default)]
    pub(crate) result_refs: Option<Value>,
    #[serde(default)]
    pub(crate) result_summary: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DocketEntryAppendKind {
    Finding,
    Decision,
    Obstacle,
    FollowUp,
    Milestone,
    Question,
}

impl From<DocketEntryAppendKind> for DocketEntryKind {
    fn from(kind: DocketEntryAppendKind) -> Self {
        match kind {
            DocketEntryAppendKind::Finding => Self::Finding,
            DocketEntryAppendKind::Decision => Self::Decision,
            DocketEntryAppendKind::Obstacle => Self::Obstacle,
            DocketEntryAppendKind::FollowUp => Self::FollowUp,
            DocketEntryAppendKind::Milestone => Self::Milestone,
            DocketEntryAppendKind::Question => Self::Question,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketEntryAppendArguments {
    #[serde(default)]
    pub(crate) job_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) task_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) run_id: Option<Uuid>,
    pub(crate) scope: DocketEntryScope,
    pub(crate) kind: DocketEntryAppendKind,
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) body: Option<String>,
    #[serde(default)]
    pub(crate) evidence_refs: Vec<Value>,
    #[serde(default)]
    pub(crate) related_task_ids: Vec<Uuid>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketEntryPromoteArguments {
    pub(crate) entry_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocketEntryListArguments {
    #[serde(default)]
    pub(crate) job_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) task_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) limit: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TaskListSyncArguments {
    pub(crate) task_list: TaskListProjection,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TaskListCheckoutArguments {
    #[serde(default)]
    pub(crate) job_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) parent_task_id: Option<Uuid>,
}

fn resolve_reference(
    reference: &str,
    kind: &str,
    candidates: impl IntoIterator<Item = Uuid>,
) -> Result<Uuid, CustomError> {
    let normalized = reference.replace('-', "").to_ascii_lowercase();
    if normalized.len() < 8
        || normalized.len() > 32
        || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CustomError::ValidationError(format!(
            "{kind} reference must be a UUID or at least 8 hexadecimal characters"
        )));
    }
    let matches: Vec<Uuid> = candidates
        .into_iter()
        .filter(|id| id.simple().to_string().starts_with(&normalized))
        .collect();
    match matches.as_slice() {
        [id] => Ok(*id),
        [] => Err(CustomError::NotFound(format!(
            "{kind} not found: {reference}"
        ))),
        _ => Err(CustomError::ValidationError(format!(
            "{kind} reference is ambiguous: {reference}"
        ))),
    }
}

fn default_job_visibility() -> TaskListVisibility {
    TaskListVisibility::SameUser
}

fn default_task_kind() -> DocketTaskKind {
    DocketTaskKind::Execution
}

fn default_task_scope() -> DocketTaskScope {
    DocketTaskScope::Template
}

#[derive(Debug, Deserialize)]
struct RuntimeDiagnosticsListArguments {
    work_run_id: Option<String>,
    runtime_run_id: Option<String>,
    session_id: Option<String>,
    docket_job_id: Option<String>,
    event_code: Option<String>,
    severity: Option<String>,
    limit: Option<i64>,
}

pub(crate) async fn list_runtime_diagnostics(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let arguments: RuntimeDiagnosticsListArguments = serde_json::from_value(arguments)
        .map_err(|error| CustomError::ValidationError(error.to_string()))?;
    let parse_optional_uuid = |name: &str, value: Option<String>| {
        value
            .map(|value| {
                Uuid::parse_str(&value).map_err(|error| {
                    CustomError::ValidationError(format!("invalid {name}: {error}"))
                })
            })
            .transpose()
    };
    let severity = match arguments.severity.as_deref() {
        None => None,
        Some("warning") => Some(RuntimeExceptionSeverity::Warning),
        Some("error") => Some(RuntimeExceptionSeverity::Error),
        Some(value) => {
            return Err(CustomError::ValidationError(format!(
                "invalid severity {value:?}; expected warning or error"
            )))
        }
    };
    let events = runtime_exception_events::list(
        pool,
        RuntimeExceptionEventFilter {
            bear_id: Some(context.bear_id),
            work_run_id: parse_optional_uuid("work_run_id", arguments.work_run_id)?,
            runtime_run_id: arguments.runtime_run_id,
            session_id: arguments.session_id,
            docket_job_id: parse_optional_uuid("docket_job_id", arguments.docket_job_id)?,
            event_code: arguments.event_code,
            severity,
            limit: arguments.limit,
        },
    )
    .await?;
    Ok(json!({ "events": events }))
}

pub(crate) async fn list_task_lists(
    pool: &PgPool,
    _config: &Config,
    stores: &MemoryStoreManager,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
    _activity_payload: fn(Option<&docket::TaskListLocalProjection>) -> Value,
    plan_mode_workplan_payload: fn(&plan_mode::PlanModeSessionRow) -> Value,
) -> Result<Value, CustomError> {
    let args: TaskListListArguments = serde_json::from_value(arguments)?;
    let include_plan_mode = args.include_plan_mode.unwrap_or(true);
    let include_artifacts = args.include_artifacts.unwrap_or(true);
    let plan_mode_gates = if include_plan_mode {
        plan_mode::list_for_bear(pool, context.bear_id, args.include_completed, 50).await?
    } else {
        Vec::new()
    };
    let plan_artifacts = if include_artifacts {
        match stores.store_for_bear(context.bear_id).await {
            Ok(store) => {
                sqlite_memory::sqlite_list_plan_artifacts(&store, BearProfile::Pair.as_str(), 50)
                    .await
                    .unwrap_or_else(|err| json!({ "error": err.to_string() }))
            }
            Err(err) => json!({ "error": err.to_string() }),
        }
    } else {
        json!([])
    };
    let linked_artifact_paths = plan_mode_gates
        .iter()
        .filter_map(|gate| gate.plan_artifact_path.as_deref())
        .collect::<Vec<_>>();
    let activity_plans: Vec<Value> = Vec::new();
    let task_lists: Vec<docket::TaskListProjection> = Vec::new();
    let workplans = plan_mode_gates
        .iter()
        .map(plan_mode_workplan_payload)
        .collect::<Vec<_>>();
    Ok(json!({
        "domain": "activity",
        "bear_id": context.bear_id,
        "viewer_role": role.as_str(),
        "planning_scope": "bear",
        "workplace": {
            "status": "unresolved",
            "note": "Workplace inference is not implemented yet; workspace/session metadata is returned as workplace reference candidates.",
            "reference_candidates": {
                "client_session_id": context.client_session_id,
                "session_id": context.session_id,
                "conversation_id": clean_optional(&context.conversation_id),
                "conversation_selection": context.conversation_selection,
                "runtime_target": context.runtime_target,
                "channel": context.channel,
            }
        },
        "task_lists": task_lists,
        "activities": activity_plans,
        "activity_plans": activity_plans,
        "workplans": workplans,
        "plan_mode_gates": plan_mode_gates,
        "plan_artifacts": plan_artifacts,
        "linked_plan_artifact_paths": linked_artifact_paths,
        "notes": [
            "list_task_lists is a Bear-level task-list/planning view. It includes checked-out Docket task-list projections, submitted/active plan-mode gates, and saved pair plan artifacts when available.",
            "A plan artifact in pair/plans/ may exist even when there is no active task list; this is planning state, not semantic memory.",
            "Role fields are provenance and policy hints, not product ownership. Cross-role visibility is not cross-role execution authority."
        ],
    }))
}

pub(crate) async fn get_task_list_status(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
    activity_payload: fn(Option<&docket::TaskListLocalProjection>) -> Value,
) -> Result<Value, CustomError> {
    let _ignored_arguments: Value = serde_json::from_value(arguments)?;
    let session = match context.client_session_id.as_deref() {
        Some(client_session_id) => {
            client_sessions::find_for_user_bear_session_id(
                pool,
                context.user_id,
                context.bear_id,
                client_session_id,
            )
            .await?
        }
        None => None,
    };
    let session_anchor_id = session.as_ref().map(|session| session.id);

    // Use the same canonical eligibility query as current-task selection. The
    // generic task list defaults to root tasks and hides an attached child task.
    let tasks = if let Some(session_anchor_id) = session_anchor_id {
        PgDocketService::from_pool(pool)
            .list_session_tasks(context.bear_id, session_anchor_id)
            .await?
    } else {
        Vec::new()
    };

    let task_list = session.as_ref().and_then(|session| {
        docket::task_list_projection_from_session_tasks_with_current_task(
            context.bear_id,
            role,
            clean_optional(&context.conversation_id)
                .as_deref()
                .unwrap_or(""),
            session.id,
            &tasks,
            session.current_task_id,
        )
    });

    let summary = task_list
        .as_ref()
        .map(task_list_summary)
        .unwrap_or_else(|| {
            if session_anchor_id.is_some() {
                "Current session has no anchored tasks.".to_string()
            } else {
                "No current client session anchor is available for session tasks.".to_string()
            }
        });
    let item_counts = task_list
        .as_ref()
        .map(task_list_item_counts)
        .unwrap_or_else(|| {
            json!({
                "total": 0,
                "pending": 0,
                "in_progress": 0,
                "blocked": 0,
                "completed": 0,
                "cancelled": 0,
            })
        });

    Ok(json!({
        "domain": "activity",
        "bear_id": context.bear_id,
        "viewer_role": role.as_str(),
        "content": task_list.as_ref().map(task_list_card_content).unwrap_or_else(|| summary.clone()),
        "summary": summary,
        "found": task_list.is_some(),
        "count": tasks.len(),
        "phase": task_list.as_ref().map(|task_list| task_list.status.as_str()),
        "status": task_list.as_ref().map(|task_list| task_list.status.as_str()),
        "execution_allowed": task_list
            .as_ref()
            .is_some_and(|task_list| task_list.status != "planned"),
        "item_counts": item_counts,
        "task_list": task_list,
        "activity": activity_payload(None),
        "plan": task_list,
        "notes": [
            "Session-anchored tasks are durable Docket tasks associated with the current client session.",
            "A planned session task list is visible for review but is not an instruction to start execution."
        ]
    }))
}

pub(crate) async fn update_task_list(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
    _activity_payload: fn(Option<&docket::TaskListLocalProjection>) -> Value,
) -> Result<Value, CustomError> {
    let policy = effective_tool_policy(role, context);
    policy
        .capabilities
        .require(den_core::BearCapability::SelectSessionTask)?;
    let args: DocketTaskSelectArguments = serde_json::from_value(arguments)?;
    let task_id = args.task_id.ok_or_else(|| {
        DenError::ValidationError(
            "update_task_list needs task_id to activate a session task list".to_string(),
        )
    })?;
    let client_session_id = context.client_session_id.as_deref().ok_or_else(|| {
        DenError::ValidationError("update_task_list needs the current client session".to_string())
    })?;
    let selection = select_session_current_task(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
        Some(task_id),
        &policy.capabilities,
    )
    .await?;
    let task_list = selection.task_list.ok_or_else(|| {
        DenError::ValidationError("session task list disappeared during activation".to_string())
    })?;
    let phase = task_list.status.clone();
    Ok(json!({
        "domain": "activity",
        "content": "Activated the session task list.",
        "summary": "Activated the session task list and selected its current task.",
        "task_id": task_id,
        "phase": phase,
        "status": task_list.status,
        "execution_allowed": true,
        "task_list": task_list,
        "plan": task_list,
    }))
}

// Resolve a `work_surface_ref` against managed work surfaces. When the ref
// names one, the bear must be assigned to it, and the canonical name + id
// come back; names matching no managed surface pass through unchanged (the
// sandbox provider is the final validator for those).

fn truncate_focused_conversation_title(title: &str) -> String {
    title
        .chars()
        .take(FOCUSED_CONVERSATION_TITLE_MAX_CHARS)
        .collect()
}

fn focused_conversation_title(goal: &str, status_report: &DocketJobStatusReport) -> String {
    let suffix = status_report
        .current_task_title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .or_else(|| {
            if status_report.task_counts.blocked > 0 || status_report.next_action == "blocked" {
                Some("blocked".to_string())
            } else if status_report.next_action == "done"
                || status_report.job_status == "completed"
                || (status_report.tasks_complete && status_report.criteria_complete)
            {
                Some("complete".to_string())
            } else if status_report.run_id.is_some() || status_report.job_status == "running" {
                Some("selecting next task".to_string())
            } else {
                None
            }
        });

    let goal = goal.trim();
    let title = match suffix {
        Some(suffix) if !goal.is_empty() => format!("{goal} - {suffix}"),
        Some(suffix) => suffix,
        None => goal.to_string(),
    };
    truncate_focused_conversation_title(&title)
}

fn oriented_task_create_policy(runtime: Option<&Value>) -> Option<OrientedTaskCreatePolicy> {
    let orientation = runtime?.get("objective_orientation")?;
    if orientation.get("kind").and_then(Value::as_str) != Some("oriented") {
        return None;
    }

    let task = orientation.get("task")?;
    let task_ref = task.get("task_ref")?;
    let root_task_id = task_ref
        .get("task_id")
        .or_else(|| task_ref.get("item_id"))
        .and_then(Value::as_str)
        .and_then(|task_id| Uuid::parse_str(task_id).ok())?;
    let child_policy = task.get("child_policy")?;
    Some(OrientedTaskCreatePolicy {
        root_task_id,
        max_children: child_policy
            .get("max_children")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        max_depth_below_oriented_task: child_policy
            .get("max_depth_below_oriented_task")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
    })
}

fn oriented_new_child_depth(
    root_task_id: Uuid,
    parent_task_id: Uuid,
    descendants: &[den_docket::DocketTaskProjection],
) -> Option<usize> {
    if parent_task_id == root_task_id {
        return Some(1);
    }

    let mut depth = 1;
    let mut current_parent_id = parent_task_id;
    // ponytail: the oriented decomposition tree is deliberately tiny; if the
    // cap grows large, replace this linear ancestor walk with an id->parent map.
    loop {
        let parent = descendants
            .iter()
            .find(|task| task.task.id == current_parent_id)?;
        depth += 1;
        match parent.task.parent_task_id {
            Some(grandparent_id) if grandparent_id == root_task_id => return Some(depth),
            Some(grandparent_id) => current_parent_id = grandparent_id,
            None => return None,
        }
    }
}

async fn enforce_oriented_task_create_policy(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    args: &DocketTaskCreateArguments,
) -> Result<(), CustomError> {
    let Some(parent_task_id) = args.parent_task_id else {
        return Ok(());
    };
    let Some(policy) = oriented_task_create_policy(context.runtime.as_ref()) else {
        return Ok(());
    };

    let docket = PgDocketService::from_pool(pool);
    let descendants = docket
        .list_tasks(
            context.bear_id,
            DocketTaskListFilter {
                job_id: None,
                pair_session_id: None,
                parent_task_id: Some(policy.root_task_id),
                include_descendants: true,
                limit: 500,
            },
        )
        .await?;
    let Some(new_child_depth) =
        oriented_new_child_depth(policy.root_task_id, parent_task_id, &descendants)
    else {
        return Err(DenError::ValidationError(
            "oriented task decomposition can only create child tasks under the oriented task"
                .to_string(),
        )
        .into());
    };
    let direct_children = docket
        .list_tasks(
            context.bear_id,
            DocketTaskListFilter {
                job_id: None,
                pair_session_id: None,
                parent_task_id: Some(parent_task_id),
                include_descendants: false,
                limit: (policy.max_children as i64).saturating_add(1).max(1),
            },
        )
        .await?;
    validate_oriented_child_capacity(policy, new_child_depth, direct_children.len())?;

    Ok(())
}

fn validate_oriented_child_capacity(
    policy: OrientedTaskCreatePolicy,
    new_child_depth: usize,
    direct_child_count: usize,
) -> Result<(), DenError> {
    if new_child_depth > policy.max_depth_below_oriented_task {
        return Err(DenError::ValidationError(format!(
            "oriented task decomposition allows at most {} level(s) below the oriented task",
            policy.max_depth_below_oriented_task
        )));
    }
    if direct_child_count >= policy.max_children {
        return Err(DenError::ValidationError(format!(
            "oriented task decomposition allows at most {} child tasks under the selected parent",
            policy.max_children
        )));
    }

    Ok(())
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn docket_job_summary(job: &DocketJobProjection) -> String {
    let task_label = count_label(job.tasks.len(), "task", "tasks");
    let criterion_label = count_label(job.criteria.len(), "criterion", "criteria");
    format!(
        "Docket job '{}' is {} with {task_label} and {criterion_label}.",
        job.job.goal, job.job.status
    )
}

fn docket_job_card_content(
    job: &DocketJobProjection,
    status_report: &DocketJobStatusReport,
) -> String {
    let tasks = &status_report.task_counts;
    let criteria = &status_report.criteria_counts;
    let mut lines = vec![
        format!("Job: {}", job.job.goal),
        format!("Status: {}", status_report.job_status),
        format!(
            "Tasks: {} pending, {} in progress, {} done, {} blocked, {} cancelled",
            tasks.pending, tasks.in_progress, tasks.done, tasks.blocked, tasks.cancelled
        ),
        format!(
            "Criteria: {} unmet, {} met, {} waived",
            criteria.unmet, criteria.met, criteria.waived
        ),
        format!("Next action: {}", status_report.next_action),
    ];
    if let Some(title) = status_report.current_task_title.as_deref() {
        lines.push(format!("Current task: {title}"));
    }
    lines.join("\n")
}

fn human_task_status_label(status: &str) -> &'static str {
    match status {
        "pending" => "pending",
        "in_progress" => "in progress",
        "done" => "done",
        "blocked" => "blocked",
        "cancelled" => "cancelled",
        _ => "updated",
    }
}

fn task_status_card_content(
    task: &docket::DocketTaskProjection,
    status: &str,
    status_report: Option<&DocketJobStatusReport>,
) -> String {
    let status_label = human_task_status_label(status);
    let mut lines = vec![
        format!("Task marked {status_label}: {}", task.task.title),
        format!("Status: {status_label}"),
    ];
    if let Some(result_summary) = task
        .run_state
        .as_ref()
        .and_then(|state| state.result_summary.as_deref())
        .filter(|summary| !summary.trim().is_empty())
    {
        lines.push(format!("Result: {result_summary}"));
    }
    if task
        .run_state
        .as_ref()
        .and_then(|state| state.result_refs.as_ref())
        .is_some()
    {
        lines.push("Result references recorded.".to_string());
    }
    if let Some(report) = status_report {
        let counts = &report.task_counts;
        lines.push(format!("Job next action: {}", report.next_action));
        lines.push(format!(
            "Job tasks: {} pending, {} in progress, {} done, {} blocked, {} cancelled",
            counts.pending, counts.in_progress, counts.done, counts.blocked, counts.cancelled
        ));
    }
    lines.join("\n")
}

fn docket_job_rows_summary(jobs: &[docket::DocketJobRow]) -> String {
    if jobs.is_empty() {
        "No Docket jobs matched the filters.".to_string()
    } else {
        format!(
            "Found {}.",
            count_label(jobs.len(), "Docket job", "Docket jobs")
        )
    }
}

fn task_projection_status(task: &docket::DocketTaskProjection) -> &str {
    task.status.as_str()
}

fn docket_task_row_summary(task: &docket::DocketTaskRow) -> String {
    format!("Task '{}' was created.", task.title)
}

fn docket_task_create_summary(
    task: &docket::DocketTaskRow,
    job_id: Option<Uuid>,
    defaulted_to_pair_task_tree: bool,
    planned_session_task: bool,
) -> String {
    if let Some(job_id) = job_id {
        return format!("Task '{}' was created in Docket Job {job_id}.", task.title);
    }

    if defaulted_to_pair_task_tree {
        return format!(
            "Task '{}' was created in the current session task tree.",
            task.title
        );
    }

    if planned_session_task {
        return format!(
            "Planned Docket task '{}' in the current session task list. Execution has not started.",
            task.title
        );
    }

    docket_task_row_summary(task)
}

fn docket_tasks_summary_for_scope(
    tasks: &[docket::DocketTaskProjection],
    job_id: Option<Uuid>,
    defaulted_to_pair_task_tree: bool,
) -> String {
    let count = count_label(tasks.len(), "Docket task", "Docket tasks");
    if let Some(job_id) = job_id {
        return format!("Found {count} in Docket Job {job_id}.");
    }
    if defaulted_to_pair_task_tree {
        return format!("Found {count} in the current session task tree.");
    }
    docket_tasks_summary(tasks)
}

fn docket_task_summary(task: &docket::DocketTaskProjection) -> String {
    format!(
        "Task '{}' is {}.",
        task.task.title,
        task_projection_status(task)
    )
}

fn docket_tasks_summary(tasks: &[docket::DocketTaskProjection]) -> String {
    if tasks.is_empty() {
        "No Docket tasks matched the filters.".to_string()
    } else {
        format!(
            "Found {}.",
            count_label(tasks.len(), "Docket task", "Docket tasks")
        )
    }
}

fn docket_tasks_card_content(tasks: &[docket::DocketTaskProjection]) -> String {
    if tasks.is_empty() {
        return "No Docket tasks matched the filters.".to_string();
    }
    let counts = docket_task_counts(tasks);
    let mut lines = vec![format!(
        "Found {}.",
        count_label(tasks.len(), "Docket task", "Docket tasks")
    )];
    lines.push(format!(
        "Status: {} pending, {} in progress, {} done, {} blocked, {} cancelled.",
        counts["pending"],
        counts["in_progress"],
        counts["done"],
        counts["blocked"],
        counts["cancelled"]
    ));
    for task in tasks.iter().take(5) {
        lines.push(format!(
            "- {} — {}",
            task.task.title,
            human_task_status_label(task_projection_status(task))
        ));
    }
    if tasks.len() > 5 {
        lines.push(format!("…and {} more.", tasks.len() - 5));
    }
    lines.join("\n")
}

fn docket_task_counts(tasks: &[docket::DocketTaskProjection]) -> Value {
    let mut pending = 0usize;
    let mut in_progress = 0usize;
    let mut done = 0usize;
    let mut blocked = 0usize;
    let mut cancelled = 0usize;
    for task in tasks {
        match task_projection_status(task) {
            "in_progress" => in_progress += 1,
            "done" => done += 1,
            "blocked" => blocked += 1,
            "cancelled" => cancelled += 1,
            _ => pending += 1,
        }
    }
    json!({
        "total": tasks.len(),
        "pending": pending,
        "in_progress": in_progress,
        "done": done,
        "blocked": blocked,
        "cancelled": cancelled,
    })
}

fn task_list_summary(task_list: &TaskListProjection) -> String {
    format!(
        "Task list '{}' is {} with {}.",
        task_list.title,
        task_list.status,
        count_label(task_list.items.len(), "item", "items")
    )
}

fn task_list_card_content(task_list: &TaskListProjection) -> String {
    let counts = task_list_item_counts(task_list);
    let mut lines = vec![task_list_summary(task_list)];
    lines.push(format!(
        "Items: {} pending, {} in progress, {} completed, {} blocked, {} cancelled.",
        counts["pending"],
        counts["in_progress"],
        counts["completed"],
        counts["blocked"],
        counts["cancelled"]
    ));
    if task_list.status == "planned" {
        lines.push("Execution has not started.".to_string());
    }
    if let Some(item) = &task_list.current_item {
        lines.push(format!("Current item: {} — {}.", item.title, item.status));
    }
    lines.join("\n")
}

fn task_list_item_counts(task_list: &TaskListProjection) -> Value {
    let mut pending = 0usize;
    let mut in_progress = 0usize;
    let mut blocked = 0usize;
    let mut completed = 0usize;
    let mut cancelled = 0usize;
    for item in &task_list.items {
        match item.status {
            docket::TaskListItemStatus::InProgress => in_progress += 1,
            docket::TaskListItemStatus::Blocked => blocked += 1,
            docket::TaskListItemStatus::Completed => completed += 1,
            docket::TaskListItemStatus::Cancelled => cancelled += 1,
            docket::TaskListItemStatus::Pending => pending += 1,
        }
    }
    json!({
        "total": task_list.items.len(),
        "pending": pending,
        "in_progress": in_progress,
        "blocked": blocked,
        "completed": completed,
        "cancelled": cancelled,
    })
}

pub(crate) async fn session_anchored_task_list_projection(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    pair_session_id: Uuid,
) -> Result<Option<TaskListProjection>, CustomError> {
    // Keep cached activity plans consistent with current-task selection.
    let tasks = PgDocketService::from_pool(pool)
        .list_session_tasks(context.bear_id, pair_session_id)
        .await?;
    let selected_task_id = if let Some(client_session_id) = context.client_session_id.as_deref() {
        client_sessions::find_for_user_bear_session_id(
            pool,
            context.user_id,
            context.bear_id,
            client_session_id,
        )
        .await?
        .and_then(|session| session.current_task_id)
    } else {
        None
    };
    Ok(
        docket::task_list_projection_from_session_tasks_with_current_task(
            context.bear_id,
            role,
            clean_optional(&context.conversation_id)
                .as_deref()
                .unwrap_or(""),
            pair_session_id,
            &tasks,
            selected_task_id,
        ),
    )
}

fn refresh_runtime_session_activity_plan(
    context: &DenToolInvocationContext,
    task_list: Option<TaskListProjection>,
) {
    let (Some(conversation_id), Some(client_session_id)) = (
        clean_optional(&context.conversation_id),
        context.client_session_id.as_deref(),
    ) else {
        return;
    };
    den_runtime::native_runtime::update_native_client_session_cached_activity_plan_projection(
        &conversation_id,
        client_session_id,
        task_list,
    );
}

async fn update_focused_conversation_title(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    job: &DocketJobProjection,
    status_report: &DocketJobStatusReport,
) -> Result<(), CustomError> {
    let Some(conversation_id) = clean_optional(&context.conversation_id) else {
        return Ok(());
    };
    let title = focused_conversation_title(&job.job.goal, status_report);
    conversation_persistence::set_conversation_title_and_sync_client_sessions(
        pool,
        context.bear_id,
        &conversation_id,
        &title,
    )
    .await?;
    Ok(())
}

async fn resolve_surface_id_for_create(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    context: &DenToolInvocationContext,
    role: BearProfile,
    explicit_id: Option<Uuid>,
) -> Result<(Option<Uuid>, bool), CustomError> {
    if let Some(surface_id) = explicit_id {
        validate_assigned_surface(pool, context, surface_id).await?;
        return Ok((Some(surface_id), false));
    }
    persist_recognized_session_surface(pool, stores, context, role).await?;
    let Some(client_session_id) = context.client_session_id.as_deref() else {
        return Ok((None, false));
    };
    let Some(session) = client_sessions::find_for_user_bear_session_id(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
    )
    .await?
    else {
        return Ok((None, false));
    };
    let Some(resolution) =
        den_service::client_session_work_surface_resolutions::find(pool, session.id).await?
    else {
        return Ok((None, false));
    };
    validate_assigned_surface(pool, context, resolution.work_surface_id).await?;
    Ok((Some(resolution.work_surface_id), true))
}

async fn validate_assigned_surface(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    surface_id: Uuid,
) -> Result<(), CustomError> {
    if !den_service::work_surfaces::bear_may_use_surface(pool, context.bear_id, surface_id).await? {
        return Err(DenError::ValidationError(
            "work_surface_id must identify a managed surface assigned to this Bear".to_string(),
        )
        .into());
    }
    Ok(())
}

pub(crate) async fn confirm_work_surface(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    effective_tool_policy(role, context)
        .capabilities
        .require(den_core::BearCapability::UseWorkSurfaces)?;
    let args: ConfirmWorkSurfaceArguments = serde_json::from_value(arguments)?;
    let client_session_id = context.client_session_id.as_deref().ok_or_else(|| {
        DenError::ValidationError(
            "confirm_work_surface requires an active client session".to_string(),
        )
    })?;
    let session = client_sessions::find_for_user_bear_session_id(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
    )
    .await?
    .ok_or_else(|| {
        DenError::ValidationError(
            "confirm_work_surface requires a known client session".to_string(),
        )
    })?;
    if !den_service::work_surfaces::bear_may_use_surface(
        pool,
        context.bear_id,
        args.work_surface_id,
    )
    .await?
    {
        return Err(DenError::ValidationError(
            "work_surface_id must identify a managed surface assigned to this Bear".to_string(),
        )
        .into());
    }
    den_service::client_session_work_surface_resolutions::upsert(
        pool,
        session.id,
        args.work_surface_id,
        den_service::client_session_work_surface_resolutions::ClientSessionWorkSurfaceResolutionStatus::Confirmed,
        json!({ "kind": "user_confirmation" }),
    )
    .await?;
    Ok(json!({
        "ok": true,
        "status": "confirmed",
        "work_surface_id": args.work_surface_id,
        "notes": ["Recorded the user's explicit work-surface selection for this Pair session."]
    }))
}

pub(crate) async fn create_job(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    effective_tool_policy(role, context)
        .capabilities
        .require(den_core::BearCapability::CreateJob)
        .map_err(|_| {
            DenError::from(DocketValidationError::InvalidJobCreatorRole {
                role: role.as_str().to_string(),
            })
        })?;
    let args: DocketJobCreateArguments = serde_json::from_value(arguments)?;
    let (work_surface_id, surface_auto_bound) = if args.work_surface_assignments.is_empty() {
        resolve_surface_id_for_create(pool, stores, context, role, args.work_surface_id).await?
    } else {
        if args.work_surface_id.is_some() {
            return Err(DenError::ValidationError(
                "provide either work_surface_id or work_surface_assignments, not both".to_string(),
            )
            .into());
        }
        for assignment in &args.work_surface_assignments {
            validate_assigned_surface(pool, context, assignment.work_surface_id).await?;
        }
        (None, false)
    };
    if work_surface_id.is_none() && args.work_surface_assignments.is_empty() {
        return Err(DenError::ValidationError(
            "work_surface_required: Docket work jobs require an assigned managed work surface; choose one from get_work_catalog or use a resolved/confirmed session anchor"
                .to_string(),
        )
        .into());
    }
    let job = PgDocketService::from_pool(pool)
        .create_job(DocketJobCreate {
            bear_id: context.bear_id,
            created_by_user_id: context.user_id,
            created_by_role: role.as_str().to_string(),
            goal: args.goal,
            work_surface_id,
            work_surface_assignments: args
                .work_surface_assignments
                .into_iter()
                .map(|assignment| DocketJobSurfaceAssignmentInput {
                    work_surface_id: assignment.work_surface_id,
                    mutation_policy: assignment.mutation_policy,
                })
                .collect(),
            commit_policy: args.commit_policy,
            work_branch: args.work_branch,
            visibility: args.visibility,
            source_conversation_id: clean_optional(&context.conversation_id),
            objective_kind: None,
            supersedes_job_id: args.supersedes_job_id,
            overlap_resolution: match args.overlap_resolution {
                JobOverlapResolution::Reject => DocketJobOverlapResolution::Reject,
                JobOverlapResolution::Independent => DocketJobOverlapResolution::Independent,
                JobOverlapResolution::Supersede => DocketJobOverlapResolution::Supersede,
            },
            criteria: args.criteria,
            tasks: args.tasks,
        })
        .await?;
    let task_list = docket::task_list_projection_from_docket_job(&job, None);
    let web_base = docket_web_base(pool, config, context.bear_id).await?;
    Ok(json!({
        "action": "job_created",
        "execution": {
            "requested": false,
            "state": "not_started",
            "run_id": null,
        },
        "domain": "docket",
        "bear_id": context.bear_id,
        "summary": docket_job_summary(&job),
        "presentation": docket_entity_presentation(
            "job",
            job.job.id,
            &[job.job.id],
            Some(docket_resource_href(&web_base, "jobs", job.job.id)),
        ),
        "job": job,
        "task_list": task_list,
        "notes": [
            "Created durable Docket job state; execution is not started by this tool.",
            "Use get_job for canonical job/task state; checkout_task_list exposes a Docket job as a session task-list projection.",
            if surface_auto_bound {
                "Auto-bound the unique assigned work surface resolved from trusted session metadata."
            } else {
                "No work surface was auto-bound; explicit selection remains available and unresolved or ambiguous session hints remain unbound."
            }
        ]
    }))
}

pub(crate) async fn list_jobs(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketJobListArguments = serde_json::from_value(arguments)?;
    let jobs = PgDocketService::from_pool(pool)
        .list_jobs(
            context.bear_id,
            DocketJobListFilter {
                statuses: args.statuses,
                include_cancelled: args.include_cancelled,
                limit: args.limit,
                ..DocketJobListFilter::default()
            },
        )
        .await?;
    let web_base = docket_web_base(pool, config, context.bear_id).await?;
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "summary": docket_job_rows_summary(&jobs),
        "count": jobs.len(),
        "presentations": docket_entity_presentations("job", jobs.iter().map(|job| job.id), |id| {
            Some(docket_resource_href(&web_base, "jobs", id))
        }),
        "jobs": jobs,
    }))
}

pub(crate) async fn find_job(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketJobFindArguments = serde_json::from_value(arguments)?;
    let job_id = resolve_reference(
        &args.job_ref,
        "job",
        PgDocketService::from_pool(pool)
            .list_jobs(
                context.bear_id,
                DocketJobListFilter {
                    include_cancelled: true,
                    limit: 200,
                    ..DocketJobListFilter::default()
                },
            )
            .await?
            .into_iter()
            .map(|job| job.id),
    )?;
    get_job(pool, context, json!({ "job_id": job_id })).await
}

pub(crate) async fn get_job(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketJobGetArguments = serde_json::from_value(arguments)?;
    let job = PgDocketService::from_pool(pool)
        .get_job(context.bear_id, args.job_id)
        .await?;
    let task_list = job
        .as_ref()
        .map(|job| docket::task_list_projection_from_docket_job(job, None));
    let status_report = job.as_ref().map(docket_job_status_report);

    // Work-run visibility: recent runs (with queue placement) and
    // latest-attempt failures that need triage, so a bear checking on its
    // job sees blocked reasons without a second tool call.
    let (work_runs, work_attention) = if job.is_some() {
        let runs = den_docket::work_runs::list_work_runs(
            pool,
            den_docket::work_runs::WorkRunListFilter {
                bear_id: Some(context.bear_id),
                job_id: Some(args.job_id),
                limit: 20,
                ..den_docket::work_runs::WorkRunListFilter::default()
            },
        )
        .await?;
        let queue_by_run = work_run_queue_map(pool, &runs).await?;
        let items: Vec<Value> = runs
            .iter()
            .map(|run| {
                let mut item = work_run_summary_json(run, None);
                if let Some(queue) = queue_by_run.get(&run.id) {
                    item["queue"] = queue.clone();
                }
                item
            })
            .collect();
        let attention = den_docket::work_runs::attention_work_runs(
            pool,
            context.bear_id,
            Some(args.job_id),
            10,
        )
        .await?;
        (Some(items), Some(attention))
    } else {
        (None, None)
    };

    let summary = job
        .as_ref()
        .map(docket_job_summary)
        .unwrap_or_else(|| format!("No Docket job found for {}.", args.job_id));
    let content = job
        .as_ref()
        .zip(status_report.as_ref())
        .map(|(job, report)| docket_job_card_content(job, report))
        .unwrap_or_else(|| summary.clone());
    let task_counts = status_report.as_ref().map(|report| &report.task_counts);
    let criteria_counts = status_report.as_ref().map(|report| &report.criteria_counts);
    let next_action = status_report
        .as_ref()
        .map(|report| report.next_action.as_str());
    let current_task_title = status_report
        .as_ref()
        .and_then(|report| report.current_task_title.as_deref());

    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "content": content,
        "summary": summary,
        "found": job.is_some(),
        "job_status": status_report.as_ref().map(|report| report.job_status.as_str()),
        "task_counts": task_counts,
        "criteria_counts": criteria_counts,
        "next_action": next_action,
        "current_task_title": current_task_title,
        "job": job,
        "task_list": task_list,
        "item_counts": task_list.as_ref().map(task_list_item_counts),
        "status_report": status_report,
        "work_runs": work_runs,
        "work_attention": work_attention,
    }))
}

pub(crate) async fn update_job(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketJobUpdateArguments = serde_json::from_value(arguments)?;
    let work_surface_id = if let Some(surface_id) = args.work_surface_id {
        validate_assigned_surface(pool, context, surface_id).await?;
        Some(Some(surface_id))
    } else {
        None
    };
    let job = PgDocketService::from_pool(pool)
        .update_job(DocketJobUpdate {
            bear_id: context.bear_id,
            job_id: args.job_id,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
            goal: args.goal,
            work_surface_id,
            commit_policy: args
                .clear_commit_policy
                .then_some(None)
                .or_else(|| args.commit_policy.map(Some)),
            work_branch: None,
            status: None,
            visibility: args.visibility,
        })
        .await?;
    let status_report = docket_job_status_report(&job);
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "job": job,
        "status_report": status_report,
    }))
}

pub(crate) async fn set_job_lifecycle(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
    status: DocketJobStatus,
) -> Result<Value, CustomError> {
    let args: DocketJobLifecycleArguments = serde_json::from_value(arguments)?;
    let job = PgDocketService::from_pool(pool)
        .update_job(DocketJobUpdate {
            bear_id: context.bear_id,
            job_id: args.job_id,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
            goal: None,
            work_surface_id: None,
            commit_policy: None,
            work_branch: None,
            status: Some(status),
            visibility: None,
        })
        .await?;
    let status_report = docket_job_status_report(&job);
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "job": job,
        "status_report": status_report,
    }))
}

pub(crate) async fn cancel_job_run(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    effective_tool_policy(role, context)
        .capabilities
        .require(den_core::BearCapability::DispatchWork)?;
    let args: DocketJobLifecycleArguments = serde_json::from_value(arguments)?;
    let job = PgDocketService::from_pool(pool)
        .cancel_job_run(context.bear_id, args.job_id)
        .await?;
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "job": job,
        "note": "cancelled the active Docket run; the job remains available for recovery or a later run",
    }))
}

pub(crate) async fn execute_job(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketJobExecuteArguments = serde_json::from_value(arguments)?;
    let policy = effective_tool_policy(role, context);
    policy
        .capabilities
        .require(den_core::BearCapability::ExecuteJob)?;
    if let Some(mode_label) = context
        .session_policy
        .as_ref()
        .and_then(|policy| policy.get("mode_label"))
        .and_then(Value::as_str)
    {
        if !mode_label.eq_ignore_ascii_case("write") {
            return Err(DenError::Authorization(format!(
                "Docket execution is active, but client mode is {mode_label}; switch the session to Write mode before proceeding"
            ))
            .into());
        }
    }
    let outcome = PgDocketService::from_pool(pool)
        .execute_job(DocketJobExecuteRequest {
            bear_id: context.bear_id,
            job_id: args.job_id,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
            session_id: Some(context.session_id.clone()),
            source_conversation_id: clean_optional(&context.conversation_id),
            source_client_session_id: context.client_session_id.clone(),
        })
        .await?;
    let pair_binding =
        bind_selected_task_to_current_session(pool, context, &outcome, &policy.capabilities)
            .await?;
    let status_report = docket_job_status_report(&outcome.job);
    update_focused_conversation_title(pool, context, &outcome.job, &status_report).await?;
    let run = outcome.job.current_run.as_ref();
    let execution_state = run
        .map(|run| run.state.clone())
        .unwrap_or_else(|| "not_started".to_string());
    Ok(json!({
        "action": "execution_requested",
        "execution": {
            "requested": true,
            "state": execution_state,
            "run_id": run.map(|run| run.id),
        },
        "outcome": outcome,
        "pair_binding": pair_binding,
        "domain": "docket",
        "bear_id": context.bear_id,
        "status_report": status_report,
    }))
}

/// Repairs an explicitly reported stale Docket execution focus. This is kept
/// separate from `execute_job` so recovery cannot be mistaken for a retry.
pub(crate) async fn reconcile_job_execution(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketJobExecuteArguments = serde_json::from_value(arguments)?;
    let policy = effective_tool_policy(role, context);
    policy
        .capabilities
        .require(den_core::BearCapability::ExecuteJob)?;
    let outcome = PgDocketService::from_pool(pool)
        .reconcile_execution(DocketJobExecuteRequest {
            bear_id: context.bear_id,
            job_id: args.job_id,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
            session_id: Some(context.session_id.clone()),
            source_conversation_id: clean_optional(&context.conversation_id),
            source_client_session_id: context.client_session_id.clone(),
        })
        .await?;
    let pair_binding =
        bind_selected_task_to_current_session(pool, context, &outcome, &policy.capabilities)
            .await?;
    let status_report = docket_job_status_report(&outcome.job);
    update_focused_conversation_title(pool, context, &outcome.job, &status_report).await?;
    let run = outcome.job.current_run.as_ref();
    Ok(json!({
        "action": "execution_reconciled",
        "execution": {
            "requested": true,
            "state": run.map(|run| run.state.clone()).unwrap_or_else(|| "not_started".to_string()),
            "run_id": run.map(|run| run.id),
        },
        "outcome": outcome,
        "pair_binding": pair_binding,
        "domain": "docket",
        "bear_id": context.bear_id,
        "status_report": status_report,
    }))
}

/// Keep hosted Docket execution aligned with the BearWire endpoint: a task
/// selected for focused execution must be attached to, and current in, this
/// client session before the result is reported to the model.
async fn bind_selected_task_to_current_session(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    outcome: &docket::DocketJobExecuteOutcome,
    capabilities: &den_core::CapabilitySet,
) -> Result<Value, CustomError> {
    if !matches!(
        outcome.control.next_action,
        DocketExecutionNextAction::WorkCurrentTask
    ) {
        return Ok(json!({
            "status": "not_applicable",
            "reason": "Docket did not select a session task.",
        }));
    }
    let Some(task_id) = outcome.control.task.selected_task_id else {
        return Ok(json!({
            "status": "not_applicable",
            "reason": "Docket did not select a session task.",
        }));
    };
    let Some(client_session_id) = context.client_session_id.as_deref() else {
        return Ok(json!({
            "status": "not_attempted",
            "reason": "no_authenticated_pair_session",
            "task_id": task_id,
            "current_task_selected": false,
        }));
    };
    let session = client_sessions::find_for_user_bear_session_id(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;
    PgDocketService::from_pool(pool)
        .attach_task_to_session(context.bear_id, task_id, session.id)
        .await?;
    select_session_current_task(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
        Some(task_id),
        capabilities,
    )
    .await?;
    Ok(json!({
        "status": "attached",
        "task_id": task_id,
        "client_session_id": client_session_id,
        "current_task_selected": true,
    }))
}

pub(crate) async fn evaluate_criterion(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketCriterionEvaluateArguments = serde_json::from_value(arguments)?;
    let job = PgDocketService::from_pool(pool)
        .evaluate_criterion(DocketCriterionStateUpdate {
            bear_id: context.bear_id,
            job_id: args.job_id,
            run_id: args.run_id,
            criterion_id: args.criterion_id,
            status: args.status,
            evidence: args.evidence,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
        })
        .await?;
    let status_report = docket_job_status_report(&job);
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "job": job,
        "status_report": status_report,
    }))
}

fn should_default_session_task_tree(
    capabilities: &den_core::CapabilitySet,
    job_id: Option<Uuid>,
) -> bool {
    capabilities.contains(den_core::BearCapability::OwnSessionTasks) && job_id.is_none()
}

pub(crate) async fn resolve_task_session_anchor_id(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    job_id: Option<Uuid>,
) -> Result<Option<Uuid>, CustomError> {
    if job_id.is_some() {
        return Ok(None);
    }

    let Some(client_session_id) = context.client_session_id.as_deref() else {
        return Err(DenError::ValidationError(
            "task operation without job_id needs the current client session, but no client session id is available in this tool context".to_string(),
        )
        .into());
    };

    let Some(session) = client_sessions::find_for_user_bear_session_id(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
    )
    .await?
    else {
        return Err(DenError::ValidationError(
            "task operation without job_id could not resolve the current client session anchor"
                .to_string(),
        )
        .into());
    };

    Ok(Some(session.id))
}

pub(crate) async fn create_task(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketTaskCreateArguments = serde_json::from_value(arguments)?;
    let capabilities = effective_tool_policy(role, context).capabilities;
    let defaulted_to_pair_task_tree = should_default_session_task_tree(&capabilities, args.job_id);
    let job_id = args.job_id;
    // ponytail: jobless tasks belong only to the authenticated current session;
    // delegated work must first become a Job-owned task.
    let pair_session_id = resolve_task_session_anchor_id(pool, context, job_id).await?;
    enforce_oriented_task_create_policy(pool, context, &args).await?;
    let pair_session_attachment_id =
        if capabilities.contains(den_core::BearCapability::OwnSessionTasks) && job_id.is_some() {
            resolve_task_session_anchor_id(pool, context, None).await?
        } else {
            None
        };
    let service = PgDocketService::from_pool(pool);
    let task = service
        .create_task(DocketTaskCreate {
            bear_id: context.bear_id,
            job_id,
            pair_session_id,
            parent_task_id: args.parent_task_id,
            sibling_order: args.sibling_order,
            placement: args.placement,
            kind: args.kind,
            scope: args.scope,
            title: args.title,
            body: args.body,
            completion_criteria: args.completion_criteria,
            difficulty: args.difficulty,
            effort_hint: args.effort_hint,
            routing_strategy: Default::default(),
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: role.as_str().to_string(),
            created_by_user_id: Some(context.user_id),
            created_by_agent_id: clean_optional(&context.binding_id),
            created_in_run_id: args.created_in_run_id,
        })
        .await?;
    if let Some(session_id) = pair_session_attachment_id {
        service
            .attach_task_to_session(context.bear_id, task.id, session_id)
            .await?;
    }
    if job_id.is_none() && pair_session_id.is_some() {
        let client_session_id = context.client_session_id.as_deref().ok_or_else(|| {
            DenError::ValidationError(
                "session task creation needs the current client session".to_string(),
            )
        })?;
        client_sessions::set_current_task(
            pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            Some(task.id),
        )
        .await?;
        if let Some(conversation_id) = clean_optional(&context.conversation_id) {
            conversation_persistence::set_conversation_title_and_sync_client_sessions(
                pool,
                context.bear_id,
                &conversation_id,
                &task.title,
            )
            .await?;
        }
    }
    let task_list = if job_id.is_none() {
        if let Some(pair_session_id) = pair_session_id {
            session_anchored_task_list_projection(pool, context, role, pair_session_id).await?
        } else {
            None
        }
    } else {
        None
    };
    if job_id.is_none() {
        refresh_runtime_session_activity_plan(context, task_list.clone());
    }
    let task_list_phase = task_list
        .as_ref()
        .map(|task_list| task_list.status.as_str());
    let execution_allowed = task_list_phase.is_some_and(|phase| phase != "planned");
    let planned_session_task = job_id.is_none() && matches!(task_list_phase, Some("planned"));
    let summary = docket_task_create_summary(
        &task,
        job_id,
        defaulted_to_pair_task_tree,
        planned_session_task,
    );
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "content": summary,
        "summary": summary,
        "task": task,
        "task_list": task_list,
        "task_list_phase": task_list_phase,
        "execution_allowed": execution_allowed,
        "item_counts": task_list.as_ref().map(task_list_item_counts),
        "docket_scope": {
            "kind": if defaulted_to_pair_task_tree { "pair_task_tree" } else if job_id.is_some() { "work_job" } else { "session_task_list" },
            "job_id": job_id,
            "pair_session_id": pair_session_id,
        },
        "notes": if job_id.is_none() && matches!(task_list_phase, Some("planned")) {
            vec![
                "Created a durable session-anchored Docket task definition.",
                "The session task list is planned; wait for an explicit start/activation request before executing pending tasks."
            ]
        } else {
            vec![
                "Created durable Docket task definition. Status and results remain run-scoped."
            ]
        }
    }))
}

pub(crate) async fn select_current_task(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let policy = effective_tool_policy(role, context);
    policy
        .capabilities
        .require(den_core::BearCapability::SelectSessionTask)?;
    let args: DocketTaskSelectArguments = serde_json::from_value(arguments)?;
    let client_session_id = context.client_session_id.as_deref().ok_or_else(|| {
        DenError::ValidationError(
            "select_current_task needs the current client session".to_string(),
        )
    })?;
    match den_runtime::current_task::select_session_current_task(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
        args.task_id,
        &policy.capabilities,
    )
    .await
    {
        Ok(selection) => Ok(json!({
            "domain": "docket",
            "status": "selected",
            "current_task_id": args.task_id,
            "task_list": selection.task_list,
            "summary": if args.task_id.is_some() { "Selected the current session task." } else { "Cleared the current session task." },
        })),
        Err(CustomError::ValidationError(message)) => Ok(json!({
            "domain": "docket",
            "status": "selection_rejected",
            "requested_task_id": args.task_id,
            "error": {
                "code": "selected_task_not_actionable_for_session",
                "message": message,
                "next_action": "Call get_task_list_status, then select only a listed actionable task ID.",
            },
            "summary": "Task selection was rejected because the requested task is not actionable for this session. Read the current task list before selecting another task.",
        })),
        Err(error) => Err(error),
    }
}

pub(crate) async fn list_tasks(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketTaskListArguments = serde_json::from_value(arguments)?;
    let capabilities = effective_tool_policy(role, context).capabilities;
    let defaulted_to_pair_task_tree = should_default_session_task_tree(&capabilities, args.job_id);
    let job_id = args.job_id;
    let pair_session_id = if capabilities.contains(den_core::BearCapability::OwnSessionTasks) {
        resolve_task_session_anchor_id(pool, context, args.job_id).await?
    } else {
        None
    };
    let tasks = PgDocketService::from_pool(pool)
        .list_tasks(
            context.bear_id,
            DocketTaskListFilter {
                job_id,
                pair_session_id,
                parent_task_id: args.parent_task_id,
                include_descendants: args.include_descendants,
                limit: args.limit,
            },
        )
        .await?;
    let content = docket_tasks_card_content(&tasks);
    let summary = docket_tasks_summary_for_scope(&tasks, job_id, defaulted_to_pair_task_tree);
    let web_base = docket_web_base(pool, config, context.bear_id).await?;
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "content": content,
        "summary": summary,
        "count": tasks.len(),
        "presentations": docket_entity_presentations("task", tasks.iter().map(|task| task.task.id), |id| {
            tasks.iter().find(|task| task.task.id == id).and_then(|task| task.task.job_id).map(|job_id| {
                format!("{}#task-{}", docket_resource_href(&web_base, "jobs", job_id), id)
            })
        }),
        "counts": docket_task_counts(&tasks),
        "docket_scope": {
            "kind": if defaulted_to_pair_task_tree { "pair_task_tree" } else if job_id.is_some() { "work_job" } else { "session_task_list" },
            "job_id": job_id,
            "pair_session_id": pair_session_id,
        },
        "tasks": tasks,
    }))
}

pub(crate) async fn find_task(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketTaskFindArguments = serde_json::from_value(arguments)?;
    let service = PgDocketService::from_pool(pool);
    let job_id = if let Some(reference) = args.job_ref.as_deref() {
        let jobs = service
            .list_jobs(
                context.bear_id,
                DocketJobListFilter {
                    include_cancelled: true,
                    limit: 200,
                    ..DocketJobListFilter::default()
                },
            )
            .await?;
        Some(resolve_reference(
            reference,
            "job",
            jobs.into_iter().map(|job| job.id),
        )?)
    } else {
        None
    };
    let tasks = service
        .list_tasks(
            context.bear_id,
            DocketTaskListFilter {
                job_id,
                include_descendants: true,
                limit: 500,
                ..DocketTaskListFilter::default()
            },
        )
        .await?;
    let task_id = resolve_reference(
        &args.task_ref,
        "task",
        tasks.iter().map(|task| task.task.id),
    )?;
    let task = tasks
        .into_iter()
        .find(|task| task.task.id == task_id)
        .expect("resolved task is listed");
    Ok(json!({ "ok": true, "task": task }))
}

pub(crate) async fn update_task(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketTaskUpdateArguments = serde_json::from_value(arguments)?;
    if args.run_id.is_some()
        || args.status.is_some()
        || args.result_refs.is_some()
        || args.result_summary.is_some()
    {
        return Err(DenError::ValidationError(
            "update_task only edits durable task definition fields; use update_current_task_status for run-scoped status/results in the active run".to_string(),
        )
        .into());
    }
    let task = PgDocketService::from_pool(pool)
        .update_task(DocketTaskUpdate {
            bear_id: context.bear_id,
            job_id: None,
            task_id: args.task_id,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
            definition: DocketTaskDefinitionPatch {
                title: args.title,
                body: args.body,
                completion_criteria: args.completion_criteria,
                parent_task_id: args
                    .clear_parent_task_id
                    .then_some(None)
                    .or_else(|| args.parent_task_id.map(Some)),
                sibling_order: args.sibling_order,
                kind: args.kind,
                scope: args.scope,
                difficulty: args.difficulty.map(Some),
                effort_hint: args.effort_hint.map(Some),
                routing_strategy: None,
                expected_context_size: None,
                result_rollup_policy: None,
            },
            run_state: None,
        })
        .await?;
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "summary": format!("Updated {}", docket_task_summary(&task)),
        "task": task,
        "notes": [
            "Updated durable Docket task definition. Use update_current_task_status for active-run status/results."
        ]
    }))
}

pub(crate) async fn settle_execution_task(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketCurrentTaskStatusArguments = serde_json::from_value(arguments)?;
    let job_id = args.job_id.ok_or_else(|| {
        DenError::ValidationError("settle_execution_task requires job_id".to_string())
    })?;
    if args.run_id.is_some() {
        return Err(DenError::ValidationError(
            "settle_execution_task uses the active Docket execution run; do not pass run_id"
                .to_string(),
        )
        .into());
    }
    let outcome = PgDocketService::from_pool(pool)
        .settle_execution_task(DocketExecutionTaskSettlement {
            execution: DocketJobExecuteRequest {
                bear_id: context.bear_id,
                job_id,
                actor_role: role,
                actor_user_id: Some(context.user_id),
                actor_agent_id: clean_optional(&context.binding_id),
                session_id: Some(context.session_id.clone()),
                source_conversation_id: clean_optional(&context.conversation_id),
                source_client_session_id: context.client_session_id.clone(),
            },
            task_id: args.task_id,
            status: args.status,
            outcome_disposition: args.outcome_disposition,
            result_refs: args.result_refs,
            result_summary: args.result_summary,
        })
        .await?;
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "outcome": outcome,
    }))
}

pub(crate) async fn update_current_task_status(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketCurrentTaskStatusArguments = serde_json::from_value(arguments)?;
    if args.job_id.is_none() && args.run_id.is_none() {
        let pair_session_id = resolve_task_session_anchor_id(pool, context, None).await?;
        let session_tasks = PgDocketService::from_pool(pool)
            .list_tasks(
                context.bear_id,
                DocketTaskListFilter {
                    pair_session_id,
                    limit: 500,
                    ..DocketTaskListFilter::default()
                },
            )
            .await?;
        if session_tasks
            .iter()
            .any(|task| task.task.id == args.task_id)
        {
            let task = PgDocketService::from_pool(pool)
                .settle_session_task(DocketSessionTaskSettlement {
                    bear_id: context.bear_id,
                    pair_session_id: pair_session_id.expect("jobless task scope resolves session"),
                    task_id: args.task_id,
                    status: args.status,
                    outcome_disposition: args.outcome_disposition,
                    result_refs: args.result_refs,
                    result_summary: args.result_summary.clone(),
                    actor_role: role,
                    actor_user_id: Some(context.user_id),
                    actor_agent_id: clean_optional(&context.binding_id),
                })
                .await?;
            let status = args.status.as_str();
            if let Some(client_session_id) = context.client_session_id.as_deref() {
                if let Some(run) =
                    den_runtime::turn_runs::active_run_for_session(pool, client_session_id)
                        .await?
                        .filter(|run| {
                            run.bear_id == context.bear_id && run.user_id == context.user_id
                        })
                {
                    if let Err(error) = den_runtime::agent_loop::record_loop_control_decision(
                        pool,
                        LoopControlLedgerInput {
                            run_id: run.run_id,
                            turn_step_id: None,
                            conversation_message_id: None,
                            decision_id: format!("task-settled:{}:{}", args.task_id, status),
                            decision_kind: LoopControlDecisionKind::TaskSettled,
                            control_level: "standard".to_string(),
                            reason: Some(status.to_string()),
                            orientation_kind: Some("task_oriented".to_string()),
                            checkpoint_id: None,
                            related_task_list_id: Some(pair_session_id.expect("jobless task scope resolves session").to_string()),
                            related_task_item_id: Some(args.task_id.to_string()),
                            related_docket_job_id: None,
                            related_docket_task_id: Some(args.task_id),
                            evidence_refs: vec![LedgerEvidenceRef {
                                kind: "task_settlement".to_string(),
                                id: status.to_string(),
                            }],
                            decision: json!({
                                "action": "settle_task",
                                "status": status,
                                "outcome_disposition": args.outcome_disposition.map(|value| value.as_str()),
                            }),
                        },
                    )
                    .await
                    {
                        tracing::warn!(task_id = %args.task_id, error = %error, "failed to record session task settlement in loop-control ledger");
                    }
                }
            }
            return Ok(json!({
                "domain": "docket",
                "bear_id": context.bear_id,
                "summary": format!("Task '{}' is now {}.", task.task.title, human_task_status_label(status)),
                "task_title": task.task.title,
                "task_status": status,
                "result_summary": args.result_summary,
                "task": task,
                "docket": { "active_task_id": args.task_id, "source": "session_task_settlement" },
            }));
        }
    }
    if args.job_id.is_some() != args.run_id.is_some() {
        return Err(DenError::ValidationError(
            "update_current_task_status requires job_id and run_id together; pass both for explicit scope or neither to use the active Docket run"
                .to_string(),
        )
        .into());
    }
    let job_id = args.job_id.ok_or_else(|| {
        DenError::ValidationError(format!(
            "update_current_task_status cannot settle Job task {} without job_id and run_id; pass both from get_job/execute_job, or use settle_execution_task when execute_job/reconcile_job_execution claimed the task",
            args.task_id
        ))
    })?;
    let run_id = args.run_id.ok_or_else(|| {
        DenError::ValidationError(
            "update_current_task_status requires explicit job_id and run_id; legacy execution-session inference is no longer supported"
                .to_string(),
        )
    })?;
    let task = PgDocketService::from_pool(pool)
        .update_task(DocketTaskUpdate {
            bear_id: context.bear_id,
            job_id: Some(job_id),
            task_id: args.task_id,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: args.status,
                outcome_disposition: args.outcome_disposition,
                result_refs: args.result_refs,
                result_summary: args.result_summary,
            }),
        })
        .await?;
    let job = PgDocketService::from_pool(pool)
        .get_job(context.bear_id, job_id)
        .await?;
    let status_report = job.as_ref().map(docket_job_status_report);
    if let (Some(job), Some(status_report)) = (&job, &status_report) {
        update_focused_conversation_title(pool, context, job, status_report).await?;
    }
    let task_list = job
        .as_ref()
        .map(|job| den_docket::task_list_projection_from_docket_job(job, None));
    let status = task_projection_status(&task).to_string();
    let status_label = human_task_status_label(&status);
    let content = task_status_card_content(&task, &status, status_report.as_ref());
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "content": content,
        "summary": format!("Task '{}' is now {status_label}.", task.task.title),
        "task_title": task.task.title,
        "task_status": status,
        "task_status_label": status_label,
        "result_summary": task.run_state.as_ref().and_then(|state| state.result_summary.as_deref()),
        "has_result_refs": task.run_state.as_ref().and_then(|state| state.result_refs.as_ref()).is_some(),
        "task_counts": status_report.as_ref().map(|report| &report.task_counts),
        "next_action": status_report.as_ref().map(|report| report.next_action.as_str()),
        "item_counts": task_list.as_ref().map(task_list_item_counts),
        "task": task,
        "docket": {
            "active_job_id": job_id,
            "active_run_id": run_id,
            "active_task_id": args.task_id,
            "source": "explicit_task_status_scope"
        },
        "task_list": task_list,
        "status_report": status_report,
    }))
}

async fn current_client_session_task_id(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<Option<Uuid>, CustomError> {
    let Some(client_session_id) = context.client_session_id.as_deref() else {
        return Ok(None);
    };
    Ok(client_sessions::find_for_user_bear_session_id(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
    )
    .await?
    .and_then(|session| session.current_task_id))
}

pub(crate) async fn append_docket_entry(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketEntryAppendArguments = serde_json::from_value(arguments)?;
    let task_id = match (args.scope, args.task_id) {
        (DocketEntryScope::TaskJournal, Some(task_id)) => Some(task_id),
        (DocketEntryScope::TaskJournal, None) => current_client_session_task_id(pool, context)
            .await?
            .ok_or_else(|| {
                DenError::ValidationError(
                    "task_journal requires task_id or a selected current task in this client session"
                        .to_string(),
                )
            })
            .map(Some)?,
        (DocketEntryScope::JobNotebook, task_id) => task_id,
    };
    let entry = PgDocketService::from_pool(pool)
        .append_entry(DocketEntryCreate {
            bear_id: context.bear_id,
            job_id: args.job_id,
            task_id,
            run_id: args.run_id,
            scope: args.scope,
            kind: args.kind.into(),
            summary: args.summary,
            body: args.body,
            evidence_refs: args.evidence_refs,
            related_task_ids: args.related_task_ids,
            tags: args.tags,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
        })
        .await?;
    Ok(json!({ "domain": "docket", "entry": entry }))
}

pub(crate) async fn promote_docket_entry(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketEntryPromoteArguments = serde_json::from_value(arguments)?;
    let entry = PgDocketService::from_pool(pool)
        .promote_entry(DocketEntryPromotion {
            bear_id: context.bear_id,
            entry_id: args.entry_id,
            actor_role: role,
            actor_user_id: Some(context.user_id),
            actor_agent_id: clean_optional(&context.binding_id),
        })
        .await?;
    Ok(json!({ "domain": "docket", "entry": entry }))
}

pub(crate) async fn list_docket_entries(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: DocketEntryListArguments = serde_json::from_value(arguments)?;
    let entries = PgDocketService::from_pool(pool)
        .list_entries(
            context.bear_id,
            DocketEntryListFilter {
                job_id: args.job_id,
                task_id: args.task_id,
                limit: args.limit,
            },
        )
        .await?;
    Ok(json!({ "domain": "docket", "entries": entries }))
}

pub(crate) async fn sync_task_list(pool: &PgPool, arguments: Value) -> Result<Value, CustomError> {
    let args: TaskListSyncArguments = serde_json::from_value(arguments)?;
    let outcome = PgDocketService::from_pool(pool)
        .sync_task_list(TaskListSyncRequest {
            task_list: args.task_list,
        })
        .await?;
    let summary = if outcome.applied {
        format!("Synced {}", task_list_summary(&outcome.task_list))
    } else if outcome.review_required {
        format!(
            "Task list '{}' needs review before sync.",
            outcome.task_list.title
        )
    } else if !outcome.conflicts.is_empty() {
        format!(
            "Task list '{}' has {}.",
            outcome.task_list.title,
            count_label(outcome.conflicts.len(), "sync conflict", "sync conflicts")
        )
    } else {
        outcome.message.clone()
    };
    Ok(json!({
        "domain": "docket",
        "summary": summary,
        "item_counts": task_list_item_counts(&outcome.task_list),
        "sync": outcome,
    }))
}

pub(crate) async fn checkout_task_list(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: TaskListCheckoutArguments = serde_json::from_value(arguments)?;
    let (source, checkout_job_id) = if let Some(job_id) = args.job_id {
        (
            TaskListCheckoutSource::DocketJob {
                job_id,
                parent_task_id: args.parent_task_id,
            },
            Some(job_id),
        )
    } else if effective_tool_policy(role, context)
        .capabilities
        .contains(den_core::BearCapability::OwnSessionTasks)
    {
        let session_anchor_id = resolve_task_session_anchor_id(pool, context, None)
            .await?
            .ok_or_else(|| {
                DenError::ValidationError(
                    "pair checkout_task_list needs the current client session anchor".to_string(),
                )
            })?;
        let task_list =
            session_anchored_task_list_projection(pool, context, role, session_anchor_id)
                .await?
                .ok_or_else(|| {
                    DenError::ValidationError(
                        "pair checkout_task_list could not project the current session task tree"
                            .to_string(),
                    )
                })?;
        (
            TaskListCheckoutSource::LocalProjection(Box::new(task_list)),
            None,
        )
    } else {
        return Err(DenError::ValidationError(
            "checkout_task_list requires job_id outside pair stance; provide a Docket job id to project as a task list"
                .to_string(),
        )
        .into());
    };
    let pair_session_id = if effective_tool_policy(role, context)
        .capabilities
        .contains(den_core::BearCapability::OwnSessionTasks)
        && checkout_job_id.is_some()
    {
        resolve_task_session_anchor_id(pool, context, None).await?
    } else {
        None
    };
    let task_list = PgDocketService::from_pool(pool)
        .checkout_task_list(
            context.bear_id,
            role,
            context.user_id,
            TaskListCheckoutRequest {
                source,
                pair_session_id,
            },
        )
        .await?;
    let summary =
        task_list
            .as_ref()
            .map(task_list_summary)
            .unwrap_or_else(|| match checkout_job_id {
                Some(job_id) => format!("No Docket job found for {job_id}."),
                None => "No Docket task list found.".to_string(),
            });
    let item_counts = task_list.as_ref().map(task_list_item_counts);
    Ok(json!({
        "domain": "docket",
        "bear_id": context.bear_id,
        "summary": summary,
        "found": task_list.is_some(),
        "item_counts": item_counts,
        "task_list": task_list,
    }))
}

#[cfg(test)]
mod test {
    use super::*;

    fn status_report_with(
        current_task_title: Option<&str>,
        next_action: &str,
    ) -> DocketJobStatusReport {
        DocketJobStatusReport {
            job_id: Uuid::nil(),
            run_id: Some(Uuid::nil()),
            job_status: "running".to_string(),
            run_state: Some("running".to_string()),
            current_task_id: None,
            current_task_title: current_task_title.map(str::to_string),
            task_counts: den_docket::DocketCountByStatus {
                pending: 0,
                in_progress: 0,
                done: 0,
                blocked: 0,
                cancelled: 0,
            },
            criteria_counts: den_docket::DocketCriteriaCountByStatus {
                unmet: 0,
                met: 0,
                waived: 0,
            },
            tasks_complete: false,
            criteria_complete: true,
            next_action: next_action.to_string(),
        }
    }

    #[test]
    fn docket_entry_arguments_reject_outcome_and_accept_evidence() {
        let outcome = serde_json::from_value::<DocketEntryAppendArguments>(json!({
            "scope": "task_journal",
            "kind": "outcome",
            "summary": "settled"
        }));
        assert!(outcome.is_err());

        let valid = serde_json::from_value::<DocketEntryAppendArguments>(json!({
            "scope": "job_notebook",
            "kind": "finding",
            "summary": "Inventory complete",
            "evidence_refs": [{"kind": "source", "ref": "inventory"}]
        }))
        .expect("valid journal arguments");
        assert_eq!(valid.kind, DocketEntryAppendKind::Finding);
        assert_eq!(valid.evidence_refs.len(), 1);
    }

    #[test]
    fn docket_entry_list_defaults_to_service_limit() {
        let args = serde_json::from_value::<DocketEntryListArguments>(json!({}))
            .expect("empty list filter");
        assert_eq!(args.limit, 0);
        assert!(args.job_id.is_none());
        assert!(args.task_id.is_none());
    }

    #[test]
    fn focused_conversation_title_uses_current_task() {
        let report = status_report_with(Some("Implement the slice"), "continue");
        assert_eq!(
            focused_conversation_title("Roadmap cleanup", &report),
            "Roadmap cleanup - Implement the slice"
        );
    }

    #[test]
    fn focused_conversation_title_truncates_to_title_limit() {
        let report = status_report_with(Some("Task"), "continue");
        let title = focused_conversation_title(&"g".repeat(200), &report);
        assert_eq!(title.chars().count(), FOCUSED_CONVERSATION_TITLE_MAX_CHARS);
    }

    #[test]
    fn oriented_task_create_policy_parses_runtime_snapshot() {
        let task_id = Uuid::new_v4();
        let runtime = json!({
            "objective_orientation": {
                "kind": "oriented",
                "task": {
                    "task_ref": {
                        "kind": "docket_task",
                        "job_id": null,
                        "task_id": task_id.to_string(),
                        "title": "Root"
                    },
                    "child_policy": {
                        "max_children": 3,
                        "max_depth_below_oriented_task": 1
                    }
                }
            }
        });

        assert_eq!(
            oriented_task_create_policy(Some(&runtime)),
            Some(OrientedTaskCreatePolicy {
                root_task_id: task_id,
                max_children: 3,
                max_depth_below_oriented_task: 1,
            })
        );
    }

    #[test]
    fn oriented_new_child_depth_counts_depth_below_root() {
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        let grandchild_parent = Uuid::new_v4();
        let descendants = vec![
            task_projection(child, Some(root)),
            task_projection(grandchild_parent, Some(child)),
        ];

        assert_eq!(oriented_new_child_depth(root, root, &descendants), Some(1));
        assert_eq!(oriented_new_child_depth(root, child, &descendants), Some(2));
        assert_eq!(
            oriented_new_child_depth(root, grandchild_parent, &descendants),
            Some(3)
        );
    }

    #[test]
    fn oriented_capacity_rejects_child_count_and_depth_caps() {
        let policy = OrientedTaskCreatePolicy {
            root_task_id: Uuid::new_v4(),
            max_children: 2,
            max_depth_below_oriented_task: 1,
        };

        assert!(validate_oriented_child_capacity(policy, 1, 1).is_ok());

        let child_cap_error = validate_oriented_child_capacity(policy, 1, 2)
            .expect_err("child count at cap should reject");
        assert!(child_cap_error
            .to_string()
            .contains("at most 2 child tasks"));

        let depth_cap_error = validate_oriented_child_capacity(policy, 2, 0)
            .expect_err("depth beyond cap should reject");
        assert!(depth_cap_error.to_string().contains("at most 1 level(s)"));
    }

    fn task_projection(id: Uuid, parent_task_id: Option<Uuid>) -> den_docket::DocketTaskProjection {
        let now = time::OffsetDateTime::now_utc();
        den_docket::DocketTaskProjection {
            task: den_docket::DocketTaskRow {
                id,
                bear_id: Uuid::nil(),
                job_id: None,
                parent_task_id,
                sibling_order: 0,
                kind: "execution".to_string(),
                scope: "template".to_string(),
                title: "Task".to_string(),
                body: "Body".to_string(),
                completion_criteria: sqlx::types::Json(vec!["Done".to_string()]),
                difficulty: None,
                effort_hint: None,
                routing_strategy: "inline".to_string(),
                expected_context_size: None,
                result_rollup_policy: None,
                created_by_role: "pair".to_string(),
                created_by_user_id: None,
                created_by_agent_id: None,
                created_in_run_id: None,
                settled_by_entry_id: None,
                created_at: now,
                updated_at: now,
            },
            run_state: None,
            status: den_docket::DocketTaskStatus::Pending,
            integrity_conflict: None,
        }
    }

    #[test]
    fn session_task_owner_defaults_only_jobless_tasks_to_its_session_tree() {
        let explicit_job_id = Uuid::new_v4();
        let session_owner =
            den_core::CapabilitySet::from_capabilities([den_core::BearCapability::OwnSessionTasks]);
        let non_owner = den_core::CapabilitySet::default();

        assert!(should_default_session_task_tree(&session_owner, None));
        assert!(!should_default_session_task_tree(
            &session_owner,
            Some(explicit_job_id)
        ));
        assert!(!should_default_session_task_tree(&non_owner, None));
    }

    #[test]
    fn task_create_summary_names_pair_task_tree_scope() {
        let mut task = task_projection(Uuid::new_v4(), None).task;
        task.title = "Capture next step".to_string();

        let summary = docket_task_create_summary(&task, None, true, false);

        assert!(summary.contains("current session task tree"));
        assert!(!summary.contains("Docket Job"));
    }

    #[test]
    fn task_list_summary_names_explicit_job_scope() {
        let task = task_projection(Uuid::new_v4(), None);
        let job_id = Uuid::new_v4();

        let summary = docket_tasks_summary_for_scope(&[task], Some(job_id), false);

        assert_eq!(
            summary,
            format!("Found 1 Docket task in Docket Job {job_id}.")
        );
    }

    #[test]
    fn docket_tasks_card_content_includes_counts_and_titles() {
        let mut task = task_projection(Uuid::new_v4(), None);
        task.task.title = "Improve cards".to_string();
        let content = docket_tasks_card_content(&[task]);

        assert!(content.contains("Found 1 Docket task."));
        assert!(content.contains("1 pending"));
        assert!(content.contains("Improve cards"));
    }

    #[test]
    fn docket_tasks_card_content_projects_settled_tasks_as_done() {
        let mut task = task_projection(Uuid::new_v4(), None);
        task.task.title = "Reconcile task state".to_string();
        task.task.settled_by_entry_id = Some(Uuid::new_v4());
        task.status = den_docket::DocketTaskStatus::Done;

        let content = docket_tasks_card_content(&[task.clone()]);
        let counts = docket_task_counts(&[task]);

        assert!(content.contains("0 pending, 0 in progress, 1 done"));
        assert!(content.contains("Reconcile task state — done"));
        assert_eq!(counts["done"], 1);
        assert_eq!(counts["pending"], 0);
    }

    #[test]
    fn task_list_card_content_marks_planned_lists_as_not_started() {
        let now = time::OffsetDateTime::now_utc();
        let item = den_docket::TaskListItem {
            id: "item-1".to_string(),
            title: "Review plan".to_string(),
            summary: None,
            status: den_docket::TaskListItemStatus::Pending,
            blocked_reason: None,
            source_ref: den_docket::TaskListSourceRef::local(vec!["local:item-1".to_string()]),
            sync_state: den_docket::TaskListSyncState::Clean,
        };
        let task_list = TaskListProjection {
            id: Uuid::new_v4(),
            bear_id: Uuid::nil(),
            title: "Session tasks".to_string(),
            summary: "Tasks anchored to the current client session".to_string(),
            owner_profile: "pair".to_string(),
            visibility: "private_to_profile".to_string(),
            status: "planned".to_string(),
            version: 1,
            source_ref: den_docket::TaskListSourceRef::local(vec![
                "session_anchor:test".to_string()
            ]),
            items: vec![item.clone()],
            current_item: Some(item),
            source_conversation_id: None,
            source_client_session_id: None,
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: now,
            updated_at: now,
        };
        let content = task_list_card_content(&task_list);

        assert!(content.contains("Task list 'Session tasks' is planned"));
        assert!(content.contains("1 pending"));
        assert!(content.contains("Execution has not started."));
        assert!(content.contains("Review plan"));
    }

    mod state_tests {
        include!("../tests/workflow_state.rs");
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkDispatchArguments {
    job_id: Uuid,
    #[serde(default)]
    git_ref: Option<String>,
    /// Catalog image name on the sandbox provider (see get_work_catalog).
    #[serde(default)]
    image: Option<String>,
}

pub(crate) async fn dispatch_work(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    effective_tool_policy(role, context)
        .capabilities
        .require(den_core::BearCapability::DispatchWork)?;
    let args: WorkDispatchArguments = serde_json::from_value(arguments)?;
    PgDocketService::from_pool(pool)
        .get_job(context.bear_id, args.job_id)
        .await?
        .ok_or_else(|| DenError::NotFound(format!("Docket job {} not found", args.job_id)))?;
    let execution_target = den_docket::work_runs::WorkExecutionTarget::Sandbox;
    let runs = den_docket::work_runs::enqueue_work_job(
        pool,
        den_docket::work_runs::WorkJobEnqueue {
            bear_id: context.bear_id,
            job_id: args.job_id,
            durable_result: den_docket::DurableResultKind::RepositoryChanges,
            git_ref: args.git_ref.as_deref().and_then(clean_optional),
            image_name: args.image.as_deref().and_then(clean_optional),
            requested_by_user_id: Some(context.user_id),
            execution_target,
            attachment_warning: None,
        },
    )
    .await?;
    let run_ids: Vec<Uuid> = runs.iter().map(|run| run.id).collect();
    let queue = den_docket::work_runs::queued_run_positions(pool, &run_ids).await?;
    let web_base = docket_web_base(pool, config, context.bear_id).await?;
    let presentations = docket_entity_presentations("work run", run_ids.iter().copied(), |id| {
        Some(docket_resource_href(&web_base, "runs", id))
    });
    Ok(json!({
        "ok": true,
        "job_id": args.job_id,
        "work_run_ids": run_ids,
        "presentations": presentations,
        "queued_tasks": runs.len(),
        "queue": queue.iter().map(|info| json!({
            "run_id": info.run_id,
            "position": info.position,
            "waiting_on_run_id": info.waiting_on_run_id,
        })).collect::<Vec<_>>(),
        "execution_target": "sandbox",
        "internal_execution_target": "sandbox",
        "note": "Job queued; one work run will execute its runnable work tasks in the shared sandbox.",
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkRunListArguments {
    #[serde(default)]
    job_id: Option<Uuid>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

pub(crate) async fn list_work_runs(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WorkRunListArguments = serde_json::from_value(arguments)?;
    let runs = den_docket::work_runs::list_work_runs(
        pool,
        den_docket::work_runs::WorkRunListFilter {
            bear_id: Some(context.bear_id),
            job_id: args.job_id,
            state: args.state.as_deref().and_then(clean_optional),
            limit: args.limit.unwrap_or(50),
        },
    )
    .await?;
    let web_base = docket_web_base(pool, config, context.bear_id).await?;
    let queue_by_run = work_run_queue_map(pool, &runs).await?;
    let items: Vec<Value> = runs
        .iter()
        .map(|run| {
            let mut item =
                work_run_summary_json(run, Some(docket_resource_href(&web_base, "runs", run.id)));
            if let Some(queue) = queue_by_run.get(&run.id) {
                item["queue"] = queue.clone();
            }
            item
        })
        .collect();
    Ok(json!({
        "ok": true,
        "presentations": docket_entity_presentations("work run", runs.iter().map(|run| run.id), |id| {
            Some(docket_resource_href(&web_base, "runs", id))
        }),
        "work_runs": items,
    }))
}

/// Queue placement for the queued runs in `runs`, keyed by run id (runs
/// serialize per job; see den-docket's `queued_run_positions`).
async fn work_run_queue_map(
    pool: &PgPool,
    runs: &[den_docket::work_runs::WorkRunRow],
) -> Result<std::collections::HashMap<Uuid, Value>, CustomError> {
    let queued_ids: Vec<Uuid> = runs
        .iter()
        .filter(|run| run.state == "queued")
        .map(|run| run.id)
        .collect();
    let infos = den_docket::work_runs::queued_run_positions(pool, &queued_ids).await?;
    Ok(infos
        .into_iter()
        .map(|info| {
            (
                info.run_id,
                json!({
                    "position": info.position,
                    "waiting_on_run_id": info.waiting_on_run_id,
                }),
            )
        })
        .collect())
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkRunFindArguments {
    #[serde(default)]
    run_ref: Option<String>,
    #[serde(default)]
    job_ref: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkRunGetArguments {
    work_run_id: Uuid,
}

pub(crate) async fn get_work_run(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WorkRunGetArguments = serde_json::from_value(arguments)?;
    let run = den_docket::work_runs::get_work_run(pool, args.work_run_id)
        .await?
        .filter(|run| run.bear_id == context.bear_id)
        .ok_or_else(|| {
            CustomError::NotFound(format!("work run not found: {}", args.work_run_id))
        })?;
    let mut value = work_run_summary_json(&run, None);
    value["work_surface"] = run.work_surface.clone().unwrap_or(Value::Null);
    // Result refs already carry the bounded log tail / diff captured at
    // harvest time; live logs for active runs are on the /work web UI.
    value["result_refs"] = run.result_refs.clone().unwrap_or(Value::Null);
    if let Some(queue) = work_run_queue_map(pool, std::slice::from_ref(&run))
        .await?
        .remove(&run.id)
    {
        value["queue"] = queue;
    }
    value["usage"] = run.usage.unwrap_or(Value::Null);
    Ok(json!({ "ok": true, "work_run": value }))
}

pub(crate) async fn find_work_run(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WorkRunFindArguments = serde_json::from_value(arguments)?;
    match (args.run_ref.as_deref(), args.job_ref.as_deref()) {
        (Some(_), Some(_)) | (None, None) => Err(CustomError::ValidationError(
            "provide exactly one of run_ref or job_ref".to_string(),
        )),
        (Some(reference), None) => {
            let runs = den_docket::work_runs::list_work_runs(
                pool,
                den_docket::work_runs::WorkRunListFilter {
                    bear_id: Some(context.bear_id),
                    limit: 200,
                    ..den_docket::work_runs::WorkRunListFilter::default()
                },
            )
            .await?;
            let run_id = resolve_reference(reference, "work run", runs.iter().map(|run| run.id))?;
            get_work_run(pool, context, json!({ "work_run_id": run_id })).await
        }
        (None, Some(reference)) => {
            let jobs = PgDocketService::from_pool(pool)
                .list_jobs(
                    context.bear_id,
                    DocketJobListFilter {
                        include_cancelled: true,
                        limit: 200,
                        ..DocketJobListFilter::default()
                    },
                )
                .await?;
            let job_id = resolve_reference(reference, "job", jobs.into_iter().map(|job| job.id))?;
            list_work_runs(
                pool,
                config,
                context,
                json!({ "job_id": job_id, "limit": args.limit.unwrap_or(50) }),
            )
            .await
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkRunCancelArguments {
    work_run_id: Uuid,
}

pub(crate) async fn cancel_work_run(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    effective_tool_policy(role, context)
        .capabilities
        .require(den_core::BearCapability::DispatchWork)?;
    let args: WorkRunCancelArguments = serde_json::from_value(arguments)?;
    let requested = den_docket::work_runs::request_work_run_cancel_with_provenance(
        pool,
        args.work_run_id,
        context.bear_id,
        &den_docket::work_runs::WorkRunCancelRequest {
            requested_by: format!("tool:{}", role.as_str()),
            reason: "cancelled through the Docket control tool".into(),
        },
    )
    .await?;
    Ok(json!({
        "ok": true,
        "cancel_requested": requested,
        "note": if requested {
            "cancellation was requested; inspect the terminal work-run record for the final outcome and any partial progress"
        } else {
            "run is already terminal or unknown; nothing to cancel"
        },
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkRunResolveStalledArguments {
    work_run_id: Uuid,
    #[serde(default = "default_stalled_resolution_reason")]
    reason: String,
}

fn default_stalled_resolution_reason() -> String {
    "resolved through the Docket control tool".into()
}

pub(crate) async fn resolve_stalled_work_run(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    effective_tool_policy(role, context)
        .capabilities
        .require(den_core::BearCapability::DispatchWork)?;
    let args: WorkRunResolveStalledArguments = serde_json::from_value(arguments)?;
    let resolved = den_docket::work_runs::resolve_stalled_work_run(
        pool,
        args.work_run_id,
        context.bear_id,
        &den_docket::work_runs::StalledWorkRunResolution {
            resolved_by: format!("tool:{}", role.as_str()),
            reason: args.reason,
        },
    )
    .await?;
    Ok(json!({
        "ok": true,
        "resolved": resolved,
        "note": if resolved {
            "recorded the stalled-run resolution without changing its terminal outcome"
        } else {
            "run is not stalled, is unknown, or already has a recorded resolution; nothing changed"
        },
    }))
}

/// a root and toolchain image before dispatching.
pub(crate) async fn get_work_catalog(
    pool: &PgPool,
    config: &crate::config::Config,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let _ignored_arguments: Value = serde_json::from_value(arguments)?;

    // Managed surfaces assigned to this bear come from Den's database and are
    // available even when the provider is unreachable.
    let surfaces: Vec<Value> =
        den_service::work_surfaces::list_surfaces_for_bears(pool, &[context.bear_id])
            .await?
            .into_iter()
            .map(|surface| {
                json!({
                    "id": surface.id,
                    "name": surface.name,
                    "description": surface.description,
                    "default_ref": surface.default_ref,
                    "default_image": surface.default_image,
                    "assigned": true,
                })
            })
            .collect();

    let Some(url) = config
        .sandbox_server_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Err(CustomError::ValidationError(
            "no sandbox provider configured (SANDBOX_SERVER_URL unset); work dispatch is unavailable"
                .to_string(),
        ));
    };
    let client = den_sandbox::SandboxClient::new(url, &config.sandbox_server_token);
    let catalog = client.catalog().await.map_err(|err| {
        CustomError::System(format!("sandbox provider catalog fetch failed: {err}"))
    })?;
    Ok(json!({
        "ok": true,
        "surfaces": surfaces,
        "images": catalog.images,
        "roots": catalog.roots,
        "notes": [
            "Pass a `surfaces[].id` UUID as create_job's work_surface_id. Use the surface name as dispatch_work's root when dispatching that job.",
            "Pass an image name from `images` as dispatch_work's `image` to select a toolchain; omit it to use the surface's default.",
            "Roots with has_upstream=true are git-backed; pushable jobs publish to their upstream work branch."
        ],
    }))
}

fn execution_surface_json(run: &den_docket::work_runs::WorkRunRow) -> Value {
    // A work run owns a separate execution surface. Pair can report its
    // durable evidence but cannot inspect or continue that worktree without
    // an explicit transfer or separately granted access.
    json!({
        "kind": execution_surface_kind(&run.execution_target),
        "access_from_pair": "report_only",
        "work_surface": run.work_surface,
        "git_ref": run.git_ref,
        "sandbox_id": run.sandbox_id,
        "sandbox_type": run.sandbox_type,
        "may_contain_partial_changes": work_run_may_contain_partial_changes(&run.state),
    })
}

fn execution_surface_kind(execution_target: &str) -> &'static str {
    match execution_target {
        "attached_armature" => "attached_armature",
        _ => "work_sandbox",
    }
}

fn work_run_may_contain_partial_changes(state: &str) -> bool {
    matches!(state, "blocked" | "failed" | "cancelled" | "timed_out")
}

#[cfg(test)]
mod docket_entity_presentation_tests {
    use super::{docket_entity_presentation, docket_entity_presentations};
    use uuid::Uuid;

    fn id(value: &str) -> Uuid {
        Uuid::parse_str(value).expect("valid UUID fixture")
    }

    #[test]
    fn uses_eight_character_typed_handle_when_unambiguous() {
        let entity = docket_entity_presentation(
            "job",
            id("e4e4797b-2d29-4800-856e-b8220a9097dd"),
            &[id("e4e4797b-2d29-4800-856e-b8220a9097dd")],
            None,
        );

        assert_eq!(entity.display, "job e4e4797b");
        assert_eq!(entity.href, None);
    }

    #[test]
    fn extends_handle_when_response_ids_share_the_default_prefix() {
        let ids = [
            id("e4e4797b-2d29-4800-856e-b8220a9097dd"),
            id("e4e4797b-2d29-4800-956e-b8220a9097dd"),
        ];

        let entities = docket_entity_presentations("work run", ids, |_| None);
        let displays: Vec<_> = entities
            .iter()
            .map(|entity| entity.display.as_str())
            .collect();

        assert_eq!(
            displays,
            ["work run e4e4797b2d2948008", "work run e4e4797b2d2948009"]
        );
    }
}

fn work_run_summary_json(run: &den_docket::work_runs::WorkRunRow, href: Option<String>) -> Value {
    fn ts(value: Option<time::OffsetDateTime>) -> Value {
        value
            .and_then(|t| {
                t.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            })
            .map_or(Value::Null, Value::String)
    }
    json!({
        "work_run_id": run.id,
        "presentation": docket_entity_presentation("work run", run.id, &[run.id], href),
        "state": run.state,
        "attempt": run.attempt,
        "job_id": run.job_id,
        "cancel_requested": run.cancel_requested,
        "cancel_requested_by": run.cancel_requested_by,
        "cancel_reason": run.cancel_reason,
        "cancel_requested_at": ts(run.cancel_requested_at),
        "work_surface": run.work_surface,
        "git_ref": run.git_ref,
        "sandbox_id": run.sandbox_id,
        "sandbox_type": run.sandbox_type,
        "sandbox_strength": run.sandbox_strength,
        "execution_surface": execution_surface_json(run),
        "result_summary": run.result_summary,
        "error": run.error,
        "queued_at": ts(Some(run.queued_at)),
        "started_at": ts(run.started_at),
        "finished_at": ts(run.finished_at),
    })
}
