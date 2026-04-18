//! Bear registry and membership (Phase 1).
//! Admin HTTP routes and Letta provisioning wire up in later milestones.

pub mod db;
pub mod letta_code_harness;
pub mod letta_drift;
pub mod model;
pub mod provision;
pub mod sync;

pub use letta_drift::{compute_letta_drift, LettaDriftFlags};
pub use model::{Bear, BearWithMembership};
