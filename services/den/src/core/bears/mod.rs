//! Bear registry and membership (Phase 1).
//! Admin HTTP routes and Letta provisioning wire up in later milestones.

pub mod db;
pub mod letta_code_harness;
pub mod letta_drift;
pub mod model;
pub mod provision;
pub mod rollout;
pub mod runtime_plan;
pub mod sync;

pub use letta_drift::{
    compute_letta_drift, compute_letta_drift_with_expected_tool_ids, LettaDriftFlags,
};
pub use model::{Bear, BearWithMembership};
pub use runtime_plan::{default_runtime_plan, effective_runtime_plan};
