use den_docket::{
    DocketExecutionAttemptState, DocketExecutionBindingKind, DocketExecutionHost,
    DocketExecutionHostKind, DocketFocusedExecutionBinding,
};
use den_http::errors::CustomError;
use den_runtime::{
    turn_ids::{ClientSessionId, TurnRunId},
    turn_runs::TurnRunState,
};
use serde::Serialize;
use uuid::Uuid;

use super::FocusedExecutionLaunchState;

mod projection;

pub use projection::load_focused_execution_snapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerDisposition {
    NotApplicable,
    Queued,
    Claimed,
    Live,
    Missing,
    Recovering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    pub fn has_active_authority(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::WaitingForClient | Self::Continuing
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FocusedExecutionTask {
    pub id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FocusedExecutionRun {
    pub id: TurnRunId,
    pub state: TurnRunState,
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FocusedExecutionAttempt {
    pub id: Uuid,
    pub state: DocketExecutionAttemptState,
    pub fence_epoch: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FocusedExecutionObligations {
    pub open: u32,
}

/// One authoritative projection of selected-task execution for a client session.
///
/// Every field is either durable state or a typed observation of the process-local
/// controller registry. `state` is derived exclusively by `reduce_focused_execution`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FocusedExecutionSnapshot {
    pub session_id: ClientSessionId,
    pub state: FocusedExecutionState,
    pub task: Option<FocusedExecutionTask>,
    pub binding: Option<DocketFocusedExecutionBinding>,
    pub run: Option<FocusedExecutionRun>,
    pub attempt: Option<FocusedExecutionAttempt>,
    pub host: Option<DocketExecutionHost>,
    pub controller: ControllerDisposition,
    pub obligations: Option<FocusedExecutionObligations>,
    pub launch_state: FocusedExecutionLaunchState,
}

impl FocusedExecutionSnapshot {
    pub fn task_id(&self) -> Option<Uuid> {
        self.task.as_ref().map(|task| task.id)
    }

    pub fn run_id(&self) -> Option<&TurnRunId> {
        self.run.as_ref().map(|run| &run.id)
    }

    pub fn attempt_id(&self) -> Option<Uuid> {
        self.attempt.as_ref().map(|attempt| attempt.id)
    }

    pub fn fence_epoch(&self) -> Option<i64> {
        self.attempt.as_ref().map(|attempt| attempt.fence_epoch)
    }

    pub fn require_active_authority(&self) -> Result<(), CustomError> {
        if self.state.has_active_authority() {
            return Ok(());
        }
        Err(CustomError::System(format!(
            "focused execution did not establish active authority: {:?}",
            self.state
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FocusedExecutionFacts {
    pub session_id: ClientSessionId,
    pub task_id: Option<Uuid>,
    pub binding: Option<DocketFocusedExecutionBinding>,
    pub run: Option<FocusedExecutionRun>,
    pub attempt: Option<FocusedExecutionAttempt>,
    pub attempt_task_id: Option<Uuid>,
    pub host: Option<DocketExecutionHost>,
    pub controller: ControllerDisposition,
    pub open_obligations: u32,
}

pub(super) fn reduce_focused_execution(
    facts: FocusedExecutionFacts,
    launch_state: FocusedExecutionLaunchState,
) -> FocusedExecutionSnapshot {
    let state = reduce_state(&facts);
    let obligations = facts.run.as_ref().map(|_| FocusedExecutionObligations {
        open: facts.open_obligations,
    });
    FocusedExecutionSnapshot {
        session_id: facts.session_id,
        state,
        task: facts.task_id.map(|id| FocusedExecutionTask { id }),
        binding: facts.binding,
        run: facts.run,
        attempt: facts.attempt,
        host: facts.host,
        controller: facts.controller,
        obligations,
        launch_state,
    }
}

fn inconsistent(violation: FocusedExecutionInvariantViolation) -> FocusedExecutionState {
    FocusedExecutionState::Inconsistent { violation }
}

fn reduce_state(facts: &FocusedExecutionFacts) -> FocusedExecutionState {
    if facts.task_id.is_none() {
        return if facts.run.is_some() {
            inconsistent(FocusedExecutionInvariantViolation::RunWithoutSelection)
        } else if facts.attempt.is_some() {
            inconsistent(FocusedExecutionInvariantViolation::AttemptWithoutRun)
        } else if facts.controller != ControllerDisposition::NotApplicable {
            inconsistent(FocusedExecutionInvariantViolation::ControllerWithoutDurableAuthority)
        } else {
            FocusedExecutionState::Unfocused
        };
    }

    let Some(run) = facts.run.as_ref() else {
        return if facts.attempt.is_some() {
            inconsistent(FocusedExecutionInvariantViolation::AttemptWithoutRun)
        } else if facts.controller != ControllerDisposition::NotApplicable {
            inconsistent(FocusedExecutionInvariantViolation::ControllerWithoutDurableAuthority)
        } else {
            FocusedExecutionState::Selected
        };
    };

    let Some(attempt) = facts.attempt.as_ref() else {
        return if run.state.is_terminal() {
            FocusedExecutionState::Selected
        } else {
            inconsistent(FocusedExecutionInvariantViolation::ActiveRunWithoutAttempt)
        };
    };

    let authority_matches = facts.attempt_task_id == facts.task_id
        && facts.binding.as_ref().is_some_and(|binding| {
            binding.kind == DocketExecutionBindingKind::ClientSession
                && binding.id == facts.session_id.as_str()
        })
        && facts.host.as_ref().is_some_and(|host| {
            host.kind == DocketExecutionHostKind::TurnRun && host.run_id == run.id.as_str()
        });
    if !authority_matches {
        return inconsistent(FocusedExecutionInvariantViolation::HostMismatch);
    }

    let attempt_is_live = attempt.state.is_live();
    if run.state.is_terminal() {
        return if attempt_is_live || facts.open_obligations > 0 {
            inconsistent(
                FocusedExecutionInvariantViolation::TerminalRunWithLiveAttemptOrOpenObligations,
            )
        } else if facts.controller != ControllerDisposition::Missing {
            inconsistent(FocusedExecutionInvariantViolation::ControllerWithoutDurableAuthority)
        } else {
            FocusedExecutionState::Terminal
        };
    }

    if !attempt_is_live {
        return inconsistent(FocusedExecutionInvariantViolation::ActiveRunWithoutAttempt);
    }
    if run.state == TurnRunState::Accepted
        && matches!(
            facts.controller,
            ControllerDisposition::Queued
                | ControllerDisposition::Claimed
                | ControllerDisposition::Live
        )
    {
        return FocusedExecutionState::Starting;
    }
    if facts.controller == ControllerDisposition::Recovering {
        return FocusedExecutionState::Recovering;
    }
    if facts.controller != ControllerDisposition::Live {
        return inconsistent(FocusedExecutionInvariantViolation::RunningWithoutController);
    }

    match run.state {
        TurnRunState::Accepted => FocusedExecutionState::Starting,
        TurnRunState::Running => FocusedExecutionState::Running,
        TurnRunState::WaitingForClient => FocusedExecutionState::WaitingForClient,
        TurnRunState::Continuing => FocusedExecutionState::Continuing,
        TurnRunState::Completed | TurnRunState::Failed | TurnRunState::Cancelled => {
            unreachable!("terminal run handled above")
        }
    }
}

#[cfg(test)]
mod tests;
