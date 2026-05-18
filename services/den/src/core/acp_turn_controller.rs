use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpTurnPhase {
    Created,
    Streaming,
    WaitingForObligations,
    ContinuingAfterTool,
    Cancelling,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpToolExecutionRoute {
    DenServer,
    AdapterLocal,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpObligationStatus {
    Pending,
    Running,
    Settled,
    Failed,
    TimedOut,
    Cancelled,
    LateIgnored,
}

impl AcpObligationStatus {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpTerminalStatus {
    Ok,
    Failed,
    Cancelled,
    Recovered,
    NeedsNewSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpTerminalReason {
    EndTurn,
    StreamComplete,
    StreamError,
    ToolTimeout,
    Cancelled,
    OrphanedRequiresApproval,
    UnsupportedTool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpTerminalOutcome {
    pub status: AcpTerminalStatus,
    pub reason: AcpTerminalReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpToolObligation {
    pub tool_call_id: String,
    pub tool_name: String,
    pub route: AcpToolExecutionRoute,
    pub status: AcpObligationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpToolResultDisposition {
    Accepted,
    LateIgnored,
    UnknownToolCall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpTurnController {
    phase: AcpTurnPhase,
    obligations: BTreeMap<String, AcpToolObligation>,
    ready_terminal: Option<AcpTerminalOutcome>,
    emitted_terminal: Option<AcpTerminalOutcome>,
    orphaned_requires_approval: bool,
    late_results_ignored: usize,
}

impl Default for AcpTurnController {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpTurnController {
    pub fn new() -> Self {
        Self {
            phase: AcpTurnPhase::Created,
            obligations: BTreeMap::new(),
            ready_terminal: None,
            emitted_terminal: None,
            orphaned_requires_approval: false,
            late_results_ignored: 0,
        }
    }

    pub fn phase(&self) -> AcpTurnPhase {
        self.phase
    }

    pub fn orphaned_requires_approval(&self) -> bool {
        self.orphaned_requires_approval
    }

    pub fn late_results_ignored(&self) -> usize {
        self.late_results_ignored
    }

    pub fn obligation(&self, tool_call_id: &str) -> Option<&AcpToolObligation> {
        self.obligations.get(tool_call_id)
    }

    pub fn open_obligation_count(&self) -> usize {
        self.obligations
            .values()
            .filter(|obligation| obligation.status.is_open())
            .count()
    }

    pub fn on_stream_started(&mut self) {
        if self.phase == AcpTurnPhase::Created {
            self.phase = AcpTurnPhase::Streaming;
        }
    }

    pub fn on_tool_request(
        &mut self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        route: AcpToolExecutionRoute,
    ) {
        let tool_call_id = tool_call_id.into();
        let tool_name = tool_name.into();
        let status = match route {
            AcpToolExecutionRoute::DenServer => AcpObligationStatus::Running,
            AcpToolExecutionRoute::AdapterLocal => AcpObligationStatus::Pending,
            AcpToolExecutionRoute::Unsupported => AcpObligationStatus::Failed,
        };
        self.obligations.insert(
            tool_call_id.clone(),
            AcpToolObligation {
                tool_call_id,
                tool_name,
                route,
                status,
            },
        );
        if self.open_obligation_count() > 0 && self.phase != AcpTurnPhase::Terminal {
            self.phase = AcpTurnPhase::WaitingForObligations;
        }
        if matches!(route, AcpToolExecutionRoute::Unsupported) {
            self.ready_terminal.get_or_insert(AcpTerminalOutcome {
                status: AcpTerminalStatus::Failed,
                reason: AcpTerminalReason::UnsupportedTool,
            });
        }
    }

    pub fn on_den_tool_settled(&mut self, tool_call_id: &str, ok: bool) -> AcpToolResultDisposition {
        self.settle_tool(tool_call_id, ok)
    }

    pub fn on_adapter_tool_result(
        &mut self,
        tool_call_id: &str,
        ok: bool,
    ) -> AcpToolResultDisposition {
        self.settle_tool(tool_call_id, ok)
    }

    pub fn on_tool_timeout(&mut self, tool_call_id: &str) -> AcpToolResultDisposition {
        if self.emitted_terminal.is_some() {
            self.late_results_ignored += 1;
            return AcpToolResultDisposition::LateIgnored;
        }
        let Some(obligation) = self.obligations.get_mut(tool_call_id) else {
            return AcpToolResultDisposition::UnknownToolCall;
        };
        if !obligation.status.is_open() {
            self.late_results_ignored += 1;
            obligation.status = AcpObligationStatus::LateIgnored;
            return AcpToolResultDisposition::LateIgnored;
        }
        obligation.status = AcpObligationStatus::TimedOut;
        self.ready_terminal = Some(AcpTerminalOutcome {
            status: AcpTerminalStatus::Failed,
            reason: AcpTerminalReason::ToolTimeout,
        });
        self.advance_after_obligation_change();
        AcpToolResultDisposition::Accepted
    }

    pub fn on_requires_approval_stop(&mut self) {
        if self.open_obligation_count() > 0 {
            self.phase = AcpTurnPhase::WaitingForObligations;
            return;
        }
        self.orphaned_requires_approval = true;
        self.ready_terminal = Some(AcpTerminalOutcome {
            status: AcpTerminalStatus::Recovered,
            reason: AcpTerminalReason::OrphanedRequiresApproval,
        });
    }

    pub fn on_stream_end(&mut self) {
        if self.open_obligation_count() > 0 {
            self.phase = AcpTurnPhase::WaitingForObligations;
            return;
        }
        self.ready_terminal.get_or_insert(AcpTerminalOutcome {
            status: AcpTerminalStatus::Ok,
            reason: AcpTerminalReason::EndTurn,
        });
    }

    pub fn on_stream_error(&mut self) {
        self.ready_terminal = Some(AcpTerminalOutcome {
            status: AcpTerminalStatus::Failed,
            reason: AcpTerminalReason::StreamError,
        });
    }

    pub fn on_cancel(&mut self) {
        if self.emitted_terminal.is_some() {
            return;
        }
        self.phase = AcpTurnPhase::Cancelling;
        for obligation in self.obligations.values_mut() {
            if obligation.status.is_open() {
                obligation.status = AcpObligationStatus::Cancelled;
            }
        }
        self.ready_terminal = Some(AcpTerminalOutcome {
            status: AcpTerminalStatus::Cancelled,
            reason: AcpTerminalReason::Cancelled,
        });
    }

    pub fn may_emit_terminal(&self) -> bool {
        self.ready_terminal.is_some()
            && self.emitted_terminal.is_none()
            && self.open_obligation_count() == 0
    }

    pub fn take_terminal_event(&mut self) -> Option<AcpTerminalOutcome> {
        if !self.may_emit_terminal() {
            return None;
        }
        let outcome = self.ready_terminal.take()?;
        self.emitted_terminal = Some(outcome.clone());
        self.phase = AcpTurnPhase::Terminal;
        Some(outcome)
    }

    fn settle_tool(&mut self, tool_call_id: &str, ok: bool) -> AcpToolResultDisposition {
        if self.emitted_terminal.is_some() {
            self.late_results_ignored += 1;
            return AcpToolResultDisposition::LateIgnored;
        }
        let Some(obligation) = self.obligations.get_mut(tool_call_id) else {
            return AcpToolResultDisposition::UnknownToolCall;
        };
        if !obligation.status.is_open() {
            self.late_results_ignored += 1;
            obligation.status = AcpObligationStatus::LateIgnored;
            return AcpToolResultDisposition::LateIgnored;
        }
        obligation.status = if ok {
            AcpObligationStatus::Settled
        } else {
            AcpObligationStatus::Failed
        };
        self.advance_after_obligation_change();
        AcpToolResultDisposition::Accepted
    }

    fn advance_after_obligation_change(&mut self) {
        if self.phase != AcpTurnPhase::Terminal && self.open_obligation_count() == 0 {
            self.phase = AcpTurnPhase::ContinuingAfterTool;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_turn_text_only_completes_once() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_stream_end();

        let terminal = turn.take_terminal_event().expect("terminal ready");
        assert_eq!(terminal.status, AcpTerminalStatus::Ok);
        assert_eq!(terminal.reason, AcpTerminalReason::EndTurn);
        assert_eq!(turn.take_terminal_event(), None);
        assert_eq!(turn.phase(), AcpTurnPhase::Terminal);
    }

    #[test]
    fn acp_turn_waits_for_adapter_local_tool_before_terminal() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_tool_request("call_1", "fs_read_text_file", AcpToolExecutionRoute::AdapterLocal);
        turn.on_requires_approval_stop();
        turn.on_stream_end();

        assert_eq!(turn.open_obligation_count(), 1);
        assert!(!turn.may_emit_terminal());
        assert_eq!(turn.take_terminal_event(), None);

        assert_eq!(
            turn.on_adapter_tool_result("call_1", true),
            AcpToolResultDisposition::Accepted
        );
        assert_eq!(turn.open_obligation_count(), 0);
        assert!(!turn.may_emit_terminal());

        turn.on_stream_started();
        turn.on_stream_end();
        let terminal = turn.take_terminal_event().expect("terminal ready");
        assert_eq!(terminal.status, AcpTerminalStatus::Ok);
        assert_eq!(turn.take_terminal_event(), None);
    }

    #[test]
    fn acp_turn_den_server_tool_does_not_create_adapter_obligation() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_tool_request("call_1", "session_info", AcpToolExecutionRoute::DenServer);

        let obligation = turn.obligation("call_1").expect("tracked Den obligation");
        assert_eq!(obligation.route, AcpToolExecutionRoute::DenServer);
        assert_eq!(obligation.status, AcpObligationStatus::Running);
        assert_eq!(turn.open_obligation_count(), 1);

        assert_eq!(
            turn.on_den_tool_settled("call_1", true),
            AcpToolResultDisposition::Accepted
        );
        assert_eq!(turn.open_obligation_count(), 0);
        turn.on_stream_end();
        assert_eq!(
            turn.take_terminal_event().expect("terminal ready").status,
            AcpTerminalStatus::Ok
        );
    }

    #[test]
    fn acp_turn_unsupported_tool_settles_without_hanging() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_tool_request("call_1", "unknown_tool", AcpToolExecutionRoute::Unsupported);

        assert_eq!(turn.open_obligation_count(), 0);
        let terminal = turn.take_terminal_event().expect("terminal ready");
        assert_eq!(terminal.status, AcpTerminalStatus::Failed);
        assert_eq!(terminal.reason, AcpTerminalReason::UnsupportedTool);
    }

    #[test]
    fn acp_turn_timeout_settles_pending_adapter_tool() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_tool_request("call_1", "fs_read_text_file", AcpToolExecutionRoute::AdapterLocal);

        assert_eq!(
            turn.on_tool_timeout("call_1"),
            AcpToolResultDisposition::Accepted
        );
        assert_eq!(turn.open_obligation_count(), 0);
        let terminal = turn.take_terminal_event().expect("terminal ready");
        assert_eq!(terminal.status, AcpTerminalStatus::Failed);
        assert_eq!(terminal.reason, AcpTerminalReason::ToolTimeout);
        assert_eq!(
            turn.on_adapter_tool_result("call_1", true),
            AcpToolResultDisposition::LateIgnored
        );
    }

    #[test]
    fn acp_turn_cancel_settles_pending_adapter_tool() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_tool_request("call_1", "fs_read_text_file", AcpToolExecutionRoute::AdapterLocal);
        turn.on_cancel();

        assert_eq!(turn.open_obligation_count(), 0);
        let obligation = turn.obligation("call_1").expect("obligation");
        assert_eq!(obligation.status, AcpObligationStatus::Cancelled);
        let terminal = turn.take_terminal_event().expect("terminal ready");
        assert_eq!(terminal.status, AcpTerminalStatus::Cancelled);
        assert_eq!(terminal.reason, AcpTerminalReason::Cancelled);
        assert_eq!(
            turn.on_adapter_tool_result("call_1", true),
            AcpToolResultDisposition::LateIgnored
        );
    }

    #[test]
    fn acp_turn_late_result_after_terminal_is_ignored() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_stream_end();
        assert!(turn.take_terminal_event().is_some());

        assert_eq!(
            turn.on_adapter_tool_result("call_1", true),
            AcpToolResultDisposition::LateIgnored
        );
        assert_eq!(turn.late_results_ignored(), 1);
        assert_eq!(turn.take_terminal_event(), None);
    }

    #[test]
    fn acp_turn_orphaned_requires_approval_triggers_recovery_path() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_requires_approval_stop();

        assert!(turn.orphaned_requires_approval());
        let terminal = turn.take_terminal_event().expect("terminal ready");
        assert_eq!(terminal.status, AcpTerminalStatus::Recovered);
        assert_eq!(terminal.reason, AcpTerminalReason::OrphanedRequiresApproval);
        assert_eq!(turn.take_terminal_event(), None);
    }
}
