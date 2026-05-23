use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use tokio::sync::watch;
use uuid::Uuid;

use crate::core::acp_tool_turns::AcpToolTurnCoordinator;

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
pub struct AcpTurnStatusSnapshot {
    pub phase: AcpTurnPhase,
    pub open_obligations: usize,
    pub pending_adapter_tools: usize,
    pub pending_den_tools: usize,
    pub pending_permissions: usize,
    pub terminal_status: Option<AcpTerminalStatus>,
    pub terminal_reason: Option<AcpTerminalReason>,
    pub orphaned_requires_approval: bool,
    pub late_results_ignored: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpTurnStatusUpdate {
    pub key: &'static str,
    pub text: &'static str,
}

#[derive(Debug, Clone)]
pub struct AcpActiveTurnCancelRegistration {
    pub acp_session_id: String,
    pub request_id: Uuid,
    pub conversation_id: Option<String>,
    pub run_ids: Vec<String>,
    pub cancel_tx: watch::Sender<bool>,
}

#[derive(Debug, Clone)]
pub struct AcpActiveTurnCancelHandle {
    registry: AcpActiveTurnCancelRegistry,
    acp_session_id: String,
    request_id: Uuid,
}

impl Drop for AcpActiveTurnCancelHandle {
    fn drop(&mut self) {
        self.registry
            .unregister_if_matches(&self.acp_session_id, self.request_id);
    }
}

#[derive(Debug, Clone, Default)]
pub struct AcpActiveTurnCancelRegistry {
    inner: Arc<Mutex<HashMap<String, AcpActiveTurnCancelRegistration>>>,
}

impl AcpActiveTurnCancelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        acp_session_id: impl Into<String>,
        request_id: Uuid,
        conversation_id: Option<String>,
    ) -> (AcpActiveTurnCancelHandle, watch::Receiver<bool>) {
        let acp_session_id = acp_session_id.into();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        if let Ok(mut inner) = self.inner.lock() {
            inner.insert(
                acp_session_id.clone(),
                AcpActiveTurnCancelRegistration {
                    acp_session_id: acp_session_id.clone(),
                    request_id,
                    conversation_id,
                    run_ids: Vec::new(),
                    cancel_tx,
                },
            );
        }
        (
            AcpActiveTurnCancelHandle {
                registry: self.clone(),
                acp_session_id,
                request_id,
            },
            cancel_rx,
        )
    }

    pub fn cancel_session(&self, acp_session_id: &str) -> Option<AcpActiveTurnCancelRegistration> {
        let registration = self.inner.lock().ok()?.get(acp_session_id).cloned()?;
        let _ = registration.cancel_tx.send(true);
        Some(registration)
    }

    pub fn record_run_id(&self, acp_session_id: &str, request_id: Uuid, run_id: &str) -> bool {
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return false;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let Some(registration) = inner.get_mut(acp_session_id) else {
            return false;
        };
        if registration.request_id != request_id {
            return false;
        }
        if registration.run_ids.iter().any(|known| known == run_id) {
            return false;
        }
        registration.run_ids.push(run_id.to_string());
        true
    }

    pub fn active_for_session(
        &self,
        acp_session_id: &str,
    ) -> Option<AcpActiveTurnCancelRegistration> {
        self.inner.lock().ok()?.get(acp_session_id).cloned()
    }

    pub fn runtime_snapshot_for_session(
        &self,
        acp_session_id: &str,
        tool_turns: &AcpToolTurnCoordinator,
    ) -> Value {
        let Some(active) = self.active_for_session(acp_session_id) else {
            return json!({
                "state": "idle",
                "active_turn": {
                    "present": false,
                    "phase": Value::Null,
                    "pending_obligations": 0,
                    "pending_adapter_tools": 0,
                    "pending_den_tools": 0,
                    "pending_permissions": 0,
                    "run_ids": [],
                },
                "last_terminal": Value::Null,
                "last_recovery": Value::Null,
                "source": "acp_active_turn_registry",
            });
        };
        let pending = tool_turns
            .pending_for_session(acp_session_id)
            .into_iter()
            .filter(|pending| pending.request_id == active.request_id)
            .collect::<Vec<_>>();
        let pending_obligations = pending.len();
        let state = if pending_obligations > 0 {
            "requires_action"
        } else {
            "running"
        };
        let phase = if pending_obligations > 0 {
            "WaitingForObligations"
        } else {
            "Streaming"
        };
        json!({
            "state": state,
            "active_turn": {
                "present": true,
                "phase": phase,
                "request_id": active.request_id,
                "conversation_id": active.conversation_id,
                "run_ids": active.run_ids,
                "pending_obligations": pending_obligations,
                "pending_adapter_tools": pending_obligations,
                "pending_den_tools": 0,
                "pending_permissions": 0,
            },
            "last_terminal": Value::Null,
            "last_recovery": Value::Null,
            "source": "acp_active_turn_registry",
        })
    }

    fn unregister_if_matches(&self, acp_session_id: &str, request_id: Uuid) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let should_remove = inner
            .get(acp_session_id)
            .is_some_and(|registration| registration.request_id == request_id);
        if should_remove {
            inner.remove(acp_session_id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpTurnController {
    phase: AcpTurnPhase,
    obligations: BTreeMap<String, AcpToolObligation>,
    ready_terminal: Option<AcpTerminalOutcome>,
    emitted_terminal: Option<AcpTerminalOutcome>,
    orphaned_requires_approval: bool,
    late_results_ignored: usize,
    last_status_key: Option<&'static str>,
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
            last_status_key: None,
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

    pub fn status_snapshot(&self) -> AcpTurnStatusSnapshot {
        let mut pending_adapter_tools = 0;
        let mut pending_den_tools = 0;
        for obligation in self.obligations.values() {
            if !obligation.status.is_open() {
                continue;
            }
            match obligation.route {
                AcpToolExecutionRoute::AdapterLocal => pending_adapter_tools += 1,
                AcpToolExecutionRoute::DenServer => pending_den_tools += 1,
                AcpToolExecutionRoute::Unsupported => {}
            }
        }
        let terminal = self
            .emitted_terminal
            .as_ref()
            .or(self.ready_terminal.as_ref());
        AcpTurnStatusSnapshot {
            phase: self.phase,
            open_obligations: self.open_obligation_count(),
            pending_adapter_tools,
            pending_den_tools,
            pending_permissions: 0,
            terminal_status: terminal.map(|outcome| outcome.status),
            terminal_reason: terminal.map(|outcome| outcome.reason),
            orphaned_requires_approval: self.orphaned_requires_approval,
            late_results_ignored: self.late_results_ignored,
        }
    }

    pub fn take_status_update(&mut self) -> Option<AcpTurnStatusUpdate> {
        let update = self.current_status_update()?;
        if self.last_status_key == Some(update.key) {
            return None;
        }
        self.last_status_key = Some(update.key);
        Some(update)
    }

    fn current_status_update(&self) -> Option<AcpTurnStatusUpdate> {
        if self.orphaned_requires_approval && self.phase != AcpTurnPhase::Terminal {
            return Some(AcpTurnStatusUpdate {
                key: "recovering_stale_approval",
                text: "Recovering stale model approval…",
            });
        }
        match self.phase {
            AcpTurnPhase::Created => None,
            AcpTurnPhase::Streaming => Some(AcpTurnStatusUpdate {
                key: "thinking",
                text: "Thinking…",
            }),
            AcpTurnPhase::WaitingForObligations => {
                let snapshot = self.status_snapshot();
                if snapshot.pending_adapter_tools > 0 {
                    Some(AcpTurnStatusUpdate {
                        key: "waiting_for_local_tool",
                        text: "Waiting for local tool result…",
                    })
                } else if snapshot.pending_den_tools > 0 {
                    Some(AcpTurnStatusUpdate {
                        key: "running_den_tool",
                        text: "Running Den server tool…",
                    })
                } else {
                    Some(AcpTurnStatusUpdate {
                        key: "waiting_for_obligations",
                        text: "Waiting for turn obligations…",
                    })
                }
            }
            AcpTurnPhase::ContinuingAfterTool => Some(AcpTurnStatusUpdate {
                key: "continuing_after_tool",
                text: "Continuing after tool result…",
            }),
            AcpTurnPhase::Cancelling => Some(AcpTurnStatusUpdate {
                key: "cancelling",
                text: "Cancelling turn…",
            }),
            AcpTurnPhase::Terminal => None,
        }
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

    pub fn on_den_tool_settled(
        &mut self,
        tool_call_id: &str,
        ok: bool,
    ) -> AcpToolResultDisposition {
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
        self.phase = AcpTurnPhase::WaitingForObligations;
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
    fn active_turn_cancel_registry_signals_and_unregisters_session_turn() {
        let registry = AcpActiveTurnCancelRegistry::new();
        let request_id = Uuid::new_v4();
        let (handle, cancel_rx) =
            registry.register("acp-session-1", request_id, Some("conv-1".to_string()));

        let active = registry
            .active_for_session("acp-session-1")
            .expect("active registration");
        assert_eq!(active.request_id, request_id);
        assert_eq!(active.conversation_id.as_deref(), Some("conv-1"));
        assert!(active.run_ids.is_empty());
        assert!(!*cancel_rx.borrow());

        let cancelled = registry
            .cancel_session("acp-session-1")
            .expect("cancelled registration");
        assert_eq!(cancelled.request_id, request_id);
        assert!(cancelled.run_ids.is_empty());
        assert!(*cancel_rx.borrow());

        drop(handle);
        assert!(registry.active_for_session("acp-session-1").is_none());
    }

    #[test]
    fn active_turn_cancel_registry_records_run_ids_for_matching_turn() {
        let registry = AcpActiveTurnCancelRegistry::new();
        let request_id = Uuid::new_v4();
        let wrong_request_id = Uuid::new_v4();
        let (_handle, _rx) = registry.register("acp-session-1", request_id, None);

        assert!(!registry.record_run_id("acp-session-1", request_id, "   "));
        assert!(!registry.record_run_id("acp-session-1", wrong_request_id, "run-wrong"));
        assert!(!registry.record_run_id("missing-session", request_id, "run-missing"));
        assert!(registry.record_run_id("acp-session-1", request_id, " run-1 "));
        assert!(!registry.record_run_id("acp-session-1", request_id, "run-1"));
        assert!(registry.record_run_id("acp-session-1", request_id, "run-2"));

        let active = registry
            .active_for_session("acp-session-1")
            .expect("active registration");
        assert_eq!(
            active.run_ids,
            vec!["run-1".to_string(), "run-2".to_string()]
        );

        let cancelled = registry
            .cancel_session("acp-session-1")
            .expect("cancelled registration");
        assert_eq!(
            cancelled.run_ids,
            vec!["run-1".to_string(), "run-2".to_string()]
        );
    }

    #[test]
    fn active_turn_cancel_registry_does_not_unregister_newer_turn_from_old_handle() {
        let registry = AcpActiveTurnCancelRegistry::new();
        let old_request_id = Uuid::new_v4();
        let new_request_id = Uuid::new_v4();
        let (old_handle, _old_rx) = registry.register("acp-session-1", old_request_id, None);
        let (_new_handle, _new_rx) = registry.register("acp-session-1", new_request_id, None);

        drop(old_handle);
        assert_eq!(
            registry
                .active_for_session("acp-session-1")
                .expect("newer turn survives")
                .request_id,
            new_request_id
        );
    }

    #[test]
    fn active_turn_runtime_snapshot_reports_idle_without_active_turn() {
        let registry = AcpActiveTurnCancelRegistry::new();
        let tool_turns = AcpToolTurnCoordinator::new();
        let snapshot = registry.runtime_snapshot_for_session("acp-session", &tool_turns);

        assert_eq!(snapshot["state"], "idle");
        assert_eq!(snapshot["active_turn"]["present"], false);
        assert_eq!(snapshot["active_turn"]["pending_obligations"], 0);
        assert_eq!(snapshot["active_turn"]["run_ids"], json!([]));
        assert_eq!(snapshot["source"], "acp_active_turn_registry");
    }

    #[test]
    fn active_turn_runtime_snapshot_reports_running_without_pending_tools() {
        let registry = AcpActiveTurnCancelRegistry::new();
        let tool_turns = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let (_handle, _rx) =
            registry.register("acp-session", request_id, Some("conv-test".to_string()));
        assert!(registry.record_run_id("acp-session", request_id, "run-snapshot"));
        let snapshot = registry.runtime_snapshot_for_session("acp-session", &tool_turns);

        assert_eq!(snapshot["state"], "running");
        assert_eq!(snapshot["active_turn"]["present"], true);
        assert_eq!(snapshot["active_turn"]["phase"], "Streaming");
        assert_eq!(snapshot["active_turn"]["request_id"], json!(request_id));
        assert_eq!(snapshot["active_turn"]["conversation_id"], "conv-test");
        assert_eq!(snapshot["active_turn"]["run_ids"], json!(["run-snapshot"]));
        assert_eq!(snapshot["active_turn"]["pending_obligations"], 0);
    }

    #[test]
    fn active_turn_runtime_snapshot_reports_requires_action_with_pending_tool() {
        let registry = AcpActiveTurnCancelRegistry::new();
        let tool_turns = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let (_handle, _rx) = registry.register("acp-session", request_id, None);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tool_turns
            .register(crate::core::acp_tool_turns::AcpToolTurnRegistration {
                user_id: 1,
                bear_id: Uuid::new_v4(),
                bear_slug: "test-bear".to_string(),
                acp_session_id: "acp-session".to_string(),
                request_id,
                tool_call_id: "call-1".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: Some("approval-1".to_string()),
                timeout_ms: 30_000,
                result_tx: tx,
            })
            .unwrap();
        let snapshot = registry.runtime_snapshot_for_session("acp-session", &tool_turns);

        assert_eq!(snapshot["state"], "requires_action");
        assert_eq!(snapshot["active_turn"]["present"], true);
        assert_eq!(snapshot["active_turn"]["phase"], "WaitingForObligations");
        assert_eq!(snapshot["active_turn"]["pending_obligations"], 1);
        assert_eq!(snapshot["active_turn"]["pending_adapter_tools"], 1);
        assert_eq!(snapshot["active_turn"]["pending_den_tools"], 0);
    }

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
        turn.on_tool_request(
            "call_1",
            "fs_read_text_file",
            AcpToolExecutionRoute::AdapterLocal,
        );
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
        turn.on_tool_request(
            "call_1",
            "fs_read_text_file",
            AcpToolExecutionRoute::AdapterLocal,
        );

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
        turn.on_tool_request(
            "call_1",
            "fs_read_text_file",
            AcpToolExecutionRoute::AdapterLocal,
        );
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
        assert_eq!(
            turn.take_status_update().expect("status update").key,
            "recovering_stale_approval"
        );
        let terminal = turn.take_terminal_event().expect("terminal ready");
        assert_eq!(terminal.status, AcpTerminalStatus::Recovered);
        assert_eq!(terminal.reason, AcpTerminalReason::OrphanedRequiresApproval);
        assert_eq!(turn.take_terminal_event(), None);
    }

    #[test]
    fn acp_turn_status_snapshot_reports_phase_and_obligations() {
        let mut turn = AcpTurnController::new();
        turn.on_stream_started();
        turn.on_tool_request(
            "call_local",
            "fs_read_text_file",
            AcpToolExecutionRoute::AdapterLocal,
        );
        turn.on_tool_request("call_den", "session_info", AcpToolExecutionRoute::DenServer);

        let snapshot = turn.status_snapshot();
        assert_eq!(snapshot.phase, AcpTurnPhase::WaitingForObligations);
        assert_eq!(snapshot.open_obligations, 2);
        assert_eq!(snapshot.pending_adapter_tools, 1);
        assert_eq!(snapshot.pending_den_tools, 1);
        assert_eq!(snapshot.pending_permissions, 0);
        assert_eq!(snapshot.terminal_status, None);
        assert_eq!(snapshot.terminal_reason, None);

        assert_eq!(
            turn.on_adapter_tool_result("call_local", true),
            AcpToolResultDisposition::Accepted
        );
        assert_eq!(
            turn.on_den_tool_settled("call_den", true),
            AcpToolResultDisposition::Accepted
        );
        turn.on_stream_end();
        assert!(turn.take_terminal_event().is_some());

        let snapshot = turn.status_snapshot();
        assert_eq!(snapshot.phase, AcpTurnPhase::Terminal);
        assert_eq!(snapshot.open_obligations, 0);
        assert_eq!(snapshot.pending_adapter_tools, 0);
        assert_eq!(snapshot.pending_den_tools, 0);
        assert_eq!(snapshot.terminal_status, Some(AcpTerminalStatus::Ok));
        assert_eq!(snapshot.terminal_reason, Some(AcpTerminalReason::EndTurn));
    }

    #[test]
    fn acp_turn_status_updates_are_deduplicated() {
        let mut turn = AcpTurnController::new();
        assert_eq!(turn.take_status_update(), None);

        turn.on_stream_started();
        assert_eq!(turn.take_status_update().expect("thinking").key, "thinking");
        assert_eq!(turn.take_status_update(), None);

        turn.on_tool_request(
            "call_1",
            "fs_read_text_file",
            AcpToolExecutionRoute::AdapterLocal,
        );
        assert_eq!(
            turn.take_status_update().expect("waiting").key,
            "waiting_for_local_tool"
        );
        assert_eq!(turn.take_status_update(), None);

        assert_eq!(
            turn.on_adapter_tool_result("call_1", true),
            AcpToolResultDisposition::Accepted
        );
        assert_eq!(
            turn.take_status_update().expect("continuing").key,
            "continuing_after_tool"
        );
        assert_eq!(turn.take_status_update(), None);
    }
}
