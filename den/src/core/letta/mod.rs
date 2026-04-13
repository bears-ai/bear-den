//! HTTP client for self-hosted Letta (agents + streaming messages).

mod agent_summary;
mod client;

pub use agent_summary::AgentSummary;
pub use client::{LettaClient, LettaModelOption};
