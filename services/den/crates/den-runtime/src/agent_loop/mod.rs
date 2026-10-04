//! Den-native in-process ReAct agent loop ([ADR-0035](../../../docs/decisions/adr-0035-den-native-in-process-agent-runtime.md)).

mod approvals;
mod assembler;
mod budget;
mod checkpoints;
mod context;
mod control;
mod key_memory_projection;
#[cfg(test)]
mod key_memory_projection_tests;
mod overflow_retry;
mod pending_tools;
mod policy;
mod recall_scope;
mod runtime_context;
mod session_store;
mod session_stream;
pub(crate) mod source_admission;
mod step;
mod strategy;
mod tool_outcome;
mod tool_policy;
mod transcript;

pub use source_admission::{require_ordinary_session_source, OrdinarySessionSource};
pub use step::RUNTIME_CHECKPOINT_TOOL_NAME;

pub use tool_outcome::{
    classify_tool_result_content, is_incomplete_tool_result,
    is_legacy_synthetic_interrupted_tool_result, tool_call_finished_event,
    tool_call_finished_event_for_content, tool_call_finished_event_for_incomplete,
    tool_message_counts_toward_llm_resolution, tool_result_content_indicates_error,
    tool_result_persistence_status, user_visible_tool_error_summary, user_visible_tool_summary,
    ToolResultContentKind, INCOMPLETE_TOOL_RESULT_MARK, LEGACY_SYNTHETIC_TOOL_RESULT_UNAVAILABLE,
};

pub use approvals::{
    create_native_approval, decide_native_approval, NativeApprovalDecision, NativeApprovalRow,
};
pub use assembler::{
    assemble_native_turn, assemble_native_turn_for_bear, assemble_native_turn_messages,
    assemble_native_turn_messages_for_bear, projected_memory_session_diagnostic,
    recalled_memory_session_diagnostic, AssembleTurnContext, AssembledNativeTurn,
};
pub use budget::{
    classify_tool_budget_class, evaluate_turn_budget, evaluate_turn_context_budget, tool_signature,
    tool_signature_from_call, PostMutationVerificationWindow, ToolBudgetClass,
    ToolCallBudgetLimits, ToolCallBudgetUsage, ToolContinuationObservation, TurnBudgetEvaluation,
    TurnBudgetPolicy, TurnBudgetState, TurnBudgetStopReason, TurnBudgetWarning,
};
pub use checkpoints::{
    aggregate_loop_control_replay_turns, compare_loop_control_replay,
    compare_loop_control_replay_profile, compare_loop_control_replay_turns,
    context_budget_pressure_action, latest_grounding_probe_signal_for_run,
    latest_grounding_probe_signal_for_tool_call, list_checkpoints_for_run,
    list_checkpoints_for_session, list_loop_control_decisions_for_run,
    non_empty_diff_grounding_probe, purge_loop_control_decisions_before, record_checkpoint_request,
    record_checkpoint_response, record_context_budget_pressure_decision,
    record_grounding_probe_result_decision, record_loop_control_decision,
    replay_loop_control_observations, summarize_loop_control_replay_profile,
    summarize_loop_control_replay_profile_for_run, summarize_recent_loop_control_replay_profile,
    CheckpointArtifactInput, CheckpointArtifactRow, CheckpointReplayPolicy,
    CheckpointResponseInput, CheckpointValidationStatus, CheckpointVisibility,
    ContextBudgetPressureLevel, ExpectedLoopControlReplayProfileSummary,
    ExpectedLoopControlReplayTurn, GroundingProbeFinding, GroundingProbeResultInput,
    GroundingProbeSignalKind, LedgerEvidenceRef, LoopControlDecisionKind, LoopControlLedgerInput,
    LoopControlLedgerRow, LoopControlReplayCount, LoopControlReplayMismatch,
    LoopControlReplayObservation, LoopControlReplayProfileMismatch,
    LoopControlReplayProfileSummary, LoopControlReplayTurn, LoopControlReplayTurnMismatch,
};
pub use context::{
    assemble_agent_messages, load_transcript_grouping_rows, load_transcript_messages,
    prune_messages_for_native_conversation, repair_tool_call_message_chain,
};
pub use control::{
    agent_loop_control_profile_fingerprint, evaluate_checkpoint_trigger,
    objective_orientation_allowed_for_origin, pre_risk_checkpoint_trigger,
    resolve_agent_loop_control, resolve_objective_orientation, validate_checkpoint_response,
    AgentLoopControlProfile, AgentLoopControlResolutionInput, AgentLoopControlSource,
    CheckpointConfidence, CheckpointEvaluation, CheckpointEvidenceRef, CheckpointField,
    CheckpointNextAction, CheckpointPolicy, CheckpointReason, CheckpointResponseValidationError,
    CheckpointState, CheckpointTaskContext, CheckpointThinkingPolicy, CheckpointTrigger,
    DocketExecutionOrientation, FreeformPolicy, KoPolicy, ObjectiveOrientation,
    ObjectiveOrientationResolutionInput, OrientationTaskRef, OrientedChildTaskPolicy,
    ResolvedAgentLoopControl, RuntimeCheckpointRequest, RuntimeCheckpointResponse, TaskOrientation,
    TaskStateChangeIntent, DEFAULT_ORIENTED_MAX_CHILDREN, DEFAULT_ORIENTED_MAX_DEPTH,
};
pub use key_memory_projection::{
    project_key_memory, KeyMemoryProjectionCacheKey, KeyMemoryProjectionResult,
};
pub use overflow_retry::compact_session_messages_for_overflow;
pub use pending_tools::pending_tool_calls;
pub use policy::{
    select_strategy_profile, StrategyDifficulty, StrategyPolicyInput, StrategyTaskKind,
};
pub use session_store::{agent_loop_session_key, AgentLoopSession, AgentLoopSessionStore};
pub use session_stream::{NativeToolDispatchMode, SessionTrackingStream};
pub use step::{native_llm_handshake_timeout, run_agent_step_stream, AgentStepOverflowContext};
pub use strategy::StrategyProfile;
pub use tool_policy::{
    maybe_pause_for_tool_approval, provider_tool_is_den_web_fetch, provider_tool_requires_approval,
    provider_tool_supports_unilateral_execution, record_approval_decision,
};
pub use transcript::{
    spawn_persist_incomplete_acp_tool_results, spawn_persist_web_chat_interrupted_turn,
    spawn_persist_web_chat_turn,
};
