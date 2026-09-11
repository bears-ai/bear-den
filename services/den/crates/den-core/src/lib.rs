//! `den-core`: foundation leaf crate for the den workspace.
//!
//! Holds shared configuration, domain types, and errors with third-party
//! dependencies only. See `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md`.

pub mod agent_loop_control;
pub mod client_tools;
pub mod config;
pub mod conversation_ids;
pub mod effective_policy;
pub mod error;
pub mod governance;
pub mod ids;
pub mod metrics;
pub mod profile;

/// Model-facing tool surface: canonical/provider names, argument shapes, the
/// descriptor table + profile gating, capability traits, and the dispatcher.
/// (The concrete DB-backed executors live in the `den` binary's `core::tools`.)
pub mod tools;

pub mod model_request_policy;
pub use agent_loop_control::{AgentLoopControlLevel, ThinkingEffort};
pub use effective_policy::{ArmatureAvailability, BearCapability, CapabilitySet, EffectivePolicy};
pub use error::DenError;
pub use governance::{Governance, RunMode};
pub use ids::{BearId, ConversationId, SessionId, UserId};
pub use model_request_policy::{
    resolve_agent_primary_request_profile, AgentPrimaryStep, ModelRequestProfile,
};
pub use profile::{BearProfile, BearStance, TrustProfile};
