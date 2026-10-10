//! Recall query (ADR-0038 Phase 2): embed a turn query, search the Bear's recall collection,
//! and shape the top hits into passages for the turn assembler's `## Recalled memory` section.
//!
//! Recall is **derived and optional**. Every entry point here is best-effort: an unset
//! `QDRANT_URL`, a disabled embedding client, or any transport error yields *no* recall
//! rather than failing the turn (the canonical key-memory projection still renders).

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use den_core::{config::Config, ids::BearId, DenError};

use super::authenticated_embedder;

use super::indexer::PassageEmbedder;
use super::policy::SOURCE_CLASS_BEAR_MEMORY;
use super::qdrant::QdrantRecall;

mod grant;
mod library;
pub use grant::{recall_for_turn_with_grant, search_bear_memory_with_grant};
pub use library::{retain_curated_candidates, search_curated_library};

/// Total character budget for the rendered recall section (ADR-0038 Phase 2: ~2–3k).
const RECALL_CHAR_BUDGET: usize = 2_600;
/// Max characters of a single passage snippet before truncation.
const SNIPPET_CHARS: usize = 480;

/// A single recalled passage, resolved from a Qdrant hit's payload.
#[derive(Debug, Clone)]
pub struct RecalledPassage {
    pub memory_id: String,
    pub logical_path: Option<String>,
    pub kind: Option<String>,
    pub score: f32,
    pub salience: String,
    pub lifecycle_status: String,
    pub freshness_trend: String,
    pub text: String,
    /// Memory ids of retrieved records this one conflicts with (ADR-0041 §8 read-time
    /// contradiction surfacing); empty when no conflict was detected.
    pub conflicts_with: Vec<String>,
}

fn salience_multiplier(salience: &str) -> f32 {
    match salience {
        "low" => 0.9,
        "high" => 1.15,
        "critical" => 1.3,
        _ => 1.0,
    }
}

fn freshness_multiplier(freshness_trend: &str) -> f32 {
    match freshness_trend {
        "strengthening" => 1.08,
        "weakening" => 0.92,
        "stale" => 0.75,
        _ => 1.0,
    }
}

/// The outcome of a recall query: the selected passages plus a diagnostic for observability.
#[derive(Debug, Clone)]
pub struct RecallProjection {
    pub passages: Vec<RecalledPassage>,
    pub diagnostic: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisabledRecallReason {
    QdrantUnset,
    EmbeddingsUnset,
    NoEntities,
}

impl DisabledRecallReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::QdrantUnset => "qdrant_unset",
            Self::EmbeddingsUnset => "embeddings_unset",
            Self::NoEntities => "no_entities",
        }
    }
}

/// An empty projection tagged `disabled` (never an error) so config-driven callers can fall
/// back to keyword search when recall isn't fully wired (`QDRANT_URL` / embeddings unset).
fn disabled_projection(reason: DisabledRecallReason) -> RecallProjection {
    RecallProjection {
        passages: Vec::new(),
        diagnostic: json!({
            "source": "recall_query",
            "status": "disabled",
            "reason": reason.as_str(),
        }),
    }
}

/// Mandatory Bear/standard isolation. Legacy profile and raw source points are
/// excluded even before reconciliation removes them. Model/member callers must
/// additionally recheck candidates against current canonical SQLite.
fn bear_scope_conditions(bear_id: Uuid, embedding_standard: &str) -> Vec<Value> {
    vec![
        json!({ "key": "bear_id", "match": { "value": bear_id.to_string() } }),
        json!({ "key": "source_class", "match": { "value": SOURCE_CLASS_BEAR_MEMORY } }),
        json!({ "key": "embedding_standard", "match": { "value": embedding_standard } }),
    ]
}

