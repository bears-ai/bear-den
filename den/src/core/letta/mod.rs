//! HTTP client for self-hosted Letta (agents + streaming messages).

mod agent_diagnostics;
mod agent_prefill;
mod agent_summary;
mod client;
mod conversation_title;
mod conversations_list;

pub use agent_diagnostics::{LettaAgentDiagnostics, LettaBlockRow, LettaToolRow};
pub use agent_prefill::AgentBearPrefill;
pub use agent_summary::AgentSummary;
pub use client::{LettaAgentListItem, LettaClient, LettaModelOption, LettaToolOption};
pub use conversation_title::{
    display_conversation_title, first_user_message_text_for_title, is_acceptable_derived_title,
    is_meaningful_conversation_title, UNTITLED_THREAD,
};
pub use conversations_list::{
    load_agent_conversations, AgentConversationsSnapshot, LettaConversationRow,
    conversation_is_archived, letta_conversations_top_array,
};
