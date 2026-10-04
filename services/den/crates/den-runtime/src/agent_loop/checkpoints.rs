use den_core::{DenError, RuntimeContextLabel};
use den_protocol::ContextBudgetReport;
use den_service::artifacts::{
    attach_artifact, attach_artifact_in_tx, create_json_artifact, create_json_artifact_in_tx,
    ArtifactStorageKind, ArtifactVisibility as RegistryVisibility, AttachArtifactInput,
    CreateJsonArtifactInput, ReserveArtifactInput,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{RuntimeCheckpointRequest, RuntimeCheckpointResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointValidationStatus {
    Requested,
    Valid,
    Invalid,
    Superseded,
}

impl CheckpointValidationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Superseded => "superseded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointVisibility {
    AuditOnly,
    LiveEphemeral,
    ModelVisibleHidden,
}

impl CheckpointVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuditOnly => "audit_only",
            Self::LiveEphemeral => "live_ephemeral",
            Self::ModelVisibleHidden => "model_visible_hidden",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointReplayPolicy {
    None,
    SummaryOnce,
    UntilSuperseded,
}

impl CheckpointReplayPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::SummaryOnce => "summary_once",
            Self::UntilSuperseded => "until_superseded",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckpointArtifactInput {
    pub bear_id: Uuid,
    pub created_by_user_id: Option<i32>,
    pub owner_profile: RuntimeContextLabel,
    pub run_id: String,
    pub turn_step_id: Option<Uuid>,
    pub orientation_kind: Option<String>,
    pub audit_context: Option<den_protocol::CheckpointAuditContext>,
    pub request: RuntimeCheckpointRequest,
    pub visibility: CheckpointVisibility,
    pub replay_policy: CheckpointReplayPolicy,
}

#[derive(Debug, Clone)]
pub struct CheckpointResponseInput {
    pub bear_id: Uuid,
    pub created_by_user_id: Option<i32>,
    pub owner_profile: RuntimeContextLabel,
    pub run_id: String,
    pub checkpoint_id: String,
    pub response: RuntimeCheckpointResponse,
    pub validation_status: CheckpointValidationStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointArtifactRow {
    pub id: Uuid,
    pub run_id: String,
    pub turn_step_id: Option<Uuid>,
    pub checkpoint_id: String,
    pub reason: String,
    pub control_level: String,
    pub request: Value,
    pub response: Option<Value>,
    pub validation_status: String,
    pub visibility: String,
    pub replay_policy: String,
    pub related_task_list_id: Option<String>,
    pub related_task_item_id: Option<String>,
    pub related_docket_task_id: Option<Uuid>,
    pub related_work_run_id: Option<Uuid>,
    pub related_docket_job_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopControlDecisionKind {
    CheckpointRequested,
    ContextBudgetPressure,
    GroundingProbeResult,
    FinalGateContinuation,
    ActiveTaskPause,
    BudgetSliceContinuation,
    BudgetSliceRecovery,
    DeliveryInterrupted,
    TaskSettled,
}

impl LoopControlDecisionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CheckpointRequested => "checkpoint_requested",
            Self::ContextBudgetPressure => "context_budget_pressure",
            Self::GroundingProbeResult => "grounding_probe_result",
            Self::FinalGateContinuation => "final_gate_continuation",
            Self::ActiveTaskPause => "active_task_pause",
            Self::BudgetSliceContinuation => "budget_slice_continuation",
            Self::BudgetSliceRecovery => "budget_slice_recovery",
            Self::DeliveryInterrupted => "delivery_interrupted",
            Self::TaskSettled => "task_settled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "checkpoint_requested" => Some(Self::CheckpointRequested),
            "context_budget_pressure" => Some(Self::ContextBudgetPressure),
            "grounding_probe_result" => Some(Self::GroundingProbeResult),
            "final_gate_continuation" => Some(Self::FinalGateContinuation),
            "active_task_pause" => Some(Self::ActiveTaskPause),
            "budget_slice_continuation" => Some(Self::BudgetSliceContinuation),
            "budget_slice_recovery" => Some(Self::BudgetSliceRecovery),
            "delivery_interrupted" => Some(Self::DeliveryInterrupted),
            "task_settled" => Some(Self::TaskSettled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextBudgetPressureLevel {
    NearBudget,
    OverBudget,
}

impl ContextBudgetPressureLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NearBudget => "near_budget",
            Self::OverBudget => "over_budget",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ContextBudgetPressureDecision {
    level: ContextBudgetPressureLevel,
    model: String,
    context_window: Option<u32>,
    estimated_input_tokens: u32,
    reserved_output_tokens: u32,
    estimated_total_tokens: u32,
    near_budget: bool,
    over_budget: bool,
    action: &'static str,
}

impl ContextBudgetPressureDecision {
    fn from_report(report: &ContextBudgetReport) -> Option<Self> {
        let level = if report.over_budget {
            ContextBudgetPressureLevel::OverBudget
        } else if report.near_budget {
            ContextBudgetPressureLevel::NearBudget
        } else {
            return None;
        };
        Some(Self {
            level,
            model: report.model.clone(),
            context_window: report.context_window,
            estimated_input_tokens: report.estimated_input_tokens,
            reserved_output_tokens: report.reserved_output_tokens,
            estimated_total_tokens: report.estimated_total_tokens,
            near_budget: report.near_budget,
            over_budget: report.over_budget,
            action: context_budget_pressure_action(level),
        })
    }
}

pub const fn context_budget_pressure_action(level: ContextBudgetPressureLevel) -> &'static str {
    match level {
        ContextBudgetPressureLevel::NearBudget => "prefer_checkpoint_before_more_context_growth",
        ContextBudgetPressureLevel::OverBudget => {
            "stop_before_model_call_and_request_compaction_or_smaller_context"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingProbeSignalKind {
    Pass,
    Fail,
    NoSignal,
}

impl GroundingProbeSignalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NoSignal => "no_signal",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pass" => Some(Self::Pass),
            "fail" => Some(Self::Fail),
            "no_signal" => Some(Self::NoSignal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundingProbeFinding {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingProbeResultInput {
    pub run_id: String,
    pub turn_step_id: Option<Uuid>,
    pub orientation_kind: Option<String>,
    pub tool_call_id: Option<String>,
    pub probe_id: String,
    pub surface_kind: String,
    pub signal: GroundingProbeSignalKind,
    pub duration_ms: u64,
    pub findings: Vec<GroundingProbeFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroundingProbeDecision {
    probe_id: String,
    surface_kind: String,
    signal: GroundingProbeSignalKind,
    duration_ms: u64,
    findings: Vec<GroundingProbeFinding>,
}

pub fn non_empty_diff_grounding_probe(
    run_id: impl Into<String>,
    diff: &str,
) -> GroundingProbeResultInput {
    let changed = !diff.trim().is_empty();
    GroundingProbeResultInput {
        run_id: run_id.into(),
        turn_step_id: None,
        orientation_kind: None,
        tool_call_id: None,
        probe_id: "generic.non_empty_diff".to_string(),
        surface_kind: "repository".to_string(),
        signal: if changed {
            GroundingProbeSignalKind::Pass
        } else {
            GroundingProbeSignalKind::Fail
        },
        duration_ms: 0,
        findings: vec![GroundingProbeFinding {
            code: if changed {
                "diff_present"
            } else {
                "empty_diff"
            }
            .to_string(),
            message: if changed {
                "Workspace diff is non-empty."
            } else {
                "Workspace diff is empty."
            }
            .to_string(),
        }],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopControlLedgerInput {
    pub run_id: String,
    pub turn_step_id: Option<Uuid>,
    pub conversation_message_id: Option<Uuid>,
    pub decision_id: String,
    pub decision_kind: LoopControlDecisionKind,
    pub control_level: String,
    pub reason: Option<String>,
    pub orientation_kind: Option<String>,
    pub checkpoint_id: Option<String>,
    pub related_task_list_id: Option<String>,
    pub related_task_item_id: Option<String>,
    pub related_docket_job_id: Option<Uuid>,
    pub related_docket_task_id: Option<Uuid>,
    pub evidence_refs: Vec<LedgerEvidenceRef>,
    pub decision: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEvidenceRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopControlLedgerRow {
    pub id: Uuid,
    pub run_id: String,
    pub turn_step_id: Option<Uuid>,
    /// Canonical transcript row that caused this controller decision, when applicable.
    pub conversation_message_id: Option<Uuid>,
    pub decision_id: String,
    pub decision_kind: String,
    pub control_level: String,
    pub reason: Option<String>,
    pub orientation_kind: Option<String>,
    pub checkpoint_id: Option<String>,
    pub related_task_list_id: Option<String>,
    pub related_task_item_id: Option<String>,
    pub related_docket_job_id: Option<Uuid>,
    pub related_docket_task_id: Option<Uuid>,
    pub evidence_refs: Value,
    pub decision: Value,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopControlReplayObservation {
    pub run_id: String,
    pub turn_step_id: Option<Uuid>,
    pub decision_id: String,
    pub decision_kind: LoopControlDecisionKind,
    pub control_level: String,
    pub reason: Option<String>,
    pub orientation_kind: Option<String>,
    pub checkpoint_id: Option<String>,
    pub related_task_list_id: Option<String>,
    pub related_task_item_id: Option<String>,
    pub related_docket_job_id: Option<Uuid>,
    pub related_docket_task_id: Option<Uuid>,
    pub evidence_refs: Vec<LedgerEvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopControlReplayMismatch {
    pub index: usize,
    pub expected: Option<LoopControlReplayObservation>,
    pub observed: Option<LoopControlReplayObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopControlReplayTurn {
    pub run_id: String,
    pub turn_step_id: Option<Uuid>,
    pub decision_count: usize,
    pub decision_ids: Vec<String>,
    pub decision_kinds: Vec<LoopControlDecisionKind>,
    pub control_levels: Vec<String>,
    pub reasons: Vec<String>,
    pub orientation_kinds: Vec<String>,
    pub checkpoint_ids: Vec<String>,
    pub related_task_list_ids: Vec<String>,
    pub related_task_item_ids: Vec<String>,
    pub related_docket_job_ids: Vec<Uuid>,
    pub related_docket_task_ids: Vec<Uuid>,
    pub evidence_refs: Vec<LedgerEvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedLoopControlReplayTurn {
    pub run_id: String,
    pub turn_step_id: Option<Uuid>,
    pub decision_count: usize,
    pub decision_kinds: Vec<LoopControlDecisionKind>,
    pub control_levels: Vec<String>,
    pub reasons: Vec<String>,
    pub orientation_kinds: Vec<String>,
    pub checkpoint_ids: Vec<String>,
    pub evidence_refs: Vec<LedgerEvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopControlReplayTurnMismatch {
    pub index: usize,
    pub expected: Option<ExpectedLoopControlReplayTurn>,
    pub observed: Option<LoopControlReplayTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopControlReplayProfileSummary {
    pub turn_count: usize,
    pub decision_count: usize,
    pub decision_kind_counts: Vec<LoopControlReplayCount<LoopControlDecisionKind>>,
    pub control_level_counts: Vec<LoopControlReplayCount<String>>,
    pub orientation_kind_counts: Vec<LoopControlReplayCount<String>>,
    pub reason_counts: Vec<LoopControlReplayCount<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedLoopControlReplayProfileSummary {
    pub turn_count: usize,
    pub decision_count: usize,
    pub decision_kind_counts: Vec<LoopControlReplayCount<LoopControlDecisionKind>>,
    pub control_level_counts: Vec<LoopControlReplayCount<String>>,
    pub orientation_kind_counts: Vec<LoopControlReplayCount<String>>,
    pub reason_counts: Vec<LoopControlReplayCount<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopControlReplayProfileMismatch {
    pub expected: ExpectedLoopControlReplayProfileSummary,
    pub observed: LoopControlReplayProfileSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopControlReplayCount<T> {
    pub value: T,
    pub count: usize,
}

pub fn replay_loop_control_observations(
    rows: &[LoopControlLedgerRow],
) -> Result<Vec<LoopControlReplayObservation>, DenError> {
    rows.iter().map(replay_observation_from_row).collect()
}

pub fn compare_loop_control_replay(
    observed: &[LoopControlReplayObservation],
    expected: &[LoopControlReplayObservation],
) -> Vec<LoopControlReplayMismatch> {
    let max_len = observed.len().max(expected.len());
    (0..max_len)
        .filter_map(|index| {
            let observed = observed.get(index).cloned();
            let expected = expected.get(index).cloned();
            (observed != expected).then_some(LoopControlReplayMismatch {
                index,
                expected,
                observed,
            })
        })
        .collect()
}

pub fn aggregate_loop_control_replay_turns(
    rows: &[LoopControlLedgerRow],
) -> Result<Vec<LoopControlReplayTurn>, DenError> {
    let observations = replay_loop_control_observations(rows)?;
    let mut turns: Vec<LoopControlReplayTurn> = Vec::new();
    for observation in observations {
        // ponytail: linear grouping is fine for per-run replay fixtures; upgrade to
        // an index map if replaying large production windows in-process.
        let turn = match turns.iter_mut().find(|turn| {
            turn.run_id == observation.run_id && turn.turn_step_id == observation.turn_step_id
        }) {
            Some(turn) => turn,
            None => {
                turns.push(LoopControlReplayTurn {
                    run_id: observation.run_id.clone(),
                    turn_step_id: observation.turn_step_id,
                    decision_count: 0,
                    decision_ids: Vec::new(),
                    decision_kinds: Vec::new(),
                    control_levels: Vec::new(),
                    reasons: Vec::new(),
                    orientation_kinds: Vec::new(),
                    checkpoint_ids: Vec::new(),
                    related_task_list_ids: Vec::new(),
                    related_task_item_ids: Vec::new(),
                    related_docket_job_ids: Vec::new(),
                    related_docket_task_ids: Vec::new(),
                    evidence_refs: Vec::new(),
                });
                turns.last_mut().expect("turn was just pushed")
            }
        };
        turn.decision_count += 1;
        push_unique(&mut turn.decision_ids, observation.decision_id);
        push_unique(&mut turn.decision_kinds, observation.decision_kind);
        push_unique(&mut turn.control_levels, observation.control_level);
        push_optional_unique(&mut turn.reasons, observation.reason);
        push_optional_unique(&mut turn.orientation_kinds, observation.orientation_kind);
        push_optional_unique(&mut turn.checkpoint_ids, observation.checkpoint_id);
        push_optional_unique(
            &mut turn.related_task_list_ids,
            observation.related_task_list_id,
        );
        push_optional_unique(
            &mut turn.related_task_item_ids,
            observation.related_task_item_id,
        );
        push_optional_unique(
            &mut turn.related_docket_job_ids,
            observation.related_docket_job_id,
        );
        push_optional_unique(
            &mut turn.related_docket_task_ids,
            observation.related_docket_task_id,
        );
        for evidence_ref in observation.evidence_refs {
            push_unique(&mut turn.evidence_refs, evidence_ref);
        }
    }
    Ok(turns)
}

pub fn compare_loop_control_replay_turns(
    observed: &[LoopControlReplayTurn],
    expected: &[ExpectedLoopControlReplayTurn],
) -> Vec<LoopControlReplayTurnMismatch> {
    let max_len = observed.len().max(expected.len());
    (0..max_len)
        .filter_map(|index| {
            let observed = observed.get(index).cloned();
            let expected = expected.get(index).cloned();
            let matches = match (&observed, &expected) {
                (Some(observed), Some(expected)) => {
                    replay_turn_matches_expected(observed, expected)
                }
                (None, None) => true,
                _ => false,
            };
            (!matches).then_some(LoopControlReplayTurnMismatch {
                index,
                expected,
                observed,
            })
        })
        .collect()
}

fn replay_turn_matches_expected(
    observed: &LoopControlReplayTurn,
    expected: &ExpectedLoopControlReplayTurn,
) -> bool {
    observed.run_id == expected.run_id
        && observed.turn_step_id == expected.turn_step_id
        && observed.decision_count == expected.decision_count
        && observed.decision_kinds == expected.decision_kinds
        && observed.control_levels == expected.control_levels
        && observed.reasons == expected.reasons
        && observed.orientation_kinds == expected.orientation_kinds
        && observed.checkpoint_ids == expected.checkpoint_ids
        && observed.evidence_refs == expected.evidence_refs
}

pub fn summarize_loop_control_replay_profile(
    turns: &[LoopControlReplayTurn],
) -> LoopControlReplayProfileSummary {
    let mut decision_kind_counts = Vec::new();
    let mut control_level_counts = Vec::new();
    let mut orientation_kind_counts = Vec::new();
    let mut reason_counts = Vec::new();
    for turn in turns {
        for decision_kind in &turn.decision_kinds {
            increment_replay_count(&mut decision_kind_counts, *decision_kind);
        }
        for control_level in &turn.control_levels {
            increment_replay_count(&mut control_level_counts, control_level.clone());
        }
        for orientation_kind in &turn.orientation_kinds {
            increment_replay_count(&mut orientation_kind_counts, orientation_kind.clone());
        }
        for reason in &turn.reasons {
            increment_replay_count(&mut reason_counts, reason.clone());
        }
    }
    LoopControlReplayProfileSummary {
        turn_count: turns.len(),
        decision_count: turns.iter().map(|turn| turn.decision_count).sum(),
        decision_kind_counts,
        control_level_counts,
        orientation_kind_counts,
        reason_counts,
    }
}

pub fn compare_loop_control_replay_profile(
    observed: &LoopControlReplayProfileSummary,
    expected: &ExpectedLoopControlReplayProfileSummary,
) -> Option<LoopControlReplayProfileMismatch> {
    let matches = observed.turn_count == expected.turn_count
        && observed.decision_count == expected.decision_count
        && observed.decision_kind_counts == expected.decision_kind_counts
        && observed.control_level_counts == expected.control_level_counts
        && observed.orientation_kind_counts == expected.orientation_kind_counts
        && observed.reason_counts == expected.reason_counts;
    (!matches).then_some(LoopControlReplayProfileMismatch {
        expected: expected.clone(),
        observed: observed.clone(),
    })
}

fn increment_replay_count<T: PartialEq>(counts: &mut Vec<LoopControlReplayCount<T>>, value: T) {
    // ponytail: vector counters preserve first-seen order and are enough for small replay
    // windows; use BTreeMap if offline replay starts scanning large production slices.
    match counts.iter_mut().find(|count| count.value == value) {
        Some(count) => count.count += 1,
        None => counts.push(LoopControlReplayCount { value, count: 1 }),
    }
}

fn push_optional_unique<T: PartialEq>(values: &mut Vec<T>, value: Option<T>) {
    if let Some(value) = value {
        push_unique(values, value);
    }
}

fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn replay_observation_from_row(
    row: &LoopControlLedgerRow,
) -> Result<LoopControlReplayObservation, DenError> {
    let decision_kind = LoopControlDecisionKind::parse(&row.decision_kind).ok_or_else(|| {
        DenError::System(format!(
            "unknown loop-control decision kind in replay ledger: {}",
            row.decision_kind
        ))
    })?;
    let evidence_refs: Vec<LedgerEvidenceRef> =
        serde_json::from_value(row.evidence_refs.clone())
            .map_err(|err| DenError::System(format!("deserialize replay evidence refs: {err}")))?;
    Ok(LoopControlReplayObservation {
        run_id: row.run_id.clone(),
        turn_step_id: row.turn_step_id,
        decision_id: row.decision_id.clone(),
        decision_kind,
        control_level: row.control_level.clone(),
        reason: row.reason.clone(),
        orientation_kind: row.orientation_kind.clone(),
        checkpoint_id: row.checkpoint_id.clone(),
        related_task_list_id: row.related_task_list_id.clone(),
        related_task_item_id: row.related_task_item_id.clone(),
        related_docket_job_id: row.related_docket_job_id,
        related_docket_task_id: row.related_docket_task_id,
        evidence_refs,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointLedgerDecision {
    checkpoint_id: String,
    reason: String,
    control_level: String,
    profile_fingerprint: Option<String>,
    active_objective_present: bool,
    required_fields: Vec<String>,
    task_refs: CheckpointLedgerTaskRefs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointLedgerTaskRefs {
    task_list_id: Option<String>,
    task_list_version: Option<String>,
    active_item_id: Option<String>,
    docket_job_id: Option<String>,
    docket_task_id: Option<String>,
}

pub async fn record_checkpoint_request(
    pool: &PgPool,
    input: CheckpointArtifactInput,
) -> Result<CheckpointArtifactRow, DenError> {
    let request_json = serde_json::to_value(&input.request)
        .map_err(|err| DenError::System(format!("serialize checkpoint request: {err}")))?;
    let task_context = input.request.task_context.as_ref();
    let related_docket_task_id = task_context
        .and_then(|context| context.docket_task_id.as_deref())
        .and_then(|value| Uuid::parse_str(value).ok());

    let checkpoint = sqlx::query_as!(
        CheckpointArtifactRow,
        r"
        INSERT INTO bear_run_checkpoints (
            run_id, turn_step_id, checkpoint_id, reason, control_level, request,
            validation_status, visibility, replay_policy, related_task_list_id,
            related_task_item_id, related_docket_task_id, related_work_run_id,
            related_docket_job_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'requested', $7, $8, $9, $10, $11, $12, $13)
        ON CONFLICT (run_id, checkpoint_id) DO UPDATE SET
            request = EXCLUDED.request,
            reason = EXCLUDED.reason,
            control_level = EXCLUDED.control_level,
            validation_status = 'requested',
            visibility = EXCLUDED.visibility,
            replay_policy = EXCLUDED.replay_policy,
            related_task_list_id = EXCLUDED.related_task_list_id,
            related_task_item_id = EXCLUDED.related_task_item_id,
            related_docket_task_id = EXCLUDED.related_docket_task_id,
            related_work_run_id = EXCLUDED.related_work_run_id,
            related_docket_job_id = EXCLUDED.related_docket_job_id,
            response = NULL,
            updated_at = NOW()
        RETURNING
            id, run_id, turn_step_id, checkpoint_id, reason, control_level, request,
            response, validation_status, visibility, replay_policy, related_task_list_id,
            related_task_item_id, related_docket_task_id, related_work_run_id,
            related_docket_job_id, created_at, updated_at
        ",
        input.run_id,
        input.turn_step_id,
        input.request.checkpoint_id,
        input.request.reason.as_str(),
        input.request.control_level.as_str(),
        request_json,
        input.visibility.as_str(),
        input.replay_policy.as_str(),
        task_context.and_then(|context| context.task_list_id.as_deref()),
        task_context.and_then(|context| context.active_item_id.as_deref()),
        related_docket_task_id,
        input.audit_context.map(|context| context.work_run_id),
        input.audit_context.map(|context| context.docket_job_id)
    )
    .fetch_one(pool)
    .await?;
    record_loop_control_decision(
        pool,
        checkpoint_request_ledger_input(
            &input.request,
            input.turn_step_id,
            input.orientation_kind.clone(),
        )?,
    )
    .await?;
    record_checkpoint_json_artifact(pool, &input).await?;
    Ok(checkpoint)
}

async fn record_checkpoint_json_artifact(
    pool: &PgPool,
    input: &CheckpointArtifactInput,
) -> Result<(), DenError> {
    let task_context = input.request.task_context.as_ref();
    let payload = serde_json::json!({
        "checkpoint_id": input.request.checkpoint_id,
        "reason": input.request.reason.as_str(),
        "control_level": input.request.control_level.as_str(),
        "visibility": input.visibility.as_str(),
        "replay_policy": input.replay_policy.as_str(),
        "run_id": input.run_id,
        "turn_step_id": input.turn_step_id,
        "orientation_kind": input.orientation_kind,
        "task_list_id": task_context.and_then(|context| context.task_list_id.as_deref()),
        "task_item_id": task_context.and_then(|context| context.active_item_id.as_deref()),
        "docket_task_id": task_context.and_then(|context| context.docket_task_id.as_deref()),
        "work_run_id": input.audit_context.as_ref().map(|context| context.work_run_id),
        "docket_job_id": input.audit_context.as_ref().map(|context| context.docket_job_id),
        "request": input.request,
    });
    let artifact = create_json_artifact(
        pool,
        CreateJsonArtifactInput {
            reserve: ReserveArtifactInput {
                bear_id: input.bear_id,
                created_by_user_id: input.created_by_user_id,
                owner_profile: input.owner_profile,
                kind: "runtime_checkpoint".to_string(),
                title: Some("Runtime checkpoint".to_string()),
                summary: Some(input.request.reason.as_str().to_string()),
                content_type: Some("application/json".to_string()),
                storage_kind: ArtifactStorageKind::DbText,
                visibility: RegistryVisibility::PrivateToProfile,
                provenance: serde_json::json!({"source": "den_runtime"}),
                metadata: serde_json::json!({
                    "replay_policy": input.replay_policy.as_str(),
                    "excluded_from_transcript": true,
                    "excluded_from_default_replay": true,
                }),
                expires_at: None,
            },
            payload,
        },
    )
    .await?;
    if let Some(work_run_id) = input
        .audit_context
        .as_ref()
        .map(|context| context.work_run_id)
    {
        attach_artifact(
            pool,
            AttachArtifactInput {
                artifact_ref: artifact.artifact_ref.clone(),
                bear_id: input.bear_id,
                target_kind: "work_run".to_string(),
                target_id: work_run_id.to_string(),
                role: "runtime_checkpoint".to_string(),
                metadata: serde_json::json!({}),
                created_by_user_id: input.created_by_user_id,
            },
        )
        .await?;
    }
    if let Some(job_id) = input
        .audit_context
        .as_ref()
        .map(|context| context.docket_job_id)
    {
        attach_artifact(
            pool,
            AttachArtifactInput {
                artifact_ref: artifact.artifact_ref.clone(),
                bear_id: input.bear_id,
                target_kind: "docket_job".to_string(),
                target_id: job_id.to_string(),
                role: "runtime_checkpoint".to_string(),
                metadata: serde_json::json!({}),
                created_by_user_id: input.created_by_user_id,
            },
        )
        .await?;
    }
    if let Some(task_id) = task_context.and_then(|context| context.docket_task_id.as_deref()) {
        if Uuid::parse_str(task_id).is_ok() {
            attach_artifact(
                pool,
                AttachArtifactInput {
                    artifact_ref: artifact.artifact_ref,
                    bear_id: input.bear_id,
                    target_kind: "docket_task".to_string(),
                    target_id: task_id.to_string(),
                    role: "runtime_checkpoint".to_string(),
                    metadata: serde_json::json!({}),
                    created_by_user_id: input.created_by_user_id,
                },
            )
            .await?;
        }
    }
    Ok(())
}

pub async fn record_context_budget_pressure_decision(
    pool: &PgPool,
    run_id: &str,
    turn_step_id: Option<Uuid>,
    orientation_kind: Option<String>,
    report: &ContextBudgetReport,
) -> Result<Option<LoopControlLedgerRow>, DenError> {
    let Some(decision) = ContextBudgetPressureDecision::from_report(report) else {
        return Ok(None);
    };
    let level = decision.level;
    let decision_json = serde_json::to_value(&decision)
        .map_err(|err| DenError::System(format!("serialize context budget decision: {err}")))?;
    let row = record_loop_control_decision(
        pool,
        LoopControlLedgerInput {
            run_id: run_id.to_string(),
            turn_step_id,
            conversation_message_id: None,
            decision_id: format!("context_budget:{}:{turn_step_id:?}", level.as_str()),
            decision_kind: LoopControlDecisionKind::ContextBudgetPressure,
            control_level: "standard".to_string(),
            reason: Some(level.as_str().to_string()),
            orientation_kind,
            checkpoint_id: None,
            related_task_list_id: None,
            related_task_item_id: None,
            related_docket_job_id: None,
            related_docket_task_id: None,
            evidence_refs: vec![LedgerEvidenceRef {
                kind: "context_budget_report".to_string(),
                id: format!(
                    "{}:{}:{}",
                    report.model,
                    report.estimated_total_tokens,
                    report.context_window.unwrap_or_default()
                ),
            }],
            decision: decision_json,
        },
    )
    .await?;
    Ok(Some(row))
}

pub async fn record_grounding_probe_result_decision(
    pool: &PgPool,
    input: GroundingProbeResultInput,
) -> Result<LoopControlLedgerRow, DenError> {
    let decision = GroundingProbeDecision {
        probe_id: input.probe_id.clone(),
        surface_kind: input.surface_kind.clone(),
        signal: input.signal,
        duration_ms: input.duration_ms,
        findings: input.findings,
    };
    let decision_json = serde_json::to_value(&decision)
        .map_err(|err| DenError::System(format!("serialize grounding probe decision: {err}")))?;
    let mut evidence_refs = vec![LedgerEvidenceRef {
        kind: "grounding_probe".to_string(),
        id: input.probe_id.clone(),
    }];
    if let Some(tool_call_id) = input.tool_call_id.as_ref() {
        evidence_refs.push(LedgerEvidenceRef {
            kind: "tool_call".to_string(),
            id: tool_call_id.clone(),
        });
    }
    record_loop_control_decision(
        pool,
        LoopControlLedgerInput {
            run_id: input.run_id,
            turn_step_id: input.turn_step_id,
            conversation_message_id: None,
            decision_id: format!("grounding_probe:{}", input.probe_id),
            decision_kind: LoopControlDecisionKind::GroundingProbeResult,
            control_level: "standard".to_string(),
            reason: Some(input.signal.as_str().to_string()),
            orientation_kind: input.orientation_kind,
            checkpoint_id: None,
            related_task_list_id: None,
            related_task_item_id: None,
            related_docket_job_id: None,
            related_docket_task_id: None,
            evidence_refs,
            decision: decision_json,
        },
    )
    .await
}

const TRANSCRIPT_LIKE_DECISION_KEYS: &[&str] = &[
    "content",
    "message_content",
    "prompt",
    "raw_message",
    "transcript",
];
const LOOP_CONTROL_LEDGER_RETENTION: time::Duration = time::Duration::days(30);

fn reject_transcript_like_decision(value: &Value) -> Result<(), DenError> {
    match value {
        Value::Array(values) => {
            for value in values {
                reject_transcript_like_decision(value)?;
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                if TRANSCRIPT_LIKE_DECISION_KEYS.contains(&key.as_str()) {
                    return Err(DenError::ValidationError(format!(
                        "loop-control decision contains transcript-like field: {key}"
                    )));
                }
                reject_transcript_like_decision(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Deletes ledger rows created before `retain_after`, returning the number removed.
///
/// The ledger is replay/tuning telemetry, not canonical conversation history.
pub async fn purge_loop_control_decisions_before(
    pool: &PgPool,
    retain_after: OffsetDateTime,
) -> Result<u64, DenError> {
    let result = sqlx::query!(
        "DELETE FROM bear_loop_control_ledger WHERE created_at < $1",
        retain_after
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn record_loop_control_decision(
    pool: &PgPool,
    input: LoopControlLedgerInput,
) -> Result<LoopControlLedgerRow, DenError> {
    reject_transcript_like_decision(&input.decision)?;
    let retain_after = OffsetDateTime::now_utc() - LOOP_CONTROL_LEDGER_RETENTION;
    // ponytail: this bounded table-wide delete runs on ledger writes; move expiry to a
    // scheduled batch job if ledger write volume makes it materially expensive.
    sqlx::query!(
        "DELETE FROM bear_loop_control_ledger WHERE created_at < $1",
        retain_after
    )
    .execute(pool)
    .await?;
    let evidence_refs = serde_json::to_value(&input.evidence_refs)
        .map_err(|err| DenError::System(format!("serialize loop-control evidence refs: {err}")))?;
    sqlx::query_as!(
        LoopControlLedgerRow,
        r"
        INSERT INTO bear_loop_control_ledger (
            run_id, turn_step_id, conversation_message_id, decision_id, decision_kind, control_level, reason,
            orientation_kind, checkpoint_id, related_task_list_id, related_task_item_id,
            related_docket_job_id, related_docket_task_id, evidence_refs, decision
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        ON CONFLICT (run_id, decision_id) DO UPDATE SET
            turn_step_id = EXCLUDED.turn_step_id,
            conversation_message_id = EXCLUDED.conversation_message_id,
            decision_kind = EXCLUDED.decision_kind,
            control_level = EXCLUDED.control_level,
            reason = EXCLUDED.reason,
            orientation_kind = EXCLUDED.orientation_kind,
            checkpoint_id = EXCLUDED.checkpoint_id,
            related_task_list_id = EXCLUDED.related_task_list_id,
            related_task_item_id = EXCLUDED.related_task_item_id,
            related_docket_job_id = EXCLUDED.related_docket_job_id,
            related_docket_task_id = EXCLUDED.related_docket_task_id,
            evidence_refs = EXCLUDED.evidence_refs,
            decision = EXCLUDED.decision
        RETURNING
            id, run_id, turn_step_id, conversation_message_id, decision_id, decision_kind, control_level, reason,
            orientation_kind, checkpoint_id, related_task_list_id, related_task_item_id,
            related_docket_job_id, related_docket_task_id, evidence_refs, decision, created_at
        ",
        input.run_id,
        input.turn_step_id,
        input.conversation_message_id,
        input.decision_id,
        input.decision_kind.as_str(),
        input.control_level,
        input.reason,
        input.orientation_kind,
        input.checkpoint_id,
        input.related_task_list_id,
        input.related_task_item_id,
        input.related_docket_job_id,
        input.related_docket_task_id,
        evidence_refs,
        input.decision
    )
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_loop_control_decisions_for_run(
    pool: &PgPool,
    run_id: &str,
) -> Result<Vec<LoopControlLedgerRow>, DenError> {
    sqlx::query_as!(
        LoopControlLedgerRow,
        r"
        SELECT
            id, run_id, turn_step_id, conversation_message_id, decision_id, decision_kind, control_level, reason,
            orientation_kind, checkpoint_id, related_task_list_id, related_task_item_id,
            related_docket_job_id, related_docket_task_id, evidence_refs, decision, created_at
        FROM bear_loop_control_ledger
        WHERE run_id = $1
        ORDER BY created_at ASC, decision_id ASC
        ",
        run_id
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Summarizes decisions recorded since `since` across all runs.
///
/// The caller owns the reporting window; this intentionally returns only
/// transcript-free ledger aggregates rather than conversation content.
pub async fn summarize_recent_loop_control_replay_profile(
    pool: &PgPool,
    since: OffsetDateTime,
) -> Result<LoopControlReplayProfileSummary, DenError> {
    let rows = sqlx::query_as!(
        LoopControlLedgerRow,
        r"
        SELECT
            id, run_id, turn_step_id, conversation_message_id, decision_id, decision_kind, control_level, reason,
            orientation_kind, checkpoint_id, related_task_list_id, related_task_item_id,
            related_docket_job_id, related_docket_task_id, evidence_refs, decision, created_at
        FROM bear_loop_control_ledger
        WHERE created_at >= $1
        ORDER BY created_at ASC, run_id ASC, decision_id ASC
        ",
        since
    )
    .fetch_all(pool)
    .await?;
    let turns = aggregate_loop_control_replay_turns(&rows)?;
    Ok(summarize_loop_control_replay_profile(&turns))
}

pub async fn latest_grounding_probe_signal_for_tool_call(
    pool: &PgPool,
    run_id: &str,
    tool_call_id: &str,
) -> Result<Option<GroundingProbeSignalKind>, DenError> {
    let evidence_ref = serde_json::json!([{ "kind": "tool_call", "id": tool_call_id }]);
    let row = sqlx::query!(
        r"
        SELECT reason
        FROM bear_loop_control_ledger
        WHERE run_id = $1
          AND decision_kind = 'grounding_probe_result'
          AND evidence_refs @> $2::jsonb
        ORDER BY created_at DESC, decision_id DESC
        LIMIT 1
        ",
        run_id,
        evidence_ref
    )
    .fetch_optional(pool)
    .await?;

    Ok(row
        .and_then(|row| row.reason)
        .and_then(|reason| GroundingProbeSignalKind::parse(&reason)))
}

pub async fn latest_grounding_probe_signal_for_run(
    pool: &PgPool,
    run_id: &str,
) -> Result<Option<GroundingProbeSignalKind>, DenError> {
    // ponytail: this is a run-level signal because current probe rows are not yet
    // tied to individual tool-call ids; add a tool-call evidence ref if multiple
    // mutation probes can overlap within one continuation.
    let row = sqlx::query!(
        r"
        SELECT reason
        FROM bear_loop_control_ledger
        WHERE run_id = $1
          AND decision_kind = 'grounding_probe_result'
        ORDER BY created_at DESC, decision_id DESC
        LIMIT 1
        ",
        run_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(row
        .and_then(|row| row.reason)
        .and_then(|reason| GroundingProbeSignalKind::parse(&reason)))
}

pub async fn summarize_loop_control_replay_profile_for_run(
    pool: &PgPool,
    run_id: &str,
) -> Result<LoopControlReplayProfileSummary, DenError> {
    let rows = list_loop_control_decisions_for_run(pool, run_id).await?;
    let turns = aggregate_loop_control_replay_turns(&rows)?;
    Ok(summarize_loop_control_replay_profile(&turns))
}

fn checkpoint_request_ledger_input(
    request: &RuntimeCheckpointRequest,
    turn_step_id: Option<Uuid>,
    orientation_kind: Option<String>,
) -> Result<LoopControlLedgerInput, DenError> {
    let task_context = request.task_context.as_ref();
    let related_docket_job_id = task_context
        .and_then(|context| context.docket_job_id.as_deref())
        .and_then(|value| Uuid::parse_str(value).ok());
    let related_docket_task_id = task_context
        .and_then(|context| context.docket_task_id.as_deref())
        .and_then(|value| Uuid::parse_str(value).ok());
    let required_fields = request
        .required_fields
        .iter()
        .map(serde_json::to_value)
        .map(|value| {
            value
                .map_err(|err| DenError::System(format!("serialize checkpoint field: {err}")))
                .and_then(|value| {
                    value.as_str().map(str::to_string).ok_or_else(|| {
                        DenError::System("checkpoint field did not serialize to string".to_string())
                    })
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let decision = serde_json::to_value(CheckpointLedgerDecision {
        checkpoint_id: request.checkpoint_id.clone(),
        reason: request.reason.as_str().to_string(),
        control_level: request.control_level.as_str().to_string(),
        profile_fingerprint: request.profile_fingerprint.clone(),
        active_objective_present: request.active_objective.is_some(),
        required_fields,
        task_refs: CheckpointLedgerTaskRefs {
            task_list_id: task_context.and_then(|context| context.task_list_id.clone()),
            task_list_version: task_context.and_then(|context| context.task_list_version.clone()),
            active_item_id: task_context.and_then(|context| context.active_item_id.clone()),
            docket_job_id: task_context.and_then(|context| context.docket_job_id.clone()),
            docket_task_id: task_context.and_then(|context| context.docket_task_id.clone()),
        },
    })
    .map_err(|err| DenError::System(format!("serialize checkpoint ledger decision: {err}")))?;

    Ok(LoopControlLedgerInput {
        run_id: request.run_id.clone(),
        turn_step_id,
        conversation_message_id: None,
        decision_id: format!("checkpoint:{}", request.checkpoint_id),
        decision_kind: LoopControlDecisionKind::CheckpointRequested,
        control_level: request.control_level.as_str().to_string(),
        reason: Some(request.reason.as_str().to_string()),
        orientation_kind,
        checkpoint_id: Some(request.checkpoint_id.clone()),
        related_task_list_id: task_context.and_then(|context| context.task_list_id.clone()),
        related_task_item_id: task_context.and_then(|context| context.active_item_id.clone()),
        related_docket_job_id,
        related_docket_task_id,
        evidence_refs: request
            .evidence_refs
            .iter()
            .map(|evidence| LedgerEvidenceRef {
                kind: evidence.kind.clone(),
                id: evidence.id.clone(),
            })
            .collect(),
        decision,
    })
}

pub async fn record_checkpoint_response(
    pool: &PgPool,
    input: CheckpointResponseInput,
) -> Result<CheckpointArtifactRow, DenError> {
    let response_json = serde_json::to_value(&input.response)
        .map_err(|err| DenError::System(format!("serialize checkpoint response: {err}")))?;
    let mut tx = pool.begin().await?;
    let checkpoint = record_checkpoint_response_in_tx(&mut tx, &input, response_json).await?;
    record_checkpoint_response_json_artifact_in_tx(&mut tx, &input, &checkpoint).await?;
    tx.commit().await?;
    Ok(checkpoint)
}

async fn record_checkpoint_response_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &CheckpointResponseInput,
    response_json: Value,
) -> Result<CheckpointArtifactRow, DenError> {
    let row = sqlx::query_as!(
        CheckpointArtifactRow,
        r"
        UPDATE bear_run_checkpoints
        SET response = $3,
            validation_status = $4,
            updated_at = NOW()
        WHERE run_id = $1 AND checkpoint_id = $2
        RETURNING
            id, run_id, turn_step_id, checkpoint_id, reason, control_level, request,
            response, validation_status, visibility, replay_policy, related_task_list_id,
            related_task_item_id, related_docket_task_id, related_work_run_id,
            related_docket_job_id, created_at, updated_at
        ",
        input.run_id,
        input.checkpoint_id,
        response_json,
        input.validation_status.as_str()
    )
    .fetch_optional(&mut **tx)
    .await?;

    let checkpoint = row.ok_or_else(|| {
        DenError::NotFound(format!(
            "checkpoint artifact not found: run_id={} checkpoint_id={}",
            input.run_id, input.checkpoint_id
        ))
    })?;
    Ok(checkpoint)
}

async fn record_checkpoint_response_json_artifact_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &CheckpointResponseInput,
    checkpoint: &CheckpointArtifactRow,
) -> Result<(), DenError> {
    let artifact = create_json_artifact_in_tx(
        tx,
        CreateJsonArtifactInput {
            reserve: ReserveArtifactInput {
                bear_id: input.bear_id,
                created_by_user_id: input.created_by_user_id,
                owner_profile: input.owner_profile,
                kind: "runtime_checkpoint".to_string(),
                title: Some("Runtime checkpoint response".to_string()),
                summary: Some(checkpoint.reason.clone()),
                content_type: Some("application/json".to_string()),
                storage_kind: ArtifactStorageKind::DbText,
                visibility: RegistryVisibility::PrivateToProfile,
                provenance: serde_json::json!({"source": "den_runtime"}),
                metadata: serde_json::json!({
                    "excluded_from_transcript": true,
                    "excluded_from_default_replay": true,
                    "checkpoint_artifact_kind": "validated_response",
                }),
                expires_at: None,
            },
            payload: serde_json::json!({
                "checkpoint_id": checkpoint.checkpoint_id,
                "run_id": checkpoint.run_id,
                "validation_status": input.validation_status.as_str(),
                "request": checkpoint.request,
                "response": input.response,
            }),
        },
    )
    .await?;
    for (target_kind, target_id) in [
        (
            "work_run",
            checkpoint.related_work_run_id.map(|id| id.to_string()),
        ),
        (
            "docket_job",
            checkpoint.related_docket_job_id.map(|id| id.to_string()),
        ),
        (
            "docket_task",
            checkpoint.related_docket_task_id.map(|id| id.to_string()),
        ),
    ] {
        if let Some(target_id) = target_id {
            attach_artifact_in_tx(
                tx,
                AttachArtifactInput {
                    artifact_ref: artifact.artifact_ref.clone(),
                    bear_id: input.bear_id,
                    target_kind: target_kind.to_string(),
                    target_id,
                    role: "runtime_checkpoint".to_string(),
                    metadata: serde_json::json!({}),
                    created_by_user_id: input.created_by_user_id,
                },
            )
            .await?;
        }
    }
    Ok(())
}

pub async fn list_checkpoints_for_run(
    pool: &PgPool,
    run_id: &str,
) -> Result<Vec<CheckpointArtifactRow>, DenError> {
    sqlx::query_as!(
        CheckpointArtifactRow,
        r"
        SELECT
            id, run_id, turn_step_id, checkpoint_id, reason, control_level, request,
            response, validation_status, visibility, replay_policy, related_task_list_id,
            related_task_item_id, related_docket_task_id, related_work_run_id,
            related_docket_job_id, created_at, updated_at
        FROM bear_run_checkpoints
        WHERE run_id = $1
        ORDER BY created_at ASC, checkpoint_id ASC
        ",
        run_id
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_checkpoints_for_session(
    pool: &PgPool,
    bear_id: Uuid,
    session_id: &str,
    limit: i64,
) -> Result<Vec<CheckpointArtifactRow>, DenError> {
    sqlx::query_as!(
        CheckpointArtifactRow,
        r"
        SELECT
            c.id, c.run_id, c.turn_step_id, c.checkpoint_id, c.reason, c.control_level,
            c.request, c.response, c.validation_status, c.visibility, c.replay_policy,
            c.related_task_list_id, c.related_task_item_id, c.related_docket_task_id,
            c.related_work_run_id, c.related_docket_job_id, c.created_at, c.updated_at
        FROM bear_run_checkpoints c
        INNER JOIN turn_runs r ON r.run_id = c.run_id
        WHERE r.bear_id = $1 AND r.session_id = $2
        ORDER BY c.created_at DESC, c.checkpoint_id DESC
        LIMIT $3
        ",
        bear_id,
        session_id,
        limit.max(1)
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{
        CheckpointEvidenceRef, CheckpointField, CheckpointNextAction, CheckpointReason,
        CheckpointTaskContext, RuntimeCheckpointResponse,
    };
    use den_core::AgentLoopControlLevel;

    async fn seed_run(pool: &PgPool, run_id: &str) -> (Uuid, i32) {
        let suffix = Uuid::new_v4().simple().to_string();
        let username = format!("ckpt{}", &suffix[..12]);
        let email = format!("{username}@example.test");
        let user_id = sqlx::query_scalar!(
            r"
            INSERT INTO users (email, username, display_name, passhash)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            ",
            email,
            username,
            "Checkpoint Test",
            "test-passhash"
        )
        .fetch_one(pool)
        .await
        .expect("create user");
        let bear_id = Uuid::new_v4();
        sqlx::query!(
            r"
            INSERT INTO bears (id, slug, name)
            VALUES ($1, $2, $3)
            ",
            bear_id,
            format!("checkpoint-bear-{}", &suffix[..12]),
            "Checkpoint Bear"
        )
        .execute(pool)
        .await
        .expect("create bear");
        sqlx::query!(
            r"
            INSERT INTO turn_runs (run_id, session_id, bear_id, user_id, state)
            VALUES ($1, $2, $3, $4, 'running')
            ",
            run_id,
            format!("session-{run_id}"),
            bear_id,
            user_id
        )
        .execute(pool)
        .await
        .expect("create run");
        (bear_id, user_id)
    }

    fn request(
        run_id: &str,
        docket_job_id: Option<Uuid>,
        docket_task_id: Option<Uuid>,
    ) -> RuntimeCheckpointRequest {
        RuntimeCheckpointRequest {
            checkpoint_id: "ckpt-1".to_string(),
            run_id: run_id.to_string(),
            reason: CheckpointReason::OverExploration,
            control_level: AgentLoopControlLevel::Careful,
            profile_fingerprint: Some("profile-test".to_string()),
            active_objective: Some("Find the failing path".to_string()),
            task_context: Some(CheckpointTaskContext {
                task_list_id: Some("list-1".to_string()),
                task_list_version: Some("2".to_string()),
                active_item_id: Some("item-1".to_string()),
                active_item_title: Some("Inspect runtime".to_string()),
                docket_job_id: docket_job_id.map(|id| id.to_string()),
                docket_task_id: docket_task_id.map(|id| id.to_string()),
            }),
            evidence_refs: vec![CheckpointEvidenceRef {
                kind: "tool_result".to_string(),
                id: "call-1".to_string(),
                summary: Some("Read runtime file".to_string()),
            }],
            required_fields: vec![CheckpointField::Learned],
        }
    }

    fn response() -> RuntimeCheckpointResponse {
        RuntimeCheckpointResponse {
            checkpoint_id: "ckpt-1".to_string(),
            active_objective: "Find the failing path".to_string(),
            summary: None,
            learned: vec!["The runtime parser is involved.".to_string()],
            remaining_uncertainty: vec![],
            more_exploration_justified: false,
            next_action: CheckpointNextAction::Validate,
            task_state_change_needed: None,
            evidence_refs: vec![],
            confidence: None,
        }
    }

    #[test]
    fn rejects_transcript_like_decision_fields_recursively() {
        let error = reject_transcript_like_decision(&serde_json::json!({
            "safe": { "raw_message": "do not retain this" }
        }))
        .expect_err("nested transcript-like field must be rejected");
        assert!(error
            .to_string()
            .contains("loop-control decision contains transcript-like field: raw_message"));
        assert!(reject_transcript_like_decision(&serde_json::json!({
            "active_objective_present": true,
            "findings": [{ "code": "diff_present" }]
        }))
        .is_ok());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn automatically_purges_loop_control_decisions_before_retention_cutoff(pool: PgPool) {
        let old_run = format!("run-{}", Uuid::new_v4().simple());
        let recent_run = format!("run-{}", Uuid::new_v4().simple());
        seed_run(&pool, &old_run).await;
        seed_run(&pool, &recent_run).await;
        for (run_id, decision_id) in [(&old_run, "old"), (&recent_run, "recent")] {
            record_loop_control_decision(
                &pool,
                LoopControlLedgerInput {
                    run_id: run_id.clone(),
                    turn_step_id: None,
                    conversation_message_id: None,
                    decision_id: decision_id.to_string(),
                    decision_kind: LoopControlDecisionKind::CheckpointRequested,
                    control_level: "standard".to_string(),
                    reason: None,
                    orientation_kind: None,
                    checkpoint_id: None,
                    related_task_list_id: None,
                    related_task_item_id: None,
                    related_docket_job_id: None,
                    related_docket_task_id: None,
                    evidence_refs: vec![],
                    decision: serde_json::json!({ "active_objective_present": true }),
                },
            )
            .await
            .expect("record ledger decision");
        }
        sqlx::query!(
            "UPDATE bear_loop_control_ledger SET created_at = NOW() - INTERVAL '31 days' WHERE run_id = $1",
            old_run
        )
        .execute(&pool)
        .await
        .expect("age old ledger decision");

        record_loop_control_decision(
            &pool,
            LoopControlLedgerInput {
                run_id: recent_run.clone(),
                turn_step_id: None,
                conversation_message_id: None,
                decision_id: "retention-trigger".to_string(),
                decision_kind: LoopControlDecisionKind::CheckpointRequested,
                control_level: "standard".to_string(),
                reason: None,
                orientation_kind: None,
                checkpoint_id: None,
                related_task_list_id: None,
                related_task_item_id: None,
                related_docket_job_id: None,
                related_docket_task_id: None,
                evidence_refs: vec![],
                decision: serde_json::json!({ "active_objective_present": true }),
            },
        )
        .await
        .expect("record retention trigger");

        assert!(list_loop_control_decisions_for_run(&pool, &old_run)
            .await
            .expect("list old rows")
            .is_empty());
        assert_eq!(
            list_loop_control_decisions_for_run(&pool, &recent_run)
                .await
                .expect("list recent rows")
                .len(),
            2
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn summarizes_only_recent_loop_control_decisions(pool: PgPool) {
        let recent_run = format!("run-{}", Uuid::new_v4().simple());
        let old_run = format!("run-{}", Uuid::new_v4().simple());
        seed_run(&pool, &recent_run).await;
        seed_run(&pool, &old_run).await;

        for (run_id, decision_id, reason) in [
            (&recent_run, "recent", "over_exploration"),
            (&old_run, "old", "same_signature"),
        ] {
            record_loop_control_decision(
                &pool,
                LoopControlLedgerInput {
                    run_id: run_id.clone(),
                    turn_step_id: None,
                    conversation_message_id: None,
                    decision_id: decision_id.to_string(),
                    decision_kind: LoopControlDecisionKind::CheckpointRequested,
                    control_level: "standard".to_string(),
                    reason: Some(reason.to_string()),
                    orientation_kind: Some("task_oriented".to_string()),
                    checkpoint_id: None,
                    related_task_list_id: None,
                    related_task_item_id: None,
                    related_docket_job_id: None,
                    related_docket_task_id: None,
                    evidence_refs: vec![],
                    decision: serde_json::json!({}),
                },
            )
            .await
            .expect("record ledger decision");
        }
        sqlx::query!(
            "UPDATE bear_loop_control_ledger SET created_at = NOW() - INTERVAL '2 days' WHERE run_id = $1",
            old_run
        )
        .execute(&pool)
        .await
        .expect("age old ledger decision");

        let summary = summarize_recent_loop_control_replay_profile(
            &pool,
            OffsetDateTime::now_utc() - time::Duration::days(1),
        )
        .await
        .expect("summarize recent ledger decisions");

        assert_eq!(summary.turn_count, 1);
        assert_eq!(summary.decision_count, 1);
        assert_eq!(
            summary.reason_counts,
            vec![LoopControlReplayCount {
                value: "over_exploration".to_string(),
                count: 1,
            }]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn records_grounding_probe_result_decision(pool: PgPool) {
        let run_id = format!("run-{}", Uuid::new_v4().simple());
        seed_run(&pool, &run_id).await;
        let mut input = non_empty_diff_grounding_probe(&run_id, "diff --git a/file b/file\n");
        input.tool_call_id = Some("call-grounded".to_string());

        let recorded = record_grounding_probe_result_decision(&pool, input)
            .await
            .expect("record grounding probe result");

        assert_eq!(recorded.decision_kind, "grounding_probe_result");
        assert_eq!(
            recorded.decision_id,
            "grounding_probe:generic.non_empty_diff"
        );
        assert_eq!(recorded.reason.as_deref(), Some("pass"));
        assert_eq!(recorded.evidence_refs[0]["kind"], "grounding_probe");
        assert_eq!(recorded.evidence_refs[0]["id"], "generic.non_empty_diff");
        assert_eq!(recorded.evidence_refs[1]["kind"], "tool_call");
        assert_eq!(recorded.evidence_refs[1]["id"], "call-grounded");
        assert_eq!(recorded.decision["surface_kind"], "repository");
        assert_eq!(recorded.decision["signal"], "pass");
        assert_eq!(recorded.decision["findings"][0]["code"], "diff_present");
        assert!(recorded.decision.get("diff").is_none());
        assert_eq!(
            latest_grounding_probe_signal_for_tool_call(&pool, &run_id, "call-grounded")
                .await
                .expect("latest tool grounding signal"),
            Some(GroundingProbeSignalKind::Pass)
        );
        assert_eq!(
            latest_grounding_probe_signal_for_tool_call(&pool, &run_id, "other-call")
                .await
                .expect("missing tool grounding signal"),
            None
        );
        assert_eq!(
            latest_grounding_probe_signal_for_run(&pool, &run_id)
                .await
                .expect("latest grounding signal"),
            Some(GroundingProbeSignalKind::Pass)
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn records_context_budget_pressure_decision(pool: PgPool) {
        let run_id = format!("run-{}", Uuid::new_v4().simple());
        seed_run(&pool, &run_id).await;
        let report = ContextBudgetReport {
            model: "test/model".to_string(),
            context_window: Some(100),
            max_output_tokens: Some(20),
            reserved_output_tokens: 10,
            estimated_input_tokens: 81,
            estimated_total_tokens: 91,
            estimate_precision: den_protocol::ContextBudgetEstimatePrecision::Approximate,
            near_budget: true,
            over_budget: false,
            calibration: None,
            components: vec![],
        };

        let recorded = record_context_budget_pressure_decision(
            &pool,
            &run_id,
            None,
            Some("oriented".to_string()),
            &report,
        )
        .await
        .expect("record context budget pressure")
        .expect("near budget writes ledger row");

        assert_eq!(recorded.decision_kind, "context_budget_pressure");
        assert_eq!(recorded.reason.as_deref(), Some("near_budget"));
        assert_eq!(recorded.orientation_kind.as_deref(), Some("oriented"));
        assert_eq!(recorded.evidence_refs[0]["kind"], "context_budget_report");
        assert_eq!(
            recorded.decision["estimated_total_tokens"],
            serde_json::json!(91)
        );
        assert_eq!(
            recorded.decision["action"],
            context_budget_pressure_action(ContextBudgetPressureLevel::NearBudget)
        );
        assert!(recorded.decision.get("components").is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn replay_turn_aggregate_handles_multi_decision_fixture(pool: PgPool) {
        let run_id = format!("run-{}", Uuid::new_v4().simple());
        seed_run(&pool, &run_id).await;
        let turn_step_id = sqlx::query_scalar!(
            r"
            INSERT INTO turn_steps (run_id, step_index, state)
            VALUES ($1, 1, 'streaming_model')
            RETURNING id
            ",
            run_id
        )
        .fetch_one(&pool)
        .await
        .expect("create turn step");

        record_loop_control_decision(
            &pool,
            LoopControlLedgerInput {
                run_id: run_id.clone(),
                turn_step_id: Some(turn_step_id),
                conversation_message_id: None,
                decision_id: "multi:checkpoint".to_string(),
                decision_kind: LoopControlDecisionKind::CheckpointRequested,
                control_level: "careful".to_string(),
                reason: Some("over_exploration".to_string()),
                orientation_kind: Some("oriented".to_string()),
                checkpoint_id: Some("ckpt-multi".to_string()),
                related_task_list_id: Some("list-multi".to_string()),
                related_task_item_id: Some("item-multi".to_string()),
                related_docket_job_id: None,
                related_docket_task_id: None,
                evidence_refs: vec![LedgerEvidenceRef {
                    kind: "tool_result".to_string(),
                    id: "call-multi".to_string(),
                }],
                decision: serde_json::json!({ "active_objective_present": true }),
            },
        )
        .await
        .expect("record checkpoint decision");
        record_loop_control_decision(
            &pool,
            LoopControlLedgerInput {
                run_id: run_id.clone(),
                turn_step_id: Some(turn_step_id),
                conversation_message_id: None,
                decision_id: "multi:context_budget".to_string(),
                decision_kind: LoopControlDecisionKind::ContextBudgetPressure,
                control_level: "standard".to_string(),
                reason: Some("near_budget".to_string()),
                orientation_kind: Some("oriented".to_string()),
                checkpoint_id: None,
                related_task_list_id: Some("list-multi".to_string()),
                related_task_item_id: Some("item-multi".to_string()),
                related_docket_job_id: None,
                related_docket_task_id: None,
                evidence_refs: vec![LedgerEvidenceRef {
                    kind: "context_budget_report".to_string(),
                    id: "test/model:91:100".to_string(),
                }],
                decision: serde_json::json!({ "level": "near_budget" }),
            },
        )
        .await
        .expect("record context-budget decision");

        let ledger = list_loop_control_decisions_for_run(&pool, &run_id)
            .await
            .expect("list ledger decisions");
        let turns = aggregate_loop_control_replay_turns(&ledger).expect("aggregate replay turns");
        let expected_turns = vec![ExpectedLoopControlReplayTurn {
            run_id: run_id.clone(),
            turn_step_id: Some(turn_step_id),
            decision_count: 2,
            decision_kinds: vec![
                LoopControlDecisionKind::CheckpointRequested,
                LoopControlDecisionKind::ContextBudgetPressure,
            ],
            control_levels: vec!["careful".to_string(), "standard".to_string()],
            reasons: vec!["over_exploration".to_string(), "near_budget".to_string()],
            orientation_kinds: vec!["oriented".to_string()],
            checkpoint_ids: vec!["ckpt-multi".to_string()],
            evidence_refs: vec![
                LedgerEvidenceRef {
                    kind: "tool_result".to_string(),
                    id: "call-multi".to_string(),
                },
                LedgerEvidenceRef {
                    kind: "context_budget_report".to_string(),
                    id: "test/model:91:100".to_string(),
                },
            ],
        }];
        assert!(compare_loop_control_replay_turns(&turns, &expected_turns).is_empty());

        let profile = summarize_loop_control_replay_profile(&turns);
        let loaded_profile = summarize_loop_control_replay_profile_for_run(&pool, &run_id)
            .await
            .expect("summarize replay profile for run");
        assert_eq!(loaded_profile, profile);
        assert_eq!(profile.turn_count, 1);
        assert_eq!(profile.decision_count, 2);
        assert_eq!(
            profile.decision_kind_counts,
            vec![
                LoopControlReplayCount {
                    value: LoopControlDecisionKind::CheckpointRequested,
                    count: 1,
                },
                LoopControlReplayCount {
                    value: LoopControlDecisionKind::ContextBudgetPressure,
                    count: 1,
                },
            ]
        );
        assert_eq!(
            profile.control_level_counts,
            vec![
                LoopControlReplayCount {
                    value: "careful".to_string(),
                    count: 1,
                },
                LoopControlReplayCount {
                    value: "standard".to_string(),
                    count: 1,
                },
            ]
        );
        assert_eq!(
            profile.orientation_kind_counts,
            vec![LoopControlReplayCount {
                value: "oriented".to_string(),
                count: 1,
            }]
        );
        assert_eq!(
            profile.reason_counts,
            vec![
                LoopControlReplayCount {
                    value: "over_exploration".to_string(),
                    count: 1,
                },
                LoopControlReplayCount {
                    value: "near_budget".to_string(),
                    count: 1,
                },
            ]
        );

        let expected_profile = ExpectedLoopControlReplayProfileSummary {
            turn_count: 1,
            decision_count: 2,
            decision_kind_counts: vec![
                LoopControlReplayCount {
                    value: LoopControlDecisionKind::CheckpointRequested,
                    count: 1,
                },
                LoopControlReplayCount {
                    value: LoopControlDecisionKind::ContextBudgetPressure,
                    count: 1,
                },
            ],
            control_level_counts: vec![
                LoopControlReplayCount {
                    value: "careful".to_string(),
                    count: 1,
                },
                LoopControlReplayCount {
                    value: "standard".to_string(),
                    count: 1,
                },
            ],
            orientation_kind_counts: vec![LoopControlReplayCount {
                value: "oriented".to_string(),
                count: 1,
            }],
            reason_counts: vec![
                LoopControlReplayCount {
                    value: "over_exploration".to_string(),
                    count: 1,
                },
                LoopControlReplayCount {
                    value: "near_budget".to_string(),
                    count: 1,
                },
            ],
        };
        assert!(compare_loop_control_replay_profile(&profile, &expected_profile).is_none());
        let mut wrong_profile = expected_profile;
        wrong_profile.decision_count = 1;
        let mismatch = compare_loop_control_replay_profile(&profile, &wrong_profile)
            .expect("profile mismatch");
        assert_eq!(mismatch.observed, profile);
        assert_eq!(mismatch.expected.decision_count, 1);

        let mut wrong_turns = expected_turns;
        wrong_turns[0].reasons = vec!["rule_of_ko".to_string()];
        let mismatches = compare_loop_control_replay_turns(&turns, &wrong_turns);
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].observed.as_ref(), Some(&turns[0]));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn records_checkpoint_request_and_response(pool: PgPool) {
        let run_id = format!("run-{}", Uuid::new_v4().simple());
        let (bear_id, user_id) = seed_run(&pool, &run_id).await;
        let job_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        sqlx::query!(
            r"
            INSERT INTO bear_jobs (id, bear_id, created_by_user_id, created_by_role, goal)
            VALUES ($1, $2, $3, 'pair', 'Checkpoint correlation test')
            ",
            job_id,
            bear_id,
            user_id
        )
        .execute(&pool)
        .await
        .expect("create job");
        sqlx::query!(
            r"
            INSERT INTO bear_tasks (
                id, bear_id, job_id, title, body, created_by_role, created_by_user_id
            )
            VALUES ($1, $2, $3, 'Checkpoint task', 'Prove correlation.', 'pair', $4)
            ",
            task_id,
            bear_id,
            job_id,
            user_id
        )
        .execute(&pool)
        .await
        .expect("create task");

        let job_run_id = Uuid::new_v4();
        sqlx::query!(
            "INSERT INTO bear_job_runs (id, job_id) VALUES ($1, $2)",
            job_run_id,
            job_id
        )
        .execute(&pool)
        .await
        .expect("create job run");
        let work_run_id = Uuid::new_v4();
        sqlx::query!(
            "INSERT INTO bear_work_runs (id, bear_id, job_id, job_run_id) VALUES ($1, $2, $3, $4)",
            work_run_id,
            bear_id,
            job_id,
            job_run_id
        )
        .execute(&pool)
        .await
        .expect("create work run");

        let recorded = record_checkpoint_request(
            &pool,
            CheckpointArtifactInput {
                bear_id,
                created_by_user_id: None,
                owner_profile: RuntimeContextLabel::JobRun,
                run_id: run_id.clone(),
                turn_step_id: None,
                orientation_kind: Some("focused".to_string()),
                audit_context: Some(den_protocol::CheckpointAuditContext {
                    work_run_id,
                    docket_job_id: job_id,
                }),
                request: request(&run_id, Some(job_id), Some(task_id)),
                visibility: CheckpointVisibility::AuditOnly,
                replay_policy: CheckpointReplayPolicy::None,
            },
        )
        .await
        .expect("record request");

        assert_eq!(recorded.run_id, run_id);
        assert_eq!(recorded.checkpoint_id, "ckpt-1");
        assert_eq!(recorded.validation_status, "requested");
        assert_eq!(recorded.visibility, "audit_only");
        assert_eq!(recorded.replay_policy, "none");
        assert_eq!(recorded.related_task_list_id.as_deref(), Some("list-1"));
        assert_eq!(recorded.related_task_item_id.as_deref(), Some("item-1"));
        assert_eq!(recorded.related_docket_task_id, Some(task_id));
        assert_eq!(recorded.related_work_run_id, Some(work_run_id));
        assert_eq!(recorded.related_docket_job_id, Some(job_id));
        assert!(recorded.response.is_none());

        use sqlx::Row;
        let artifact = sqlx::query(
            "SELECT a.lifecycle, a.visibility, a.metadata, p.payload
             FROM artifacts a
             JOIN artifact_json_payloads p ON p.artifact_id = a.id
             WHERE a.bear_id = $1 AND a.kind = 'runtime_checkpoint'",
        )
        .bind(bear_id)
        .fetch_one(&pool)
        .await
        .expect("load checkpoint artifact");
        assert_eq!(
            artifact.try_get::<String, _>("lifecycle").unwrap(),
            "finalized"
        );
        assert_eq!(
            artifact.try_get::<String, _>("visibility").unwrap(),
            "private_to_profile"
        );
        let metadata: Value = artifact.try_get("metadata").unwrap();
        assert_eq!(metadata["excluded_from_transcript"], true);
        assert_eq!(metadata["excluded_from_default_replay"], true);
        let payload: Value = artifact.try_get("payload").unwrap();
        assert_eq!(payload["checkpoint_id"], "ckpt-1");
        assert_eq!(payload["run_id"], run_id);
        assert_eq!(payload["request"]["reason"], "over_exploration");

        let links: i64 = sqlx::query_scalar(
            "SELECT count(*)
             FROM artifact_links l
             JOIN artifacts a ON a.id = l.artifact_id
             WHERE a.bear_id = $1
                AND a.kind = 'runtime_checkpoint'
                AND l.role = 'runtime_checkpoint'",
        )
        .bind(bear_id)
        .fetch_one(&pool)
        .await
        .expect("count checkpoint artifact links");
        assert_eq!(links, 3);

        let updated = record_checkpoint_response(
            &pool,
            CheckpointResponseInput {
                bear_id,
                created_by_user_id: Some(user_id),
                owner_profile: RuntimeContextLabel::JobRun,
                run_id: run_id.clone(),
                checkpoint_id: "ckpt-1".to_string(),
                response: response(),
                validation_status: CheckpointValidationStatus::Valid,
            },
        )
        .await
        .expect("record response");
        assert_eq!(updated.validation_status, "valid");
        assert!(updated.response.is_some());

        let response_artifact = sqlx::query(
            "SELECT p.payload
             FROM artifacts a
             JOIN artifact_json_payloads p ON p.artifact_id = a.id
             WHERE a.bear_id = $1
               AND a.kind = 'runtime_checkpoint'
               AND a.metadata->>'checkpoint_artifact_kind' = 'validated_response'",
        )
        .bind(bear_id)
        .fetch_one(&pool)
        .await
        .expect("load validated checkpoint response artifact");
        let response_payload: Value = response_artifact.try_get("payload").unwrap();
        assert_eq!(response_payload["checkpoint_id"], "ckpt-1");
        assert_eq!(response_payload["validation_status"], "valid");
        assert_eq!(response_payload["request"]["reason"], "over_exploration");
        assert_eq!(response_payload["response"]["next_action"], "validate");

        let links_after_response: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM artifact_links l JOIN artifacts a ON a.id = l.artifact_id
             WHERE a.bear_id = $1 AND a.kind = 'runtime_checkpoint' AND l.role = 'runtime_checkpoint'",
        )
        .bind(bear_id)
        .fetch_one(&pool)
        .await
        .expect("count response artifact links");
        assert_eq!(links_after_response, 6);

        let all = list_checkpoints_for_run(&pool, &run_id)
            .await
            .expect("list checkpoints");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].checkpoint_id, "ckpt-1");
        assert_eq!(all[0].related_work_run_id, Some(work_run_id));
        assert_eq!(all[0].related_docket_job_id, Some(job_id));

        let ledger = list_loop_control_decisions_for_run(&pool, &run_id)
            .await
            .expect("list ledger decisions");
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].decision_id, "checkpoint:ckpt-1");
        assert_eq!(ledger[0].decision_kind, "checkpoint_requested");
        assert_eq!(ledger[0].checkpoint_id.as_deref(), Some("ckpt-1"));
        assert_eq!(ledger[0].orientation_kind.as_deref(), Some("focused"));
        assert_eq!(ledger[0].reason.as_deref(), Some("over_exploration"));
        assert_eq!(ledger[0].related_task_list_id.as_deref(), Some("list-1"));
        assert_eq!(ledger[0].related_task_item_id.as_deref(), Some("item-1"));
        assert_eq!(ledger[0].related_docket_job_id, Some(job_id));
        assert_eq!(ledger[0].related_docket_task_id, Some(task_id));
        assert_eq!(ledger[0].evidence_refs[0]["id"], "call-1");
        assert!(ledger[0].evidence_refs[0].get("summary").is_none());
        assert_eq!(
            ledger[0].decision["active_objective_present"],
            serde_json::json!(true)
        );
        assert_eq!(
            ledger[0].decision["profile_fingerprint"],
            serde_json::json!("profile-test")
        );
        assert!(ledger[0].decision.get("active_objective").is_none());

        let observed = replay_loop_control_observations(&ledger).expect("replay observations");
        assert_eq!(observed.len(), 1);
        let expected = vec![LoopControlReplayObservation {
            run_id: run_id.clone(),
            turn_step_id: None,
            decision_id: "checkpoint:ckpt-1".to_string(),
            decision_kind: LoopControlDecisionKind::CheckpointRequested,
            control_level: "careful".to_string(),
            reason: Some("over_exploration".to_string()),
            orientation_kind: Some("focused".to_string()),
            checkpoint_id: Some("ckpt-1".to_string()),
            related_task_list_id: Some("list-1".to_string()),
            related_task_item_id: Some("item-1".to_string()),
            related_docket_job_id: Some(job_id),
            related_docket_task_id: Some(task_id),
            evidence_refs: vec![LedgerEvidenceRef {
                kind: "tool_result".to_string(),
                id: "call-1".to_string(),
            }],
        }];
        assert!(compare_loop_control_replay(&observed, &expected).is_empty());

        let turns = aggregate_loop_control_replay_turns(&ledger).expect("aggregate replay turns");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].run_id, run_id);
        assert_eq!(turns[0].turn_step_id, None);
        assert_eq!(turns[0].decision_count, 1);
        assert_eq!(turns[0].decision_ids, vec!["checkpoint:ckpt-1"]);
        assert_eq!(
            turns[0].decision_kinds,
            vec![LoopControlDecisionKind::CheckpointRequested]
        );
        assert_eq!(turns[0].control_levels, vec!["careful"]);
        assert_eq!(turns[0].orientation_kinds, vec!["focused"]);
        assert_eq!(turns[0].checkpoint_ids, vec!["ckpt-1"]);
        assert_eq!(turns[0].related_task_list_ids, vec!["list-1"]);
        assert_eq!(turns[0].related_task_item_ids, vec!["item-1"]);
        assert_eq!(
            turns[0].evidence_refs,
            vec![LedgerEvidenceRef {
                kind: "tool_result".to_string(),
                id: "call-1".to_string(),
            }]
        );

        let expected_turns = vec![ExpectedLoopControlReplayTurn {
            run_id: run_id.clone(),
            turn_step_id: None,
            decision_count: 1,
            decision_kinds: vec![LoopControlDecisionKind::CheckpointRequested],
            control_levels: vec!["careful".to_string()],
            reasons: vec!["over_exploration".to_string()],
            orientation_kinds: vec!["focused".to_string()],
            checkpoint_ids: vec!["ckpt-1".to_string()],
            evidence_refs: vec![LedgerEvidenceRef {
                kind: "tool_result".to_string(),
                id: "call-1".to_string(),
            }],
        }];
        assert!(compare_loop_control_replay_turns(&turns, &expected_turns).is_empty());

        let mismatches = compare_loop_control_replay_turns(&turns, &[]);
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].observed.as_ref(), Some(&turns[0]));
        assert_eq!(mismatches[0].expected, None);

        let by_session =
            list_checkpoints_for_session(&pool, bear_id, &format!("session-{run_id}"), 10)
                .await
                .expect("list checkpoints by session");
        assert_eq!(by_session.len(), 1);
        assert_eq!(by_session[0].checkpoint_id, "ckpt-1");
        assert_eq!(by_session[0].related_work_run_id, Some(work_run_id));
        assert_eq!(by_session[0].related_docket_job_id, Some(job_id));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn task_settlement_decision_preserves_pair_run_and_task_correlation(pool: PgPool) {
        let run_id = format!("run-{}", Uuid::new_v4().simple());
        let (bear_id, user_id) = seed_run(&pool, &run_id).await;
        let task_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        let task_id_text = task_id.to_string();
        sqlx::query!(
            "INSERT INTO bear_jobs (id, bear_id, created_by_user_id, created_by_role, goal) VALUES ($1, $2, $3, 'pair', 'Settlement correlation test')",
            job_id,
            bear_id,
            user_id
        )
        .execute(&pool)
        .await
        .expect("create correlation job");
        sqlx::query!(
            "INSERT INTO bear_tasks (id, bear_id, job_id, title, body, created_by_role, created_by_user_id) VALUES ($1, $2, $3, 'Settlement task', 'Prove settlement correlation.', 'pair', $4)",
            task_id,
            bear_id,
            job_id,
            user_id
        )
        .execute(&pool)
        .await
        .expect("create correlation task");

        record_loop_control_decision(
            &pool,
            LoopControlLedgerInput {
                run_id: run_id.clone(),
                turn_step_id: None,
                conversation_message_id: None,
                decision_id: format!("task-settled:{task_id}:done"),
                decision_kind: LoopControlDecisionKind::TaskSettled,
                control_level: "standard".to_string(),
                reason: Some("done".to_string()),
                orientation_kind: Some("task_oriented".to_string()),
                checkpoint_id: None,
                related_task_list_id: Some("session-correlation".to_string()),
                related_task_item_id: Some(task_id_text.clone()),
                related_docket_job_id: None,
                related_docket_task_id: Some(task_id),
                evidence_refs: vec![LedgerEvidenceRef {
                    kind: "task_settlement".to_string(),
                    id: "done".to_string(),
                }],
                decision: serde_json::json!({
                    "action": "settle_task",
                    "status": "done",
                    "outcome_disposition": "completed",
                }),
            },
        )
        .await
        .expect("record settlement decision");

        let ledger = list_loop_control_decisions_for_run(&pool, &run_id)
            .await
            .expect("list settlement decision");
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].decision_kind, "task_settled");
        assert_eq!(ledger[0].related_docket_task_id, Some(task_id));
        assert_eq!(
            ledger[0].related_task_item_id.as_deref(),
            Some(task_id_text.as_str())
        );
        assert_eq!(ledger[0].evidence_refs[0]["kind"], "task_settlement");
        assert_eq!(ledger[0].decision["status"], "done");

        let replay = replay_loop_control_observations(&ledger).expect("replay settlement decision");
        assert_eq!(replay[0].run_id, run_id);
        assert_eq!(
            replay[0].decision_kind,
            LoopControlDecisionKind::TaskSettled
        );
        assert_eq!(replay[0].related_docket_task_id, Some(task_id));
    }
}