/// Mandatory bear-scope conditions plus an **entity-membership** clause: passages whose
/// denormalized `entity_ids` array contains *any* of `entity_ids` (ADR-0042 §7 descriptive
/// relations; the access-bearing gate is never denormalized here). The empty-`entity_ids` case is
/// handled by callers (no entities ⇒ no scope ⇒ skip). Bear-wide; a turn-time caller layers role
/// scoping on top.
fn entity_scope_filter(bear_id: Uuid, embedding_standard: &str, entity_ids: &[String]) -> Value {
    let mut must = bear_scope_conditions(bear_id, embedding_standard);
    must.push(json!({ "key": "entity_ids", "match": { "any": entity_ids } }));
    json!({ "must": must })
}

/// Embed `query_text`, run a `filter`-scoped Qdrant search, and dedupe to the best-scoring chunk
/// per memory id, returning up to `limit` passages ordered by similarity. Best-effort: errors
/// surface as `Err` so the caller can log + degrade; an empty/blank query returns no passages.
async fn search_passages<E: PassageEmbedder + ?Sized>(
    qdrant: &QdrantRecall,
    embedder: &E,
    filter: Value,
    embedding_standard: &str,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    let trimmed = query_text.trim();
    if trimmed.is_empty() || limit == 0 {
        return Ok(RecallProjection {
            passages: Vec::new(),
            diagnostic: json!({
                "source": "recall_query",
                "status": "skipped",
                "reason": "empty_query",
            }),
        });
    }

    let vectors = embedder
        .embed(std::slice::from_ref(&trimmed.to_string()))
        .await?;
    let query_vec = vectors
        .into_iter()
        .next()
        .ok_or_else(|| DenError::System("recall query embedding returned no vector".to_string()))?;

    // Overfetch so per-memory dedupe still yields `limit` distinct records.
    let fetch = (limit.saturating_mul(3)).clamp(limit, 50) as u64;
    let mut filter = filter;
    let must = filter["must"]
        .as_array_mut()
        .expect("internal recall filter");
    must.push(json!({ "key": "scope_type", "match": { "any": ["shared", "hat"] } }));
    must.push(json!({ "key": "visibility", "match": { "value": "normal" } }));
    let hits = qdrant.search(&query_vec, filter, fetch).await?;
    let raw_hits = hits.len();

    let mut passages: Vec<RecalledPassage> = Vec::new();
    for hit in hits {
        let memory_id = hit
            .payload
            .get("memory_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(memory_id) = memory_id else { continue };
        // Keep only the best-scoring chunk per memory record (hits arrive best-first).
        if passages.iter().any(|p| p.memory_id == memory_id) {
            continue;
        }
        let text = hit
            .payload
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        let lifecycle_status = hit
            .payload
            .get("lifecycle_status")
            .and_then(Value::as_str)
            .unwrap_or("active")
            .to_string();
        if matches!(
            lifecycle_status.as_str(),
            "archived" | "archive-candidate" | "superseded"
        ) {
            continue;
        }
        let salience = hit
            .payload
            .get("salience")
            .and_then(Value::as_str)
            .unwrap_or("normal")
            .to_string();
        let freshness_trend = hit
            .payload
            .get("freshness_trend")
            .and_then(Value::as_str)
            .unwrap_or("stable")
            .to_string();
        passages.push(RecalledPassage {
            memory_id,
            logical_path: hit
                .payload
                .get("logical_path")
                .and_then(Value::as_str)
                .map(str::to_string),
            kind: hit
                .payload
                .get("kind")
                .and_then(Value::as_str)
                .map(str::to_string),
            score: hit.score
                * salience_multiplier(&salience)
                * freshness_multiplier(&freshness_trend),
            salience,
            lifecycle_status,
            freshness_trend,
            text,
            conflicts_with: Vec::new(),
        });
        if passages.len() >= limit {
            break;
        }
    }

    let diagnostic = json!({
        "source": "recall_query",
        "status": "ok",
        "raw_hits": raw_hits,
        "passages": passages.len(),
        "embedding_standard": embedding_standard,
    });
    Ok(RecallProjection {
        passages,
        diagnostic,
    })
}

/// Embed `query_text`, search the Bear's recall collection (bear-wide), dedupe by memory id, and
/// return up to `limit` passages. Used by the turn assembler's `## Recalled memory` section.
pub async fn recall_for_turn<E: PassageEmbedder + ?Sized>(
    qdrant: &QdrantRecall,
    embedder: &E,
    embedding_standard: &str,
    bear_id: Uuid,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    let filter = json!({ "must": bear_scope_conditions(bear_id, embedding_standard) });
    search_passages(
        qdrant,
        embedder,
        filter,
        embedding_standard,
        query_text,
        limit,
    )
    .await
}

/// Convenience **bear-wide** semantic search for the admin UI (the human admin sees all of a
/// Bear's memory): builds the live Qdrant + Bifrost embedding clients from config. Returns a
/// `disabled`-tagged projection (never an error) when recall isn't fully configured.
pub async fn semantic_search_for_bear(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    let Some(qdrant) = QdrantRecall::from_config(config) else {
        return Ok(disabled_projection(DisabledRecallReason::QdrantUnset));
    };
    let Some(embedder) = authenticated_embedder(pool, config, BearId::new(bear_id)).await? else {
        return Ok(disabled_projection(DisabledRecallReason::EmbeddingsUnset));
    };
    let filter = json!({ "must": bear_scope_conditions(bear_id, &config.embedding_standard) });
    search_passages(
        &qdrant,
        &embedder,
        filter,
        &config.embedding_standard,
        query_text,
        limit,
    )
    .await
}

/// **Entity-scoped** semantic search (ADR-0042 Phase 4 recall leg): rank the Bear's passages that
/// are linked by a descriptive relation to any of `entity_ids`, by relevance to `query_text`.
/// This is the query-side consumer of the denormalized passage `entity_ids` and the seed leg for
/// future bounded-graph expansion + entity-centric admin recall. Bear-wide (the human admin sees
/// all). Builds live Qdrant + Bifrost embedding clients from config; returns a `disabled`/`skipped`
/// projection (never an error) when recall isn't configured or no entities are supplied.
pub async fn search_bear_memory_for_entities(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    entity_ids: &[String],
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    if entity_ids.is_empty() {
        return Ok(disabled_projection(DisabledRecallReason::NoEntities));
    }
    let Some(qdrant) = QdrantRecall::from_config(config) else {
        return Ok(disabled_projection(DisabledRecallReason::QdrantUnset));
    };
    let Some(embedder) = authenticated_embedder(pool, config, BearId::new(bear_id)).await? else {
        return Ok(disabled_projection(DisabledRecallReason::EmbeddingsUnset));
    };
    let filter = entity_scope_filter(bear_id, &config.embedding_standard, entity_ids);
    search_passages(
        &qdrant,
        &embedder,
        filter,
        &config.embedding_standard,
        query_text,
        limit,
    )
    .await
}

/// Detect read-time contradictions among the records already retrieved for a turn and emit
/// best-effort `memory_conflict` observations (ADR-0041 §8). Bounded: the predicate runs only
/// over `memory_ids`, never the corpus. Best-effort on the hot path — any store or write error
/// is logged and yields no conflicts; recall never fails or slows because of detection.
pub async fn surface_recall_conflicts(
    stores: &den_memory::MemoryStoreManager,
    bear_id: Uuid,
    memory_ids: &[String],
) -> Vec<den_memory::MemoryConflict> {
    if memory_ids.len() < 2 {
        return Vec::new();
    }
    let store = match stores.store_for_bear(bear_id).await {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(%error, "recall conflict detection skipped: store unavailable");
            return Vec::new();
        }
    };
    let conflicts = match den_memory::memory_conflicts_among(&store, memory_ids).await {
        Ok(conflicts) => conflicts,
        Err(error) => {
            tracing::warn!(%error, "recall conflict detection failed; continuing without markers");
            return Vec::new();
        }
    };
    for conflict in &conflicts {
        if let Err(error) = den_memory::record_conflict_observation(&store, conflict).await {
            tracing::warn!(%error, "memory_conflict observation write failed; continuing");
        }
    }
    conflicts
}

