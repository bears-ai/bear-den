//! HTTP client for self-hosted Letta (agents + streaming messages).

mod agent_diagnostics;
mod agent_summary;
mod client;

pub use agent_diagnostics::{LettaAgentDiagnostics, LettaBlockRow, LettaToolRow};
pub use agent_summary::AgentSummary;
pub use client::{LettaClient, LettaModelOption, LettaToolOption};
