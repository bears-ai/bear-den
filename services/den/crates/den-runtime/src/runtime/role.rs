use serde_json::{json, Value};
use uuid::Uuid;

use den_core::DenError;
use den_service::{
    tool_turns::ToolTurnCoordinator,
    turn_controller::{ActiveTurnCancelHandle, ActiveTurnCancelRegistry},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleRuntimeRole {
    Pair,
    Work,
    Chat,
    Curate,
    Watch,
}

impl RoleRuntimeRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pair => "pair",
            Self::Work => "work",
            Self::Chat => "chat",
            Self::Curate => "curate",
            Self::Watch => "watch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleRuntimeChannelKind {
    ClientSession,
    BearChannel,
    Workplace,
    Task,
}

impl RoleRuntimeChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientSession => "client_session",
            Self::BearChannel => "bear_channel",
            Self::Workplace => "workplace",
            Self::Task => "task",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoleTurnScope {
    pub bear_id: Uuid,
    pub role: RoleRuntimeRole,
    pub channel_kind: RoleRuntimeChannelKind,
    pub channel_id: String,
    pub conversation_id: Option<String>,
}

impl RoleTurnScope {
    pub fn client_pair(
        bear_id: Uuid,
        client_session_id: impl Into<String>,
        conversation_id: Option<String>,
    ) -> Self {
        Self {
            bear_id,
            role: RoleRuntimeRole::Pair,
            channel_kind: RoleRuntimeChannelKind::ClientSession,
            channel_id: client_session_id.into(),
            conversation_id,
        }
    }

