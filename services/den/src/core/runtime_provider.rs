// Transitional compatibility re-exports while phase-0 naming is cleaned up.
pub use crate::core::runtime_contracts::{
    acp_requires_letta_runtime, AcpConversationRuntime, AcpTurnRunner, CancelTurnRequest,
    ContinueTurnRequest, EnsureConversationRequest, EnsureConversationResult,
    InteractionRunStore, RetrievalService, RoleProfileRegistry, RoleRunner,
    RoleRuntimeBinding, RuntimeConversationRef, RuntimeErrorCategory, RuntimeHealthCheck,
    RuntimeHistoryRecord, RuntimeStartupCapabilities, RuntimeStreamEvent, RuntimeTurnRef,
    StartTurnRequest, ToolActuatorRegistry,
};
