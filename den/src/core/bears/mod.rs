//! Bear registry and membership (Phase 1).
//! Admin HTTP routes and Letta provisioning wire up in later milestones.

pub mod db;
pub mod lettabot;
pub mod model;
pub mod provision;
pub mod sync;

pub use model::{Bear, BearWithMembership};
