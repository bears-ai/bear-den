// Transitional compatibility re-exports while phase-0 naming is cleaned up.
pub use crate::core::runtime_contracts::{
    acp_requires_runtime, AcpConversationRuntime, AcpTurnRunner, CancelTurnRequest,
    CancelTurnResult, ContinueTurnRequest, ContinueTurnResult, EnsureConversationRequest,
    EnsureConversationResult, InteractionRunStore, RetrievalService, RoleProfileRegistry,
    RoleRunner, RoleRuntimeBinding, RuntimeApprovalDecision, RuntimeByteStream,
    RuntimeContinuation, RuntimeConversationRef, RuntimeErrorCategory, RuntimeEventStream,
    RuntimeHealthCheck, RuntimeHistoryRecord, RuntimeStartupCapabilities,
    RuntimeStreamContinuation, RuntimeStreamEvent, RuntimeToolResultStatus, RuntimeTurnRef,
    StartTurnRequest, StartTurnResult, ToolActuatorRegistry,
};
