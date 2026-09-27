//! Recall query (ADR-0038 Phase 2): embed a turn query, search the Bear's recall collection,
//! and shape the top hits into passages for the turn assembler's `## Recalled memory` section.
//!
//! Recall is **derived and optional**. Every entry point here is best-effort: an unset
//! `QDRANT_URL`, a disabled embedding client, or any transport error yields *no* recall
//! rather than failing the turn (the canonical key-memory projection still renders).

use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::{config::Config, DenError};

use super::indexer::PassageEmbedder;
use super::policy::SOURCE_CLASS_BEAR_MEMORY;
use super::qdrant::QdrantRecall;
use super::temporal::{parse_time_expression, TemporalQuery};

mod grant;
pub use grant::{recall_for_turn_with_grant, search_bear_memory_with_grant};

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
    VectorError,
}

impl DisabledRecallReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::QdrantUnset => "qdrant_unset",
            Self::EmbeddingsUnset => "embeddings_unset",
            Self::NoEntities => "no_entities",
            Self::VectorError => "vector_error",
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

/// Mandatory filter conditions scoping a search to this Bear's own memory passages under the
/// active embedding standard. Access-bearing gating (AccessContext) is a no-op today (no access
/// rules exist yet) and lands with the entity layer (Phase 6).
fn bear_scope_conditions(bear_id: Uuid, embedding_standard: &str) -> Vec<Value> {
    vec![
        json!({ "key": "bear_id", "match": { "value": bear_id.to_string() } }),
        json!({ "key": "source_class", "match": { "value": SOURCE_CLASS_BEAR_MEMORY } }),
        json!({ "key": "embedding_standard", "match": { "value": embedding_standard } }),
    ]
}

