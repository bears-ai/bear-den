//! Per-Bear canonical memory ([ADR-0031](../../../docs/decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)).

pub mod bear_observations;
pub mod curate_executor;
pub mod curation;
pub mod extraction;

#[cfg(test)]
mod store_round_trip_tests;

pub use curation::{
    create_observation, create_proposal, get_observation, get_proposal, list_proposals,
    mark_observation_review_queued_for_bear, record_reflection_outcome_complete,
    record_reflection_outcome_start, resolve_proposal,
};
