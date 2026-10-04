//! Native agent runtime support: role harness, turn state, runtime
//! conversation reads, context compaction (+ observability/store), runtime
//! the provider client, and BearWire projection.

pub mod bearwire_projection;
pub mod compaction;
pub mod compaction_observability;
pub mod compaction_store;
pub mod completion_policy;
pub mod conversations;
pub mod pair_turn;
pub mod provider;
pub mod role;
pub mod task_context;
pub(crate) mod text;
pub mod turn_state;
