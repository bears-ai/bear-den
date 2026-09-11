use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Accepted,
    Running,
    WaitingForClient,
    Continuing,
    Completed,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn terminal_outcome(self) -> Option<RunTerminalOutcome> {
        match self {
            Self::Completed => Some(RunTerminalOutcome::Completed),
            Self::Failed => Some(RunTerminalOutcome::Failed),
            Self::Cancelled => Some(RunTerminalOutcome::Cancelled),
            Self::Accepted | Self::Running | Self::WaitingForClient | Self::Continuing => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTerminalOutcome {
    Completed,
    Failed,
    Cancelled,
}

impl RunTerminalOutcome {
    pub const fn event_type(self) -> &'static str {
        match self {
            Self::Completed => "run.completed",
            Self::Failed => "run.failed",
            Self::Cancelled => "run.cancelled",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_error(self) -> bool {
        matches!(self, Self::Failed)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLaunchState {
    Queued,
    Claimed,
    Started,
    AlreadyRunning,
}

impl RunLaunchState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Started => "started",
            Self::AlreadyRunning => "already_running",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerDisposition {
    NotApplicable,
    Queued,
    Claimed,
    Live,
    Missing,
    Recovering,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusedExecutionInvariantViolation {
    RunWithoutSelection,
    AttemptWithoutRun,
    ActiveRunWithoutAttempt,
    HostMismatch,
    TerminalRunWithLiveAttemptOrOpenObligations,
    RunningWithoutController,
    ControllerWithoutDurableAuthority,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum FocusedExecutionState {
    Unfocused,
    Selected,
    Starting,
    Running,
    WaitingForClient,
    Continuing,
    Recovering,
    Terminal,
    Inconsistent {
        violation: FocusedExecutionInvariantViolation,
    },
}

impl FocusedExecutionState {
    pub const fn has_active_authority(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::WaitingForClient | Self::Continuing
        )
    }
}

pub const FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE: &str = "diagnostic.state_transition";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusedExecutionTransitionReason {
    AuthorityClaimed,
    AuthorityStarted,
    FocusAcquired,
    ClientWaitOpened,
    ClientWaitCleared,
    RunStateChanged,
    RunCompleted,
    RunFailed,
    RunCancelled,
    TaskSettled,
    SteeringInterrupted,
    Reconciled,
    OrphanedControllerReconciled,
    StaleSessionAuthorityReleased,
    StaleForeignAuthorityReleased,
}

impl FocusedExecutionTransitionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorityClaimed => "authority_claimed",
            Self::AuthorityStarted => "authority_started",
            Self::FocusAcquired => "focus_acquired",
            Self::ClientWaitOpened => "client_wait_opened",
            Self::ClientWaitCleared => "client_wait_cleared",
            Self::RunStateChanged => "run_state_changed",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
            Self::RunCancelled => "run_cancelled",
            Self::TaskSettled => "task_settled",
            Self::SteeringInterrupted => "steering_interrupted",
            Self::Reconciled => "reconciled",
            Self::OrphanedControllerReconciled => "orphaned_controller_reconciled",
            Self::StaleSessionAuthorityReleased => "stale_session_authority_released",
            Self::StaleForeignAuthorityReleased => "stale_foreign_authority_released",
        }
    }
}

/// Append-only diagnostic projection of a canonical focused-execution transition.
///
/// `state_version` orders this aggregate's diagnostic history; it is assigned by
/// Den while holding the session event lock. The referenced task, run, and attempt
/// remain the authorities for their respective domains.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FocusedExecutionTransition {
    pub state_version: u64,
    #[serde(default)]
    pub from: Option<FocusedExecutionState>,
    pub to: FocusedExecutionState,
    pub reason: FocusedExecutionTransitionReason,
    pub correlation_id: String,
    #[serde(default)]
    pub causation_id: Option<String>,
    pub session_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub attempt_id: Option<String>,
    #[serde(default)]
    pub fence_epoch: Option<i64>,
    pub open_obligations: u32,
    pub task_selection_preserved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FocusedExecutionTransitionRecord {
    pub sequence: u64,
    pub time: String,
    pub transition: FocusedExecutionTransition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FocusedExecutionDiagnostics {
    pub snapshot: FocusedExecutionProjection,
    pub transitions: Vec<FocusedExecutionTransitionRecord>,
    pub history_truncated: bool,
    pub version_gap: bool,
    pub snapshot_matches_latest_transition: bool,
    pub reason_counts: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunLaunchProjection {
    pub run_id: String,
    pub state: Option<RunState>,
    pub launch_state: Option<RunLaunchState>,
}

impl RunLaunchProjection {
    pub fn decode(value: &Value) -> Result<Self, String> {
        let run_id = value
            .get("run_id")
            .or_else(|| value.pointer("/pair_binding/run/id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|run_id| !run_id.is_empty())
            .ok_or_else(|| "run launch projection omitted run_id".to_string())?;
        let state = value
            .get("state")
            .or_else(|| value.pointer("/pair_binding/run/state"))
            .or_else(|| value.pointer("/pair_binding/control/state"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| format!("invalid run launch state: {error}"))?;
        let launch_state = value
            .get("launch_state")
            .or_else(|| value.pointer("/pair_binding/control/launch_state"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| format!("invalid execution launch state: {error}"))?;
        Ok(Self {
            run_id: run_id.to_string(),
            state,
            launch_state,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBindingKind {
    ClientSession,
    WorkAssignment,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionBinding {
    pub kind: ExecutionBindingKind,
    pub id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ExecutionHostKind {
    #[serde(rename = "pair")]
    TurnRun,
    #[serde(rename = "work")]
    WorkRun,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionHost {
    pub kind: ExecutionHostKind,
    pub run_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAttemptState {
    Authorized,
    Running,
    Paused,
    AwaitingUser,
    Stopping,
    Settled,
    Released,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FocusedExecutionTask {
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FocusedExecutionRun {
    pub id: String,
    pub state: RunState,
    #[serde(default)]
    pub terminal_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FocusedExecutionAttempt {
    pub id: String,
    pub state: ExecutionAttemptState,
    pub fence_epoch: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FocusedExecutionObligations {
    pub open: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FocusedExecutionProjection {
    pub session_id: String,
    pub state: FocusedExecutionState,
    #[serde(default)]
    pub task: Option<FocusedExecutionTask>,
    #[serde(default)]
    pub binding: Option<ExecutionBinding>,
    #[serde(default)]
    pub run: Option<FocusedExecutionRun>,
    #[serde(default)]
    pub attempt: Option<FocusedExecutionAttempt>,
    #[serde(default)]
    pub host: Option<ExecutionHost>,
    pub controller: ControllerDisposition,
    #[serde(default)]
    pub obligations: Option<FocusedExecutionObligations>,
    pub launch_state: RunLaunchState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunRecoveryHandoff {
    pub run_id: String,
    pub replacement_run_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub reason: String,
    #[serde(default)]
    pub launch_state: Option<RunLaunchState>,
    pub task_selection_preserved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub run_id: String,
    pub session_id: String,
    pub state: RunState,
    #[serde(default)]
    pub terminal_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunStateEvent {
    event: Value,
}

impl RunStateEvent {
    pub fn event(&self) -> &Value {
        &self.event
    }

    pub fn event_type(&self) -> Option<&str> {
        self.event.get("type").and_then(Value::as_str)
    }
}

impl<'de> Deserialize<'de> for RunStateEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let event = value
            .get("event")
            .filter(|event| event.is_object())
            .cloned()
            .unwrap_or(value);
        if !event.is_object() {
            return Err(serde::de::Error::custom(
                "run state event must be an event object or { event } envelope",
            ));
        }
        Ok(Self { event })
    }
}

impl Serialize for RunStateEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.event.serialize(serializer)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationKind {
    ToolResult,
    PermissionDecision,
    HumanInput,
    ResourceBinding,
    HandoffDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedResponderAction {
    ToolResult,
    PermissionDecision,
    HumanInput,
    ResourceBinding,
    HandoffDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationState {
    Requested,
    WaitingForClient,
    ResultReceived,
    Continued,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunObligation {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub kind: Option<ObligationKind>,
    #[serde(default)]
    pub expected_responder_action: Option<ExpectedResponderAction>,
    #[serde(default)]
    pub state: Option<ObligationState>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub permission_id: Option<String>,
    #[serde(default)]
    pub turn_step_id: Option<String>,
    #[serde(default)]
    pub request_payload: Value,
    #[serde(default)]
    pub result_payload: Value,
    #[serde(flatten)]
    pub extensions: std::collections::BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunStateProjection {
    pub run: RunSnapshot,
    #[serde(default)]
    pub blocking_reason: Option<String>,
    #[serde(default)]
    pub open_obligations: Vec<RunObligation>,
    #[serde(default)]
    pub obligations: Vec<RunObligation>,
    #[serde(default)]
    pub recent_events: Vec<RunStateEvent>,
}

impl RunStateProjection {
    pub fn terminal_outcome(&self) -> Option<RunTerminalOutcome> {
        self.run.state.terminal_outcome()
    }

    pub fn matching_terminal_event(&self) -> Option<&RunStateEvent> {
        let event_type = self.terminal_outcome()?.event_type();
        self.recent_events
            .iter()
            .rev()
            .find(|event| event.event_type() == Some(event_type))
    }

    pub fn latest_terminal_event(&self) -> Option<&RunStateEvent> {
        self.recent_events.iter().rev().find(|event| {
            matches!(
                event.event_type(),
                Some("run.completed" | "run.failed" | "run.cancelled" | "run.interrupted")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_projection_normalizes_event_envelopes_and_terminal_outcomes() {
        let projection: RunStateProjection = serde_json::from_value(serde_json::json!({
            "run": {
                "run_id": "run-1",
                "session_id": "session-1",
                "state": "failed",
                "terminal_reason": "stream_error",
                "extra_server_field": true
            },
            "open_obligations": [],
            "recent_events": [
                { "event": { "type": "run.started", "run_id": "run-1" } },
                { "type": "run.failed", "run_id": "run-1", "data": { "reason": "stream_error" } }
            ],
            "extra_projection_field": true
        }))
        .unwrap();

        assert_eq!(projection.run.state, RunState::Failed);
        assert_eq!(
            projection.terminal_outcome(),
            Some(RunTerminalOutcome::Failed)
        );
        assert_eq!(
            projection
                .matching_terminal_event()
                .and_then(RunStateEvent::event_type),
            Some("run.failed")
        );
        assert_eq!(
            projection.recent_events[0].event_type(),
            Some("run.started")
        );

        let top_level = RunLaunchProjection::decode(&serde_json::json!({
            "run_id": "run-top",
            "state": "accepted",
            "launch_state": "claimed"
        }))
        .unwrap();
        assert_eq!(top_level.run_id, "run-top");
        assert_eq!(top_level.state, Some(RunState::Accepted));
        assert_eq!(top_level.launch_state, Some(RunLaunchState::Claimed));

        let nested = RunLaunchProjection::decode(&serde_json::json!({
            "pair_binding": {
                "run": { "id": "run-nested", "state": "running" },
                "control": { "launch_state": "started" }
            }
        }))
        .unwrap();
        assert_eq!(nested.run_id, "run-nested");
        assert_eq!(nested.state, Some(RunState::Running));
        assert_eq!(nested.launch_state, Some(RunLaunchState::Started));

        let focused: FocusedExecutionProjection = serde_json::from_value(serde_json::json!({
            "session_id": "session-1",
            "state": { "phase": "starting" },
            "task": { "id": "task-1" },
            "binding": { "kind": "client_session", "id": "session-1" },
            "run": { "id": "run-1", "state": "accepted" },
            "attempt": { "id": "attempt-1", "state": "authorized", "fence_epoch": 3 },
            "host": { "kind": "pair", "run_id": "run-1" },
            "controller": "claimed",
            "obligations": { "open": 0 },
            "launch_state": "claimed"
        }))
        .unwrap();
        assert_eq!(focused.controller, ControllerDisposition::Claimed);
        assert_eq!(
            focused.attempt.as_ref().map(|attempt| attempt.state),
            Some(ExecutionAttemptState::Authorized)
        );
        assert_eq!(
            focused.host.as_ref().map(|host| host.kind),
            Some(ExecutionHostKind::TurnRun)
        );

        let transition: FocusedExecutionTransition = serde_json::from_value(serde_json::json!({
            "state_version": 2,
            "from": { "phase": "starting" },
            "to": { "phase": "running" },
            "reason": "authority_started",
            "correlation_id": "run-1",
            "causation_id": "call-1",
            "session_id": "session-1",
            "task_id": "task-1",
            "run_id": "run-1",
            "attempt_id": "attempt-1",
            "fence_epoch": 3,
            "open_obligations": 0,
            "task_selection_preserved": true
        }))
        .unwrap();
        assert_eq!(transition.state_version, 2);
        assert_eq!(transition.from, Some(FocusedExecutionState::Starting));
        assert_eq!(transition.to, FocusedExecutionState::Running);
        assert_eq!(
            transition.reason,
            FocusedExecutionTransitionReason::AuthorityStarted
        );
    }
}
