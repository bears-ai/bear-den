//! Runtime-facing protocol re-exports.
//!
//! This module preserves the `runtime::provider::*` namespace used by Den runtime code while
//! keeping the canonical DTO/trait definitions in `den_protocol`, avoiding implementation → edge
//! dependency cycles.

pub use den_protocol::{
    classify_runtime_error, edge_gateway_requires_runtime,
    runtime_error_is_conflict_pending_approval, runtime_error_is_no_active_runs_cancel,
    CancelTurnRequest, CancelTurnResult, ContinueTurnRequest, ContinueTurnResult,
    EnsureConversationRequest, EnsureConversationResult, InteractionRunStore, RetrievalService,
    RoleRunner, RoleRuntimeBinding, RuntimeApprovalDecision, RuntimeByteStream,
    RuntimeContinuation, RuntimeConversationRef, RuntimeErrorCategory, RuntimeEventStream,
    RuntimeHealthCheck, RuntimeHistoryPage, RuntimeHistoryRecord, RuntimeSemanticEvent,
    RuntimeStartupCapabilities, RuntimeStreamContinuation, RuntimeStreamEvent,
    RuntimeToolResultStatus, RuntimeTurnRef, SessionConversationRuntime, StartTurnRequest,
    StartTurnResult, ToolActuatorRegistry,
};

#[cfg(test)]
mod tests;
