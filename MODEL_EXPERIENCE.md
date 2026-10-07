# Model Experience

How Bear Den manages what the model experiences: what it sees each turn, which tools appear and where they run, what memory is visible, what counts as a legal continuation, and what warnings arrive in-band. This is a consolidated view; the canonical sources are indexed at the end and win on any disagreement.

## The governing idea

Den does not hand the model "everything," and it does not let the model guess its own situation. Every turn, Den deliberately compiles a bounded **turn context** from explicit, separately-owned layers — and wherever Den already knows a runtime condition (permissions, budgets, active work), Den renders the single applicable instruction before inference instead of asking the model to choose. The model's experience is a managed product of the system, assembled the same way regardless of which surface (ACP armature, web chat, background dispatch) the turn came from.

## What the model sees: the context layers

Each turn is assembled in this order ([docs/architecture/den-runtime.md § turn context assembly](docs/architecture/den-runtime.md#turn-context-assembly), [docs/architecture/context-compilation-scenarios.md](docs/architecture/context-compilation-scenarios.md)):

```text
system:  compiled stance prompt            (pre-turn compiled, Postgres)
       + key memory projection             (bounded anchors from per-Bear SQLite)
       + derived recall                    (optional vector passages, labeled as recall)
       + runtime supplements               (prompt-memory blocks, compaction state,
                                            capability guidance/recent discoveries,
                                            channel/tool reminders)
messages: projected transcript incl. replayable tool calls/results + current step
tools:    merged Den-hosted + client descriptors
```

| Layer | What it is | Who owns it |
|-------|------------|-------------|
| **Compiled stance prompt** | Identity, stance contract, operator steering. Compiled ahead of the turn from repository-authored prompt fragments plus runtime-authored (e.g. admin-edited) content into `bear_compiled_configs`. The turn hot path never parses prompt files or renders DB templates. | Prompt compiler ([ADR-0046](docs/decisions/adr-0046-file-backed-prompt-fragments-and-compiled-runtime-prompts.md), [prompt fragment registry](docs/architecture/prompt-fragment-registry.md)) |
| **Key memory projection** | A small, stance-scoped briefing of canonical memory: shared identity anchors, active work-surface anchors, stance-local highlights. Latest-head records only, under strict character budgets — never the whole memory store. | Context assembler over per-Bear SQLite |
| **Derived recall** | Semantically retrieved passages when the vector index is configured. Explicitly labeled as recall — a search result over memory, not memory itself. | Derived recall index ([ADR-0038](docs/decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)) |
| **Prompt-memory blocks** | Editable standing context scoped by Bear/stance/session/work surface. Distinct from long-term memory and from transcript. | Den Postgres ([contract](docs/architecture/den-prompt-memory-block-contract.md)) |
| **Runtime supplements** | Turn-local reminders rendered from typed runtime state: tool/memory scope for a trusted session, active Docket execution, capability-discovery guidance and a bounded recently-discovered working set, date/budget reminders, compaction state. Recently discovered capabilities are context only, not an authority grant. | Repository-owned runtime fragments |
| **Transcript** | The model-replay projection of canonical conversation storage — a distinct projection from user-visible history. | Den conversation persistence |
| **Tool/action descriptors** | The model-facing action surface and schemas for this stance and surface. Descriptors also carry runtime semantics: blocking tool, client obligation, non-blocking structured update, or ephemeral progress. | Descriptor registry + client capabilities |

## Primary model configurations

In the current branch, named Bear-owned configurations select a catalog model and optional Low/Medium/High reasoning effort; Model default leaves ordinary effort unset. Conversation pins override the entire configuration (no inherited effort), otherwise the current canonical hat override wins over the Bear configuration and deployment default. Work uses its eligible Job hat, not a transcript's model pin. Automatic historical model rows do not freeze inheritance.

Explicit effort reaches ordinary and continuation requests and takes precedence over checkpoint effort; with Model default, existing loop-control checkpoint behavior remains. Unknown or unsupported catalog capabilities reject explicit effort. An incompatible Chat Completions function-tool bridge omits effort and records that disposition instead of claiming it was sent. Persistent model-request diagnostics include configuration identity/name, selection source, model, configured and effective request effort, and transport. Browser turns without a run ID retain session-scoped diagnostics.

New turns reload current settings. Before further inference, a changed in-flight model/effort requires a fresh turn; revoked model/capability choices fail closed. Configuration does not confer routing, permissions or delegation authority. Helper/subagent execution, fallback chains and advanced request-limit controls are not implemented by this slice. Live provider acceptance and deployment are deferred; see [the maintained topic](docs/topics/bear-memory-hats.md#primary-model-configurations).

## Reviewed instruction-only Skills

For verified hat-bound browser/editor/Work turns, Den selects enabled attachments whose catalog version remains approved and whose pinned content hash still matches. The `bound_skills` Markdown fragment renders those procedures through the context compiler; attached uses are explicit. Drafts, disabled catalog versions and detached procedures are absent from subsequent bound prompts. Skills do not install binaries, add tools or grant credentials/resources; existing descriptors and live policy still determine execution. This initial implementation is human-authored catalog review, not model-facing skill proposal/installation tools or internal Curate/Watch procedure configuration.

## What the model deliberately does not see

The exclusions are as designed as the inclusions:

- **Other stances' raw memory branches.** Projection is stance-scoped; `work` never sees raw `pair/` or `chat/` (see [SAFETY.md](SAFETY.md)).
- **Unreviewed proposals and pending observations** — reachable only through tools or curation review, never proactively projected.
- **Docket/task state as "memory."** Work management is control-plane infrastructure, surfaced through task tools and runtime fragments, not memory projection.
- **Superseded record history.** Proactive projection shows latest heads; history stays tool-mediated.
- **The whole transcript, forever.** Long conversations are compacted: older context becomes derived summary state, kept explicitly separate from raw transcript and from memory, with active goals, constraints, and tool/approval state preserved.
- **Raw provider/protocol machinery.** ACP is an edge adapter; the model should never learn ACP-specific fake capabilities as core semantics ([ADR-0043](docs/decisions/adr-0043-acp-as-edge-adapter-protocol-agnostic-core.md)).
- **Provider reasoning display streams.** Provider-emitted reasoning deltas may be projected live to capable clients as thought UI, but they are display telemetry: not assistant answer content, not model transcript replay, not canonical conversation history, not task state, and not memory.

## The tool and action surface

- **Names are stable and provider-safe.** Model-facing names are concise action names (`memory_search`, `web_fetch`, `fs_edit_file`); canonical internal names are scoped and dotted (`den.memory.search`). Legacy aliases are accepted at boundaries but never advertised ([ADR-0017](docs/decisions/adr-0017-provider-safe-tool-naming.md), [ADR-0025](docs/decisions/adr-0025-tool-naming-and-execution-strategy.md)).
- **Execution location is descriptor-owned.** Whether an action runs inside Den, through the armature, or in a sandbox is metadata, never inferred from the name.
- **Runtime semantics are descriptor-owned.** The model may see simple, tool-like verbs, but Den decides whether each action is a blocking tool exchange, a client obligation, a non-blocking structured update, or ephemeral progress. The model does not need to understand implementation terms such as "non-blocking"; it needs clear actions with narrow schemas.
- **The surface is stable across turns.** Actions are not hidden or revealed per-turn by prompt heuristics — that teaches the model false capabilities. Availability changes only with the stance, surface, or explicit policy.
- **Blocking tool exchanges are replayable transcript state**: stable call id, canonical name, typed arguments, matching result or error, bounded output. What the model saw a data-dependent tool do is always reconstructable next turn.

### Blocking tools vs structured updates

The operative question for a model-facing action is: **does the model need the result before it can continue reasoning?**

| Model-facing action | Runtime semantics | Model needs result before continuing? |
|---------------------|-------------------|---------------------------------------|
| `fs_read_text_file`, `git_diff`, `web_fetch` | Blocking tool exchange | Yes |
| approval / local tool execution | Client obligation | Yes, as a decision/result |
| `set_conversation_title` | Non-blocking structured session update | Usually no |
| `report_progress`, in-flight task status | Non-blocking structured update or ephemeral progress | No |
| task completion, handoff, resource binding | Blocking/gated update or client obligation | Often yes |

Non-blocking structured updates may be persisted, replayed, and projected to UI, but they must not by themselves create open client obligations, require tool-result continuation, or cause model continuation. They are how Den records surface/control-plane facts such as conversation titles or in-flight task status without turning metadata into a model dependency ([non-blocking structured updates](docs/architecture/non-blocking-structured-updates.md)). For smaller or less reliable models, keep these actions concrete (`report_progress`, `update_task_status`) and schema-limited; Den may coalesce, rate-limit, or gate authoritative terminal state changes.

Model-facing communication should use provider-native function/tool calls whenever available. If Den needs a text fallback for a weaker provider path, the fallback should be a small function-call DSL such as `update_task_status(status="in_progress", summary="Reading config")`, not raw JSON, Markdown fences, XML blocks, or protocol event names.

## Memory visibility

The model touches memory in exactly three ways, each bounded:

1. **Projection** — the proactive briefing described above.
2. **Recall** — optional derived passages, labeled as recall.
3. **Tools** — `memory_browse` / `memory_read` / `memory_search` for on-demand retrieval, and stance-local writes (e.g. `pair` writes `pair/` entries; promotion to shared memory goes through review, never directly).

Trusted facts come from typed context, not chat text: human identity for an ACP `pair` session comes from the session token via `session_info`, and conflicting claims in the conversation do not override it.

### Cabinet: shared knowledge, distinct from memory

Cabinet is the Den-wide shared knowledge wiki that humans and Bears read and edit together — a tree of pages, with no separate folder or collection concept ([contract](docs/architecture/cabinet-contract.md)). A "Mission" is simply a page whose child pages are the grouped material. Descriptor audiences advertise `cabinet_search`, `cabinet_read`, and `cabinet_history` to verified ordinary Chat/editor/browser-task/eligible-Work sources; writes/source changes are interactive Chat/editor/browser-task operations with mutable governance. Internal Curate/observation sources do not borrow that ordinary surface, and lifecycle is not currently advertised to models. The distinction the descriptors teach:

- **Bear memory is private cognition; Cabinet is shared, durable, human-visible knowledge.** Memory tools never write Cabinet, and Cabinet tools never write memory.
- **Every write creates an immutable revision.** Human writes publish directly; page/ancestor policy can require Bear-written versions to remain `pending` until an authorized human review. Rejection preserves the version; approval fails if its published base changed. `cabinet_update` requires the `base_version` from a fresh `cabinet_read`; a stale base returns a structured conflict with the new current version — the model re-reads, merges, and retries. Nothing merges silently.
- **Authorization is server-side.** Bears with `cabinet_enabled` off do not see the tools, and the facade independently rejects them. Page and ancestor membership/policy now narrow access server-side, including search before LIMIT and known-ref reads/writes. A Cabinet-enabled Bear does not thereby gain access to every page. The current model tool schemas still expose the existing root-create/search operations; child-page organization and policy/review administration are web/facade operations.
- **Nothing is destructive.** Archiving (via `cabinet_lifecycle`) is reversible and keeps every revision readable, and `cabinet_history` plus a `version_ref` read recovers any earlier state. Deleting an item is reserved to people — the facade refuses a Bear even if it asks — so the model's worst case is an edit someone can revert, not lost knowledge.
- **Provenance is separable from content.** `cabinet_source_link` records where knowledge came from without publishing a revision; Cabinet stores the link, never the linked bytes.
- **Attachments and private copies do not grant access.** The web/facade supports linking existing finalized artifacts and independently checks page and payload access. Docket Job Mission annotations point at ordinary Cabinet pages, not containers. Human snapshot capture creates private immutable published-document evidence, retained and downloadable from its authorized Job; it does not change Job status or inject private content into Work. There are no new model-facing attachment/capture tools in this slice; configured-storage human uploads are private by default and require an explicit sharing choice before becoming Bear-visible; they do not automatically inject file content into Work. Human attachment inspection now offers bounded escaped text/JSON and signature-checked sandboxed raster/PDF previews under current page/artifact access; it does not add a model capability or inject file content into a turn. Registered Cabinet-upload recovery retires expired/unretained files after a lease/grace and retries physical deletion while preserving audit records; owner-only web history/retry does not make retired bytes readable, change Job state or add a model capability. `cabinet_read` now also discovers readable current-page attachments and returns bounded UTF-8 text/JSON when given an `attachment_ref` plus optional character range. Only explicit same-Bear shared files qualify; acting for a human does not grant that human's private-file access. Page/file/source authority is rechecked after I/O and full byte integrity precedes text slicing. Storage keys/URLs and raw metadata/provenance remain hidden. The descriptor-owned tool names, `cabinet.read` class and audiences are unchanged; tool description and source-data safety guidance live in repository Markdown. Derived Cabinet recall, model-facing file uploads and binary extraction remain pending.

## Continuation, obligations, and budgets

- **Continuation is core runtime behavior.** After a blocking tool result or an approval decision, the core turn coordinator — not an edge or the model — decides when the turn may legally continue; open client obligations block continuation ([ADR-0048](docs/decisions/adr-0048-core-turn-client-obligation-coordinator.md)).
- **Non-blocking updates are not continuation gates.** Surface/control updates such as conversation title or advisory in-flight task status can be persisted and projected without forcing the model/tool/result continuation loop. If the model must observe the result before continuing, the action is not non-blocking and should be modeled as a blocking tool or obligation.
- **Approvals pause, then resume, the same turn.** A denied action returns as a result the model can react to, not a broken session.
- **Agent loop control is ledger-first.** Wall-clock, tool-class, failure, ko, checkpoint, and control-level signals govern loop health; the model receives explicit low-budget warnings and concise runtime checkpoint nudges in-band, while hidden operational outcome records preserve the truth for future replay ([ADR-0050](docs/decisions/adr-0050-agent-loop-control-adaptive-budgets-and-runtime-checkpoints.md)). When a checkpoint is pending, Den exposes a runtime-owned `checkpoint` tool and expects the model to report the checkpoint through that tool rather than embedding JSON in assistant text. Some exploration budgets such as `read`/`search` may be replenished after a successful meaningful mutative step so interactive turns can perform bounded post-edit verification, but global fuses such as wall-clock, total tool calls, repeated failure, and emergency hard-step limits remain turn-global. Context-window pressure is tracked against the fully assembled request before inference ([ADR-0047](docs/decisions/adr-0047-context-window-budget-and-token-estimation.md)); on provider overflow, Den compacts and retries once, preserving in-flight tool results and unresolved approvals.
- **Work has stopping conditions.** Docket tasks carry concrete `completion_criteria`, so the model has a defined "done" rather than an open-ended loop, and completion requires a factual `result_summary` ([ADR-0034](docs/decisions/adr-0034-jobs-and-tasks-work-management.md)). How hard the loop drives vs. yields is owned by the run's governance, referenced against those criteria ([ADR-0039](docs/decisions/adr-0039-trust-profiles-and-governance.md)).
- **Session task lists are working projections**, session/stance-local; durable cross-stance work belongs in Docket ([ADR-0045](docs/decisions/adr-0045-session-task-lists-and-docket-checkout.md)).

## Hygiene rules that protect the experience

From `AGENTS.md` (binding for contributors):

- **No control data smuggled through transcript text.** If a typed protocol field, event, or runtime tool exists, use it; sentinel strings and embedded XML/JSON blocks in prose are forbidden. Runtime checkpoints use the `checkpoint` tool as their primary structured path.
- **Parse once at boundaries**; typed concepts stay typed all the way through.
- **Model replay and user-visible history are different projections** of one canonical store. History fixes must verify both persistence and next-turn request construction.
- **Prompt prose lives in fragments or managed data, not Rust literals**, so what the model reads is versioned, compiled, and auditable.

## Canonical source index

Core runtime: [ADR-0035](docs/decisions/adr-0035-den-native-in-process-agent-runtime.md) · [ADR-0043](docs/decisions/adr-0043-acp-as-edge-adapter-protocol-agnostic-core.md) · [ADR-0048](docs/decisions/adr-0048-core-turn-client-obligation-coordinator.md)

Context assembly: [den-runtime.md](docs/architecture/den-runtime.md) · [context-compilation-scenarios.md](docs/architecture/context-compilation-scenarios.md) · [prompt-fragment-registry.md](docs/architecture/prompt-fragment-registry.md) · [ADR-0046](docs/decisions/adr-0046-file-backed-prompt-fragments-and-compiled-runtime-prompts.md) · [prompt-memory contract](docs/architecture/den-prompt-memory-block-contract.md)

Transcript: `AGENTS.md` ("Conversation History and Transcript Projection") · `services/den/AGENTS.md` ("Conversation history")

Agent loop control and budgets: [ADR-0050](docs/decisions/adr-0050-agent-loop-control-adaptive-budgets-and-runtime-checkpoints.md) · [ADR-0047](docs/decisions/adr-0047-context-window-budget-and-token-estimation.md)

Work management: [ADR-0034](docs/decisions/adr-0034-jobs-and-tasks-work-management.md) · [ADR-0045](docs/decisions/adr-0045-session-task-lists-and-docket-checkout.md) · [DOCKET_IMPLEMENTATION_PLAN.md](docs/roadmap/DOCKET_IMPLEMENTATION_PLAN.md)

Tool/action surface: `AGENTS.md` ("Tool Naming", "BearWire, ACP, and Tool Routing") · [ADR-0025](docs/decisions/adr-0025-tool-naming-and-execution-strategy.md) · [ADR-0017](docs/decisions/adr-0017-provider-safe-tool-naming.md) · [non-blocking structured updates](docs/architecture/non-blocking-structured-updates.md)

Memory visibility: `AGENTS.md` ("Memory and Reflection") · [ADR-0031](docs/decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md) · [ADR-0041](docs/decisions/adr-0041-archival-recall-and-async-curation.md)

Stances and governance: [ADR-0039](docs/decisions/adr-0039-trust-profiles-and-governance.md) · [ADR-0037](docs/decisions/adr-0037-work-sandbox-egress-gateway-and-upstream-auth.md) · [SAFETY.md](SAFETY.md)

Typed boundaries: `AGENTS.md` ("Typed Boundaries and String Hygiene")
