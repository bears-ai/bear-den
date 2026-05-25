# Letta Dependency Matrix

## Purpose

This document inventories the current repository's dependency on Letta so Bear Den can plan an orderly migration away from the deprecated Letta API server.

It focuses on:

- where Letta appears in the codebase and deployment stack
- what capability Letta currently provides in each place
- how critical each dependency is
- what likely replacement owner should exist in a post-Letta architecture

## Summary

Bear Den currently depends on Letta in four major ways:

1. **Runtime execution substrate** for API-direct roles (`pair`, `curate`, `watch`)
2. **Agent registry and provisioning target** for all Bear role agents
3. **Harness backend** for `talk` and `work` via Codepool / Letta Code
4. **Operational datastore/read model** for conversations, diagnostics, and admin UI

A fifth category, **memory/indexing**, is present but appears less foundational than the runtime and registry dependencies because canonical Bear memory already lives primarily in MemFS/git role branches rather than inside Letta-native memory blocks.

## Dependency matrix

| Area | Component / file(s) | Current Letta dependency | Capability Bear Den is using | Criticality | Suggested future owner |
|---|---|---|---|---|---|
| Deployment | `docker-compose.yaml`, `.env.example` | Runs `bears-letta`, Letta Postgres, shared Letta volume, Letta auth, health checks | Core runtime service, API endpoint, storage layout, ops wiring | High | Den-owned runtime service plus direct MemFS/index services |
| Runtime config | `services/den/src/config.rs`, `services/den/src/startup.rs` | `LETTA_BASE_URL`, `LETTA_API_KEY`, `LETTA_MEMFS_SERVICE_URL`, `LETTA_PG_URI`; startup health validation | Runtime discovery, auth, health enforcement | High | Den-native runtime config plus narrow Letta compatibility settings |
| Letta client layer | `services/den/src/core/letta/mod.rs`, `services/den/src/core/letta/client.rs` | Dedicated Letta REST client and message lifecycle helpers | Agent CRUD, conversation APIs, streaming, approvals, cancellation, compaction | High | Den role runner + interaction/run store + Letta compatibility adapter |
| Bear provisioning | `services/den/src/core/bears/provision.rs` | Creates role-specific Letta agents; resolves Letta tool ids; stores `letta_agent_id`; registers MemFS role views keyed by agent | Bear role runtime provisioning and identity | High | Den-owned role profile registry with temporary compatibility bindings where needed |
| Bear reconciliation / drift | `services/den/src/core/bears/sync.rs`, `services/den/src/core/bears/letta_drift.rs` | PATCH/recompile Letta agents; compare Den desired state vs Letta state | Runtime config sync and drift detection | High | Den-owned runtime registry and diagnostics snapshots |
| ACP / pair runtime | `services/den/src/api/acp.rs` | Uses Letta conversation/run lifecycle, tool continuation, pending approval handling, run cancellation hygiene | Stateful API-direct agent loop for `pair` | High | Den-native API-direct runner |
| API state wiring | `services/den/src/api/service.rs` | Injects shared `LettaClient` into API state | Makes Letta a first-class API dependency | High | Runtime trait implementation selected by config |
| Web conversation data | `services/den/src/web/data/letta.rs` | Fetches agent state, conversations, message history, title/archive/delete operations | Read model for Den web/admin UI | High | Den-owned conversation store/read model |
| Bear management UI | `services/den/src/web/bear_management.rs` | Lists Letta conversations for talk/pair, shows drift/diagnostics, filters tool ids via Letta catalog | Admin/operator visibility into runtime state | Medium-High | Den read model + runtime diagnostics service |
| Bear spec / architecture | `services/den/docs/bear-spec.md` | Role model explicitly names Letta API direct and Letta Code harness runtime families | Architectural contract and current assumptions | High | New Bear Den-native runtime spec |
| Letta Code harness config | `services/den/src/core/bears/letta_code_harness.rs` | Generates Letta Code harness YAML with Letta server URL/API key and role agents | Channel harness integration for `talk`/`work` | High | Codepool-next or alternative Bear Den-native harness config |
| Codepool integration | `services/den/src/core/codepool/client.rs`, `docker-compose.yaml` | Codepool is documented and configured as Letta Code SDK harness; depends on Letta base URL/API key | Harness-backed runtime for `talk` and `work` | High | Codepool backend replacement or new Bear Den runtime service |
| Seed/dev bootstrap | `services/den/src/seeds.rs` | Smoke seed provisions missing Bear role agents through Letta | Local/dev environment bootstrap | Medium | Den-native runtime bootstrap |
| Startup / ACP validation | `services/den/src/startup.rs` | ACP currently requires Letta to be configured | Hard runtime gate for API-direct operation | High | ACP should target generic runtime provider |
| MemFS integration | `docker-compose.yaml`, `services/den/src/core/bears/provision.rs`, `services/den/src/core/memory_manager_head.rs` | Letta-shaped storage layout, Letta memfs sidecar URL, agent-associated role views | Memory repo compatibility and per-agent role view registration | Medium-High | MemFS manager should become direct owner of repo/view lifecycle |
| Git proxy behavior | `docker-compose.yaml` comments, Codepool env comments | Letta `/v1/git/*` proxy updates Letta-side cache / memory block state | Git smart-HTTP and Letta cache synchronization side effects | Medium-High | Direct MemFS/git service plus explicit indexing hooks |
| Tool catalog resolution | `services/den/src/core/letta/client.rs`, provisioning/sync call sites | Den resolves canonical tools to environment-specific Letta tool ids | Runtime tool attachment / compatibility mapping | Medium | Den-native tool registry |
| Model catalog lookup | `services/den/src/core/letta/client.rs`, provisioning flows | `GET /v1/models/` used for Letta-compatible model selection | Provisioning-time model options | Medium | Bifrost/Den model registry |
| Conversation lifecycle UI actions | `services/den/src/web/data/letta.rs`, Letta client | Patch summary/title, archive/unarchive, delete | Thread management UX | Medium | Den-owned conversation metadata store |
| Conversation history | `services/den/src/core/letta/conversations_list.rs`, `client.rs`, web call sites | Lists conversations and messages per role agent | User-visible and operator-visible thread history | High | Den-owned message/event store |
| Approval recovery | `services/den/src/core/letta/client.rs`, ACP call sites | Inspects Letta approval variants and denies pending approvals to recover poisoned runs | Recovery semantics for interrupted tool flows | High | Den-native approval state machine |
| Run cancellation | `services/den/src/core/letta/client.rs`, ACP call sites | Cancels Letta runs by run id to avoid unsafe agent-wide cancellation | Concurrent session safety | High | Den-owned run controller |
| Conversation compaction | `services/den/src/core/letta/client.rs` | Calls Letta compact endpoint | Context maintenance / summarization | Medium | Den summarizer / transcript compactor |
| Agent diagnostics | `services/den/src/core/letta/agent_diagnostics.rs`, `agent_summary.rs`, `agent_document.rs` | Reads Letta agent docs/diagnostics/block/tool state | Debugging, drift analysis, admin visibility | Medium | Den runtime diagnostics snapshots |
| Archives / retrieval | `AGENTS.md` and architecture guidance | Letta archives described as derived semantic retrieval indexes | Semantic retrieval over canonical sources | Medium | Qdrant or another owned retrieval/index layer |
| Skills for API-direct roles | `services/den/docs/bear-spec.md` | API-direct roles currently expected to attach skills through Letta API | Skill projection/install path | Medium-High | Den-owned skill projection system |

