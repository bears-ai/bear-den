//! Recall consistency watermark (ADR-0038 §8): a derived, per-Bear answer to "is this
//! Bear's memory fully recallable right now?".
//!
//! Canonical write → passage registry → Qdrant is asynchronous, so the watermark joins
//! registry state (Postgres) against canonical indexable heads (SQLite) in Rust — no
//! cross-store SQL, nothing stored. When Qdrant is not configured the watermark surface
//! is *unavailable* (recall is keyword-only by design), never reported as infinite lag.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::{config::Config, DenError};
use den_memory::{memory_sequence_high_water, BearMemoryStore, MemoryStoreManager};

use super::chunking::chunk_text;
use super::policy::IndexRequest;
use super::reconcile::list_indexable_heads;
use super::registry;

/// Reflection lane whose runs feed the recall index (see `den-runtime` conductor).
const RECALL_INDEX_LANE: &str = "recall_index";

/// Per-Bear recall consistency watermark (ADR-0038 §8). Derived state: computed from the
/// passage registry against canonical SQLite, never stored as truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecallWatermark {
    /// The Bear's canonical SQLite `MAX(sequence_no)`.
    pub canonical_seq: i64,
    /// Highest `sequence_no` S such that every indexable head at or below S has live,
    /// content-hash-matching registry passages. Non-indexable records advance the
    /// watermark without indexing work; with no indexable head pending this equals
    /// [`Self::canonical_seq`].
    pub indexed_seq: i64,
    /// Indexable heads above `indexed_seq` that are not fully indexed.
    pub lag_count: i64,
    /// `indexed_seq == canonical_seq`.
    pub fully_recallable: bool,
    /// RFC 3339 completion time of the last successful `recall_index` run (`None` when
    /// no run has ever succeeded).
    pub last_success_at: Option<String>,
    /// Failed `recall_index` runs since the last success (all-time when none succeeded).
    pub failed_run_count: i64,
}

impl RecallWatermark {
    /// Operational health summary: fully recallable with no failed `recall_index` runs
    /// since the last success. Admin hub stats and health checks key off this.
    pub const fn is_healthy(&self) -> bool {
        self.fully_recallable && self.failed_run_count == 0
    }
}

/// Compute the recall watermark for one Bear, or `None` when recall is not configured
/// (`qdrant_url` unset — the watermark surface is unavailable, not infinitely lagged).
pub async fn recall_watermark(
    pg: &PgPool,
    config: &Config,
    store: &BearMemoryStore,
) -> Result<Option<RecallWatermark>, DenError> {
    if config.qdrant_url.is_none() {
        return Ok(None);
    }
    let bear_id = store.bear_id();
    let canonical_seq = memory_sequence_high_water(store).await?;
    let heads = list_indexable_heads(store).await?;
    let live_hashes =
        registry::live_chunk_hashes_by_memory(pg, bear_id, &config.embedding_standard).await?;
    let (indexed_seq, lag_count) = compute_indexed_seq(canonical_seq, &heads, &live_hashes);
    let runs = recall_index_run_stats(pg, bear_id).await?;
    Ok(Some(RecallWatermark {
        canonical_seq,
        indexed_seq,
        lag_count,
        fully_recallable: indexed_seq == canonical_seq,
        last_success_at: runs.last_success_at.and_then(|at| at.format(&Rfc3339).ok()),
        failed_run_count: runs.failed_run_count,
    }))
}

