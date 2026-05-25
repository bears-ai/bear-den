//! HTTP client for self-hosted Letta (agents + streaming messages).

mod agent_diagnostics;
mod agent_document;
mod agent_prefill;
mod agent_summary;
mod assistant_display;
mod client;
mod conversation_title;
mod conversations_list;
pub mod tool_policy;

pub use agent_diagnostics::{LettaAgentDiagnostics, LettaBlockRow, LettaToolRow};
pub use agent_document::unwrap_letta_agent_document;
pub use agent_prefill::AgentBearPrefill;
pub use agent_summary::AgentSummary;
pub use assistant_display::{
    normalize_display_status_text, sanitize_visible_transcript_text, strip_letta_harness_for_user,
};
pub use client::{
    LettaAgentListItem, LettaClient, LettaCompatibilityHealthCheck, LettaContinuationContext,
    LettaCreateAgentParams, LettaModelOption, LettaPatchAgentParams, LettaPendingApproval,
    LettaToolOption, PendingApprovalDenialMode,
};
pub use conversation_title::{
    display_conversation_title, first_user_message_text_for_title, is_acceptable_derived_title,
    is_meaningful_conversation_title, UNTITLED_THREAD,
};
pub use conversations_list::{
    conversation_is_archived, letta_conversations_top_array, load_agent_conversations,
    AgentConversationsSnapshot, LettaConversationRow,
};
pub use tool_policy::{filter_legacy_memory_tool_ids, is_legacy_memory_tool_name};