## Detailed dependency notes

### 1. Deployment and infrastructure

The root compose stack treats Letta as a first-class service:

- `bears-letta`
- `bears-letta-postgres`
- shared Letta home/data volume
- Letta-specific health checks
- Letta-specific env defaults
- Letta-specific backup target names

This means migration is not just application-code work. It will also require:

- compose/Coolify changes
- health/readiness changes
- backup/restore changes
- environment variable cleanup

### 2. Runtime loop dependency is stronger than memory dependency

After inspecting the repo, the heaviest dependency appears to be Letta's runtime execution and conversation lifecycle, especially for API-direct roles.

The repository uses Letta for:

- durable conversation creation/listing/history
- streaming message execution
- tool-return continuation
- approval and pending-approval recovery
- run cancellation
- conversation compaction

By contrast, canonical Bear memory appears to be more strongly rooted in MemFS/git role branches. That means the immediate migration challenge is less "replace memory storage" and more "replace runtime orchestration and conversation persistence."

### 3. Pair is the highest-risk API-direct migration

`services/den/src/api/acp.rs` contains substantial Letta-aware logic for:

- active turn hygiene
- run id tracking
- cancellation without agent-wide collateral damage
- waiting-for-approval recovery
- client tool continuation

This suggests `pair` should not be the first role migrated off Letta, even though it is one of the API-direct roles. `watch` is likely a better first proving ground, then `curate`, then `pair`.