/// Conflict presence summary for diagnostics: pair count plus the involved record ids.
pub fn conflict_summary_json(conflicts: &[den_memory::MemoryConflict]) -> Value {
    let mut records: Vec<&str> = conflicts
        .iter()
        .flat_map(|c| [c.memory_id_a.as_str(), c.memory_id_b.as_str()])
        .collect();
    records.sort_unstable();
    records.dedup();
    json!({ "pairs": conflicts.len(), "records": records })
}

/// Mark the projection's conflicting passages (fill `conflicts_with`) and record conflict
/// presence in its diagnostic so the session diagnostic can surface it.
pub fn mark_projection_conflicts(
    projection: &mut RecallProjection,
    conflicts: &[den_memory::MemoryConflict],
) {
    if conflicts.is_empty() {
        return;
    }
    for passage in &mut projection.passages {
        for conflict in conflicts {
            if let Some(other) = conflict.other(&passage.memory_id) {
                passage.conflicts_with.push(other.to_string());
            }
        }
    }
    if let Some(diagnostic) = projection.diagnostic.as_object_mut() {
        diagnostic.insert("conflicts".to_string(), conflict_summary_json(conflicts));
    }
}

fn passage_label(passage: &RecalledPassage) -> &str {
    passage
        .logical_path
        .as_deref()
        .filter(|p| !p.is_empty())
        .unwrap_or(&passage.memory_id)
}

