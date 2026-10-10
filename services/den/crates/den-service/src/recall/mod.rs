//! Derived recall index (ADR-0038): a Qdrant-backed semantic index over canonical Bear
//! memory. Vectors are **derived** — SQLite is the source of truth and Qdrant may be absent
//! (callers degrade to keyword fallback).
//!
//! - [`qdrant`] — minimal Qdrant REST client (readiness, collection bootstrap, points).
//! - [`chunking`] — passage chunking + content hashing.
//! - [`policy`] — indexing policy + payload shaping.
//! - [`registry`] — Postgres `recall_passages` metadata.
//! - [`indexer`] — orchestration (chunk → embed → upsert → register).
//! - [`reconcile`] — whole-Bear reconcile against canonical heads.
//! - [`query`] — recall query for the turn assembler (embed → search → render).
//! - [`temporal`] — time-expression parsing for the temporal recall leg (Phase 3.5).
//! - [`watermark`] — per-Bear recall consistency watermark (ADR-0038 §8).

mod authenticated_embedder;
pub mod chunking;
pub mod indexer;
pub mod policy;
pub mod qdrant;
pub mod query;
pub mod reconcile;
pub mod registry;
pub mod temporal;
pub mod watermark;

pub use authenticated_embedder::authenticated_embedder;
pub use indexer::{IndexOutcome, PassageEmbedder, RecallIndexer};
pub use policy::IndexRequest;
pub use qdrant::{collection_name, QdrantPoint, QdrantRecall, RecallHit};
pub use query::{
    conflict_summary_json, mark_projection_conflicts, recall_for_turn, render_recall_block,
    search_bear_memory_for_entities, semantic_search_for_bear, surface_recall_conflicts,
    RecallProjection, RecalledPassage,
};
pub use reconcile::{reconcile_bear, reindex_bear_now, ReconcileOutcome};
pub use temporal::{parse_time_expression, TemporalQuery};
pub use watermark::{
    recall_status_json, recall_watermark, recall_watermark_for_bear, RecallWatermark,
};

#[cfg(any(test, feature = "test-util"))]
pub use indexer::DeterministicEmbedder;