    pub fn diagnostic(&self) -> Value {
        json!({
            "bear_id": self.bear_id,
            "role": self.role.as_str(),
            "channel_kind": self.channel_kind.as_str(),
            "channel_id": self.channel_id,
            "conversation_id": self.conversation_id,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnResultStatus {
    Ok,
    Failed,
    Recovered,
    NeedsNewSession,
    Cancelled,
}

impl TurnResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Recovered => "recovered",
            Self::NeedsNewSession => "needs_new_session",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnResultReason {
    StreamComplete,
    RuntimeCleanup,
    CompactedRetry,
    StaleApproval,
    Timeout,
    TurnAlreadyActive,
    Cancelled,
}

impl TurnResultReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StreamComplete => "stream_complete",
            Self::RuntimeCleanup => "runtime_cleanup",
            Self::CompactedRetry => "compacted_retry",
            Self::StaleApproval => "stale_approval",
            Self::Timeout => "timeout",
            Self::TurnAlreadyActive => "turn_already_active",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoleTurnResult {
    pub status: TurnResultStatus,
    pub reason: TurnResultReason,
    pub request_id: Uuid,
    pub scope: RoleTurnScope,
    pub retryable: bool,
    pub diagnostics: Value,
}

#[derive(Debug, Clone)]
pub struct RoleTurnTerminalEvent {
    pub status: String,
    pub reason: String,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub retryable: bool,
    pub diagnostics: Value,
}

impl RoleTurnResult {
    pub fn to_terminal_event(&self) -> RoleTurnTerminalEvent {
        RoleTurnTerminalEvent {
            status: self.status.as_str().to_string(),
            reason: self.reason.as_str().to_string(),
            request_id: Some(self.request_id.to_string()),
            session_id: Some(self.scope.channel_id.clone()),
            retryable: self.retryable,
            diagnostics: json!({
                "scope": self.scope.diagnostic(),
                "details": self.diagnostics,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoleRuntime {
    tool_turns: ToolTurnCoordinator,
    turn_cancellations: Option<ActiveTurnCancelRegistry>,
}

#[derive(Debug, Clone)]
pub struct ClientTurnLifecycleRuntime {
    role_runtime: RoleRuntime,
}

#[derive(Debug, Clone)]
pub struct ClientTurnLifecycleContext {
    pub bear_id: Uuid,
    pub client_session_id: String,
    pub run_id: String,
    pub resolved_conversation_id: Option<String>,
}

#[derive(Debug)]
pub struct ClientTurnLifecycleLease {
    pub role_runtime: RoleRuntime,
    pub turn_scope: RoleTurnScope,
    pub active_turn_guard: RoleTurnGuard,
    pub cancel_handle: ActiveTurnCancelHandle,
    pub cancel_rx: tokio::sync::watch::Receiver<bool>,
}

impl ClientTurnLifecycleRuntime {
    pub fn new(
        tool_turns: ToolTurnCoordinator,
        turn_cancellations: ActiveTurnCancelRegistry,
    ) -> Self {
        Self {
            role_runtime: RoleRuntime::with_turn_cancellations(tool_turns, turn_cancellations),
        }
    }

    pub fn runtime(&self) -> &RoleRuntime {
        &self.role_runtime
    }

    pub fn acquire_pair_turn(
        &self,
        context: ClientTurnLifecycleContext,
        request_id: Uuid,
    ) -> Result<ClientTurnLifecycleLease, DenError> {
        let turn_scope = RoleTurnScope::client_pair(
            context.bear_id,
            context.client_session_id,
            context.resolved_conversation_id,
        );
        let active_turn_guard = self
            .role_runtime
            .acquire_turn(turn_scope.clone(), request_id)?;
        let (cancel_handle, cancel_rx) = self
            .role_runtime
            .turn_cancellations()
            .ok_or_else(|| {
                DenError::System(
                    "client turn lifecycle runtime requires cancellation registry".to_string(),
                )
            })?
            .register(
                turn_scope.channel_id.clone(),
                context.run_id,
                request_id,
                turn_scope.conversation_id.clone(),
            );
        Ok(ClientTurnLifecycleLease {
            role_runtime: self.role_runtime.clone(),
            turn_scope,
            active_turn_guard,
            cancel_handle,
            cancel_rx,
        })
    }
}

impl RoleRuntime {
    pub fn new(tool_turns: ToolTurnCoordinator) -> Self {
        Self {
            tool_turns,
            turn_cancellations: None,
        }
    }

    pub fn with_turn_cancellations(
        tool_turns: ToolTurnCoordinator,
        turn_cancellations: ActiveTurnCancelRegistry,
    ) -> Self {
        Self {
            tool_turns,
            turn_cancellations: Some(turn_cancellations),
        }
    }

    pub fn turn_cancellations(&self) -> Option<&ActiveTurnCancelRegistry> {
        self.turn_cancellations.as_ref()
    }

    pub fn tool_turn_runtime_snapshot(
        &self,
        client_session_id: &str,
        run_id: &str,
        tool_turns: &ToolTurnCoordinator,
    ) -> Value {
        if let Some(registry) = self.turn_cancellations.as_ref() {
            registry.runtime_snapshot_for_run(client_session_id, run_id, tool_turns)
        } else {
            json!({
                "state": "idle",
                "active_turn": {
                    "present": false,
                    "phase": Value::Null,
                    "pending_obligations": 0,
                    "pending_adapter_tools": 0,
                    "pending_den_tools": 0,
                    "pending_permissions": 0,
                },
                "last_terminal": Value::Null,
                "last_recovery": Value::Null,
                "source": "role_runtime_no_active_turn_registry",
            })
        }
    }

    pub fn acquire_turn(
        &self,
        scope: RoleTurnScope,
        request_id: Uuid,
    ) -> Result<RoleTurnGuard, DenError> {
        let guard = self.tool_turns.acquire_active_turn(
            &scope.channel_id,
            request_id,
            scope.conversation_id.clone(),
        )?;
        Ok(RoleTurnGuard { guard })
    }

    pub fn pending_diagnostics(&self, scope: &RoleTurnScope) -> Value {
        self.tool_turns.diagnostic_snapshot(&scope.channel_id)
    }

    pub fn timeout_denial_message(&self, tool_name: &str, timeout_ms: u64) -> String {
        format!(
            "BEARS denied this approval automatically because `{tool_name}` timed out after {timeout_ms}ms."
        )
    }

    pub fn turn_result(
        &self,
        status: TurnResultStatus,
        reason: TurnResultReason,
        request_id: Uuid,
        scope: RoleTurnScope,
        retryable: bool,
        diagnostics: Value,
    ) -> RoleTurnResult {
        RoleTurnResult {
            status,
            reason,
            request_id,
            scope,
            retryable,
            diagnostics,
        }
    }
}

#[derive(Debug)]
pub struct RoleTurnGuard {
    guard: den_service::tool_turns::ActiveTurnGuard,
}

impl RoleTurnGuard {
    pub fn release(self) {
        self.guard.release();
    }
}
