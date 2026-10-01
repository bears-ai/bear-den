//! Den-native in-process turn runtime ([ADR-0035](../../../docs/decisions/adr-0035-den-native-in-process-agent-runtime.md)).
//!
//! `start_native_turn_event_stream` accepts Den-verified origin for BearWire turns;
//! browser chat and internal Curate use their own typed entry paths. Continuations
//! recover the compatibility profile from the originating native session and
//! recheck live Work eligibility; model request metadata still carries its label. Rule-based
//! Curate (`memory_curate_executor`) may add an internal briefing turn via
//! `run_native_curate_briefing_collect_assistant_text`.

pub mod legacy_memory_tools;
mod openai_stream;
#[cfg(test)]
mod openai_stream_tests;
mod profile;
mod profile_briefing;
mod search_availability;
pub mod tool_invoker;
mod tools;
mod turn;
mod web_chat_loop;

pub use tool_invoker::{set_tool_invoker, tool_invoker, RuntimeToolInvocation, RuntimeToolInvoker};

pub use openai_stream::{
    openai_byte_stream_to_event_stream, openai_byte_stream_to_event_stream_with_telemetry,
    responses_byte_stream_to_event_stream, responses_byte_stream_to_event_stream_with_telemetry,
    ObservedPromptTokensSink,
};
pub use profile::{is_native_api_direct_role, NativeCapabilityProfile};
pub use profile_briefing::compose_curate_briefing_prompt;
pub use tools::{is_task_definition_or_delegation_tool_provider_name, merge_den_and_client_tools};
pub use turn::{
    continue_native_client_turn_event_stream, native_client_run_exists,
    record_native_client_tool_result, remove_native_client_run,
    run_native_curate_briefing_collect_assistant_text, start_native_client_turn_event_stream,
    start_native_turn_event_stream, start_native_web_chat_turn_event_stream,
    update_native_client_session_cached_activity_plan_projection, NativeRuntimeConversationBackend,
    NativeRuntimeDeps, NativeWebChatTurnParams,
};
#[cfg(feature = "test-fixtures")]
pub use turn::{
    scripted_runtime_invocation_count, set_next_scripted_runtime_streams,
    set_scripted_runtime_streams_for_run, ScriptedRuntimeStream,
};
