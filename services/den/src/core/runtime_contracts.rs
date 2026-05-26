use crate::{config::Config, errors::CustomError};
use bytes::Bytes;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleRuntimeBinding {
    /// Den-owned opaque handle for the configured compatibility/runtime binding for a Bear role.
    pub binding_id: String,
    /// Transitional compatibility backend name (for diagnostics and migration only).
    pub compatibility_backend: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeConversationRef {
    /// Den-owned opaque runtime conversation handle. Backends may currently back this with a
    /// Letta `conv-*` id, but ACP should treat it as opaque.
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTurnRef {
    /// Den-owned opaque runtime turn handle. Backends may back this with a provider run id.
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnsureConversationRequest {
    pub bear_id: uuid::Uuid,
    pub role: String,
    pub acp_session_id: String,
    pub requested_selection: Option<String>,
    pub binding: RoleRuntimeBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnsureConversationResult {
    pub conversation: RuntimeConversationRef,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHistoryRecord {
    pub message_id: Option<String>,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTurnRequest {
    pub conversation: RuntimeConversationRef,
    pub binding: RoleRuntimeBinding,
    pub human_message: String,
    pub runtime_context: Option<String>,
    pub acp_session_id: Option<String>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinueTurnResult {
    pub turn: Option<RuntimeTurnRef>,
    pub stream: RuntimeStreamContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeStreamContinuation {
    Deferred,
    BytesSse,
}

pub type RuntimeByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, crate::errors::CustomError>> + Send + 'static>>;
pub type RuntimeEventStream = Pin<
    Box<
        dyn Stream<Item = Result<RuntimeStreamEvent, crate::errors::CustomError>> + Send + 'static,
    >,
>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelTurnResult {
    pub skipped: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeStreamEvent {
    RawSseFrame { frame_body: Vec<u8> },
    JsonValue { value: serde_json::Value },
    ConversationResolved { conversation: RuntimeConversationRef },
    AssistantTextDelta { text: String },
    AssistantMessageCompleted { message_id: Option<String> },
    ToolCallRequested {
        tool_call_id: String,
        tool_name: String,
        arguments_json: String,
        approval_required: bool,
    },
    ToolCallSettled { tool_call_id: String, status: String },
    WaitingForContinuation { turn: Option<RuntimeTurnRef> },
    TurnCompleted { turn: Option<RuntimeTurnRef> },
    TurnFailed {
        turn: Option<RuntimeTurnRef>,
        category: RuntimeErrorCategory,
        message: String,
    },
    TurnCancelled { turn: Option<RuntimeTurnRef> },
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

#[allow(async_fn_in_trait)]
pub trait RuntimeHealthCheck {
    fn compatibility_backend_name(&self) -> &'static str;
    fn enabled(&self) -> bool;
    async fn check_health(&self) -> Result<String, CustomError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStartupCapabilities {
    pub acp_gateway_enabled: bool,
    pub letta_required_for_acp: bool,
}

impl RuntimeStartupCapabilities {
    pub fn from_config(config: &Config) -> Self {
        Self {
            acp_gateway_enabled: config.acp_gateway_enabled,
            letta_required_for_acp: config.acp_gateway_enabled,
        }
    }
}

pub fn acp_requires_letta_runtime(config: &Config) -> bool {
    RuntimeStartupCapabilities::from_config(config).letta_required_for_acp
}

#[allow(async_fn_in_trait)]
pub trait RoleProfileRegistry {
    async fn resolve_compatibility_binding(
        &self,
        bear_id: uuid::Uuid,
        role: &str,
    ) -> Result<Option<RoleRuntimeBinding>, CustomError>;
}

#[allow(async_fn_in_trait)]
pub trait AcpConversationRuntime {
    async fn ensure_session_conversation(
        &self,
        request: EnsureConversationRequest,
    ) -> Result<EnsureConversationResult, CustomError>;

    async fn load_history(
        &self,
        binding: &RoleRuntimeBinding,
        conversation: &RuntimeConversationRef,
    ) -> Result<Vec<RuntimeHistoryRecord>, CustomError>;
}

#[allow(async_fn_in_trait)]
pub trait AcpTurnRunner {
    async fn preflight_hygiene(
        &self,
        binding: &RoleRuntimeBinding,
        conversation: Option<&RuntimeConversationRef>,
        reason: &str,
    ) -> Result<(), CustomError>;

    async fn start_turn(&self, request: StartTurnRequest) -> Result<StartTurnResult, CustomError>;

    async fn continue_turn(
        &self,
        request: ContinueTurnRequest,
    ) -> Result<ContinueTurnResult, CustomError>;

    async fn cancel_turn(
        &self,
        request: CancelTurnRequest,
    ) -> Result<CancelTurnResult, CustomError>;
}

#[allow(async_fn_in_trait)]
pub trait RoleRunner {
    async fn check_health(&self) -> Result<String, CustomError>;
}

#[allow(async_fn_in_trait)]
pub trait InteractionRunStore {
    async fn check_health(&self) -> Result<String, CustomError>;
}

pub trait ToolActuatorRegistry {}

#[allow(async_fn_in_trait)]
pub trait RetrievalService {
    async fn check_health(&self) -> Result<String, CustomError>;
}
