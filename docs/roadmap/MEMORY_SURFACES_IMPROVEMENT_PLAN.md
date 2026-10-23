# Memory surfaces improvement plan

**Status:** Proposed  
**Purpose:** Make Bear memory surfaces boring, inspectable, scoped, source-linked, and easy to correct or forget.  
**Hub:** [PLAN.md](PLAN.md)  
**Related:** [BEAR_MEMORY_REMAINING_WORK_PLAN.md](BEAR_MEMORY_REMAINING_WORK_PLAN.md), [MEMORY_TOOLS_IMPLEMENTATION_PLAN.md](MEMORY_TOOLS_IMPLEMENTATION_PLAN.md), [DEN_CONTEXT_COMPACTION_IMPLEMENTATION_PLAN.md](DEN_CONTEXT_COMPACTION_IMPLEMENTATION_PLAN.md), [CONTEXT_WINDOW_BUDGET_IMPLEMENTATION_PLAN.md](CONTEXT_WINDOW_BUDGET_IMPLEMENTATION_PLAN.md), [PERSONALIZATION_PLAN.md](PERSONALIZATION_PLAN.md)

---

## Goal

Improve the model- and user-facing memory environment so agents can accurately answer:

1. What short-term conversation/context is present?
2. What persistent memory exists?
3. Why was this memory projected or recalled?
4. Where did a remembered fact come from?
5. How can a user correct, forget, rescope, or approve it?

This plan is about **surfaces and controls** around existing memory/recall systems. It should reuse the canonical SQLite memory store, derived recall, existing task/Docket state, and context-budget work where possible.

## Non-goals

- Do not make memory feel magical or human-like.
- Do not silently retain personal or secret-like facts.
- Do not create a second memory store for UI convenience.
- Do not mix task/workplan state into semantic memory.
- Do not add a new dependency where existing Den diagnostics, SQLite metadata, or Docket state can cover the need.

## Implementation constraints

- Prefer surfacing fields and state that already exist before adding schema.
- Keep task state in Docket/task-list surfaces; only promote project facts to semantic memory through explicit review.
- Treat `session_info` as the low-friction diagnostic entry point, but move anything large or expensive to a dedicated diagnostic tool.
- Keep user-facing answers short by default; detailed provenance and context attribution should be available on request.
- Make every destructive or externally visible memory operation explicit and reversible where practical.

---

## Workstreams

### 1. Context-budget status

Expose enough context-window information for agents and users to reason about context loss, compaction, and tool-schema overhead.

Deliverables:

- Current model context window and estimated request size.
- Remaining/reserved output budget.
- Component attribution: system/developer prompt, projected memory, recalled memory, transcript, tool schemas, tool messages, request overhead.
- Compaction status, source range, and summary pointers when available.
- A concise `session_info`/diagnostic view suitable for chat answers.

Acceptance checks:

- The agent can answer "how much context do you have?" without claiming the value is unavailable when the runtime knows it.
- The answer distinguishes context window from persistent memory.
- A developer can see which component is consuming the most context.

### 2. Distinct memory/context layers

Make layers visible and labeled instead of relying on prompt prose.

Deliverables:

- A grouped context/memory diagnostic view for:
  - current transcript;
  - projected memory;
  - turn-start recalled memory;
  - durable memory records;
  - task/Docket/workplan state;
  - tool descriptions and runtime instructions.
- Layer metadata: scope, lifetime, mutability, authority, and source.
- Model-facing labels that prevent projected memory from being treated as fresh user instruction.

Acceptance checks:

- The agent can explain whether a fact came from the current conversation, projected memory, durable memory, or task state.
- A user can inspect whether a fact is durable, transient, or task-local.

### 3. Provenance for memory facts

Every active remembered fact should be auditable.

Deliverables:

- Source identifiers: conversation/session/run/task where applicable.
- Timestamp and actor/asserter.
- Assertion mode: user-stated, assistant-inferred, system-generated, imported, or curated.
- Confidence and sensitivity classification.
- Provenance display in memory search/read/admin/member surfaces.

Acceptance checks:

- Given a remembered preference, the system can show where it came from.
- Inferred facts are visibly distinct from direct user statements.
- Sensitive provenance does not leak across scopes.

### 4. Candidate-memory confirmation flow

Make durable memory opt-in and lightweight.

Deliverables:

- Candidate-memory proposals for durable preferences, project facts, and recurring working agreements.
- Scope choices: session-only, work-surface/project, Bear-local profile, or curated/core review.
- A concise confirmation UI/prompt.
- Secret-like content refusal or quarantine path.

Acceptance checks:

- Personal or long-lived facts are not silently saved.
- The user can approve, reject, or rescope a memory candidate in one turn.
- Rejected candidates do not remain active guidance.

### 5. Forgetting and correction

Support reliable user correction without guessing or destructive surprises.

