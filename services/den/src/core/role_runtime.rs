use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    core::{
        acp_tool_turns::AcpToolTurnCoordinator, acp_turn_controller::AcpActiveTurnCancelRegistry,
    },
    errors::CustomError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleRuntimeRole {
    Pair,
    Work,
    Talk,
    Curate,
    Watch,
}

impl RoleRuntimeRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pair => "pair",
            Self::Work => "work",
            Self::Talk => "talk",
            Self::Curate => "curate",
            Self::Watch => "watch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleRuntimeChannelKind {
    AcpSession,
    BearChannel,
    Workplace,
    Task,
}

impl RoleRuntimeChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcpSession => "acp_session",
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
    pub fn acp_pair(
        bear_id: Uuid,
        acp_session_id: impl Into<String>,
        conversation_id: Option<String>,
    ) -> Self {
        Self {
            bear_id,
            role: RoleRuntimeRole::Pair,
            channel_kind: RoleRuntimeChannelKind::AcpSession,
            channel_id: acp_session_id.into(),
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

impl RoleTurnResult {
    pub fn to_event_fields(&self) -> (String, String, Option<String>, Option<String>, bool, Value) {
        (
            self.status.as_str().to_string(),
            self.reason.as_str().to_string(),
            Some(self.request_id.to_string()),
            Some(self.scope.channel_id.clone()),
            self.retryable,
            json!({
                "scope": self.scope.diagnostic(),
                "details": self.diagnostics,
            }),
        )
    }
}

#[derive(Debug, Clone)]
pub struct RoleRuntime {
    tool_turns: AcpToolTurnCoordinator,
    turn_cancellations: Option<AcpActiveTurnCancelRegistry>,
}

#[derive(Debug, Clone)]
pub struct AcpTurnLifecycleRuntime {
    role_runtime: RoleRuntime,
}

#[derive(Debug, Clone)]
pub struct AcpTurnLifecycleContext {
    pub bear_id: Uuid,
    pub acp_session_id: String,
    pub resolved_conversation_id: Option<String>,
}

#[derive(Debug)]
pub struct AcpTurnLifecycleLease {
    pub role_runtime: RoleRuntime,
    pub turn_scope: RoleTurnScope,
    pub active_turn_guard: RoleTurnGuard,
}

impl AcpTurnLifecycleRuntime {
    pub fn new(tool_turns: AcpToolTurnCoordinator, turn_cancellations: AcpActiveTurnCancelRegistry) -> Self {
        Self {
            role_runtime: RoleRuntime::with_turn_cancellations(tool_turns, turn_cancellations),
        }
    }

    pub fn runtime(&self) -> &RoleRuntime {
        &self.role_runtime
    }

    pub fn acquire_pair_turn(
        &self,
        context: AcpTurnLifecycleContext,
        request_id: Uuid,
    ) -> Result<AcpTurnLifecycleLease, CustomError> {
        let turn_scope = RoleTurnScope::acp_pair(
            context.bear_id,
            context.acp_session_id,
            context.resolved_conversation_id,
        );
        let active_turn_guard = self.role_runtime.acquire_turn(turn_scope.clone(), request_id)?;
        Ok(AcpTurnLifecycleLease {
            role_runtime: self.role_runtime.clone(),
            turn_scope,
            active_turn_guard,
        })
    }
}

impl RoleRuntime {
    pub fn new(tool_turns: AcpToolTurnCoordinator) -> Self {
        Self {
            tool_turns,
            turn_cancellations: None,
        }
    }

    pub fn with_turn_cancellations(
        tool_turns: AcpToolTurnCoordinator,
        turn_cancellations: AcpActiveTurnCancelRegistry,
    ) -> Self {
        Self {
            tool_turns,
            turn_cancellations: Some(turn_cancellations),
        }
    }

    pub fn turn_cancellations(&self) -> Option<&AcpActiveTurnCancelRegistry> {
        self.turn_cancellations.as_ref()
    }

    pub fn tool_turn_runtime_snapshot(
        &self,
        acp_session_id: &str,
        tool_turns: &AcpToolTurnCoordinator,
    ) -> Value {
        if let Some(registry) = self.turn_cancellations.as_ref() {
            registry.runtime_snapshot_for_session(acp_session_id, tool_turns)
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
    ) -> Result<RoleTurnGuard, CustomError> {
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
    guard: crate::core::acp_tool_turns::AcpActiveTurnGuard,
}

impl RoleTurnGuard {
    pub fn release(self) {
        self.guard.release();
    }
}