/// Render the `## Recalled memory` section, dropping passages whose `logical_path` already
/// appears in `anchor_text` (the key-memory projection) so recall never duplicates anchors.
/// Conflicting passages carry an explicit `conflicting with` marker naming the counterpart
/// record (ADR-0041 §8) so the model sees the disagreement instead of a silent ranked winner.
/// Returns `None` when nothing survives dedupe/budget.
pub fn render_recall_block(projection: &RecallProjection, anchor_text: &str) -> Option<String> {
    let labels: std::collections::HashMap<&str, &str> = projection
        .passages
        .iter()
        .map(|p| (p.memory_id.as_str(), passage_label(p)))
        .collect();
    let mut body = String::new();
    let mut used = 0usize;
    let mut rendered = 0usize;
    for passage in &projection.passages {
        // Dedupe against anchors already injected by the key-memory projection.
        if let Some(path) = passage.logical_path.as_deref() {
            if !path.is_empty() && anchor_text.contains(path) {
                continue;
            }
        }
        let label = passage_label(passage);
        let kind = passage.kind.as_deref().unwrap_or("memory");
        let snippet = truncate_chars(&passage.text, SNIPPET_CHARS);
        let conflict_marker = if passage.conflicts_with.is_empty() {
            String::new()
        } else {
            let others = passage
                .conflicts_with
                .iter()
                .map(|id| format!("`{}`", labels.get(id.as_str()).copied().unwrap_or(id)))
                .collect::<Vec<_>>()
                .join(", ");
            format!(", conflicting with {others}")
        };
        let line = format!(
            "- `{label}` ({kind}, score {:.2}{conflict_marker}): {snippet}\n",
            passage.score
        );
        if used + line.len() > RECALL_CHAR_BUDGET && rendered > 0 {
            break;
        }
        used += line.len();
        body.push_str(&line);
        rendered += 1;
    }

    if rendered == 0 {
        return None;
    }
    Some(format!(
        "## Recalled memory\n\nSemantically related memory (derived recall; lower precision than projected anchors above):\n\n{body}"
    ))
}

fn truncate_chars(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(max).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests;
