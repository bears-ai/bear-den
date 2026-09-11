//! Den-native in-process turn runtime ([ADR-0035](../../../docs/decisions/adr-0035-den-native-in-process-agent-runtime.md)).
//!
//! `start_native_profile_turn_event_stream` / `continue_native_profile_turn_event_stream` are the
//! capability-profile entry points for API-direct operating profiles (`pair`, `curate`; `watch`
//! stays rule-based). Browser web chat uses `start_native_web_chat_turn_event_stream` for
//! `BearProfile::Chat` when `AGENT_RUNTIME=native`. Rule-based curate (`memory_curate_executor`)
//! runs first; when briefing items remain under native runtime,
//! `run_native_profile_turn_collect_assistant_text` can add an LLM briefing turn projected into the
//! memory_curate conversation.

pub mod legacy_memory_tools;
mod openai_stream;
#[cfg(test)]
mod openai_stream_tests;
mod profile;
mod profile_briefing;
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
pub use tools::{
    chat_turn_is_capabilities_meta_query, is_task_definition_or_delegation_tool_provider_name,
    merge_den_and_client_tools,
};
pub use turn::{
    continue_native_client_turn_event_stream, continue_native_profile_turn_event_stream,
    native_client_session_cached_activity_plan_projection, native_client_session_exists,
    native_client_session_runtime_state, record_native_client_tool_result,
    run_native_profile_turn_collect_assistant_text, start_native_client_turn_event_stream,
    start_native_profile_turn_event_stream, start_native_web_chat_turn_event_stream,
    take_session_overflow_compaction_recovered,
    update_native_client_session_cached_activity_plan_projection, NativeRuntimeConversationBackend,
    NativeRuntimeDeps, NativeWebChatTurnParams,
};
#[cfg(feature = "test-fixtures")]
pub use turn::{
    scripted_runtime_invocation_count, set_next_scripted_runtime_streams,
    set_scripted_runtime_streams_for_run, ScriptedRuntimeStream,
};
