use den_core::{AgentLoopControlLevel, DenError, Governance, ThinkingEffort, TurnExecutionOrigin};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use super::{
    GroundingProbeSignalKind, PostMutationVerificationWindow, ToolBudgetClass,
    ToolCallBudgetLimits, ToolContinuationObservation, TurnBudgetPolicy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLoopControlSource {
    ContextDefault,
    ModelDefault,
    BearOverride,

    TaskEscalation,
    PreRiskEscalation,
    SystemDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointReason {
    OverExploration,
    ConsecutiveFailure,
    SameSignatureNearKo,
    LowBudget,
    PreRiskMutation,
}

impl CheckpointReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OverExploration => "over_exploration",
            Self::ConsecutiveFailure => "consecutive_failure",
            Self::SameSignatureNearKo => "same_signature_near_ko",
            Self::LowBudget => "low_budget",
            Self::PreRiskMutation => "pre_risk_mutation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KoPolicy {
    pub same_signature_warning_threshold: Option<u32>,
    pub max_same_tool_signature_repeats: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointPolicy {
    pub enabled: bool,
    pub exploration_without_mutation_threshold: Option<u32>,
    pub consecutive_failure_threshold: Option<u32>,
    pub same_signature_warning_threshold: Option<u32>,
    pub require_on_low_budget: bool,
    pub require_before_broad_mutation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointThinkingPolicy {
    pub enabled: bool,
    pub checkpoint_turn_effort: Option<ThinkingEffort>,
    pub pre_risk_turn_effort: Option<ThinkingEffort>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLoopControlProfile {
    pub budget: TurnBudgetPolicy,
    pub ko: KoPolicy,
    pub checkpoints: CheckpointPolicy,
    pub thinking: CheckpointThinkingPolicy,
}

pub fn agent_loop_control_profile_fingerprint(
    profile: &AgentLoopControlProfile,
) -> Result<String, DenError> {
    let bytes = serde_json::to_vec(profile)
        .map_err(|err| DenError::System(format!("serialize agent-loop profile: {err}")))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CheckpointState {
    pub read_search_since_mutation: u32,
    pub consecutive_failures: u32,
    pub same_signature_repeat_count: u32,
    pub last_signature: Option<String>,
    pub last_checkpoint_reason: Option<CheckpointReason>,
}

impl CheckpointState {
    /// Open a fresh checkpoint-observation window after the model has responded to a checkpoint.
    ///
    /// This intentionally resets only checkpoint trigger state. It does not replenish the
    /// authoritative turn-budget ledger or bypass rule-of-ko/failure hard stops.
    pub fn reset_after_checkpoint_report(&mut self) {
        self.read_search_since_mutation = 0;
        self.consecutive_failures = 0;
        self.same_signature_repeat_count = 0;
        self.last_signature = None;
        self.last_checkpoint_reason = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointTrigger {
    // Steering pattern reminder: keep reusable checkpoint prompt prose in prompt
    // fragments or named renderers. Loop-control source should choose *when* a
    // fragment applies and pass structured state; avoid scattering human-facing
    // steering literals here because compiled context needs one auditable place
    // to suppress stale or contradictory instructions.
    pub reason: CheckpointReason,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointEvaluation {
    pub next_state: CheckpointState,
    pub trigger: Option<CheckpointTrigger>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointField {
    ActiveObjective,
    Learned,
    RemainingUncertainty,
    MoreExplorationJustified,
    NextAction,
    TaskStateChangeNeeded,
    EvidenceRefs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCheckpointRequest {
    pub checkpoint_id: String,
    pub run_id: String,
    pub reason: CheckpointReason,
    pub control_level: AgentLoopControlLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_fingerprint: Option<String>,
    pub active_objective: Option<String>,
    pub task_context: Option<CheckpointTaskContext>,
    pub evidence_refs: Vec<CheckpointEvidenceRef>,
    pub required_fields: Vec<CheckpointField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointTaskContext {
    pub task_list_id: Option<String>,
    pub task_list_version: Option<String>,
    pub active_item_id: Option<String>,
    pub active_item_title: Option<String>,
    pub docket_job_id: Option<String>,
    pub docket_task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointEvidenceRef {
    pub kind: String,
    pub id: String,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCheckpointResponse {
    pub checkpoint_id: String,
    pub active_objective: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub learned: Vec<String>,
    #[serde(default)]
    pub remaining_uncertainty: Vec<String>,
    pub more_exploration_justified: bool,
    pub next_action: CheckpointNextAction,
    #[serde(default)]
    pub task_state_change_needed: Option<TaskStateChangeIntent>,
    #[serde(default)]
    pub evidence_refs: Vec<CheckpointEvidenceRef>,
    #[serde(default)]
    pub confidence: Option<CheckpointConfidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointNextAction {
    CallTool { tool_name: Option<String> },
    Edit,
    Validate,
    UpdateTaskList,
    SyncTaskList,
    RequestHandoff,
    FinalIfGateAllows,
    StopBlocked,
}

impl<'de> Deserialize<'de> for CheckpointNextAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        checkpoint_next_action_from_value(value).map_err(serde::de::Error::custom)
    }
}

fn checkpoint_next_action_from_value(
    value: serde_json::Value,
) -> Result<CheckpointNextAction, String> {
    match value {
        serde_json::Value::String(value) => checkpoint_next_action_from_str(&value),
        serde_json::Value::Object(mut object) => {
            if let Some(tool_name) = object
                .remove("call_tool")
                .or_else(|| object.remove("tool_name"))
            {
                return Ok(CheckpointNextAction::CallTool {
                    tool_name: tool_name.as_str().map(str::to_string),
                });
            }
            let Some(action) = object
                .remove("action")
                .or_else(|| object.remove("type"))
                .and_then(|value| value.as_str().map(str::to_string))
            else {
                return Err(
                    "next_action object must include action/type or call_tool/tool_name"
                        .to_string(),
                );
            };
            let mut action = checkpoint_next_action_from_str(&action)?;
            if let CheckpointNextAction::CallTool { tool_name } = &mut action {
                if tool_name.is_none() {
                    *tool_name = object
                        .remove("tool_name")
                        .and_then(|value| value.as_str().map(str::to_string));
                }
            }
            Ok(action)
        }
        other => Err(format!(
            "next_action must be a string enum or object, got {other}"
        )),
    }
}

fn checkpoint_next_action_from_str(raw: &str) -> Result<CheckpointNextAction, String> {
    let normalized = raw.trim().to_ascii_lowercase();
    let compact = normalized.replace(['-', ' '], "_");
    match compact.as_str() {
        "call_tool" | "tool" | "use_tool" => Ok(CheckpointNextAction::CallTool { tool_name: None }),
        "edit" => Ok(CheckpointNextAction::Edit),
        "validate" => Ok(CheckpointNextAction::Validate),
        "update_task_list" | "update_current_task_status" => Ok(CheckpointNextAction::UpdateTaskList),
        "sync_task_list" => Ok(CheckpointNextAction::SyncTaskList),
        "request_handoff" | "request_task_list_handoff" => Ok(CheckpointNextAction::RequestHandoff),
        "final_if_gate_allows" => Ok(CheckpointNextAction::FinalIfGateAllows),
        "stop_blocked" => Ok(CheckpointNextAction::StopBlocked),
        _ => classify_natural_language_checkpoint_action(&normalized).ok_or_else(|| {
            format!(
                "unknown next_action `{raw}`; expected one of call_tool, edit, validate, update_task_list, sync_task_list, request_handoff, final_if_gate_allows, stop_blocked"
            )
        }),
    }
}

fn classify_natural_language_checkpoint_action(text: &str) -> Option<CheckpointNextAction> {
    if text.contains("update_task")
        || text.contains("update task")
        || text.contains("task status")
        || text.contains("mark ")
    {
        return Some(CheckpointNextAction::UpdateTaskList);
    }
    if text.contains("sync_task") || text.contains("sync task") {
        return Some(CheckpointNextAction::SyncTaskList);
    }
    if text.contains("handoff") || text.contains("human review") || text.contains("escalat") {
        return Some(CheckpointNextAction::RequestHandoff);
    }
    if text.contains("test")
        || text.contains("validate")
        || text.contains("verify")
        || text.contains("check")
    {
        return Some(CheckpointNextAction::Validate);
    }
    if text.contains("edit")
        || text.contains("patch")
        || text.contains("mutat")
        || text.contains("change")
        || text.contains("introduce")
        || text.contains("implement")
    {
        return Some(CheckpointNextAction::Edit);
    }
    if text.contains("stop") || text.contains("blocked") {
        return Some(CheckpointNextAction::StopBlocked);
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStateChangeIntent {
    pub target_state: String,
    pub reason: String,
    pub evidence_refs: Vec<CheckpointEvidenceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointResponseValidationError {
    CheckpointIdMismatch { expected: String, actual: String },
    MissingRequiredField(CheckpointField),
}

pub const DEFAULT_ORIENTED_MAX_CHILDREN: u8 = 6;
pub const DEFAULT_ORIENTED_MAX_DEPTH: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeformPolicy {
    pub may_define_task: bool,
}

impl FreeformPolicy {
    pub const fn closed() -> Self {
        Self {
            may_define_task: false,
        }
    }

    pub const fn task_definition_permitted() -> Self {
        Self {
            may_define_task: true,
        }
    }
}

impl Default for FreeformPolicy {
    fn default() -> Self {
        Self::closed()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrientedChildTaskPolicy {
    pub max_children: u8,
    pub max_depth_below_oriented_task: u8,
}

impl Default for OrientedChildTaskPolicy {
    fn default() -> Self {
        Self {
            max_children: DEFAULT_ORIENTED_MAX_CHILDREN,
            max_depth_below_oriented_task: DEFAULT_ORIENTED_MAX_DEPTH,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrientationTaskRef {
    TaskListItem {
        task_list_id: String,
        item_id: String,
        title: Option<String>,
    },
    DocketTask {
        job_id: Option<String>,
        task_id: String,
        title: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOrientation {
    pub task_ref: OrientationTaskRef,
    pub child_policy: OrientedChildTaskPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocketExecutionOrientation {
    pub job_id: String,
    pub active_task_ref: Option<OrientationTaskRef>,
    pub mutable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectiveOrientation {
    Freeform { policy: FreeformPolicy },
    Oriented { task: TaskOrientation },
    DocketExecution { job: DocketExecutionOrientation },
}

impl ObjectiveOrientation {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Freeform { .. } => "freeform",
            Self::Oriented { .. } => "oriented",
            Self::DocketExecution { .. } => "docket_execution",
        }
    }
}

pub fn objective_orientation_allowed_for_origin(
    origin: TurnExecutionOrigin,
    objective_orientation: &ObjectiveOrientation,
) -> bool {
    origin.require_ordinary_session().is_ok()
        && (!matches!(origin, TurnExecutionOrigin::AuthorizedWorkRun(_))
            || matches!(
                objective_orientation,
                ObjectiveOrientation::DocketExecution { .. }
            ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveOrientationResolutionInput {
    pub docket_job_id: Option<String>,
    pub docket_execution_mutable: bool,
    pub active_task_ref: Option<OrientationTaskRef>,
    pub freeform_policy: FreeformPolicy,
}

pub fn resolve_objective_orientation(
    input: ObjectiveOrientationResolutionInput,
) -> ObjectiveOrientation {
    if let Some(job_id) = input.docket_job_id {
        return ObjectiveOrientation::DocketExecution {
            job: DocketExecutionOrientation {
                job_id,
                active_task_ref: input.active_task_ref,
                mutable: input.docket_execution_mutable,
            },
        };
    }

    if let Some(task_ref) = input.active_task_ref {
        return ObjectiveOrientation::Oriented {
            task: TaskOrientation {
                task_ref,
                child_policy: OrientedChildTaskPolicy::default(),
            },
        };
    }

    ObjectiveOrientation::Freeform {
        policy: input.freeform_policy,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAgentLoopControl {
    pub level: AgentLoopControlLevel,
    pub source: AgentLoopControlSource,
    pub model_handle: Option<String>,
    pub profile: AgentLoopControlProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLoopControlResolutionInput<'a> {
    pub model_handle: Option<&'a str>,
    pub model_default: Option<AgentLoopControlLevel>,
    pub bear_override: Option<AgentLoopControlLevel>,
    pub task_escalation: Option<AgentLoopControlLevel>,
    pub origin: TurnExecutionOrigin,
    pub governance: Governance,
    pub objective_orientation: Option<&'a ObjectiveOrientation>,
    pub pre_risk: bool,
}

pub fn resolve_agent_loop_control(
    input: AgentLoopControlResolutionInput<'_>,
) -> Result<ResolvedAgentLoopControl, DenError> {
    input.origin.require_ordinary_session()?;
    let context_default = context_agent_loop_control_default(
        input.origin,
        input.governance,
        input.objective_orientation,
    );
    let (mut level, mut source) = if let Some(level) = input.model_default {
        (
            context_default.map_or(level, |default| level.max(default)),
            AgentLoopControlSource::ModelDefault,
        )
    } else if let Some(model_handle) = input.model_handle {
        let model_default =
            den_llm::model_registry::default_agent_loop_control_for_model(model_handle);
        (
            context_default.map_or(model_default, |default| model_default.max(default)),
            AgentLoopControlSource::ModelDefault,
        )
    } else if let Some(context_default) = context_default {
        (context_default, AgentLoopControlSource::ContextDefault)
    } else {
        (
            AgentLoopControlLevel::default(),
            AgentLoopControlSource::SystemDefault,
        )
    };

    if let Some(override_level) = input.bear_override {
        level = override_level;
        source = AgentLoopControlSource::BearOverride;
    }

    if let Some(escalation) = input.task_escalation {
        let escalated = level.max(escalation);
        if escalated != level {
            level = escalated;
            source = AgentLoopControlSource::TaskEscalation;
        }
    }
    if input.pre_risk {
        let escalated = level.max(AgentLoopControlLevel::Strict);
        if escalated != level {
            level = escalated;
            source = AgentLoopControlSource::PreRiskEscalation;
        }
    }

    Ok(ResolvedAgentLoopControl {
        level,
        source,
        model_handle: input.model_handle.map(str::to_string),
        profile: AgentLoopControlProfile::for_level(level),
    })
}

fn context_agent_loop_control_default(
    origin: TurnExecutionOrigin,
    governance: Governance,
    objective_orientation: Option<&ObjectiveOrientation>,
) -> Option<AgentLoopControlLevel> {
    let policy = den_core::EffectivePolicy::compile_for_origin(origin, governance);
    let docket_execution = matches!(
        objective_orientation,
        Some(ObjectiveOrientation::DocketExecution { .. })
    ) && (policy
        .capabilities
        .contains(den_core::BearCapability::ExecuteJob)
        || policy
            .capabilities
            .contains(den_core::BearCapability::ExecuteFocusedTask));
    match origin {
        TurnExecutionOrigin::AuthorizedWorkRun(_) if docket_execution => {
            Some(AgentLoopControlLevel::Careful)
        }
        TurnExecutionOrigin::BrowserTaskSession | TurnExecutionOrigin::ArmatureConversation(_)
            if docket_execution =>
        {
            Some(AgentLoopControlLevel::Careful)
        }
        TurnExecutionOrigin::ChannelConversation
        | TurnExecutionOrigin::BrowserTaskSession
        | TurnExecutionOrigin::ArmatureConversation(_)
            if !matches!(governance, Governance::Observational | Governance::Frozen) =>
        {
            Some(AgentLoopControlLevel::Standard)
        }
        _ => None,
    }
}

pub fn evaluate_checkpoint_trigger(
    profile: &AgentLoopControlProfile,
    prior_state: &CheckpointState,
    observations: &[ToolContinuationObservation],
    low_budget: bool,
) -> CheckpointEvaluation {
    let mut next_state = prior_state.clone();
    for observation in observations {
        observe_checkpoint_tool_result(&mut next_state, observation);
    }

    let trigger = if !profile.checkpoints.enabled {
        None
    } else if profile.checkpoints.require_on_low_budget && low_budget {
        Some(checkpoint_trigger(
            CheckpointReason::LowBudget,
            "Loop checkpoint: remaining budget is low; synthesize what is known before continuing.",
        ))
    } else if threshold_reached(
        next_state.consecutive_failures,
        profile.checkpoints.consecutive_failure_threshold,
    ) {
        Some(checkpoint_trigger(
            CheckpointReason::ConsecutiveFailure,
            "Loop checkpoint: recent tool calls are failing; identify the failure pattern and choose a different recovery action.",
        ))
    } else if threshold_reached(
        next_state.same_signature_repeat_count,
        profile.checkpoints.same_signature_warning_threshold,
    ) {
        Some(checkpoint_trigger(
            CheckpointReason::SameSignatureNearKo,
            "Loop checkpoint: the same tool-call signature is repeating; choose a different action or explain why task state should change.",
        ))
    } else if threshold_reached(
        next_state.read_search_since_mutation,
        profile.checkpoints.exploration_without_mutation_threshold,
    ) {
        Some(checkpoint_trigger(
            CheckpointReason::OverExploration,
            "Loop checkpoint: enough exploration has happened without a meaningful mutation; summarize evidence and choose the next action.",
        ))
    } else {
        None
    };

    next_state.last_checkpoint_reason = trigger.as_ref().map(|trigger| trigger.reason);

    CheckpointEvaluation {
        next_state,
        trigger,
    }
}

pub fn validate_checkpoint_response(
    request: &RuntimeCheckpointRequest,
    response: &RuntimeCheckpointResponse,
) -> Result<(), CheckpointResponseValidationError> {
    if response.checkpoint_id != request.checkpoint_id {
        return Err(CheckpointResponseValidationError::CheckpointIdMismatch {
            expected: request.checkpoint_id.clone(),
            actual: response.checkpoint_id.clone(),
        });
    }

    for field in &request.required_fields {
        if checkpoint_field_missing(*field, response) {
            return Err(CheckpointResponseValidationError::MissingRequiredField(
                *field,
            ));
        }
    }

    Ok(())
}

fn checkpoint_field_missing(field: CheckpointField, response: &RuntimeCheckpointResponse) -> bool {
    match field {
        CheckpointField::ActiveObjective => response.active_objective.trim().is_empty(),
        CheckpointField::Learned => {
            response.learned.is_empty()
                && response
                    .summary
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
        }
        CheckpointField::RemainingUncertainty => response.remaining_uncertainty.is_empty(),
        CheckpointField::MoreExplorationJustified => false,
        CheckpointField::NextAction => false,
        CheckpointField::TaskStateChangeNeeded => response.task_state_change_needed.is_none(),
        CheckpointField::EvidenceRefs => response.evidence_refs.is_empty(),
    }
}

pub fn pre_risk_checkpoint_trigger(profile: &AgentLoopControlProfile) -> Option<CheckpointTrigger> {
    profile
        .checkpoints
        .require_before_broad_mutation
        .then(|| checkpoint_trigger(
            CheckpointReason::PreRiskMutation,
            "Loop checkpoint: before a broad or risky mutation, state the evidence, intended change, and validation plan.",
        ))
}

fn observe_checkpoint_tool_result(
    state: &mut CheckpointState,
    observation: &ToolContinuationObservation,
) {
    if observation.failed {
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    } else {
        state.consecutive_failures = 0;
    }

    if state.last_signature.as_deref() == Some(observation.signature.as_str()) {
        state.same_signature_repeat_count = state.same_signature_repeat_count.saturating_add(1);
    } else {
        state.last_signature = Some(observation.signature.clone());
        state.same_signature_repeat_count = 0;
    }

    if observation_is_meaningful_mutation(observation) {
        state.read_search_since_mutation = 0;
    } else if matches!(
        observation.class,
        ToolBudgetClass::Read | ToolBudgetClass::Search
    ) {
        state.read_search_since_mutation = state.read_search_since_mutation.saturating_add(1);
    }
}

fn observation_is_meaningful_mutation(observation: &ToolContinuationObservation) -> bool {
    !observation.failed
        && observation.grounding_probe_signal != Some(GroundingProbeSignalKind::Fail)
        && matches!(
            observation.class,
            ToolBudgetClass::Write | ToolBudgetClass::Destructive
        )
}

fn threshold_reached(count: u32, threshold: Option<u32>) -> bool {
    threshold.is_some_and(|threshold| threshold > 0 && count >= threshold)
}

fn checkpoint_trigger(reason: CheckpointReason, message: &str) -> CheckpointTrigger {
    CheckpointTrigger {
        reason,
        message: message.to_string(),
    }
}

impl AgentLoopControlProfile {
    pub fn for_level(level: AgentLoopControlLevel) -> Self {
        match level {
            AgentLoopControlLevel::Light => Self::new(
                level,
                budget(
                    240_000,
                    96,
                    limits(128, 48, 32, 16, 20, 28, 3, 24),
                    3,
                    2,
                    Some((6, 3)),
                ),
                CheckpointPolicy {
                    enabled: true,
                    exploration_without_mutation_threshold: Some(12),
                    consecutive_failure_threshold: Some(2),
                    same_signature_warning_threshold: Some(2),
                    require_on_low_budget: true,
                    require_before_broad_mutation: false,
                },
                CheckpointThinkingPolicy {
                    enabled: false,
                    checkpoint_turn_effort: None,
                    pre_risk_turn_effort: None,
                },
            ),
            AgentLoopControlLevel::Standard => Self::new(
                level,
                budget(
                    360_000,
                    80,
                    limits(112, 32, 20, 12, 16, 24, 2, 16),
                    3,
                    2,
                    Some((4, 2)),
                ),
                CheckpointPolicy {
                    enabled: true,
                    exploration_without_mutation_threshold: Some(10),
                    consecutive_failure_threshold: Some(2),
                    same_signature_warning_threshold: Some(2),
                    require_on_low_budget: true,
                    require_before_broad_mutation: false,
                },
                CheckpointThinkingPolicy {
                    enabled: true,
                    checkpoint_turn_effort: Some(ThinkingEffort::Medium),
                    pre_risk_turn_effort: None,
                },
            ),
            AgentLoopControlLevel::Careful => Self::new(
                level,
                budget(
                    360_000,
                    80,
                    limits(96, 24, 16, 10, 14, 20, 2, 14),
                    2,
                    2,
                    Some((4, 2)),
                ),
                CheckpointPolicy {
                    enabled: true,
                    exploration_without_mutation_threshold: Some(3),
                    consecutive_failure_threshold: Some(1),
                    same_signature_warning_threshold: Some(1),
                    require_on_low_budget: true,
                    require_before_broad_mutation: true,
                },
                CheckpointThinkingPolicy {
                    enabled: true,
                    checkpoint_turn_effort: Some(ThinkingEffort::High),
                    pre_risk_turn_effort: Some(ThinkingEffort::High),
                },
            ),
            AgentLoopControlLevel::Strict => Self::new(
                level,
                budget(
                    240_000,
                    64,
                    limits(72, 16, 12, 8, 10, 14, 1, 10),
                    1,
                    1,
                    Some((2, 1)),
                ),
                CheckpointPolicy {
                    enabled: true,
                    exploration_without_mutation_threshold: Some(2),
                    consecutive_failure_threshold: Some(1),
                    same_signature_warning_threshold: Some(1),
                    require_on_low_budget: true,
                    require_before_broad_mutation: true,
                },
                CheckpointThinkingPolicy {
                    enabled: true,
                    checkpoint_turn_effort: Some(ThinkingEffort::High),
                    pre_risk_turn_effort: Some(ThinkingEffort::High),
                },
            ),
        }
    }

    pub fn with_budget(mut self, budget: TurnBudgetPolicy) -> Self {
        self.budget = budget;
        self.ko.max_same_tool_signature_repeats = budget.max_same_tool_signature_repeats;
        self
    }

    /// Apply a model-specific multiplier only to ordinary tool capacity.
    ///
    /// The resolved profile remains authoritative for wall-clock, hard-step,
    /// failure, and KO limits; those are safety boundaries, not throughput knobs.
    pub fn with_tool_budget_multiplier(mut self, multiplier: f64) -> Self {
        if !multiplier.is_finite() || (multiplier - 1.0).abs() < f64::EPSILON {
            return self;
        }
        self.budget.tool_call_limits = ToolCallBudgetLimits {
            total: scaled_count(self.budget.tool_call_limits.total, multiplier),
            read: scaled_count(self.budget.tool_call_limits.read, multiplier),
            search: scaled_count(self.budget.tool_call_limits.search, multiplier),
            fetch: scaled_count(self.budget.tool_call_limits.fetch, multiplier),
            execute: scaled_count(self.budget.tool_call_limits.execute, multiplier),
            write: scaled_count(self.budget.tool_call_limits.write, multiplier),
            destructive: scaled_count(self.budget.tool_call_limits.destructive, multiplier),
            other: scaled_count(self.budget.tool_call_limits.other, multiplier),
        };
        if let Some(window) = self.budget.post_mutation_verification_window.as_mut() {
            window.replenish_read = scaled_count(window.replenish_read, multiplier);
            window.replenish_search = scaled_count(window.replenish_search, multiplier);
        }
        self
    }

    fn new(
        _level: AgentLoopControlLevel,
        budget: TurnBudgetPolicy,
        checkpoints: CheckpointPolicy,
        thinking: CheckpointThinkingPolicy,
    ) -> Self {
        Self {
            budget,
            ko: KoPolicy {
                same_signature_warning_threshold: checkpoints.same_signature_warning_threshold,
                max_same_tool_signature_repeats: budget.max_same_tool_signature_repeats,
            },
            checkpoints,
            thinking,
        }
    }
}

fn scaled_count(value: u32, multiplier: f64) -> u32 {
    (f64::from(value) * multiplier)
        .ceil()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

fn limits(
    total: u32,
    read: u32,
    search: u32,
    fetch: u32,
    execute: u32,
    write: u32,
    destructive: u32,
    other: u32,
) -> ToolCallBudgetLimits {
    ToolCallBudgetLimits {
        total,
        read,
        search,
        fetch,
        execute,
        write,
        destructive,
        other,
    }
}

fn budget(
    max_wall_clock_ms: u64,
    emergency_hard_steps: u32,
    tool_call_limits: ToolCallBudgetLimits,
    max_consecutive_tool_failures: u32,
    max_same_tool_signature_repeats: u32,
    post_mutation_verification: Option<(u32, u32)>,
) -> TurnBudgetPolicy {
    TurnBudgetPolicy {
        max_wall_clock_ms,
        emergency_hard_steps,
        tool_call_limits,
        max_consecutive_tool_failures,
        max_same_tool_signature_repeats,
        post_mutation_verification_window: post_mutation_verification.map(
            |(replenish_read, replenish_search)| PostMutationVerificationWindow {
                replenish_read,
                replenish_search,
            },
        ),
    }
}

#[cfg(test)]
mod tests;
