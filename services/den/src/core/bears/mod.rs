//! Bear registry and membership (Phase 1).
//! Admin HTTP routes and Letta provisioning wire up in later milestones.

pub mod context_composition;
pub mod db;
pub mod letta_code_harness;
pub mod letta_drift;
pub mod model;
pub mod provision;
pub mod rollout;
pub mod runtime_plan;
pub mod sync;
pub mod templates;

pub use context_composition::{
    compose_role_context, context_profile_from_json, context_profile_to_json,
    default_role_contracts_for_bear, BearContextProfile, ComposedRoleContext, RoleContracts,
};
pub use letta_drift::{
    compute_letta_drift, compute_letta_drift_with_expected_tool_ids, LettaDriftFlags,
};
pub use model::{
    Bear, BearAgent, BearAgentRole, BearSkillManifestEntry, BearSkillProposal, BearWithMembership,
};
pub use runtime_plan::{default_runtime_plan, effective_runtime_plan};
