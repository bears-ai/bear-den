//! Turn completion policy.
//!
//! This module owns the behavioral question "may this current-task turn end now?".
//! Runtime budget code may report pressure or steer the model to checkpoint, but
//! budget pressure is not task completion. Session streaming, diagnostics, and
//! prompt classification should feed inputs here rather than independently
//! interpreting `may_stop`, final-response text, or budget flags.
//!
//! Invariant: while a resolved current task-list has incomplete, unblocked
//! items, a runtime-limit final response is not accepted as completion. The
//! runtime must either continue the next actionable slice or pause with a
//! structured resumable reason; it must not accept an ordinary terminal
//! response merely because the model described a limit.

use den_core::CapabilitySet;
use den_docket::TaskListProjection;

use crate::runtime::turn_state::{
    autonomous_execution_gate_for_task_list, classify_autonomous_final_response,
    detect_task_focus_loop, AutonomousExecutionGate, AutonomousFinalResponseKind,
    TaskFocusLoopDetection,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnCompletionDecision {
    Complete {
        reason: TurnCompletionCompleteReason,
        gate: AutonomousExecutionGate,
        final_response_kind: AutonomousFinalResponseKind,
        loop_detection: Option<TaskFocusLoopDetection>,
    },
    Continue {
        reason: TurnCompletionContinueReason,
        next_task: String,
        gate: AutonomousExecutionGate,
        final_response_kind: AutonomousFinalResponseKind,
    },
    Pause {
        reason: TurnCompletionPauseReason,
        gate: AutonomousExecutionGate,
        final_response_kind: AutonomousFinalResponseKind,
        loop_detection: TaskFocusLoopDetection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnCompletionCompleteReason {
    NoActiveFocusedTask,
    FocusedWorkCompleteFinalizationDrain,
    FocusedWorkCompleteOrTerminallyBlocked,
    RepeatedTerminalObjection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnCompletionContinueReason {
    FocusedWorkRemains,
    RuntimeLimitIsNotFocusedCompletion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnCompletionPauseReason {
    RepeatedTerminalObjection,
}

#[derive(Debug, Clone, Copy)]
pub struct TurnCompletionPolicyInput<'a> {
    /// Capabilities from the caller's verified execution policy, not owner metadata.
    pub capabilities: &'a CapabilitySet,
    pub current_task_list: Option<&'a TaskListProjection>,
    pub assistant_text: &'a str,
    pub recent_texts: &'a [String],
}

pub fn decide_turn_completion(input: TurnCompletionPolicyInput<'_>) -> TurnCompletionDecision {
    let final_response_kind = classify_autonomous_final_response(input.assistant_text);
    let gate = autonomous_execution_gate_for_task_list(
        input.capabilities,
        input.current_task_list,
        final_response_kind,
    );

    if !gate.is_active_autonomous_task {
        return TurnCompletionDecision::Complete {
            reason: TurnCompletionCompleteReason::NoActiveFocusedTask,
            gate,
            final_response_kind,
            loop_detection: None,
        };
    }

    if !gate.has_incomplete_unblocked_items && !gate.has_hard_blocker {
        return TurnCompletionDecision::Complete {
            reason: TurnCompletionCompleteReason::FocusedWorkCompleteFinalizationDrain,
            gate,
            final_response_kind,
            loop_detection: None,
        };
    }

    if should_force_focused_continuation(&gate) {
        let loop_detection = detect_task_focus_loop(input.recent_texts);
        if loop_detection.detected
            && final_response_kind != AutonomousFinalResponseKind::RuntimeLimitBlockedFinal
        {
            return TurnCompletionDecision::Pause {
                reason: TurnCompletionPauseReason::RepeatedTerminalObjection,
                gate,
                final_response_kind,
                loop_detection,
            };
        }

        let next_task = gate
            .next_incomplete_task_title
            .clone()
            .unwrap_or_else(|| "the next incomplete task".to_string());
        let reason = if final_response_kind == AutonomousFinalResponseKind::RuntimeLimitBlockedFinal
        {
            TurnCompletionContinueReason::RuntimeLimitIsNotFocusedCompletion
        } else {
            TurnCompletionContinueReason::FocusedWorkRemains
        };
        return TurnCompletionDecision::Continue {
            reason,
            next_task,
            gate,
            final_response_kind,
        };
    }

    TurnCompletionDecision::Complete {
        reason: TurnCompletionCompleteReason::FocusedWorkCompleteOrTerminallyBlocked,
        gate,
        final_response_kind,
        loop_detection: None,
    }
}

fn should_force_focused_continuation(gate: &AutonomousExecutionGate) -> bool {
    gate.has_incomplete_unblocked_items && !gate.may_stop
}

#[cfg(test)]
mod tests;
