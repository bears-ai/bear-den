use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use tokio::sync::watch;
use uuid::Uuid;

use crate::tool_turns::ToolTurnCoordinator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPhase {
    Created,
    Streaming,
    WaitingForObligations,
    ContinuingAfterTool,
    Cancelling,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionRoute {
    DenServer,
    AdapterLocal,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObligationStatus {
    Pending,
    Running,
    Settled,
    Failed,
    TimedOut,
    Cancelled,
    LateIgnored,
}

impl ObligationStatus {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalStatus {
    Ok,
    Failed,
    Cancelled,
    Recovered,
    NeedsNewSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalReason {
    EndTurn,
    StreamComplete,
    StreamError,
    ToolExecutionFailed,
    ToolTimeout,
    Cancelled,
    OrphanedRequiresApproval,
    UnsupportedTool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOutcome {
    pub status: TerminalStatus,
    pub reason: TerminalReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolObligation {
    pub tool_call_id: String,
    pub tool_name: String,
    pub route: ToolExecutionRoute,
    pub status: ObligationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolResultDisposition {
    Accepted,
    LateIgnored,
    UnknownToolCall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStatusSnapshot {
    pub phase: TurnPhase,
    pub open_obligations: usize,
    pub pending_adapter_tools: usize,
    pub pending_den_tools: usize,
    pub pending_permissions: usize,
    pub terminal_status: Option<TerminalStatus>,
    pub terminal_reason: Option<TerminalReason>,
    pub orphaned_requires_approval: bool,
    pub late_results_ignored: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStatusUpdate {
    pub key: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ActiveTurnCancelRegistration {
    pub client_session_id: String,
    pub run_id: String,
    pub request_id: Uuid,
    pub conversation_id: Option<String>,
    pub cancel_tx: watch::Sender<bool>,
}

#[derive(Debug, Clone)]
pub struct ActiveTurnCancelHandle {
    registry: ActiveTurnCancelRegistry,
    client_session_id: String,
    run_id: String,
    request_id: Uuid,
}

impl Drop for ActiveTurnCancelHandle {
    fn drop(&mut self) {
        self.registry
            .unregister_if_matches(&self.client_session_id, &self.run_id, self.request_id);
    }
}

#[derive(Debug, Clone, Default)]
pub struct ActiveTurnCancelRegistry {
    inner: Arc<Mutex<HashMap<(String, String), ActiveTurnCancelRegistration>>>,
}

impl ActiveTurnCancelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        client_session_id: impl Into<String>,
        run_id: impl Into<String>,
        request_id: Uuid,
        conversation_id: Option<String>,
    ) -> (ActiveTurnCancelHandle, watch::Receiver<bool>) {
        let client_session_id = client_session_id.into();
        let run_id = run_id.into();
        let key = (client_session_id.clone(), run_id.clone());
        let (cancel_tx, cancel_rx) = watch::channel(false);
        if let Ok(mut inner) = self.inner.lock() {
            inner.insert(
                key,
                ActiveTurnCancelRegistration {
                    client_session_id: client_session_id.clone(),
                    run_id: run_id.clone(),
                    request_id,
                    conversation_id,
                    cancel_tx,
                },
            );
        }
        (
            ActiveTurnCancelHandle {
                registry: self.clone(),
                client_session_id,
                run_id,
                request_id,
            },
            cancel_rx,
        )
    }

    pub fn cancel_run(
        &self,
        client_session_id: &str,
        run_id: &str,
    ) -> Option<ActiveTurnCancelRegistration> {
        let key = (client_session_id.to_string(), run_id.to_string());
        let registration = self.inner.lock().ok()?.get(&key).cloned()?;
        let _ = registration.cancel_tx.send(true);
        Some(registration)
    }

    pub fn active_for_run(
        &self,
        client_session_id: &str,
        run_id: &str,
    ) -> Option<ActiveTurnCancelRegistration> {
        self.inner
            .lock()
            .ok()?
            .get(&(client_session_id.to_string(), run_id.to_string()))
            .cloned()
    }

    pub fn runtime_snapshot_for_run(
        &self,
        client_session_id: &str,
        run_id: &str,
        tool_turns: &ToolTurnCoordinator,
    ) -> Value {
        let Some(active) = self.active_for_run(client_session_id, run_id) else {
            return json!({
                "state": "idle",
                "active_turn": {
                    "present": false,
                    "phase": Value::Null,
                    "pending_obligations": 0,
                    "pending_adapter_tools": 0,
                    "pending_den_tools": 0,
                    "pending_permissions": 0,
                    "run_id": run_id,
                },
                "last_terminal": Value::Null,
                "last_recovery": Value::Null,
                "source": "client_active_turn_registry",
            });
        };
        let pending = tool_turns
            .pending_for_session(client_session_id)
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
                "run_id": active.run_id,
                "pending_obligations": pending_obligations,
                "pending_adapter_tools": pending_obligations,
                "pending_den_tools": 0,
                "pending_permissions": 0,
            },
            "last_terminal": Value::Null,
            "last_recovery": Value::Null,
            "source": "client_active_turn_registry",
        })
    }

    fn unregister_if_matches(&self, client_session_id: &str, run_id: &str, request_id: Uuid) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let key = (client_session_id.to_string(), run_id.to_string());
        let should_remove = inner
            .get(&key)
            .is_some_and(|registration| registration.request_id == request_id);
        if should_remove {
            inner.remove(&key);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnController {
    phase: TurnPhase,
    obligations: BTreeMap<String, ToolObligation>,
    failed_tool_names: BTreeSet<String>,
    ready_terminal: Option<TerminalOutcome>,
    emitted_terminal: Option<TerminalOutcome>,
    orphaned_requires_approval: bool,
    late_results_ignored: usize,
    last_status_key: Option<String>,
    client_label: Option<String>,
    last_settled_tool_name: Option<String>,
    heartbeat_tick: u32,
}

impl Default for TurnController {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnController {
    pub fn new() -> Self {
        Self {
            phase: TurnPhase::Created,
            obligations: BTreeMap::new(),
            failed_tool_names: BTreeSet::new(),
            ready_terminal: None,
            emitted_terminal: None,
            orphaned_requires_approval: false,
            late_results_ignored: 0,
            last_status_key: None,
            client_label: None,
            last_settled_tool_name: None,
            heartbeat_tick: 0,
        }
    }

    pub fn set_client_label(&mut self, client: impl Into<String>) {
        let label = client.into();
        if !label.trim().is_empty() {
            self.client_label = Some(label);
        }
    }

    pub fn phase(&self) -> TurnPhase {
        self.phase
    }

    pub fn orphaned_requires_approval(&self) -> bool {
        self.orphaned_requires_approval
    }

    pub fn late_results_ignored(&self) -> usize {
        self.late_results_ignored
    }

    pub fn obligation(&self, tool_call_id: &str) -> Option<&ToolObligation> {
        self.obligations.get(tool_call_id)
    }

    pub fn open_obligation_count(&self) -> usize {
        self.obligations
            .values()
            .filter(|obligation| obligation.status.is_open())
            .count()
    }

    pub fn status_snapshot(&self) -> TurnStatusSnapshot {
        let mut pending_adapter_tools = 0;
        let mut pending_den_tools = 0;
        for obligation in self.obligations.values() {
            if !obligation.status.is_open() {
                continue;
            }
            match obligation.route {
                ToolExecutionRoute::AdapterLocal => pending_adapter_tools += 1,
                ToolExecutionRoute::DenServer => pending_den_tools += 1,
                ToolExecutionRoute::Unsupported => {}
            }
        }
        let terminal = self
            .emitted_terminal
            .as_ref()
            .or(self.ready_terminal.as_ref());
        TurnStatusSnapshot {
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

    pub fn take_status_update(&mut self) -> Option<TurnStatusUpdate> {
        let update = self.current_status_update()?;
        if self.last_status_key.as_deref() == Some(update.key.as_str()) {
            return None;
        }
        self.last_status_key = Some(update.key.clone());
        Some(update)
    }

    /// Phase-aware status for SSE heartbeats during quiet periods (LLM handshake, idle stream, tool waits).
    /// Unlike [`Self::take_status_update`], heartbeats may repeat with rotated copy while the phase is unchanged.
    pub fn heartbeat_status_update(&mut self) -> TurnStatusUpdate {
        self.heartbeat_tick = self.heartbeat_tick.wrapping_add(1);
        let tick = self.heartbeat_tick;
        if self.orphaned_requires_approval && self.phase != TurnPhase::Terminal {
            return TurnStatusUpdate {
                key: format!("heartbeat:recovering:{tick}"),
                text: if tick.is_multiple_of(2) {
                    "Recovering stale model approval…".to_string()
                } else {
                    "Cleaning up interrupted approval state…".to_string()
                },
            };
        }
        match self.phase {
            TurnPhase::Created => TurnStatusUpdate {
                key: format!("heartbeat:starting:{tick}"),
                text: "Starting turn…".to_string(),
            },
            TurnPhase::Streaming => {
                let variants = [
                    "Connecting to model…",
                    "Waiting for response…",
                    "Still thinking…",
                ];
                TurnStatusUpdate {
                    key: format!("heartbeat:streaming:{tick}"),
                    text: variants[(tick as usize) % variants.len()].to_string(),
                }
            }
            TurnPhase::WaitingForObligations => {
                let base = self
                    .waiting_for_obligations_status()
                    .text
                    .trim_end_matches('…')
                    .to_string();
                TurnStatusUpdate {
                    key: format!("heartbeat:waiting:{tick}"),
                    text: if tick.is_multiple_of(3) {
                        format!("{base} (still waiting)…")
                    } else {
                        base
                    },
                }
            }
            TurnPhase::ContinuingAfterTool => {
                let tool_name = self.last_settled_tool_name.as_deref().unwrap_or("tool");
                let label = humanize_tool_name(tool_name);
                let variants = [
                    format!("Continuing after {label}…"),
                    "Waiting for model…".to_string(),
                    format!("Resuming after {label}…"),
                ];
                TurnStatusUpdate {
                    key: format!("heartbeat:continuing:{tick}"),
                    text: variants[(tick as usize) % variants.len()].clone(),
                }
            }
            TurnPhase::Cancelling => TurnStatusUpdate {
                key: format!("heartbeat:cancelling:{tick}"),
                text: "Cancelling turn…".to_string(),
            },
            TurnPhase::Terminal => TurnStatusUpdate {
                key: format!("heartbeat:terminal:{tick}"),
                text: "Finishing turn…".to_string(),
            },
        }
    }

    fn current_status_update(&self) -> Option<TurnStatusUpdate> {
        if self.orphaned_requires_approval && self.phase != TurnPhase::Terminal {
            return Some(TurnStatusUpdate {
                key: "recovering_stale_approval".to_string(),
                text: "Recovering stale model approval…".to_string(),
            });
        }
        match self.phase {
            TurnPhase::Created => None,
            TurnPhase::Streaming => Some(self.planning_status()),
            TurnPhase::WaitingForObligations => Some(self.waiting_for_obligations_status()),
            TurnPhase::ContinuingAfterTool => Some(self.continuing_after_tool_status()),
            TurnPhase::Cancelling => Some(TurnStatusUpdate {
                key: "cancelling".to_string(),
                text: "Cancelling turn…".to_string(),
            }),
            TurnPhase::Terminal => None,
        }
    }

    fn planning_status(&self) -> TurnStatusUpdate {
        let client = self
            .client_label
            .as_deref()
            .map(client_display_name)
            .unwrap_or("your editor");
        TurnStatusUpdate {
            key: "planning".to_string(),
            text: format!(
                "Planning next step — may call Den memory tools or {client} workspace tools…"
            ),
        }
    }

    fn waiting_for_obligations_status(&self) -> TurnStatusUpdate {
        let open: Vec<_> = self
            .obligations
            .values()
            .filter(|obligation| obligation.status.is_open())
            .collect();
        let client = self
            .client_label
            .as_deref()
            .map(client_display_name)
            .unwrap_or("your editor");
        let extra = if open.len() > 1 {
            format!(" (+{} more)", open.len() - 1)
        } else {
            String::new()
        };
        if let Some(first) = open.first() {
            let label = humanize_tool_name(&first.tool_name);
            return match first.route {
                ToolExecutionRoute::AdapterLocal => TurnStatusUpdate {
                    key: format!("waiting_local:{}", first.tool_name),
                    text: format!("Waiting for {label} in {client}{extra}…"),
                },
                ToolExecutionRoute::DenServer => TurnStatusUpdate {
                    key: format!("running_den:{}", first.tool_name),
                    text: format!("Running {label} on Den{extra}…"),
                },
                ToolExecutionRoute::Unsupported => TurnStatusUpdate {
                    key: format!("unsupported_tool:{}", first.tool_name),
                    text: format!("Tool not available: {label}{extra}…"),
                },
            };
        }
        TurnStatusUpdate {
            key: "waiting_for_obligations".to_string(),
            text: "Waiting for turn obligations…".to_string(),
        }
    }

    fn continuing_after_tool_status(&self) -> TurnStatusUpdate {
        let tool_name = self.last_settled_tool_name.as_deref().unwrap_or("tool");
        let label = humanize_tool_name(tool_name);
        TurnStatusUpdate {
            key: format!("continuing_after:{tool_name}"),
            text: format!("Continuing after {label}…"),
        }
    }

    pub fn on_stream_started(&mut self) {
        if self.phase == TurnPhase::Created {
            self.phase = TurnPhase::Streaming;
        }
    }

    pub fn on_tool_request(
        &mut self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        route: ToolExecutionRoute,
    ) {
        let tool_call_id = tool_call_id.into();
        let tool_name = tool_name.into();
        let status = match route {
            ToolExecutionRoute::DenServer => ObligationStatus::Running,
            ToolExecutionRoute::AdapterLocal => ObligationStatus::Pending,
            ToolExecutionRoute::Unsupported => ObligationStatus::Failed,
        };
        self.obligations.insert(
            tool_call_id.clone(),
            ToolObligation {
                tool_call_id,
                tool_name,
                route,
                status,
            },
        );
        if self.open_obligation_count() > 0 && self.phase != TurnPhase::Terminal {
            self.phase = TurnPhase::WaitingForObligations;
        }
        if matches!(route, ToolExecutionRoute::Unsupported) {
            self.ready_terminal.get_or_insert(TerminalOutcome {
                status: TerminalStatus::Failed,
                reason: TerminalReason::UnsupportedTool,
            });
        }
    }

    pub fn on_den_tool_settled(&mut self, tool_call_id: &str, ok: bool) -> ToolResultDisposition {
        self.settle_tool(tool_call_id, ok)
    }

    pub fn on_adapter_tool_result(
        &mut self,
        tool_call_id: &str,
        ok: bool,
    ) -> ToolResultDisposition {
        self.settle_tool(tool_call_id, ok)
    }

    pub fn on_tool_timeout(&mut self, tool_call_id: &str) -> ToolResultDisposition {
        if self.emitted_terminal.is_some() {
            self.late_results_ignored += 1;
            return ToolResultDisposition::LateIgnored;
        }
        let Some(obligation) = self.obligations.get_mut(tool_call_id) else {
            return ToolResultDisposition::UnknownToolCall;
        };
        if !obligation.status.is_open() {
            self.late_results_ignored += 1;
            obligation.status = ObligationStatus::LateIgnored;
            return ToolResultDisposition::LateIgnored;
        }
        obligation.status = ObligationStatus::TimedOut;
        self.ready_terminal = Some(TerminalOutcome {
            status: TerminalStatus::Failed,
            reason: TerminalReason::ToolTimeout,
        });
        self.advance_after_obligation_change();
        ToolResultDisposition::Accepted
    }

    pub fn on_requires_approval_stop(&mut self) {
        if self.open_obligation_count() > 0 {
            self.phase = TurnPhase::WaitingForObligations;
            return;
        }
        self.orphaned_requires_approval = true;
        self.phase = TurnPhase::WaitingForObligations;
        self.ready_terminal = Some(TerminalOutcome {
            status: TerminalStatus::Recovered,
            reason: TerminalReason::OrphanedRequiresApproval,
        });
    }

    pub fn on_stream_end(&mut self) {
        if self.open_obligation_count() > 0 {
            self.phase = TurnPhase::WaitingForObligations;
            return;
        }
        let tool_failed = !self.failed_tool_names.is_empty();
        self.ready_terminal.get_or_insert(TerminalOutcome {
            status: if tool_failed {
                TerminalStatus::Failed
            } else {
                TerminalStatus::Ok
            },
            reason: if tool_failed {
                TerminalReason::ToolExecutionFailed
            } else {
                TerminalReason::EndTurn
            },
        });
    }

    pub fn on_stream_error(&mut self) {
        self.ready_terminal = Some(TerminalOutcome {
            status: TerminalStatus::Failed,
            reason: TerminalReason::StreamError,
        });
    }

    pub fn on_cancel(&mut self) {
        if self.emitted_terminal.is_some() {
            return;
        }
        self.phase = TurnPhase::Cancelling;
        for obligation in self.obligations.values_mut() {
            if obligation.status.is_open() {
                obligation.status = ObligationStatus::Cancelled;
            }
        }
        self.ready_terminal = Some(TerminalOutcome {
            status: TerminalStatus::Cancelled,
            reason: TerminalReason::Cancelled,
        });
    }

    pub fn may_emit_terminal(&self) -> bool {
        self.ready_terminal.is_some()
            && self.emitted_terminal.is_none()
            && self.open_obligation_count() == 0
    }

    pub fn take_terminal_event(&mut self) -> Option<TerminalOutcome> {
        if !self.may_emit_terminal() {
            return None;
        }
        let outcome = self.ready_terminal.take()?;
        self.emitted_terminal = Some(outcome.clone());
        self.phase = TurnPhase::Terminal;
        Some(outcome)
    }

    fn settle_tool(&mut self, tool_call_id: &str, ok: bool) -> ToolResultDisposition {
        if self.emitted_terminal.is_some() {
            self.late_results_ignored += 1;
            return ToolResultDisposition::LateIgnored;
        }
        let Some(obligation) = self.obligations.get_mut(tool_call_id) else {
            return ToolResultDisposition::UnknownToolCall;
        };
        if !obligation.status.is_open() {
            self.late_results_ignored += 1;
            obligation.status = ObligationStatus::LateIgnored;
            return ToolResultDisposition::LateIgnored;
        }
        obligation.status = if ok {
            ObligationStatus::Settled
        } else {
            ObligationStatus::Failed
        };
        if ok {
            self.last_settled_tool_name = Some(obligation.tool_name.clone());
            self.failed_tool_names.remove(&obligation.tool_name);
        } else {
            self.failed_tool_names.insert(obligation.tool_name.clone());
        }
        self.advance_after_obligation_change();
        ToolResultDisposition::Accepted
    }

    fn advance_after_obligation_change(&mut self) {
        if self.phase != TurnPhase::Terminal && self.open_obligation_count() == 0 {
            self.phase = TurnPhase::ContinuingAfterTool;
        }
    }
}

fn client_display_name(client: &str) -> &'static str {
    match client.trim().to_ascii_lowercase().as_str() {
        "zed" => "Zed",
        "cursor" => "Cursor",
        "vscode" | "code" => "VS Code",
        "" => "your editor",
        _ => "your editor",
    }
}

fn humanize_tool_name(tool_name: &str) -> String {
    if let Some(tool) = den_core::client_tools::ClientToolName::from_provider_alias(tool_name) {
        return tool.descriptor().title.to_string();
    }
    if let Some(rest) = tool_name.strip_prefix("mcp__") {
        let parts: Vec<&str> = rest.split("__").collect();
        if parts.len() >= 2 {
            let server = parts[0].replace('_', " ");
            let action = parts[1..].join(" ").replace('_', " ");
            return format!("{server}: {action}");
        }
    }
    tool_name.replace('_', " ")
}

#[cfg(test)]
mod tests;
