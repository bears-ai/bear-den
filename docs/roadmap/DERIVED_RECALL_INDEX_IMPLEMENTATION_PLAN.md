# Derived Recall Index — Implementation Plan

**Status:** Phases 0–3.5 + 5 landed; Phase 4 (Cabinet) deferred on [Cabinet publication and research-ingestion contracts](CABINET_IMPLEMENTATION_PLAN.md)  
**Architecture:** [ADR-0038 — Platform embedding standard and derived recall index](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)  
**Contracts:** [Den archival memory and ingestion](../architecture/den-archival-memory-and-ingestion-contract.md), [Den runtime — turn context](../architecture/den-runtime.md#turn-context-assembly)

## Goal

Ship Den-owned **semantic recall** over canonical sources using:

- platform embedding standard **`bears-embed-v1`** (`text-embedding-3-small`, 1536d, via Bifrost),
- **Qdrant** derived vectors,
- Postgres **passage registry** metadata,
- turn-start bounded recall + hybrid **`memory_search`**.

Complements existing **key memory projection** (path anchors); does not replace SQLite canonical memory.

## Non-goals (initial phases)

- Rerank model task
- Transcript indexing
- Vectors in bear packages
- pgvector on Den Postgres
- Separate embedding models per source class (unless a future ADR adds e.g. code-only `bears-embed-code-v1`)

## Phase 0 — Platform wiring  ✅ landed (compose, env, config, embedding client, status check)

- ✅ Add env: `EMBEDDING_STANDARD`, `EMBEDDING_MODEL`, `EMBEDDING_DIMENSIONS`, `QDRANT_URL` — set on `bears-den` in `docker-compose.yaml` (+ `.env.example`); `QDRANT_URL` empty = recall disabled (LIKE fallback).
- ✅ Compose: optional `bears-qdrant` service (profile `recall`) with persistent `bears-qdrant-data` volume. Nothing `depends_on` it; the default stack is unchanged. Enable with `COMPOSE_PROFILES=recall` + `QDRANT_URL=http://bears-qdrant:6333`.
- ✅ Den `Config` consumes the new env (`qdrant_url`, `embedding_standard`, `embedding_model`, `embedding_dimensions`) in `den-core/src/config.rs`.
- ✅ Register embedding model in Bifrost `config.json` (`text-embedding-3-small` authorized on the openai provider key); Den embedding client (`den-llm::EmbeddingClient`) calls `/v1/embeddings` with the active standard's model + dimensions. *(Invocation from the indexer lands in Phase 1.)*
- ✅ Qdrant client (`den-runtime::recall::QdrantRecall`) + startup collection bootstrap (`den/src/startup.rs`) + `/status.json` health check (`web/stack_health.rs`); `SMOKE_RECALL=1` in `scripts/smoke-stack.sh` starts the optional service, but the default smoke suite does not assert retrieval. Use the opt-in Qdrant regression below to test the vector round trip without external embedding calls.
- ✅ Preflight: startup `bootstrap_recall_index` (`den/src/startup.rs`) logs a warning when `QDRANT_URL` is set but Qdrant is unreachable (`ensure_collection` fails), then continues with LIKE fallback; the runtime `/status.json` check also covers this at request time.

**Exit:** smoke embed of fixture text; health check passes. *(Health check ✅; live fixture embed pending a real embedding-capable key in the smoke env.)*

## Phase 1 — Passage registry + Bear indexer  ✅ landed (1a indexer core + 1b auto-enqueue)

- ✅ Migration: `recall_passages` Postgres table (`bear_id` FK, `memory_id`, `logical_path`, `chunk_index`, `content_hash`, `embedding_standard`, `source_class`, `point_id`, `indexed_at`, `deleted_at`; unique on `(bear_id, memory_id, chunk_index, embedding_standard)`). Registry metadata only — vectors in Qdrant, canonical text in SQLite (ADR-0038 §3).
- ✅ `den-runtime::recall` module: `qdrant` (point upsert/delete/count), `chunking` (~2.4k-char chunks + overlap, SHA-256 `content_hash`), `policy` (indexable decision, deterministic `point_id`, Qdrant payload), `registry` (Postgres CRUD with dedup + soft-delete), `indexer` (`RecallIndexer::index_record` / `remove_record`, embedder-agnostic via `PassageEmbedder`; `EmbeddingClient` impl + `DeterministicEmbedder` test stub).
- ✅ Indexing policy per ADR-0038/ADR-0041: `visibility = normal`; `shared` records indexed; `profile_local` limited to `note`/`decision`/`summary` only while a Bear has no hats; `scratch`/`log` excluded; lifecycle `archived`/`superseded` records are not indexed. Head-only selection uses canonical head queries, skips `invalid_at` records, and respects store-level/reviewed supersession writes. **Hat-boundary follow-up:** reviewed `hat` heads with canonical hat IDs are now indexable; source-local raw notes and access-bearing records are not. Reconcile removes no-longer-indexable derived points. The new Bear-admin source→hat, unattributed legacy→hat, and hat→core review routes best-effort enqueue the same whole-Bear reconcile when recall is configured. First-hat creation also enqueues it: configured Bears omit legacy profile-local heads and remove any previously indexed legacy points on successful reconcile, while no-hat Bears retain legacy profile recall. The watermark uses the same eligibility filter; cleanup may lag if the recall worker is unavailable.
- ✅ **Phase 1b — async queue**: a `bear_reflection_runs` `recall_index` lane + dedicated worker loop, mirroring `memory_curate` (Den has no Docket job queue). `enqueue_recall_index` coalesces (one queued run per Bear); the worker claims with `FOR UPDATE SKIP LOCKED` and calls `recall::reconcile_bear`, which indexes every indexable canonical **head** per logical path and removes passages for memory ids that are no longer heads (supersede/delete). Worker is spawned only when `RUN_WORKERS` **and** `QDRANT_URL` are set; recall-disabled runs are drained as skipped.
- ✅ **Enqueue coverage** via the shared `enqueue_recall_index_if_enabled` helper (guards on `QDRANT_URL`, best-effort/logged — never fails the caller's tool/turn) on canonical producers with a `PgPool` in scope: reviewed source→hat/hat→core publication, role `memory_write_entry` (`DenRoleMemoryStore`), work-surface scaffolds (`DenWorkSurfaceOps`), and pair-reflection summaries. The former proposal-based curate core promotion (`apply_core_promotion`) has been retired. Because reconcile is whole-Bear, any one trigger re-indexes all of that Bear's indexable heads — no separate periodic sweeper is needed (it reuses the existing conductor worker).

**Exit:** ✅ unit tests (chunking/policy/payload/point-id) + gated **live Postgres + Qdrant** integration test (`tests/recall_indexer.rs`, stub embedder, no API key) proving write memory → one Qdrant point per chunk → idempotent re-index (no duplicates) → `remove_record` deletes the points; plus an infra-free `list_indexable_heads` test proving head-selection + policy filtering (latest-at-path wins, `scratch` excluded).

> **Follow-ups (not blocking):** live worker exercise end-to-end (rebuilt Den + a real embedding key) is pending. Curate/consolidation now has reviewed supersession writes and unsafe-promotion gates; semantic dedup/synthesis policy remains open.

## Phase 2 — Bear recall in turn context  ✅ landed (retrieval + assembler section)

- ✅ `den-runtime::recall::query` (`recall_for_turn` / `recall_for_turn_scoped`): embeds the turn query via `EmbeddingClient` (embedder-agnostic, same `PassageEmbedder` trait as the indexer), runs a `bear_id` + `source_class` + `embedding_standard`-scoped Qdrant vector search (`QdrantRecall::search`), then dedupes to the best-scoring chunk per `memory_id` and returns the top *N* (default 5) `RecalledPassage`s. The chunk `text`, `salience`, lifecycle status, and freshness trend are denormalized into the Qdrant payload (ADR-0038 §2 derived data) so recall renders and scores with **no** SQLite round-trip.
- ✅ Context assembler (`assemble_native_turn_for_bear`): best-effort `## Recalled memory` section appended after the key-memory projection block, with a ~2.6k-char budget and per-snippet truncation. `render_recall_block` drops any passage whose `logical_path` already appears in the projection text, so **recall never duplicates anchors**. Recall is skipped only for empty queries (all profiles, including `chat`, get projection + recall when configured).
- ✅ **Legacy role-scoped**: no-hat turns still use the role/profile filter. Hat-bound turns instead use the Den-verified source/hat/core filter and canonically recheck hits before projection. Current bound turn snippets are rebuilt from SQLite rather than trusting denormalized payload text; unindexed source-local notes remain available by on-demand scoped tools and key memory projection.
- ✅ Recall availability is best-effort: unset `QDRANT_URL`, a disabled embedding client (no `LLM_API_URL`), or transport errors yield no recall section rather than failing a turn. For bound turns, access-bearing and stale-record gates are now enforced by canonical SQLite before any passage text reaches the model.
- ✅ Diagnostic JSON (`recall_query`: status, raw_hits, passages, embedding_standard) surfaced on `AssembledNativeTurn.recall_diagnostic` alongside `key_memory_projection`.

**Exit:** ✅ unit tests (anchor dedupe, all-deduped→`None`, snippet truncation) + a gated **live Postgres + Qdrant** retrieval test (`recall_query_retrieves_indexed_passage_against_live_qdrant`, deterministic embedder, no API key): index a passage → query the same text → the passage returns as a top hit (score ~1.0) with its payload text + path → render dedupes it against an anchored path.

> **Follow-ups (not blocking):** richer query text (session focus / primary work surface, not just the human message) — *deferred: the assembler `client_context` carries no topical work-surface field yet; lands with the Phase 6 session→entity focus*; persisting `recall_diagnostic` to turn telemetry — *deferred: no turn-telemetry sink exists; the diagnostic is already returned on `AssembledNativeTurn` and surfaced in the recall admin panel*; live exercise with a real embedding key (shared with the Phase 1 live-worker follow-up).

## Phase 3 — Hybrid `memory_search`  ✅ landed

- ✅ The model-facing `memory_search` tool is hybrid: `DenRoleMemoryStore::search` delegates to `den-runtime::recall::hybrid_memory_search` (orchestration in `den-core` stays runtime-agnostic). It returns the **union** of the derived **vector** index and the SQL `LIKE` **keyword** leg over canonical SQLite — vector hits rank first (semantic relevance is higher-signal + carries a `score`), then keyword-only matches fill remaining slots, surfacing exact-substring hits the vector leg missed. The vector leg is best-effort (an unconfigured index or any transport error degrades to keyword-only, logged, never failing the tool); the keyword leg reads the canonical store, so its errors propagate.
- ✅ Legacy tool legs remain **role-scoped to the same visibility**: shared (core) records OR the calling role's own profile-local records, so semantic recall honors the role-local boundary (AGENTS.md: `work` must not read raw `pair/`). Hat-bound tools use the Den-verified source/hat/core grant instead. Vector leg uses a nested `should` (shared ∨ `scope_profile = role`) inside the mandatory bear scope (`role_scope_filter` via `search_bear_memory_for_role`); the keyword `LIKE` query was broadened from `scope_profile = role` to `(scope_type = 'shared' OR scope_profile = role)` to match.
- ✅ Unified provenance: each hit is `{ memory_id, path, kind, score, snippet, source, salience, lifecycle_status, freshness_trend }` (`source` ∈ `vector`/`keyword`/`graph`; `score` null for unranked SQL/graph legs), with a top-level `strategy` (`vector`, `keyword`, `graph`, or a `+` union), `storage: "hybrid"`, and the vector leg's recall `diagnostic`. Results de-dupe by `memory_id` and cap to the requested limit, prioritizing vector hits. Vector scores apply ADR-0041 salience and freshness multipliers; keyword fallback excludes lifecycle-archived rows.

**Exit:** ✅ tool tests for both legs — infra-free unit tests for the role-scope filter shape and the merge (union + de-dupe, `strategy` selection, limit-capping prioritizing vector), an infra-free keyword-leg test proving the role boundary (shared + own role visible, another role's profile-local excluded), and the existing gated **live Postgres + Qdrant** retrieval test covering the shared `search_passages` core.

**Member-library follow-up:** the browser member semantic mode now searches Bear-core and owned-hat candidates only. It discards missing, stale, restricted, foreign-hat, source-local, and legacy IDs by looking up each result in the canonical curated SQLite library, then rebuilds all displayed metadata/snippets from SQLite. An opt-in integration regression (`BEARS_RECALL_TEST_QDRANT_URL=http://bears-qdrant:6333 cargo test -p den-service --lib real_qdrant_reconciles_first_hat_and_rechecks_member_hits --offline -- --ignored`) passed against live Qdrant on 2026-09-30: it indexes synthetic records with an 8D deterministic embedder, creates a first hat, reconciles old profile-local points away, filters another hat, and rebuilds a poisoned/stale vector hit from canonical SQLite. It uses a unique test collection and removes it. It does **not** call Bifrost or validate production embedding quality, background worker execution, or a browser HTTP round trip. Keyword remains the fallback when recall is disabled or produces no authorized current hits; this is not a grant from a Qdrant payload or a promise that embeddings are configured in every deployment.

## Phase 3.5 — Temporal + bounded graph recall legs  ✅ landed

Extends the hybrid retriever ([ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md) §6) beyond vector + keyword + anchors with two cheap legs, both over **canonical SQLite** (no new store). Borrowed from Hindsight's TEMPR (temporal + graph strategies) without adopting its graph/temporal store.

**Temporal leg.**  ✅ landed

- ✅ Additive `valid_from`/`invalid_at` event-time columns on `memory_records` (idempotent ALTER for existing per-Bear SQLite; `append` writes `valid_from = created_at`; recall reads `COALESCE(valid_from, created_at)`). Store-level supersession writes set predecessor `invalid_at`.
- ✅ `recall::temporal::parse_time_expression` parses explicit/relative expressions (`today`, `yesterday`, `last N days|weeks|months|years`, `this week|month|year`, `in June [2026]`, explicit `YYYY-MM-DD`, `before`/`since`/`after`, `as of`) into an effective-time window + residual query. `hybrid_memory_search` strips the temporal phrase (so the direct legs match the topical remainder), over-fetches, and filters hits by window, emitting a `temporal` diagnostic.
- ✅ Point-in-time recall: `den_memory::head_record_as_of` returns the valid head at an instant by walking the supersession chain (`effective ≤ at` and not superseded-by-then); `as of` queries also drop hits already superseded as of the upper bound.
- Recency stays a ranking factor for untimed queries.

**Bounded graph leg (record↔entity expansion).**  ✅ landed

- ✅ _Prerequisite landed_ ([Bear Entity Layer Phase 4](BEAR_ENTITY_LAYER_IMPLEMENTATION_PLAN.md)): each passage payload denormalizes its resolved **descriptive** `entity_ids`, and `recall::search_bear_memory_for_entities` gives an entity-membership-scoped vector leg.
- ✅ `den_memory::bounded_graph_expand` expands over the **bipartite** record↔entity relation graph (`memory_relations`): record → entity → co-occurring records. Seeded by the vector/keyword hits, it surfaces records related through a shared entity that the direct legs never matched. Exposed as a third `memory_search` leg (`source: "graph"`, with `hop` + `entity_overlap`), fail-open.
- ✅ **Depth-capped (default 2 hops), read-only, retrieval-time only.** No stored transitive edges, no inference, no entity↔entity edges — consistent with [ADR-0042](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md) anti-RDF guardrails. Reached records are role-scoped (shared ∨ own role-local); access-bearing relations are excluded by construction (traversal is over the descriptive table) and `AccessContext` is applied by the caller once access rules exist.
- ✅ **Entity-overlap boost:** within each hop tier, reached records sharing more entities with the seed set rank higher. Graph hits also carry lifecycle/freshness indicators from canonical SQLite.

**Exit:** ✅ recall tests for (a) a relative-time / `before`-window query filtering by effective time + an as-of supersession-chain walk, and (b) an infra-free 2-hop entity query returning a record never directly matched by keyword (vector disabled, no Qdrant); plus parser-grammar units and a merge dedupe/ordering unit test.

## Phase 4 — Cabinet integration  ◻ deferred (blocked on the Cabinet ingestion pipeline)

- Cabinet pipeline ([ADR-0008](../decisions/adr-0008-cabinet-reading-pipeline.md)) would use the **same** `bears-embed-v1` into Qdrant (`source_class=cabinet_passage`).
- Cross-corpus query policy: optional Cabinet hits when mission/work-surface link + ACL allow.

> **Deferred:** there is no Cabinet passage/ingestion pipeline in the codebase yet (no `cabinet_passage` producer). Building the recall side alone would be unwired dead code; this phase unblocks once ADR-0008 ingestion lands.

**Exit:** end-to-end test — Cabinet passage + related Bear memory rank near each other for shared topic query.

## Phase 5 — `archive_index` + migration tooling  ✅ lane + reindex CLI landed (embedding-standard migration deferred)

- ✅ The `recall_index` reflection lane **is** the reconcile sweep (registry vs canonical vs Qdrant): `bear_reflection_runs` `recall_index` lane + worker claim (`FOR UPDATE SKIP LOCKED`) → `reconcile_bear`. (`archive_index` was the aspirational name; the implemented lane is `recall_index`.)
- ✅ CLI: `den reindex (--bear <uuid> | --all)` runs a synchronous whole-Bear reconcile (`recall::reindex_bear_now`) with per-Bear counts, bypassing the queue so it works one-shot without `RUN_WORKERS`; bails cleanly when `QDRANT_URL` is unset.
- ◻ `migrate-embedding-standard` (build v2 collection, progress, alias flip) and `reindex-cabinet` are **deferred**: the former needs Qdrant alias plumbing + a second embedding standard (only `bears-embed-v1` exists); the latter waits on Phase 4.

**Exit:** documented runbook for `bears-embed-v2` dry run on staging *(pending the migration tooling above)*.

## Risks

| Risk | Mitigation |
|------|------------|
| Embed cost at Cabinet scale | Content-hash dedup; incremental indexing; batch embed jobs |
| Model lock-in | Versioned standards; parallel collections; measure before v2 flip |
| ACL leaks on unified collection | Mandatory payload filters; tests per source_class |
| Qdrant downtime | Degrade to anchors + LIKE search; no turn failure |

## Related

- [DEN_RUNTIME_PLAN.md](DEN_RUNTIME_PLAN.md) — in-process Den loop and context assembler
- [model-tasks-strategy.md](../research/model-tasks-strategy.md) — `embedding` task class
