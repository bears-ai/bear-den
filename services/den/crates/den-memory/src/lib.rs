//! `den-memory`: per-Bear canonical memory store (per-Bear SQLite), per ADR-0031.
//!
//! Service-layer leaf crate (see `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md`):
//! depends only on `den-core`. Higher-level curation/tools live in `den`.

pub mod access;
pub mod admin_inspect;
mod clock;
mod conflicts;
pub mod descriptors;
pub mod entity;
pub mod harvest;
pub mod hat_promotion;
pub mod hat_review;
pub mod import;
pub mod legacy_review;
pub mod library;
mod logical_path;
mod manager;
mod migrate;
mod observations;
mod promotions;
mod proposals;
mod records;
pub mod reflection_outcomes;
pub mod relations;
pub mod resolver;
pub mod reviewed_core;
pub mod scoped;
pub mod tools;

#[cfg(test)]
mod memory_scope_tests;
#[cfg(test)]
mod test_support;

pub use access::{record_visible, AccessContext};
pub use admin_inspect::{
    bear_memory_admin_stats, bear_sqlite_db_path, count_records_by_kind, count_records_by_profile,
    get_memory_record_by_id, get_memory_record_detail, head_entry_count, list_all_logical_paths,
    list_path_summaries, list_recent_memory_records, search_memory_records, BearMemoryAdminStats,
    MemoryCountBucket, MemoryRecordDetail, PathSummary,
};
pub use conflicts::{
    conflict_candidates, detect_conflicts, memory_conflicts_among, record_conflict_observation,
    ConflictCandidate, ConflictReason, MemoryConflict, MEMORY_CONFLICT_OBSERVATION_KIND,
};
pub use descriptors::{EntityTrust, HandleStrength, RecallEffect, RelationClass, ResolutionState};
pub use entity::{
    attach_handle, create_entity, detach_handle, find_entity_by_handle, get_entity, list_entities,
    list_handles, merge_entities, resolve_live_entity, set_canonical_ref, set_resolution,
    split_entity, EntityHandleRow, EntityRow,
};
pub use harvest::{
    harvest_mark_for_source, harvest_source_hash_marked, harvest_source_marked,
    record_harvest_mark, MemoryHarvestMark,
};
pub use import::{
    import_legacy_memory_bundle, import_legacy_memory_git_dir, LegacyMemoryBranchReport,
    LegacyMemoryImportOptions, LegacyMemoryImportReport, LegacyMemoryImportSource,
};
pub use logical_path::{entity_anchor_path, LogicalMemoryPath, MemoryScopeType, MemorySource};
pub use manager::MemoryStoreManager;
pub use observations::{
    create_memory_observation, get_memory_observation, mark_observation_review_queued,
    MemoryObservationStatus, SqliteMemoryObservation,
};
pub use promotions::{
    append_memory_promotion, promote_to_shared_core, promote_to_shared_core_at_path,
    CorePromotionOutcome,
};
pub use proposals::{
    count_memory_proposals, create_memory_proposal, get_memory_proposal, list_memory_proposals,
    list_reviewable_memory_proposals, resolve_memory_proposal, SqliteMemoryProposal,
};
pub use records::{
    append_memory_record, effective_time_by_ids, fetch_record_by_id, fetch_records_min,
    freshness_trend, has_work_surface_canonical_anchor, head_record_as_of,
    head_record_for_logical_path, lifecycle_status, list_entity_anchor_head_records,
    list_profile_local_head_records, list_record_history_for_logical_path,
    list_records_for_logical_path, mark_memory_record_lifecycle, memory_sequence_high_water,
    superseder_times_by_superseded, BearMemoryStore, MemoryRecordRow, RecallRecordMin,
};
pub use reflection_outcomes::{
    complete_reflection_run_outcome, create_reflection_run_outcome, reflection_outcome_exists,
};
pub use relations::{
    append_relation, bounded_graph_expand, descriptive_entity_ids_for_records,
    list_access_rules_for_source, list_relations_for_entity, list_relations_for_source,
    record_ids_for_entities, GraphReach, RelationRow,
};
pub use resolver::{resolve, resolve_work_surface, Assertion, Resolution, Signal};