**2026-10-23 planning clarification:** broader forgetting, aging and automatic expiry are being scoped in the [draft forgetting plan](BEAR_MEMORY_FORGETTING_PLAN.md). Explicit owner-requested personal-body/derived-copy erasure remains in the [active user-associated memory plan](USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#approved-v1-policy-amendment-2026-10-23); neither this older workstream nor the new skeleton authorizes retention durations or automatic deletion.

Deliverables:

- User intents: "forget that", "update that", "only remember this for this project".
- Search/selection flow for ambiguous matches.
- Supersession and deletion paths using existing lifecycle fields where possible.
- Audit behavior that preserves required operational traces without resurfacing forgotten content as active memory.

Acceptance checks:

- A corrected preference stops influencing future turns.
- Ambiguous deletion asks before changing memory.
- Superseded entries are marked inactive/stale and excluded from active projection/recall.

### 6. Expiry and lifecycle policy

Let temporary memory decay while durable user-approved preferences persist.

Deliverables:

- Consistent lifecycle use: session, short, durable, archive.
- Consistent status use: active, stale, superseded, archived.
- Review prompts for stale project/task context.
- Default expiry for transient goals, one-off experiments, and snapshots.

Acceptance checks:

- Temporary task context does not become durable personal memory.
- Stale project facts can be identified and retired.
- Durable approved preferences remain active unless corrected.

### 7. Relevance-ranked projection

Reduce prompt noise from irrelevant projected memory.

Deliverables:

- Projection ranking by current request relevance, scope, recency, confidence, and lifecycle status.
- Deprioritization of stale session summaries unless directly relevant.
- Projection diagnostics explaining why each block was selected.
- **Persist per-turn projection identities.** The per-turn `ContextBudgetReport`
  (ADR-0047, landed) is persisted to `conversations.latest_context_budget_json`
  and gives the management UI's Context page per-source *sizes* — but not
  *identities*: which memory records were actually projected exists only in
  in-memory assembly diagnostics and is discarded after the turn. Persist the
  projected memory ids (and omission reasons) alongside the budget report so
  the Context page's "last assembled turn" view can link each projected memory
  to its record, closing the loop from prompt back to provenance.

Acceptance checks:

- Irrelevant old summaries are not projected by default.
- The agent can explain why a memory appeared in context.
- Relevant work-surface memory beats generic historical notes.
- From the management UI, a user can see which memory records were in the
  last turn's context and open each one.

### 8. Tool-description compression

Spend fewer context tokens on verbose tool schemas while keeping exact validation available.

Deliverables:

- Compact tool summaries by default.
- Lazy expansion of full schemas only when needed by the runtime/provider path.
- Grouping by purpose and risk.
- Context-budget attribution for tool descriptions.

Acceptance checks:

- Common tool availability is understandable without large schema blocks.
- Exact schema details remain available before execution.
- Context-budget diagnostics show the tool-schema footprint.

### 9. Memory conflict detection

Flag contradictions instead of silently choosing one memory over another.

Deliverables:

- Detection when current conversation conflicts with active durable memory.
- User-facing resolution choices: update, supersede, keep both with scopes, or ignore.
- Conflict-resolution records tied to provenance.
- Projection/recall filters that avoid applying superseded guidance.

Acceptance checks:

- If memory says "prefers X" and the user now says "prefers Y", the agent asks before updating durable memory.
- Resolved conflicts mark old entries superseded or narrowed in scope.
- Superseded guidance stops appearing as active instruction.

### 10. Task memory vs personal memory separation

Keep "what we are doing" separate from "what the user prefers" and "who the user is".

Deliverables:

- Clear storage/projection rules for task state, project facts, user preferences, and person/identity facts.
- Docket/task-list state surfaced through task tools, not semantic-memory paths.
- Promotion/review path for project facts that become durable memory.
- UI labels that distinguish task/workplan artifacts from semantic memory.

Acceptance checks:

- Active plans are managed as task state, not personal memory.
- User preferences are not mixed with project implementation details.
- Shared/core updates require review instead of unilateral promotion.

---

## First implementation slice

Start with the smallest useful observability pass before changing memory behavior:

1. Audit existing runtime data for context budget, projected memory, recalled memory, task-list state, and memory-record metadata.
2. Extend `session_info` or the nearest existing diagnostic path to expose:
   - context-budget status when known;
   - active memory/context layers;
   - projected-memory counts and source labels;
   - whether compaction ran and which source range it summarized.
3. Shape the model-facing output around the [Bear memory model-experience guide](../guides/bear-memory.md#model-experience): each field should identify the layer it describes, use explicit `unknown`/`unavailable` values when data is missing, and point at the next tool/surface the model can inspect.
4. Add one small runnable check around the diagnostic serialization so missing/unknown fields degrade to explicit `unknown` values instead of disappearing.
5. Update this plan with the exact follow-up workstream owners once the audit identifies which fields are already available.

This slice should not add new memory-retention behavior. It only makes the current environment easier to inspect and debug.

---

## Suggested implementation order

1. Context-budget status.
2. Distinct memory/context layer diagnostic.
3. Provenance fields surfaced in search/read/UI.
4. Lifecycle and expiry consistency.
5. Forget/correct flow.
6. Candidate-memory confirmation flow.
7. Task-vs-personal separation rules.
8. Relevance-ranked projection.
9. Conflict detection.
10. Tool-description compression.

This order favors observability first, then safety controls, then quality-of-life and context-efficiency improvements.

## Open questions

- Which provenance fields already exist on `memory_records`, proposals, recall passages, and transcript artifacts?
- Should user-visible deletion physically remove content, mark records inactive, or do both depending on sensitivity?
- Which scope choices should appear in chat UI versus admin/operator UI?
- Can tool-description compression be implemented at the runtime/provider layer without changing model-facing tool validation?
- How much of this belongs in `session_info` versus a dedicated context/memory diagnostic tool?
