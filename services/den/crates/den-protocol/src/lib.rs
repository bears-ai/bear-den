use bytes::Bytes;
use den_core::{config::Config, DenError};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

pub mod context_budget;

pub use context_budget::{
    ContextBudgetCalibrationReport, ContextBudgetComponentReport, ContextBudgetEstimatePrecision,
    ContextBudgetReport,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleRuntimeBinding {
    /// Den-owned opaque handle for the configured compatibility/runtime binding for a Bear role.
    pub binding_id: String,
    /// Transitional compatibility backend name (for diagnostics and migration only).
    /// Prefer treating this as a runtime/provider label rather than a concrete vendor name.
    pub compatibility_backend: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeConversationRef {
    /// Den-owned opaque runtime conversation handle. Backends may currently back this with a
    /// provider `conv-*` id, but protocol adapters should treat it as opaque.
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTurnRef {
    /// Den-owned opaque runtime turn handle. Backends may back this with a provider run id.
    pub id: String,
}

/// Opaque audit correlation supplied by the adapter that owns Work/Docket lookup.
/// Runtime persists these stable identifiers but does not inspect their state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointAuditContext {
    pub work_run_id: uuid::Uuid,
    pub docket_job_id: uuid::Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnsureConversationRequest {
    pub bear_id: uuid::Uuid,
    pub role: String,
    pub client_session_id: String,
    pub requested_selection: Option<String>,
    pub binding: RoleRuntimeBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnsureConversationResult {
    pub conversation: RuntimeConversationRef,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeHistoryRecord {
    Message {
        message_id: Option<String>,
        role: String,
        content: String,
        created_at: Option<String>,
    },
    ToolCall {
        message_id: Option<String>,
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        created_at: Option<String>,
    },
    ToolResult {
        message_id: Option<String>,
        tool_call_id: Option<String>,
        tool_name: Option<String>,
        status: Option<String>,
        content: Option<String>,
        structured_content: serde_json::Value,
        created_at: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHistoryPage {
    pub records: Vec<RuntimeHistoryRecord>,
    pub raw_payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTurnRequest {
    pub conversation: RuntimeConversationRef,
    pub binding: RoleRuntimeBinding,
    pub human_message: String,
    pub runtime_context: Option<String>,
    pub client_session_id: Option<String>,
    pub client_tools: Option<serde_json::Value>,
    pub stream_tokens: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeToolResultStatus {
    Ok,
    Error,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeApprovalDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeContinuation {
    ToolResult {
        tool_call_id: String,
        approval_request_id: Option<String>,
        status: RuntimeToolResultStatus,
        content: String,
    },
    ApprovalDecision {
        approval_request_id: String,
        tool_call_id: Option<String>,
        decision: RuntimeApprovalDecision,
        reason: Option<String>,
    },
    /// Docket, not the runtime, authorized another Pair slice for the same run.
    DocketBoundedSlice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinueTurnRequest {
    pub conversation: RuntimeConversationRef,
    pub turn: Option<RuntimeTurnRef>,
    pub binding: RoleRuntimeBinding,
    pub continuation: RuntimeContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelTurnRequest {
    pub conversation: RuntimeConversationRef,
    pub turn: Option<RuntimeTurnRef>,
    pub reason: Option<String>,
    pub binding: Option<RoleRuntimeBinding>,
    pub run_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTurnResult {
    pub turn: Option<RuntimeTurnRef>,
    pub stream: RuntimeStreamContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinueTurnResult {
    pub turn: Option<RuntimeTurnRef>,
    pub stream: RuntimeStreamContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeContinuationEnvelope {
    pub stream: RuntimeStreamContinuation,
    pub turn: Option<RuntimeTurnRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeStreamContinuation {
    Deferred,
    BytesSse,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeEventParser {
    pub parse_json_event: RuntimeParserFn,
}

pub type RuntimeByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, den_core::DenError>> + Send + 'static>>;
pub type RuntimeEventStream =
    Pin<Box<dyn Stream<Item = Result<RuntimeStreamEvent, den_core::DenError>> + Send + 'static>>;

pub type RuntimeParserFn = fn(&serde_json::Value) -> Option<RuntimeStreamEvent>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelTurnResult {
    pub skipped: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCleanupRequest {
    pub conversation: RuntimeConversationRef,
    pub binding: RoleRuntimeBinding,
    pub client_session_id: String,
    pub bear_id: uuid::Uuid,
    pub run_ids: Vec<String>,
    pub reason: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCleanupResult {
    pub payload: serde_json::Value,
}

/// Terminal status for a server or adapter tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallFinishStatus {
    Ok,
    Error,
    Incomplete,
    Cancelled,
}

impl ToolCallFinishStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Incomplete => "incomplete",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeSemanticEvent {
    AssistantTextDelta {
        text: String,
    },
    ReasoningTextDelta {
        text: String,
    },
    StatusText {
        text: String,
    },
    RunProgress {
        kind: String,
        text: Option<String>,
        phase: Option<String>,
        detail: Option<serde_json::Value>,
    },
    RunPaused {
        reason: String,
        resume_token: Option<String>,
        expires_at: Option<String>,
    },
    /// A durable Pair slice boundary. Unlike ordinary progress or a model turn
    /// completion, this explicitly says Docket must decide whether to continue.
    BoundedSlice {
        reason: String,
    },
    ToolCallRequested {
        tool_call_id: String,
        tool_name: String,
        title: Option<String>,
        kind: Option<String>,
        arguments: serde_json::Value,
        approval_request_id: Option<String>,
        approval_required: bool,
        approval_reason: Option<String>,
        run_id: Option<String>,
    },
    ToolCallFinished {
        tool_call_id: String,
        tool_name: String,
        status: ToolCallFinishStatus,
        summary: Option<String>,
        error_message: Option<String>,
    },
    Error {
        message: String,
        detail: Option<String>,
        error_type: Option<String>,
        request_id: Option<String>,
        context: Option<serde_json::Value>,
    },
    ConversationResolved {
        conversation: RuntimeConversationRef,
    },
    TurnCompleted {
        turn: Option<RuntimeTurnRef>,
    },
    TurnFailed {
        turn: Option<RuntimeTurnRef>,
        category: RuntimeErrorCategory,
        message: String,
    },
    TurnCancelled {
        turn: Option<RuntimeTurnRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeStreamEvent {
    Semantic(RuntimeSemanticEvent),
    /// Process-local liveness observation emitted when the provider stream yields bytes.
    /// Projection layers must not persist or expose this as transcript/BearWire content.
    ProviderActivity,
    UntranslatedProviderEvent {
        value: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeErrorCategory {
    Unavailable,
    Misconfigured,
    InvalidIdentity,
    PermissionDenied,
    ConflictPendingApproval,
    Cancelled,
    Timeout,
    BackendProtocol,
    Internal,
}

impl RuntimeErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Misconfigured => "misconfigured",
            Self::InvalidIdentity => "invalid_identity",
            Self::PermissionDenied => "permission_denied",
            Self::ConflictPendingApproval => "conflict_pending_approval",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::BackendProtocol => "backend_protocol",
            Self::Internal => "internal",
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait RuntimeHealthCheck {
    fn compatibility_backend_name(&self) -> &'static str;
    fn enabled(&self) -> bool;
    async fn check_health(&self) -> Result<String, DenError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStartupCapabilities {
    pub runtime_required_for_edge_gateway: bool,
}

impl RuntimeStartupCapabilities {
    pub fn from_config(config: &Config) -> Self {
        Self {
            runtime_required_for_edge_gateway: config.run_api,
        }
    }
}

pub fn edge_gateway_requires_runtime(config: &Config) -> bool {
    RuntimeStartupCapabilities::from_config(config).runtime_required_for_edge_gateway
}

#[allow(async_fn_in_trait)]
pub trait SessionConversationRuntime {
    async fn ensure_session_conversation(
        &self,
        request: EnsureConversationRequest,
    ) -> Result<EnsureConversationResult, DenError>;

    async fn load_history(
        &self,
        binding: &RoleRuntimeBinding,
        conversation: &RuntimeConversationRef,
    ) -> Result<RuntimeHistoryPage, DenError>;
}

/// Runtime conversation materialization backend (create, verify, load_history).
#[allow(async_fn_in_trait)]
pub trait RuntimeConversationBackend {
    async fn create_conversation(
        &self,
        binding: &RoleRuntimeBinding,
    ) -> Result<RuntimeConversationRef, DenError>;

    async fn verify_conversation_belongs_to_binding(
        &self,
        binding: &RoleRuntimeBinding,
        conversation_id: &str,
    ) -> Result<(), DenError>;

    async fn load_history(
        &self,
        binding: &RoleRuntimeBinding,
        conversation: &RuntimeConversationRef,
    ) -> Result<RuntimeHistoryPage, DenError>;
}

#[allow(async_fn_in_trait)]
pub trait RoleRunner {
    async fn check_health(&self) -> Result<String, DenError>;
}

#[allow(async_fn_in_trait)]
pub trait InteractionRunStore {
    async fn check_health(&self) -> Result<String, DenError>;
}

/// Marker for registries that expose tool actuators to a runtime boundary.
///
/// Concrete registries live in runtime/service crates; this protocol trait keeps the
/// dependency direction from shared contracts toward implementations one-way.
pub trait ToolActuatorRegistry {}

#[allow(async_fn_in_trait)]
pub trait RetrievalService {
    async fn check_health(&self) -> Result<String, DenError>;
}

/// Classify a runtime-facing error into a stable Den-owned category for runner policy.
pub fn classify_runtime_error(err: &DenError) -> RuntimeErrorCategory {
    let message = err.to_string().to_ascii_lowercase();
    if message.contains("waiting on an unresolved tool approval")
        || message.contains("waiting for approval")
        || message.contains("please approve or deny")
        || message.contains("requires_approval")
    {
        RuntimeErrorCategory::ConflictPendingApproval
    } else if message.contains("no active runs to cancel") {
        RuntimeErrorCategory::Cancelled
    } else if message.contains("not configured") || message.contains("runtime is not configured") {
        RuntimeErrorCategory::Misconfigured
    } else if message.contains("not found for this bear") {
        RuntimeErrorCategory::InvalidIdentity
    } else if matches!(err, DenError::Authorization(_)) {
        RuntimeErrorCategory::PermissionDenied
    } else if message.contains("timed out") || message.contains("timeout") {
        RuntimeErrorCategory::Timeout
    } else if message.contains("unavailable") {
        RuntimeErrorCategory::Unavailable
    } else {
        RuntimeErrorCategory::Internal
    }
}

pub fn runtime_error_is_conflict_pending_approval(err: &DenError) -> bool {
    classify_runtime_error(err) == RuntimeErrorCategory::ConflictPendingApproval
}

pub fn runtime_error_is_no_active_runs_cancel(err: &DenError) -> bool {
    err.to_string()
        .to_ascii_lowercase()
        .contains("no active runs to cancel")
}

#[cfg(test)]
mod tests {
    use super::RuntimeErrorCategory;

    #[test]
    fn runtime_error_category_preserves_wire_strings() {
        let cases = [
            (RuntimeErrorCategory::Unavailable, "unavailable"),
            (RuntimeErrorCategory::Misconfigured, "misconfigured"),
            (RuntimeErrorCategory::InvalidIdentity, "invalid_identity"),
            (RuntimeErrorCategory::PermissionDenied, "permission_denied"),
            (
                RuntimeErrorCategory::ConflictPendingApproval,
                "conflict_pending_approval",
            ),
            (RuntimeErrorCategory::Cancelled, "cancelled"),
            (RuntimeErrorCategory::Timeout, "timeout"),
            (RuntimeErrorCategory::BackendProtocol, "backend_protocol"),
            (RuntimeErrorCategory::Internal, "internal"),
        ];

        for (category, expected) in cases {
            assert_eq!(category.as_str(), expected);
        }
    }
}
