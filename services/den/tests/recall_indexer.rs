//! Integration test for the derived recall indexer (ADR-0038 Phase 1).
//!
//! Exercises the full write path against a **live Postgres + Qdrant** using a deterministic
//! stub embedder (no OpenAI key required): write memory → passages appear in Qdrant →
//! re-index is idempotent → remove deletes the points.
//!
//! Gated on `DATABASE_URL` + `QDRANT_URL`; skips (passes) when either is unset, so it is a
//! no-op in environments without the recall stack.

use den::{config::Config, startup::run_sqlx_migrations};
use den_memory::tools::sqlite_memory_search;
use den_memory::{
    append_memory_record, append_relation, resolve, Assertion, LogicalMemoryPath,
    MemoryStoreManager, Resolution, Signal,
};
use den_runtime::recall::{
    hybrid_memory_search, recall_for_turn, reconcile::list_indexable_heads, render_recall_block,
    DeterministicEmbedder, IndexRequest, PassageEmbedder, QdrantRecall, RecallIndexer,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn recall_env_ready() -> bool {
    std::env::var("DATABASE_URL").is_ok()
        && !std::env::var("QDRANT_URL").unwrap_or_default().is_empty()
}

#[tokio::test]
async fn recall_indexer_round_trip_against_live_qdrant() {
    dotenvy::dotenv().ok();
    if !recall_env_ready() {
        eprintln!("skipping: DATABASE_URL/QDRANT_URL not set");
        return;
    }

    let config = Config::load();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .expect("connect Postgres");
    run_sqlx_migrations(&pool)
        .await
        .expect("apply migrations (recall_passages)");

    let qdrant = QdrantRecall::from_config(&config).expect("QDRANT_URL set");
    qdrant.ensure_collection().await.expect("ensure collection");

    // recall_passages.bear_id has an FK to bears(id); use an existing bear from the DB.
    let bear_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM bears LIMIT 1")
        .fetch_optional(&pool)
        .await
        .expect("query bears");
    let Some(bear_id) = bear_id else {
        eprintln!("skipping: no seeded bear in DB");
        return;
    };

    let embedder = DeterministicEmbedder::new(config.embedding_dimensions);
    let indexer = RecallIndexer::new(&pool, &qdrant, &embedder, config.embedding_standard.clone());

    // Long enough to split into multiple chunks (chunk target is ~2400 chars).
    let body = "Bears keep canonical memory in SQLite. ".repeat(120);
    let memory_id = format!("smoke-recall-{}", Uuid::new_v4());
    let req = IndexRequest {
        bear_id,
        memory_id: memory_id.clone(),
        sequence_no: 1,
        logical_path: Some("core/recall/smoke.md".into()),
        scope_type: "shared".into(),
        scope_profile: None,
        scope_hat_id: None,
        work_surface_ref: None,
        kind: "summary".into(),
        visibility: "normal".into(),
        content_text: body.clone(),
        salience: "normal".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        entity_ids: Vec::new(),
    };

    let mem_filter = json!({ "must": [{ "key": "memory_id", "match": { "value": memory_id } }] });

    // First index: embeds and upserts every chunk.
    let outcome = indexer.index_record(&req).await.expect("index_record");
    assert!(
        outcome.embedded_chunks >= 2,
        "expected multi-chunk embed, got {outcome:?}"
    );
    assert_eq!(outcome.reused_chunks, 0, "{outcome:?}");

    let count = qdrant
        .count_with_filter(mem_filter.clone())
        .await
        .expect("count after index");
    assert_eq!(
        count as usize, outcome.embedded_chunks,
        "Qdrant should hold one point per embedded chunk"
    );

    // Re-index identical content: fully deduped, no new embeds, count unchanged.
    let again = indexer.index_record(&req).await.expect("re-index");
    assert_eq!(again.embedded_chunks, 0, "{again:?}");
    assert_eq!(again.reused_chunks, outcome.embedded_chunks, "{again:?}");
    let count_after_reindex = qdrant
        .count_with_filter(mem_filter.clone())
        .await
        .expect("count after re-index");
    assert_eq!(
        count_after_reindex, count,
        "re-index must not duplicate points"
    );

    // Remove (supersede/delete): points disappear from Qdrant.
    let removed = indexer
        .remove_record(req.bear_id, &req.memory_id)
        .await
        .expect("remove_record");
    assert_eq!(removed, outcome.embedded_chunks, "removed all chunk points");
    let count_after_remove = qdrant
        .count_with_filter(mem_filter)
        .await
        .expect("count after remove");
    assert_eq!(count_after_remove, 0, "supersede must remove old passages");
}

/// Phase 2 recall query: index a passage, then query it back. The deterministic embedder maps
/// identical text to an identical vector, so querying with the indexed body must return that
/// passage as a top hit (score ~1.0) — proving embed → filtered search → payload shaping works
/// end-to-end without an embedding key. Gated on `DATABASE_URL` + `QDRANT_URL`.
#[tokio::test]
async fn recall_query_retrieves_indexed_passage_against_live_qdrant() {
    dotenvy::dotenv().ok();
    if !recall_env_ready() {
        eprintln!("skipping: DATABASE_URL/QDRANT_URL not set");
        return;
    }

    let config = Config::load();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .expect("connect Postgres");
    run_sqlx_migrations(&pool).await.expect("apply migrations");

    let qdrant = QdrantRecall::from_config(&config).expect("QDRANT_URL set");
    qdrant.ensure_collection().await.expect("ensure collection");

    let bear_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM bears LIMIT 1")
        .fetch_optional(&pool)
        .await
        .expect("query bears");
    let Some(bear_id) = bear_id else {
        eprintln!("skipping: no seeded bear in DB");
        return;
    };

    let embedder = DeterministicEmbedder::new(config.embedding_dimensions);
    let indexer = RecallIndexer::new(&pool, &qdrant, &embedder, config.embedding_standard.clone());

    let unique = Uuid::new_v4();
    // Short, distinctive, single-chunk body (well under the chunk target).
    let body = format!(
        "Zephyr protocol calibration notes for run {unique}; the lighthouse keeper logs tidal anomalies."
    );
    let logical_path = "core/recall/query-smoke.md";
    let memory_id = format!("smoke-recall-query-{unique}");
    let req = IndexRequest {
        bear_id,
        memory_id: memory_id.clone(),
        sequence_no: 1,
        logical_path: Some(logical_path.into()),
        scope_type: "shared".into(),
        scope_profile: None,
        scope_hat_id: None,
        work_surface_ref: None,
        kind: "summary".into(),
        visibility: "normal".into(),
        content_text: body.clone(),
        salience: "normal".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        entity_ids: Vec::new(),
    };

    let outcome = indexer.index_record(&req).await.expect("index_record");
    assert_eq!(outcome.embedded_chunks, 1, "single-chunk body: {outcome:?}");

    let projection = recall_for_turn(
        &qdrant,
        &embedder,
        &config.embedding_standard,
        bear_id,
        &body,
        5,
    )
    .await
    .expect("recall_for_turn");

    let hit = projection
        .passages
        .iter()
        .find(|p| p.memory_id == memory_id)
        .unwrap_or_else(|| {
            panic!(
                "indexed passage should be recalled: {:?}",
                projection.passages
            )
        });
    assert!(
        hit.score > 0.99,
        "identical vector should score ~1.0, got {}",
        hit.score
    );
    assert_eq!(hit.logical_path.as_deref(), Some(logical_path));
    assert!(
        hit.text.contains("Zephyr protocol"),
        "payload text round-trips: {hit:?}"
    );

    // Renders without anchors; dedupes the passage when its path is already an anchor.
    assert!(
        render_recall_block(&projection, "").is_some(),
        "renders a block"
    );
    if let Some(block) = render_recall_block(&projection, logical_path) {
        assert!(
            !block.contains(logical_path),
            "anchored path must be deduped"
        );
    }

    indexer
        .remove_record(bear_id, &memory_id)
        .await
        .expect("remove_record cleanup");
}

/// Entity recall leg (ADR-0042 Phase 4): a record's resolved descriptive `entity_ids` are
/// denormalized into the passage payload, so an entity-membership filter retrieves it and an
/// unrelated entity filter excludes it. Uses the deterministic embedder + a manual entity filter
/// (the `search_bear_memory_for_entities` config path needs a live embedding key). Gated on
/// `DATABASE_URL` + `QDRANT_URL`.
#[tokio::test]
async fn entity_scoped_recall_filters_by_payload_entity_ids() {
    dotenvy::dotenv().ok();
    if !recall_env_ready() {
        eprintln!("skipping: DATABASE_URL/QDRANT_URL not set");
        return;
    }

    let config = Config::load();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .expect("connect Postgres");
    run_sqlx_migrations(&pool).await.expect("apply migrations");

    let qdrant = QdrantRecall::from_config(&config).expect("QDRANT_URL set");
    qdrant.ensure_collection().await.expect("ensure collection");

    let bear_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM bears LIMIT 1")
        .fetch_optional(&pool)
        .await
        .expect("query bears");
    let Some(bear_id) = bear_id else {
        eprintln!("skipping: no seeded bear in DB");
        return;
    };

    let embedder = DeterministicEmbedder::new(config.embedding_dimensions);
    let indexer = RecallIndexer::new(&pool, &qdrant, &embedder, config.embedding_standard.clone());

    let unique = Uuid::new_v4();
    let entity_id = format!("ent-alpha-{unique}");
    let body = format!("Entity-linked recall note {unique}: the cartographer charts the fjords.");
    let memory_id = format!("smoke-recall-entity-{unique}");
    let req = IndexRequest {
        bear_id,
        memory_id: memory_id.clone(),
        sequence_no: 1,
        logical_path: Some("core/recall/entity-smoke.md".into()),
        scope_type: "shared".into(),
        scope_profile: None,
        scope_hat_id: None,
        work_surface_ref: None,
        kind: "summary".into(),
        visibility: "normal".into(),
        content_text: body.clone(),
        salience: "normal".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        entity_ids: vec![entity_id.clone()],
    };

    let outcome = indexer.index_record(&req).await.expect("index_record");
    assert_eq!(outcome.embedded_chunks, 1, "single-chunk body: {outcome:?}");

    let query_vec = embedder
        .embed(std::slice::from_ref(&body))
        .await
        .expect("embed query")
        .into_iter()
        .next()
        .expect("one vector");

    // Matching entity filter retrieves the passage, and the payload carries entity_ids.
    let match_filter = json!({ "must": [
        { "key": "bear_id", "match": { "value": bear_id.to_string() } },
        { "key": "entity_ids", "match": { "any": [entity_id] } },
    ] });
    let hits = qdrant
        .search(&query_vec, match_filter, 5)
        .await
        .expect("entity-scoped search");
    let hit = hits
        .iter()
        .find(|h| h.payload.get("memory_id").and_then(|v| v.as_str()) == Some(memory_id.as_str()))
        .unwrap_or_else(|| panic!("entity-linked passage should be recalled: {hits:?}"));
    let payload_entities = hit
        .payload
        .get("entity_ids")
        .and_then(|v| v.as_array())
        .expect("payload entity_ids array");
    assert!(
        payload_entities
            .iter()
            .any(|e| e.as_str() == Some(entity_id.as_str())),
        "payload entity_ids round-trips: {hit:?}"
    );

    // An unrelated entity filter excludes the passage.
    let miss_filter = json!({ "must": [
        { "key": "bear_id", "match": { "value": bear_id.to_string() } },
        { "key": "entity_ids", "match": { "any": [format!("ent-nonexistent-{unique}")] } },
    ] });
    let miss = qdrant
        .search(&query_vec, miss_filter, 5)
        .await
        .expect("entity miss search");
    assert!(
        !miss.iter().any(
            |h| h.payload.get("memory_id").and_then(|v| v.as_str()) == Some(memory_id.as_str())
        ),
        "unrelated entity filter must exclude the passage: {miss:?}"
    );

    indexer
        .remove_record(bear_id, &memory_id)
        .await
        .expect("remove_record cleanup");
}

/// Phase 3.5 bounded-graph leg: `hybrid_memory_search` surfaces a record never matched by the
/// keyword/vector legs but reachable via a **shared entity** (bipartite record↔entity expansion).
/// Infra-free: temp SQLite, no Qdrant ⇒ the vector leg is disabled, so the keyword + graph legs
/// run against canonical SQLite alone — exactly the ADR-0038 Phase 3.5 "record never directly
/// matched" exit case.
#[tokio::test]
async fn hybrid_search_graph_leg_surfaces_indirectly_linked_record() {
    let tmp = std::env::temp_dir().join(format!("den-recall-graph-{}", Uuid::new_v4()));
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = tmp.to_string_lossy().into_owned();

    let stores = MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    let store = stores.store_for_bear(bear_id).await.expect("temp store");

    let token = "graphonlytoken";
    // Direct hit: a shared record containing the query token.
    let direct = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("summary"),
        "summary",
        "curate",
        None,
        &format!("shared note mentioning {token}"),
        &json!({}),
    )
    .await
    .expect("write direct");
    // Neighbor: a shared record with no query term — keyword/vector can never match it directly.
    let neighbor = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("knowledge"),
        "note",
        "curate",
        None,
        "neighbor note with no query term at all",
        &json!({}),
    )
    .await
    .expect("write neighbor");

    // Link both records to one shared entity so the graph leg can bridge direct → neighbor.
    let entity_id = match resolve(
        &store,
        "person",
        Some("Alice"),
        &[Signal::new("email", "alice@acme.com")],
        Assertion::Inferred,
    )
    .await
    .unwrap()
    {
        Resolution::Resolved(e) | Resolution::Created(e) => e.entity_id,
        other => panic!("expected a resolved/created entity, got {other:?}"),
    };
    append_relation(
        &store,
        &direct.memory_id,
        &entity_id,
        "subject",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .expect("link direct");
    append_relation(
        &store,
        &neighbor.memory_id,
        &entity_id,
        "participant",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .expect("link neighbor");

    let result = hybrid_memory_search(&config, &stores, bear_id, "work", token, 10)
        .await
        .expect("hybrid search");

    // Vector disabled (no Qdrant); keyword finds the direct hit; the graph leg reaches the neighbor.
    assert_eq!(result["strategy"], "keyword+graph", "{result}");
    let hits = result["hits"].as_array().expect("hits array");
    let by_id: std::collections::HashMap<&str, &serde_json::Value> = hits
        .iter()
        .filter_map(|h| h["memory_id"].as_str().map(|id| (id, h)))
        .collect();
    let direct_hit = by_id
        .get(direct.memory_id.as_str())
        .unwrap_or_else(|| panic!("direct keyword hit present: {hits:?}"));
    assert_eq!(direct_hit["source"], "keyword");
    let neighbor_hit = by_id.get(neighbor.memory_id.as_str()).unwrap_or_else(|| {
        panic!("graph leg should surface the indirectly-linked neighbor: {hits:?}")
    });
    assert_eq!(neighbor_hit["source"], "graph");
    assert_eq!(neighbor_hit["hop"], 1);
    // Entity-overlap boost: the neighbor shares exactly the one bridging entity with the seed.
    assert_eq!(neighbor_hit["entity_overlap"], 1, "{hits:?}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Phase 3.5 temporal leg: `hybrid_memory_search` parses a time expression off the query and
/// filters hits by effective event time. Infra-free (temp SQLite, no Qdrant). Records written now
/// survive a `today` window but are pruned by a `before <past-date>` window; the result carries a
/// `temporal` diagnostic either way.
#[tokio::test]
async fn hybrid_search_temporal_leg_filters_by_effective_time() {
    let tmp = std::env::temp_dir().join(format!("den-recall-temporal-{}", Uuid::new_v4()));
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = tmp.to_string_lossy().into_owned();

    let stores = MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    let store = stores.store_for_bear(bear_id).await.expect("temp store");

    let token = "temporaltoken";
    for kind in ["summary", "knowledge"] {
        append_memory_record(
            &store,
            &LogicalMemoryPath::shared_core(kind),
            kind,
            "curate",
            None,
            &format!("shared note mentioning {token}"),
            &json!({}),
        )
        .await
        .expect("write record");
    }

    // A `today` window keeps just-written records and strips the temporal phrase from the query.
    let today = hybrid_memory_search(
        &config,
        &stores,
        bear_id,
        "work",
        &format!("{token} today"),
        10,
    )
    .await
    .expect("today search");
    assert_eq!(today["temporal"]["matched"], "today", "{today}");
    assert!(
        !today["hits"].as_array().expect("hits").is_empty(),
        "records written now fall in the today window: {today}"
    );

    // A window entirely in the past prunes every just-written record.
    let past = hybrid_memory_search(
        &config,
        &stores,
        bear_id,
        "work",
        &format!("{token} before 2000-01-01"),
        10,
    )
    .await
    .expect("past search");
    assert!(past["temporal"]["to"].is_string(), "{past}");
    assert_eq!(
        past["hits"].as_array().expect("hits").len(),
        0,
        "nothing is effective before the year 2000: {past}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Head selection + policy filtering for whole-Bear reconcile (Phase 1b). Infra-free: uses a
/// throwaway temp SQLite store, no Postgres/Qdrant, so it always runs.
#[tokio::test]
async fn list_indexable_heads_selects_latest_and_filters_policy() {
    let tmp = std::env::temp_dir().join(format!("den-recall-heads-{}", Uuid::new_v4()));
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = tmp.to_string_lossy().into_owned();

    let stores = MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    let store = stores.store_for_bear(bear_id).await.expect("temp store");

    // Two versions of a shared/core summary at the same path → head is the latest.
    let core_path = LogicalMemoryPath::shared_core("summary");
    append_memory_record(
        &store,
        &core_path,
        "summary",
        "curate",
        None,
        "old core body",
        &json!({}),
    )
    .await
    .expect("write old core");
    let head = append_memory_record(
        &store,
        &core_path,
        "summary",
        "curate",
        None,
        "new core body",
        &json!({}),
    )
    .await
    .expect("write new core");

    // Ephemeral scratch is excluded by policy.
    let scratch_path = LogicalMemoryPath::profile_local("pair", "scratch");
    append_memory_record(
        &store,
        &scratch_path,
        "scratch",
        "pair",
        None,
        "ephemeral junk",
        &json!({}),
    )
    .await
    .expect("write scratch");

    let heads = list_indexable_heads(&store).await.expect("list heads");

    assert_eq!(
        heads.len(),
        1,
        "only the shared summary head is indexable: {heads:?}"
    );
    let req = &heads[0];
    assert_eq!(
        req.memory_id, head.memory_id,
        "head must be the latest at the path"
    );
    assert_eq!(req.kind, "summary");
    assert_eq!(req.scope_type, "shared");
    assert!(req.content_text.contains("new core body"), "{req:?}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Phase 3 keyword fallback: `sqlite_memory_search` is the `memory_search` tool's non-vector
/// path. Infra-free (temp SQLite, no Postgres/Qdrant). Asserts the role-scope boundary — a
/// `work`-role search sees shared (core) memory and its own role-local notes, but **not** another
/// role's profile-local memory (AGENTS.md: `work` must not read raw `pair/`) — plus the unified
/// provenance shape (`memory_id`, `path`, `snippet`, `strategy: "keyword"`, null `score`).
#[tokio::test]
async fn keyword_memory_search_scopes_to_shared_plus_own_role() {
    let tmp = std::env::temp_dir().join(format!("den-recall-search-{}", Uuid::new_v4()));
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = tmp.to_string_lossy().into_owned();

    let stores = MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    let store = stores.store_for_bear(bear_id).await.expect("temp store");

    // A distinctive token present in all three records so only scope, not content, gates results.
    let token = "zephyrcalibration";
    let shared = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("summary"),
        "summary",
        "curate",
        None,
        &format!("shared core note about {token}"),
        &json!({}),
    )
    .await
    .expect("write shared");
    let work = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("work", "note"),
        "note",
        "work",
        None,
        &format!("work-local note about {token}"),
        &json!({}),
    )
    .await
    .expect("write work-local");
    let pair = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "note"),
        "note",
        "pair",
        None,
        &format!("pair-local note about {token}"),
        &json!({}),
    )
    .await
    .expect("write pair-local");

    let result = sqlite_memory_search(&store, "work", token, 10)
        .await
        .expect("keyword search");

    assert_eq!(result["storage"], "sqlite");
    assert_eq!(result["strategy"], "keyword");
    let hits = result["hits"].as_array().expect("hits array");
    let ids: Vec<&str> = hits
        .iter()
        .filter_map(|h| h["memory_id"].as_str())
        .collect();
    assert!(
        ids.contains(&shared.memory_id.as_str()),
        "shared visible to work: {ids:?}"
    );
    assert!(
        ids.contains(&work.memory_id.as_str()),
        "own role-local visible: {ids:?}"
    );
    assert!(
        !ids.contains(&pair.memory_id.as_str()),
        "another role's profile-local must not leak: {ids:?}"
    );

    // Provenance shape: path present, score null (keyword is unranked), snippet carries content.
    let first = &hits[0];
    assert!(first["path"].is_string(), "{first}");
    assert!(first["score"].is_null(), "{first}");
    assert!(
        first["snippet"].as_str().unwrap().contains(token),
        "{first}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