/// Filter scoping a search to memory **visible to `role`**: shared (core) records OR this role's
/// own profile-local records. Other roles' profile-local memory is excluded, honoring the
/// role-local boundary (AGENTS.md: `work` must not read raw `pair/`). The nested `should`
/// (≥1 match) acts as an OR clause within the mandatory bear scope.
fn role_scope_filter(bear_id: Uuid, embedding_standard: &str, role: &str) -> Value {
    let mut must = bear_scope_conditions(bear_id, embedding_standard);
    must.push(json!({
        "should": [
            { "key": "scope_type", "match": { "value": "shared" } },
            { "key": "scope_profile", "match": { "value": role } },
        ]
    }));
    json!({ "must": must })
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
        if matches!(lifecycle_status.as_str(), "archived" | "superseded") {
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

/// Role-scoped turn recall: like [`recall_for_turn`] but limited to memory **visible to `role`**
/// (shared + own role-local), so the assembler's `## Recalled memory` section honors the same
/// role-local boundary as the `memory_search` tool (AGENTS.md: `work` must not read raw `pair/`).
pub async fn recall_for_turn_scoped<E: PassageEmbedder + ?Sized>(
    qdrant: &QdrantRecall,
    embedder: &E,
    embedding_standard: &str,
    bear_id: Uuid,
    role: &str,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    let filter = role_scope_filter(bear_id, embedding_standard, role);
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
    config: &Config,
    bear_id: Uuid,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    let Some(qdrant) = QdrantRecall::from_config(config) else {
        return Ok(disabled_projection(DisabledRecallReason::QdrantUnset));
    };
    let embedder = den_llm::EmbeddingClient::new(config);
    if !embedder.is_enabled() {
        return Ok(disabled_projection(DisabledRecallReason::EmbeddingsUnset));
    }
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

/// **Role-scoped** semantic search for the `memory_search` tool: scopes to memory visible to
/// `role` (shared + own role-local). Builds the live Qdrant + Bifrost embedding clients from
/// config and returns a `disabled`-tagged projection (never an error) when recall isn't
/// configured, so the tool can fall back to keyword search.
pub async fn search_bear_memory_for_role(
    config: &Config,
    bear_id: Uuid,
    role: &str,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    let Some(qdrant) = QdrantRecall::from_config(config) else {
        return Ok(disabled_projection(DisabledRecallReason::QdrantUnset));
    };
    let embedder = den_llm::EmbeddingClient::new(config);
    if !embedder.is_enabled() {
        return Ok(disabled_projection(DisabledRecallReason::EmbeddingsUnset));
    }
    let filter = role_scope_filter(bear_id, &config.embedding_standard, role);
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
    let embedder = den_llm::EmbeddingClient::new(config);
    if !embedder.is_enabled() {
        return Ok(disabled_projection(DisabledRecallReason::EmbeddingsUnset));
    }
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

/// Default hop cap for the bounded-graph recall leg (ADR-0042 §4 / DERIVED_RECALL Phase 3.5).
const GRAPH_MAX_DEPTH: u32 = 2;

/// Bounded-graph recall leg: expand from `seed_memory_ids` over the descriptive record↔entity
/// graph (depth `max_depth`) and render the newly reached records as role-scoped hits, closest
/// hops first. Read-only, retrieval-time; no stored edges/inference (ADR-0042 anti-RDF). Surfaces
/// records related through a shared entity that the vector/keyword legs never matched. Reached
/// records are role-gated by `fetch_records_min` (shared ∨ own role-local); access-bearing gating
/// is inherent (traversal is over the descriptive table only) and `AccessContext` is applied by
/// the caller once access rules exist.
pub async fn graph_expand_hits(
    stores: &den_memory::MemoryStoreManager,
    bear_id: Uuid,
    role: &str,
    seed_memory_ids: &[String],
    max_depth: u32,
    limit: usize,
) -> Result<Vec<Value>, DenError> {
    if seed_memory_ids.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let store = stores.store_for_bear(bear_id).await?;
    let reached =
        den_memory::bounded_graph_expand(&store, seed_memory_ids, max_depth, limit).await?;
    if reached.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = reached.iter().map(|r| r.memory_id.clone()).collect();
    let records = den_memory::fetch_records_min(&store, &ids, role).await?;
    let by_id: std::collections::HashMap<&str, &den_memory::RecallRecordMin> =
        records.iter().map(|r| (r.memory_id.as_str(), r)).collect();

    // Entity-overlap boost: rank reached records that share more entities with the seed set higher
    // within each hop tier (a stronger association than a single shared entity).
    let seed_entities: std::collections::HashSet<String> =
        den_memory::relations::descriptive_entity_ids_for_records(&store, seed_memory_ids)
            .await?
            .into_iter()
            .collect();
    let entities_by_record =
        den_memory::relations::descriptive_entity_ids_by_source(&store).await?;
    let overlap = |memory_id: &str| -> usize {
        entities_by_record
            .get(memory_id)
            .map(|entities| {
                entities
                    .iter()
                    .filter(|e| seed_entities.contains(*e))
                    .count()
            })
            .unwrap_or(0)
    };

    // Role-visible reached records, ordered by hop asc, then overlap desc, then id (determinism).
    let mut ordered: Vec<(&den_memory::GraphReach, usize)> = reached
        .iter()
        .filter(|reach| by_id.contains_key(reach.memory_id.as_str()))
        .map(|reach| {
            let score = overlap(&reach.memory_id);
            (reach, score)
        })
        .collect();
    ordered.sort_by(|(a, oa), (b, ob)| {
        a.hop
            .cmp(&b.hop)
            .then(ob.cmp(oa))
            .then(a.memory_id.cmp(&b.memory_id))
    });

    let hits = ordered
        .into_iter()
        .map(|(reach, entity_overlap)| {
            let rec = by_id[reach.memory_id.as_str()];
            json!({
                "memory_id": rec.memory_id,
                "path": rec.logical_path,
                "kind": rec.kind,
                "score": Value::Null,
                "snippet": truncate_chars(&rec.content_text, SNIPPET_CHARS),
                "salience": rec.salience,
                "lifecycle_status": rec.lifecycle_status,
                "freshness_trend": rec.freshness_trend,
                "source": "graph",
                "hop": reach.hop,
                "entity_overlap": entity_overlap,
            })
        })
        .collect();
    Ok(hits)
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

/// Tag conflicting hits in the merged `memory_search` result with an explicit marker:
/// `conflicting: true` plus `conflicts_with` naming the counterpart record ids.
fn apply_conflict_markers(hits: &mut [Value], conflicts: &[den_memory::MemoryConflict]) {
    for hit in hits {
        let Some(memory_id) = hit.get("memory_id").and_then(Value::as_str) else {
            continue;
        };
        let others: Vec<&str> = conflicts
            .iter()
            .filter_map(|conflict| conflict.other(memory_id))
            .collect();
        if others.is_empty() {
            continue;
        }
        hit["conflicting"] = json!(true);
        hit["conflicts_with"] = json!(others);
    }
}

/// Final top-N cap that never ranks a conflict away (ADR-0041 §8): after truncating to
/// `limit`, re-appends any cut hit that a retained hit names in `conflicts_with`, so both
/// sides of a surfaced disagreement stay in the result.
fn cap_hits_retaining_conflicts(mut hits: Vec<Value>, limit: usize) -> Vec<Value> {
    if hits.len() <= limit {
        return hits;
    }
    let cut = hits.split_off(limit);
    let mut counterparts: std::collections::HashSet<&str> = hits
        .iter()
        .filter_map(|hit| hit.get("conflicts_with").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    for hit in &hits {
        if let Some(memory_id) = hit.get("memory_id").and_then(Value::as_str) {
            counterparts.remove(memory_id);
        }
    }
    let retained_counterparts: Vec<Value> = cut
        .into_iter()
        .filter(|hit| {
            hit.get("memory_id")
                .and_then(Value::as_str)
                .is_some_and(|id| counterparts.contains(id))
        })
        .collect();
    hits.extend(retained_counterparts);
    hits
}

/// Hybrid `memory_search` (ADR-0038 Phase 3 + 3.5): the **union** of three role-scoped legs over
/// the same visibility — the derived **vector** index, the **keyword** (`LIKE`) leg over canonical
/// SQLite, and the bounded-**graph** leg that expands from the direct hits over the record↔entity
/// relation graph. Vector hits rank first (higher-signal, carry a `score`), then keyword-only
/// exact matches, then graph-reached records (relevant by association, never directly matched).
///
/// Every leg beyond keyword is best-effort: an unconfigured index/transport error (vector) or a
/// relation-store error (graph) degrades to the remaining legs (logged), never failing the tool.
/// The keyword leg reads the canonical store, so its errors propagate.
pub async fn hybrid_memory_search(
    config: &Config,
    stores: &den_memory::MemoryStoreManager,
    bear_id: Uuid,
    role: &str,
    query: &str,
    limit: usize,
) -> Result<Value, DenError> {
    // Temporal leg (Phase 3.5): split a time expression off the query so the direct legs match on
    // the topical remainder while recall filters/boosts on effective event time.
    let temporal = parse_time_expression(query, OffsetDateTime::now_utc());
    let effective_query = temporal
        .as_ref()
        .map(|t| t.residual_query.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(query);
    // Over-fetch when a temporal filter will prune, so enough in-window hits survive the cap.
    let fetch_limit = if temporal.is_some() {
        limit.saturating_mul(5).clamp(limit, 200)
    } else {
        limit
    };

    let vector = match search_bear_memory_for_role(
        config,
        bear_id,
        role,
        effective_query,
        fetch_limit,
    )
    .await
    {
        Ok(projection) => projection,
        Err(error) => {
            tracing::warn!(%error, "recall vector leg failed; returning keyword results only");
            disabled_projection(DisabledRecallReason::VectorError)
        }
    };

    let store = stores.store_for_bear(bear_id).await?;
    let limit_i64 = i64::try_from(fetch_limit).unwrap_or(10);
    let keyword =
        den_memory::tools::sqlite_memory_search(&store, role, effective_query, limit_i64).await?;

    // Seed the graph leg with the records the direct legs already matched.
    let mut seeds: Vec<String> = vector
        .passages
        .iter()
        .map(|p| p.memory_id.clone())
        .collect();
    if let Some(arr) = keyword.get("hits").and_then(Value::as_array) {
        for hit in arr {
            if let Some(id) = hit.get("memory_id").and_then(Value::as_str) {
                seeds.push(id.to_string());
            }
        }
    }
    let graph = match graph_expand_hits(stores, bear_id, role, &seeds, GRAPH_MAX_DEPTH, fetch_limit)
        .await
    {
        Ok(hits) => hits,
        Err(error) => {
            tracing::warn!(%error, "recall graph leg failed; returning direct results only");
            Vec::new()
        }
    };

    let mut result = merge_search_results(&vector, &keyword, &graph, query);
    let mut hits = result
        .get("hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if let Some(temporal) = &temporal {
        hits = match filter_hits_by_temporal(&store, hits, temporal).await {
            Ok(hits) => hits,
            Err(error) => {
                tracing::warn!(%error, "recall temporal leg failed; returning unfiltered results");
                Vec::new()
            }
        };
        result["temporal"] = json!({
            "matched": temporal.matched,
            "as_of": temporal.as_of,
            "from": temporal.from.and_then(|d| d.format(&Rfc3339).ok()),
            "to": temporal.to.and_then(|d| d.format(&Rfc3339).ok()),
        });
    }

    // Read-time contradiction surfacing (ADR-0041 §8): detect over the full merged set
    // *before* the final top-N cap so a conflicting counterpart is never ranked away.
    let ids: Vec<String> = hits
        .iter()
        .filter_map(|hit| {
            hit.get("memory_id")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect();
    let conflicts = surface_recall_conflicts(stores, bear_id, &ids).await;
    if !conflicts.is_empty() {
        apply_conflict_markers(&mut hits, &conflicts);
        result["conflicts"] = conflict_summary_json(&conflicts);
    }
    result["hits"] = Value::Array(cap_hits_retaining_conflicts(hits, limit));

    Ok(result)
}

/// Drop hits whose effective event time (`COALESCE(valid_from, created_at)`) falls outside the
/// parsed window. For point-in-time (`as of`) queries, also drop records already superseded as of
/// the upper bound, walking the supersession chain (Phase 3.5).
async fn filter_hits_by_temporal(
    store: &den_memory::BearMemoryStore,
    hits: Vec<Value>,
    temporal: &TemporalQuery,
) -> Result<Vec<Value>, DenError> {
    if hits.is_empty() {
        return Ok(hits);
    }
    let ids: Vec<String> = hits
        .iter()
        .filter_map(|h| h.get("memory_id").and_then(Value::as_str).map(String::from))
        .collect();
    let times = den_memory::effective_time_by_ids(store, &ids).await?;
    let superseders = if temporal.as_of {
        den_memory::superseder_times_by_superseded(store, &ids).await?
    } else {
        std::collections::HashMap::new()
    };

    Ok(hits
        .into_iter()
        .filter(|hit| {
            let Some(id) = hit.get("memory_id").and_then(Value::as_str) else {
                return false;
            };
            // No effective time ⇒ can't satisfy a temporal constraint.
            let Some(effective) = times.get(id) else {
                return false;
            };
            if let Some(from) = temporal.from {
                if *effective < from {
                    return false;
                }
            }
            if let Some(to) = temporal.to {
                if *effective >= to {
                    return false;
                }
                if temporal.as_of {
                    if let Some(sup_times) = superseders.get(id) {
                        if sup_times.iter().any(|st| *st < to) {
                            return false;
                        }
                    }
                }
            }
            true
        })
        .collect())
}

/// Merge the vector projection, the keyword leg's JSON, and the bounded-graph leg's hits into the
/// unified `memory_search` result. De-dupes by `memory_id` in priority order (vector ≻ keyword ≻
/// graph — a record surfaced by a higher-signal leg is not repeated) and tags each hit's `source`
/// plus a top-level `strategy` (the `+`-joined list of contributing legs, e.g.
/// `vector+keyword+graph`, or a single leg, or `none`). The final top-N cap is applied by the
/// caller after conflict detection (see [`cap_hits_retaining_conflicts`]), never here.
fn merge_search_results(
    vector: &RecallProjection,
    keyword: &Value,
    graph: &[Value],
    query: &str,
) -> Value {
    let mut hits: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for passage in &vector.passages {
        seen.insert(passage.memory_id.clone());
        hits.push(json!({
            "memory_id": passage.memory_id,
            "path": passage.logical_path,
            "kind": passage.kind,
            "score": passage.score,
            "salience": passage.salience,
            "lifecycle_status": passage.lifecycle_status,
            "freshness_trend": passage.freshness_trend,
            "snippet": truncate_chars(&passage.text, SNIPPET_CHARS),
            "source": "vector",
        }));
    }
    let vector_count = hits.len();

    let mut keyword_count = 0usize;
    if let Some(arr) = keyword.get("hits").and_then(Value::as_array) {
        for hit in arr {
            let Some(memory_id) = hit.get("memory_id").and_then(Value::as_str) else {
                continue;
            };
            if !seen.insert(memory_id.to_string()) {
                continue;
            }
            keyword_count += 1;
            hits.push(json!({
                "memory_id": memory_id,
                "path": hit.get("path").cloned().unwrap_or(Value::Null),
                "kind": hit.get("kind").cloned().unwrap_or(Value::Null),
                "score": Value::Null,
                "snippet": hit.get("snippet").cloned().unwrap_or(Value::Null),
                "salience": hit.get("salience").cloned().unwrap_or_else(|| json!("normal")),
                "lifecycle_status": hit.get("lifecycle_status").cloned().unwrap_or_else(|| json!("active")),
                "freshness_trend": hit.get("freshness_trend").cloned().unwrap_or_else(|| json!("stable")),
                "source": "keyword",
            }));
        }
    }

    let mut graph_count = 0usize;
    for hit in graph {
        let Some(memory_id) = hit.get("memory_id").and_then(Value::as_str) else {
            continue;
        };
        if !seen.insert(memory_id.to_string()) {
            continue;
        }
        graph_count += 1;
        hits.push(hit.clone());
    }

    let mut legs: Vec<&str> = Vec::new();
    if vector_count > 0 {
        legs.push("vector");
    }
    if keyword_count > 0 {
        legs.push("keyword");
    }
    if graph_count > 0 {
        legs.push("graph");
    }
    let strategy = if legs.is_empty() {
        "none".to_string()
    } else {
        legs.join("+")
    };
    json!({
        "ok": true,
        "configured": true,
        "storage": "hybrid",
        "strategy": strategy,
        "query": query,
        "hits": hits,
        "diagnostic": vector.diagnostic,
    })
}

/// The rendered label of a passage: its logical path, or the memory id when pathless.
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
mod tests {
    use super::*;

    fn passage(memory_id: &str, path: &str, score: f32) -> RecalledPassage {
        RecalledPassage {
            memory_id: memory_id.into(),
            logical_path: Some(path.into()),
            kind: Some("note".into()),
            score,
            salience: "normal".into(),
            lifecycle_status: "active".into(),
            freshness_trend: "stable".into(),
            text: "the quick brown fox jumps over the lazy dog".into(),
            conflicts_with: Vec::new(),
        }
    }

    #[test]
    fn disabled_recall_reason_preserves_diagnostic_strings() {
        let cases = [
            (DisabledRecallReason::QdrantUnset, "qdrant_unset"),
            (DisabledRecallReason::EmbeddingsUnset, "embeddings_unset"),
            (DisabledRecallReason::NoEntities, "no_entities"),
            (DisabledRecallReason::VectorError, "vector_error"),
        ];

        for (reason, expected) in cases {
            let projection = disabled_projection(reason);
            assert_eq!(projection.diagnostic["reason"], expected);
        }
    }

    #[test]
    fn render_drops_paths_already_in_anchors() {
        let projection = RecallProjection {
            passages: vec![
                passage("m1", "core/a.md", 0.91),
                passage("m2", "core/b.md", 0.80),
            ],
            diagnostic: Value::Null,
        };
        let anchors = "# Projected memory\n- core/a.md: ...";
        let block = render_recall_block(&projection, anchors).expect("block");
        assert!(block.contains("core/b.md"));
        assert!(!block.contains("core/a.md"));
    }

    #[test]
    fn render_none_when_all_deduped() {
        let projection = RecallProjection {
            passages: vec![passage("m1", "core/a.md", 0.91)],
            diagnostic: Value::Null,
        };
        assert!(render_recall_block(&projection, "core/a.md").is_none());
    }

    #[test]
    fn truncate_collapses_whitespace_and_caps_length() {
        let out = truncate_chars("a\n\n  b   c", 100);
        assert_eq!(out, "a b c");
        let long = "x".repeat(600);
        let capped = truncate_chars(&long, SNIPPET_CHARS);
        assert_eq!(capped.chars().count(), SNIPPET_CHARS + 1); // + ellipsis
    }

    #[test]
    fn role_scope_filter_requires_shared_or_own_role() {
        let bear = Uuid::nil();
        let filter = role_scope_filter(bear, "bears-embed-v1", "work");
        let must = filter["must"].as_array().expect("must array");
        // Three mandatory scope conditions + one nested should clause.
        assert_eq!(must.len(), 4, "{filter}");
        assert_eq!(must[0]["key"], "bear_id");
        assert_eq!(must[1]["match"]["value"], SOURCE_CLASS_BEAR_MEMORY);
        assert_eq!(must[2]["match"]["value"], "bears-embed-v1");
        let should = must[3]["should"].as_array().expect("nested should");
        assert_eq!(should[0]["key"], "scope_type");
        assert_eq!(should[0]["match"]["value"], "shared");
        assert_eq!(should[1]["key"], "scope_profile");
        assert_eq!(should[1]["match"]["value"], "work");
    }

    #[test]
    fn entity_scope_filter_requires_entity_membership() {
        let bear = Uuid::nil();
        let filter = entity_scope_filter(bear, "bears-embed-v1", &["e1".into(), "e2".into()]);
        let must = filter["must"].as_array().expect("must array");
        // Three mandatory scope conditions + one entity-membership clause.
        assert_eq!(must.len(), 4, "{filter}");
        assert_eq!(must[0]["key"], "bear_id");
        assert_eq!(must[3]["key"], "entity_ids");
        let any = must[3]["match"]["any"].as_array().expect("any array");
        assert_eq!(any.len(), 2);
        assert_eq!(any[0], "e1");
        assert_eq!(any[1], "e2");
    }

    #[test]
    fn merge_unions_vector_then_keyword_and_dedupes() {
        let vector = RecallProjection {
            passages: vec![passage("m1", "core/a.md", 0.91)],
            diagnostic: json!({ "status": "ok" }),
        };
        // Keyword leg re-surfaces m1 (must be deduped) and adds a unique m2.
        let keyword = json!({
            "hits": [
                { "memory_id": "m1", "path": "core/a.md", "kind": "note", "snippet": "dup" },
                { "memory_id": "m2", "path": "work/b.md", "kind": "note", "snippet": "exact match" },
            ]
        });
        let value = merge_search_results(&vector, &keyword, &[], "fox");

        assert_eq!(value["storage"], "hybrid");
        assert_eq!(value["strategy"], "vector+keyword");
        let hits = value["hits"].as_array().expect("hits array");
        assert_eq!(hits.len(), 2, "m1 deduped, m2 appended: {hits:?}");
        // Vector hit ranks first and carries its score + source.
        assert_eq!(hits[0]["memory_id"], "m1");
        assert_eq!(hits[0]["source"], "vector");
        assert!((hits[0]["score"].as_f64().unwrap() - 0.91).abs() < 1e-6);
        assert!(hits[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("quick brown fox"));
        // Keyword-only hit follows, unranked.
        assert_eq!(hits[1]["memory_id"], "m2");
        assert_eq!(hits[1]["source"], "keyword");
        assert!(hits[1]["score"].is_null());
    }

    #[test]
    fn freshness_multiplier_downranks_stale_and_boosts_strengthening() {
        assert!(freshness_multiplier("strengthening") > freshness_multiplier("stable"));
        assert!(freshness_multiplier("weakening") < freshness_multiplier("stable"));
        assert!(freshness_multiplier("stale") < freshness_multiplier("weakening"));
    }

    #[test]
    fn merge_strategy_reflects_contributing_legs() {
        let only_vector = merge_search_results(
            &RecallProjection {
                passages: vec![passage("m1", "core/a.md", 0.9)],
                diagnostic: Value::Null,
            },
            &json!({ "hits": [] }),
            &[],
            "q",
        );
        assert_eq!(only_vector["strategy"], "vector");

        let only_keyword = merge_search_results(
            &RecallProjection {
                passages: vec![],
                diagnostic: Value::Null,
            },
            &json!({ "hits": [{ "memory_id": "m9", "path": "p", "kind": "note", "snippet": "s" }] }),
            &[],
            "q",
        );
        assert_eq!(only_keyword["strategy"], "keyword");
    }

    #[test]
    fn merge_appends_graph_leg_after_direct_hits_and_dedupes() {
        let vector = RecallProjection {
            passages: vec![passage("m1", "core/a.md", 0.9)],
            diagnostic: Value::Null,
        };
        let keyword = json!({ "hits": [{ "memory_id": "m2", "path": "work/b.md", "kind": "note", "snippet": "kw" }] });
        // m1 is re-surfaced by the graph leg (must dedupe); m3 is a genuine 2-hop reach.
        let graph = vec![
            json!({ "memory_id": "m1", "path": "core/a.md", "kind": "note", "score": Value::Null, "snippet": "dup", "source": "graph", "hop": 1 }),
            json!({ "memory_id": "m3", "path": "core/c.md", "kind": "note", "score": Value::Null, "snippet": "reached via shared entity", "source": "graph", "hop": 2 }),
        ];
        let value = merge_search_results(&vector, &keyword, &graph, "q");

        assert_eq!(value["strategy"], "vector+keyword+graph");
        let hits = value["hits"].as_array().expect("hits array");
        assert_eq!(hits.len(), 3, "m1 deduped against the vector hit: {hits:?}");
        assert_eq!(hits[0]["memory_id"], "m1");
        assert_eq!(hits[1]["memory_id"], "m2");
        // Graph-reached record ranks last and carries its provenance + hop distance.
        assert_eq!(hits[2]["memory_id"], "m3");
        assert_eq!(hits[2]["source"], "graph");
        assert_eq!(hits[2]["hop"], 2);
        assert!(hits[2]["score"].is_null());
    }

    #[test]
    fn cap_hits_caps_to_limit_prioritizing_vector() {
        let vector = RecallProjection {
            passages: vec![
                passage("m1", "core/a.md", 0.9),
                passage("m2", "core/b.md", 0.8),
            ],
            diagnostic: Value::Null,
        };
        let keyword =
            json!({ "hits": [{ "memory_id": "m3", "path": "p", "kind": "note", "snippet": "s" }] });
        let value = merge_search_results(&vector, &keyword, &[], "q");
        let hits = value["hits"].as_array().expect("hits").clone();
        let hits = cap_hits_retaining_conflicts(hits, 2);
        assert_eq!(hits.len(), 2, "capped to limit");
        assert_eq!(hits[0]["memory_id"], "m1");
        assert_eq!(
            hits[1]["memory_id"], "m2",
            "vector hits retained over keyword when capping"
        );
    }

    fn conflict(a: &str, b: &str, path: &str) -> den_memory::MemoryConflict {
        den_memory::MemoryConflict {
            memory_id_a: a.min(b).to_string(),
            memory_id_b: a.max(b).to_string(),
            reason: den_memory::ConflictReason::SharedLogicalPath(path.to_string()),
        }
    }

    #[test]
    fn conflict_markers_tag_both_sides_and_summary_lists_records() {
        let mut hits = vec![
            json!({ "memory_id": "m1", "source": "vector" }),
            json!({ "memory_id": "m2", "source": "keyword" }),
            json!({ "memory_id": "m3", "source": "keyword" }),
        ];
        let conflicts = vec![conflict("m1", "m3", "core/a.md")];
        apply_conflict_markers(&mut hits, &conflicts);

        assert_eq!(hits[0]["conflicting"], true);
        assert_eq!(hits[0]["conflicts_with"], json!(["m3"]));
        assert!(hits[1].get("conflicting").is_none(), "{:?}", hits[1]);
        assert_eq!(hits[2]["conflicting"], true);
        assert_eq!(hits[2]["conflicts_with"], json!(["m1"]));

        let summary = conflict_summary_json(&conflicts);
        assert_eq!(summary["pairs"], 1);
        assert_eq!(summary["records"], json!(["m1", "m3"]));
    }

    #[test]
    fn cap_retains_conflicting_counterpart_that_ranking_would_cut() {
        let mut hits = vec![
            json!({ "memory_id": "m1", "source": "vector" }),
            json!({ "memory_id": "m2", "source": "vector" }),
            json!({ "memory_id": "m3", "source": "keyword" }),
            json!({ "memory_id": "m4", "source": "keyword" }),
        ];
        // m1 conflicts with m3, which the top-2 cap would otherwise drop.
        apply_conflict_markers(&mut hits, &[conflict("m1", "m3", "core/a.md")]);
        let capped = cap_hits_retaining_conflicts(hits, 2);

        let ids: Vec<&str> = capped
            .iter()
            .filter_map(|h| h["memory_id"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["m1", "m2", "m3"],
            "counterpart re-appended past the cap; unrelated m4 stays cut"
        );
    }

    #[test]
    fn mark_projection_conflicts_fills_passages_and_diagnostic() {
        let mut projection = RecallProjection {
            passages: vec![
                passage("m1", "core/a.md", 0.91),
                passage("m2", "core/b.md", 0.80),
            ],
            diagnostic: json!({ "status": "ok" }),
        };
        mark_projection_conflicts(&mut projection, &[conflict("m1", "m2", "core/a.md")]);

        assert_eq!(
            projection.passages[0].conflicts_with,
            vec!["m2".to_string()]
        );
        assert_eq!(
            projection.passages[1].conflicts_with,
            vec!["m1".to_string()]
        );
        assert_eq!(projection.diagnostic["conflicts"]["pairs"], 1);
        assert_eq!(
            projection.diagnostic["conflicts"]["records"],
            json!(["m1", "m2"])
        );
    }

    #[test]
    fn render_marks_conflicting_passages_naming_the_counterpart() {
        let mut projection = RecallProjection {
            passages: vec![
                passage("m1", "core/a.md", 0.91),
                passage("m2", "core/b.md", 0.80),
                passage("m3", "core/c.md", 0.70),
            ],
            diagnostic: json!({ "status": "ok" }),
        };
        mark_projection_conflicts(&mut projection, &[conflict("m1", "m2", "core/a.md")]);
        let block = render_recall_block(&projection, "").expect("block");

        assert!(
            block.contains("- `core/a.md` (note, score 0.91, conflicting with `core/b.md`):"),
            "{block}"
        );
        assert!(
            block.contains("- `core/b.md` (note, score 0.80, conflicting with `core/a.md`):"),
            "{block}"
        );
        assert!(
            block.contains("- `core/c.md` (note, score 0.70):"),
            "unconflicted passage keeps the plain marker: {block}"
        );
    }
}