/// [`recall_watermark`] by Bear id, resolving the shared per-process store manager
/// (ADR-0031). Workspace-visible health entry point for admin surfaces and checks.
pub async fn recall_watermark_for_bear(
    pg: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Option<RecallWatermark>, DenError> {
    if config.qdrant_url.is_none() {
        return Ok(None);
    }
    let store = stores.store_for_bear(bear_id).await?;
    recall_watermark(pg, config, &store).await
}

/// Tool/admin JSON for a watermark surface: `{"available": false, ...}` when recall is
/// not configured, otherwise the watermark fields plus `"available": true`.
pub fn recall_status_json(watermark: Option<&RecallWatermark>) -> Value {
    watermark.map_or_else(
        || json!({ "available": false, "reason": "recall not configured" }),
        |wm| {
            let mut value = serde_json::to_value(wm).unwrap_or_else(|_| json!({}));
            if let Some(obj) = value.as_object_mut() {
                obj.insert("available".to_string(), json!(true));
            }
            value
        },
    )
}

/// Pure watermark computation over canonical indexable heads and live registry chunk
/// hashes: returns `(indexed_seq, lag_count)`. Heads that are not effectively indexable
/// (policy, lifecycle, or empty content) advance the watermark without indexing work.
fn compute_indexed_seq(
    canonical_seq: i64,
    heads: &[IndexRequest],
    live_hashes: &HashMap<String, HashMap<i32, String>>,
) -> (i64, i64) {
    let mut pending: Vec<i64> = heads
        .iter()
        .filter(|head| head_is_pending(head, live_hashes))
        .map(|head| head.sequence_no)
        .collect();
    if pending.is_empty() {
        return (canonical_seq, 0);
    }
    pending.sort_unstable();
    let lag_count = i64::try_from(pending.len()).unwrap_or(i64::MAX);
    (pending[0] - 1, lag_count)
}

/// Whether an indexable head still needs indexing work: some chunk of its canonical
/// content lacks a live registry passage with a matching `content_hash`.
fn head_is_pending(
    head: &IndexRequest,
    live_hashes: &HashMap<String, HashMap<i32, String>>,
) -> bool {
    if !head.is_indexable() {
        return false;
    }
    let chunks = chunk_text(&head.content_text);
    if chunks.is_empty() {
        return false;
    }
    live_hashes.get(&head.memory_id).is_none_or(|indexed| {
        chunks
            .iter()
            .any(|chunk| indexed.get(&chunk.index) != Some(&chunk.content_hash))
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RecallRunStats {
    last_success_at: Option<OffsetDateTime>,
    failed_run_count: i64,
}

/// Last-success timestamp and failed-run count for the `recall_index` reflection lane.
async fn recall_index_run_stats(pg: &PgPool, bear_id: Uuid) -> Result<RecallRunStats, DenError> {
    let last_success_at: Option<OffsetDateTime> = sqlx::query_scalar!(
        r#"
        SELECT MAX(completed_at) AS "last_success_at?"
        FROM bear_reflection_runs
        WHERE bear_id = $1 AND lane = $2 AND status = 'completed'
        "#,
        bear_id,
        RECALL_INDEX_LANE,
    )
    .fetch_one(pg)
    .await
    .map_err(|e| DenError::System(format!("recall_index run stats (last success): {e}")))?;

    let failed_run_count: i64 = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*)::bigint AS "failed_run_count!"
        FROM bear_reflection_runs
        WHERE bear_id = $1 AND lane = $2 AND status = 'failed'
          AND ($3::timestamptz IS NULL OR created_at > $3)
        "#,
        bear_id,
        RECALL_INDEX_LANE,
        last_success_at,
    )
    .fetch_one(pg)
    .await
    .map_err(|e| DenError::System(format!("recall_index run stats (failed count): {e}")))?;

    Ok(RecallRunStats {
        last_success_at,
        failed_run_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bears::db::{create_bear, BearParams};
    use crate::recall::chunking;

    fn head(memory_id: &str, sequence_no: i64, content: &str) -> IndexRequest {
        IndexRequest {
            bear_id: Uuid::nil(),
            memory_id: memory_id.into(),
            sequence_no,
            logical_path: Some(format!("core/{memory_id}.md")),
            scope_type: "shared".into(),
            scope_profile: None,
            scope_hat_id: None,
            work_surface_ref: None,
            kind: "note".into(),
            visibility: "normal".into(),
            content_text: content.into(),
            salience: "normal".into(),
            lifecycle_status: "active".into(),
            freshness_trend: "stable".into(),
            entity_ids: Vec::new(),
        }
    }

    /// Registry fixture entry mirroring a fully indexed head: every chunk of `content`
    /// registered live under its canonical hash.
    fn indexed_entry(memory_id: &str, content: &str) -> (String, HashMap<i32, String>) {
        let chunks = chunking::chunk_text(content)
            .into_iter()
            .map(|c| (c.index, c.content_hash))
            .collect();
        (memory_id.to_string(), chunks)
    }

    #[test]
    fn fully_indexed_bear_reaches_canonical_seq() {
        let heads = vec![
            head("mem-a", 3, "alpha body"),
            head("mem-b", 7, "beta body"),
        ];
        let live: HashMap<_, _> = [
            indexed_entry("mem-a", "alpha body"),
            indexed_entry("mem-b", "beta body"),
        ]
        .into_iter()
        .collect();
        assert_eq!(compute_indexed_seq(9, &heads, &live), (9, 0));
    }

    #[test]
    fn lagging_head_holds_watermark_back() {
        let heads = vec![
            head("mem-a", 3, "alpha body"),
            head("mem-b", 7, "beta body"),
        ];
        let live: HashMap<_, _> = std::iter::once(indexed_entry("mem-a", "alpha body")).collect();
        // mem-b (seq 7) is unindexed: the watermark stops just below it.
        assert_eq!(compute_indexed_seq(9, &heads, &live), (6, 1));
    }

    #[test]
    fn stale_content_hash_counts_as_pending() {
        let heads = vec![head("mem-a", 3, "new body")];
        // Registry still holds passages for the *old* content.
        let live: HashMap<_, _> = std::iter::once(indexed_entry("mem-a", "old body")).collect();
        assert_eq!(compute_indexed_seq(3, &heads, &live), (2, 1));
    }

    #[test]
    fn non_indexable_records_advance_the_watermark() {
        // A corpus whose newest records are non-indexable (filtered out of the head list
        // entirely, or archived / empty-content heads) is fully recallable without work.
        let empty_live = HashMap::new();
        assert_eq!(compute_indexed_seq(5, &[], &empty_live), (5, 0));

        let mut archived = head("mem-old", 4, "archived body");
        archived.lifecycle_status = "archived".into();
        let empty_content = head("mem-blank", 5, "   ");
        assert_eq!(
            compute_indexed_seq(5, &[archived, empty_content], &empty_live),
            (5, 0)
        );
    }

    #[test]
    fn status_json_shapes_available_and_unavailable() {
        let unavailable = recall_status_json(None);
        assert_eq!(unavailable["available"], false);
        assert_eq!(unavailable["reason"], "recall not configured");

        let wm = RecallWatermark {
            canonical_seq: 9,
            indexed_seq: 6,
            lag_count: 1,
            fully_recallable: false,
            last_success_at: Some("2026-07-30T12:00:00Z".into()),
            failed_run_count: 2,
        };
        assert!(!wm.is_healthy());
        let available = recall_status_json(Some(&wm));
        assert_eq!(available["available"], true);
        assert_eq!(available["canonical_seq"], 9);
        assert_eq!(available["indexed_seq"], 6);
        assert_eq!(available["lag_count"], 1);
        assert_eq!(available["fully_recallable"], false);
        assert_eq!(available["last_success_at"], "2026-07-30T12:00:00Z");
        assert_eq!(available["failed_run_count"], 2);
    }

    async fn insert_run(
        pool: &PgPool,
        bear_id: Uuid,
        lane: &str,
        status: &str,
        completed_hours_ago: Option<i32>,
        created_hours_ago: i32,
    ) {
        sqlx::query!(
            r"
            INSERT INTO bear_reflection_runs (bear_id, lane, trigger, status, completed_at, created_at)
            VALUES ($1, $2, 'watermark-test', $3,
                    CASE WHEN $4::int IS NULL THEN NULL
                         ELSE NOW() - make_interval(hours => $4) END,
                    NOW() - make_interval(hours => $5))
            ",
            bear_id,
            lane,
            status,
            completed_hours_ago,
            created_hours_ago,
        )
        .execute(pool)
        .await
        .expect("insert reflection run");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn run_stats_count_failures_since_last_success(pool: PgPool) {
        let bear_id = create_bear(
            &pool,
            BearParams {
                slug: "watermark-test-bear",
                name: "Watermark Test Bear",
                description: "test",
                system_prompt: "test",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .expect("create bear");

        // No runs yet.
        let stats = recall_index_run_stats(&pool, bear_id).await.expect("stats");
        assert_eq!(stats, RecallRunStats::default());

        // A failure before the last success does not count; failures after it do.
        insert_run(&pool, bear_id, RECALL_INDEX_LANE, "failed", Some(5), 5).await;
        insert_run(&pool, bear_id, RECALL_INDEX_LANE, "completed", Some(4), 4).await;
        insert_run(&pool, bear_id, RECALL_INDEX_LANE, "failed", Some(2), 2).await;
        insert_run(&pool, bear_id, RECALL_INDEX_LANE, "failed", Some(1), 1).await;
        // Other lanes and non-terminal statuses are ignored.
        insert_run(&pool, bear_id, "memory_curate", "failed", Some(1), 1).await;
        insert_run(&pool, bear_id, RECALL_INDEX_LANE, "queued", None, 0).await;

        let stats = recall_index_run_stats(&pool, bear_id).await.expect("stats");
        assert!(stats.last_success_at.is_some(), "success recorded");
        assert_eq!(stats.failed_run_count, 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn run_stats_count_all_failures_when_never_succeeded(pool: PgPool) {
        let bear_id = create_bear(
            &pool,
            BearParams {
                slug: "watermark-nosuccess-bear",
                name: "Watermark No-Success Bear",
                description: "test",
                system_prompt: "test",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .expect("create bear");

        insert_run(&pool, bear_id, RECALL_INDEX_LANE, "failed", Some(3), 3).await;
        insert_run(&pool, bear_id, RECALL_INDEX_LANE, "failed", Some(1), 1).await;

        let stats = recall_index_run_stats(&pool, bear_id).await.expect("stats");
        assert_eq!(stats.last_success_at, None);
        assert_eq!(stats.failed_run_count, 2);
    }
}
