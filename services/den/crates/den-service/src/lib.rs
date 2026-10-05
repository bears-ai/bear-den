//! `den-service`: shared concrete service/application state for Den edges.
//!
//! Keep this below `den-runtime`: it owns process-local coordination and service
//! handles needed by HTTP edges, but not runtime execution or model turns.

pub mod archived_conversations;
pub mod artifacts;
pub mod bears;
pub mod bifrost;
pub mod bifrost_governance;
pub mod cabinet;
pub mod client_session_work_surface_resolutions;
pub mod client_sessions;
pub mod connections;
pub mod conversation;
pub mod memory_proposals;
pub mod model_selection;
pub mod pair_reflection;
pub mod prompt_memory_block_store;
pub mod prompt_memory_blocks;
pub mod recall;
pub mod rust_dependencies;
pub mod secrets;
pub mod skills;
pub mod state;
pub mod tool_turns;
pub mod turn_controller;
pub mod work_surfaces;

pub use archived_conversations as archived;
pub use conversation::events as conversation_events;
pub use conversation::message_types as conversation_message_types;
pub use memory_proposals as proposals;
pub use state::DenState;