### 4. Talk/work remain indirectly Letta-dependent through Codepool

Even if Den stops calling Letta directly for API-direct roles, the system is still Letta-dependent because Codepool is currently configured as a Letta Code harness.

So migration likely needs two tracks:

1. replace API-direct runtime for `pair` / `curate` / `watch`
2. replace or evolve Codepool backend for `talk` / `work`

### 5. MemFS is promising, but still Letta-shaped operationally

Canonical memory appears portable because it lives in role branches, but operations still assume Letta in several places:

- Letta-compatible storage layout under `~/.letta`
- Letta MemFS sidecar URL naming
- Letta `/v1/git/*` proxy behavior
- agent-associated view registration
- comments about Letta updating its Postgres memory block cache

That suggests MemFS is a strong foundation for migration, but still needs its own cleanup phase.

## Architectural caution

This inventory should not be read as a reason to build a broad, symmetric runtime-provider abstraction layer.

The desired destination is a Den-native runtime that owns role execution, interaction persistence, approvals, and policy. Letta should be pushed into temporary compatibility adapters and legacy bindings only where needed during migration. In particular, replacing `letta_agent_id` with a new generic `provider` concept everywhere would preserve the same coupling in a more abstract form.

## Migration implications by dependency area

| Dependency area | Near-term strategy |
|---|---|
| Agent provisioning | Introduce a Den-owned role profile registry and temporary compatibility-binding handling before replacing the implementation |
| API-direct execution | Build a Den-native runner, starting with `watch` |
| Conversation persistence | Dual-write into Den-owned tables before cutting Letta over |
| UI/admin read model | Repoint to Den-owned conversation and runtime tables |
| Codepool harness | Either replace Letta backend under Codepool or replace Codepool's Letta-specific role |
| MemFS/git side effects | Move git/view/indexing ownership out of Letta and into MemFS manager + explicit index jobs |
| Retrieval/indexing | Replace Letta archives with Qdrant or another owned retrieval layer after runtime separation |

## Recommended replacement abstractions

To make migration incremental, Bear Den should define internal interfaces that Letta currently spans implicitly.

### Agent registry

Responsibilities:

- create/update role runtime instances
- persist runtime handles and config hashes
- store diagnostics snapshots
- expose drift state

### Conversation runtime

Responsibilities:

- start/continue runs
- expose tool calls
- accept tool returns
- track approvals
- cancel runs safely
- emit stream events

### Conversation store

Responsibilities:

- persist threads
- persist messages/events/tool calls
- manage title/archive/delete metadata
- support web/admin read APIs

### Retrieval/index service

Responsibilities:

- index canonical sources from MemFS/core/Cabinet
- serve semantic retrieval independent of runtime engine

## Source references

Key repository references used for this matrix:

- `docker-compose.yaml`
- `.env.example`
- `AGENTS.md`
- `services/den/src/config.rs`
- `services/den/src/startup.rs`
- `services/den/src/core/letta/mod.rs`
- `services/den/src/core/letta/client.rs`
- `services/den/src/core/bears/provision.rs`
- `services/den/src/core/bears/sync.rs`
- `services/den/src/core/bears/letta_code_harness.rs`
- `services/den/src/core/codepool/client.rs`
- `services/den/src/web/data/letta.rs`
- `services/den/src/web/bear_management.rs`
- `services/den/src/api/service.rs`
- `services/den/src/api/acp.rs`
- `services/den/src/seeds.rs`
- `services/den/docs/bear-spec.md`

## Conclusion

The repository's Letta dependency is broad and real, but it is structured enough to unwind incrementally.

The most important insight from the codebase is that **Letta is currently more of a runtime/conversation/agent-control substrate than the canonical memory system of record**.

That is good news for migration because canonical memory already appears substantially owned by Bear Den. The hard part will be replacing:

- runtime execution loops
- conversation state and approvals
- agent provisioning/registry
- Codepool's Letta-backed harness behavior

rather than merely swapping out a vector store.
