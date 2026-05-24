# Planning-Tool and Memory-Orientation Recommendations

## Objective
Recommend a lightweight, implementation-oriented set of changes that improves two related environment behaviors without making prompt construction overly heavy:

1. when the user asks to make or update a plan, agents should prefer planning-state tools over informal prose or durable memory writes when the environment supports those tools;
2. agents should be able to discover the memory/schema structure for work surfaces and other important role-local material without relying on lucky subtree exploration.

This document synthesizes:
- the current role-environment prompt-construction direction in this repository;
- the existing BEARS ontology separating workplan, activity, memory, and execution;
- practical Letta feedback about pinned versus progressive/on-demand memory.

---

## Summary of recommendations

Implement two small but high-leverage improvements:

### A. Prompt/compiler improvements
Add a more concrete, capability-aware planning trigger to the structured workflow-policy layer so direct user requests such as "make a plan" reliably map to planning-state handling when planning tools are available.

### B. Memory/orientation improvements
Add a compact pinned orientation file that explains the memory/work-surface schema and links explicitly to anchor files such as work-surface `overview.md` documents.

These should be treated as complementary:
- prompt/compiler changes improve behavioral selection;
- memory/orientation changes improve retrieval and schema discoverability.

---

## Problem statement

Two related issues are currently easy for agents to fall into:

1. **Plan requests are under-triggered.**
   When a user says "make a plan" or similar, the agent may:
   - respond with conversational bullet points instead of using planning-state tools; or
   - write a requested plan to durable memory instead of representing it as active planning state.

2. **Memory structure is discoverable only on demand.**
   Even when the memory tree is well-organized, important role-local structures such as work surfaces may be easy to ignore if they exist only in progressive/on-demand memory and are not explained by a pinned orientation artifact.

The existing architecture already contains the right ontology and most of the right concepts. The main gap is that operational triggers and discovery affordances are still too implicit.

---

## Relevant repository context

The current prompt-construction direction already provides the right structural layers.

From `ROLE_ENVIRONMENT_PROMPT_CONSTRUCTION_SPEC.md` and `ROLE_ENVIRONMENT_PROMPT_COMPILER_SCHEMA.md`:

- role environments are moving from prose-first concatenation to schema-first assembly;
- `WorkflowPolicy` explicitly carries:
  - `memory_vs_plan_rules`
  - `plan_representation_rules`
  - `stop_conditions`
- the current ontology already distinguishes:
  - `workplan`
  - `activity`
  - `memory`
  - `execution`
- existing rules already include:
  - `DoNotUseDurableMemoryForActivePlansOrEphemeralProgress`
  - `UseUpdatePlanForLiveActivityProgressTracking`
  - `FormalWorkplanArtifactsRequireExplicitNeed`

This means the core conceptual model does **not** need a rewrite. The primary opportunity is to sharpen how that model is rendered and surfaced.

---

## Design goals

The recommended changes should:

1. improve reliability for direct plan requests such as "make a plan";
2. remain capability-aware rather than naming unavailable tools as if they are always present;
3. reinforce the existing ontology instead of inventing new categories;
4. keep the prompt architecture lightweight and role-general;
5. improve discoverability of memory/work-surface structure without pinning large amounts of low-value material;
6. preserve the distinction between pinned orientation and progressive/on-demand local memory.

---

## Workstream A: Prompt/compiler recommendations

### A1. Add a direct-request planning trigger to `WorkflowPolicy`

#### Recommendation
Extend the structured workflow-policy layer so that direct user requests to make/create/draft/update/track a plan or task list are treated as requests for planning-state handling when planning-state tools are available.

#### Why
Current policy correctly distinguishes active plans from durable memory and formal plan artifacts, but it remains somewhat abstract at the precise trigger point where a model must interpret requests like:
- "make a plan"
- "create a plan"
- "draft a plan"
- "update the plan"
- "track this as a plan"

A compact trigger rule would reduce the common failure mode where the agent satisfies the request with only conversational bullets.

#### Implementation guidance
Do **not** solve this by adding more freeform reminder prose. Prefer a small extension to the structured workflow-policy model and its standardized render section.

Possible implementation paths:
- add a new normalized `PlanRepresentationRule` enum value capturing direct user plan-request handling; or
- keep the enum set unchanged and derive a more explicit rendered rule from existing workflow policy plus capability state.

If a new enum is added, keep it compact and role-general. For example, conceptually:
- `DirectPlanRequestsPreferPlanningState`

Avoid naming specific tools in the enum itself unless the ontology truly requires it.

---

### A2. Keep plan behavior capability-aware

#### Recommendation
Render plan-handling guidance in a way that depends on actual current-turn capability state rather than assuming planning tools are always available.

#### Why
The same prompt-construction architecture is meant to support multiple roles and environments. Hardcoding tool-specific directives into static prompt text risks instructing the agent to use tools that are unavailable in the current runtime.

#### Implementation guidance
The compiler should derive a capability-aware instruction at render time.

Conceptual behavior:
- if planning-state tools are available, direct plan requests should prefer planning-state handling over prose-only responses;
- if planning-state tools are unavailable, the agent should explain the limitation and provide a provisional conversational plan if still helpful.

This can be expressed by combining:
- `CapabilityPolicy`
- `OperationalState`
- `WorkflowPolicy`

Rather than by placing unconditional tool references in stable prompt prose.

---

### A3. Clarify the boundary among live planning state, formal workplans, and durable memory

#### Recommendation
Preserve and sharpen the existing distinctions already present in the ontology.

#### Why
The current schema already points in the right direction, but a sharper interpretation will reduce common confusion:
- a user request to "make a plan" should usually create or update active planning state;
- a formal implementation workplan artifact should require explicit need or explicit planning-mode intent;
- durable memory should not be the default destination for active plans.

#### Implementation guidance
Retain and emphasize existing policy such as:
- `UseUpdatePlanForLiveActivityProgressTracking`
- `FormalWorkplanArtifactsRequireExplicitNeed`
- `DoNotUseDurableMemoryForActivePlansOrEphemeralProgress`

If examples are rendered or documented for operators, keep them concise:
- "make a plan" → active planning state
- "enter planning mode" / "draft an implementation plan for approval" → formal workplan path
- "save this as memory" → durable memory only when explicitly asked and appropriate

The prompt need not include all examples every turn; they may instead belong in tests, docs, or debug views.

---

### A4. Keep the change in `[WORKFLOW RULES]`, not scattered across multiple sections

#### Recommendation
Implement the new behavior primarily through `WorkflowPolicy` and the standardized `[WORKFLOW RULES]` render section.

#### Why
The repository direction already favors schema-first rendering and reduced duplication. Spreading plan-request handling across memory reminders, style guidance, and general prose would make the system heavier and less legible.

#### Implementation guidance
Prefer a single authoritative source of truth for:
- when active planning state should be used;
- when durable memory should not be used;
- when formal plan artifacts are appropriate.

The prompt/compiler should avoid multiple overlapping prose reminders once the structured workflow section exists.

---

### A5. Add test coverage for the direct-request planning trigger

#### Recommendation
Add tests that specifically cover common plan-request phrasings and validate capability-aware rendering/behavior.

#### Why
This issue is easy to regress if only abstract ontology tests exist.

#### Suggested test cases
At minimum, test behavior or rendered prompt expectations for inputs conceptually equivalent to:
- "make a plan"
- "create a plan"
- "update the plan"
- "track this as a plan"

Across at least two runtime states:
- planning-state tools available;
- planning-state tools unavailable.

#### Acceptance criteria
The rendered environment or downstream behavior should make it difficult for the agent to:
- satisfy a direct plan request with only prose when planning-state tools are available;
- write active requested plans to durable memory by default.

---

## Workstream B: Memory/orientation recommendations

### B1. Add a compact pinned orientation file

#### Recommendation
Create a small pinned orientation file such as:
- `system/memory-map.md`, or
- `system/work-surfaces.md`

#### Why
Letta feedback indicates that progressive/on-demand memory under paths such as `pair/...` is visible in the tree but not automatically semantically legible. Agents may ignore well-structured subtrees if no pinned artifact explains what they are and where to start.

#### Implementation guidance
The pinned file should be:
- compact;
- link-heavy;
- explanatory rather than exhaustive;
- focused on helping an agent decide where to read next.

It should explain:
- major namespaces or memory regions;
- which areas are pinned orientation versus progressive/on-demand;
- where work-surface anchors live;
- which files are preferred starting points.

Avoid turning it into a giant manual.

---

### B2. Use `overview.md` as the work-surface anchor

#### Recommendation
Keep one compact `overview.md` per work surface and treat it as the canonical reading anchor.

#### Why
This pattern matches the Letta guidance and gives both humans and agents a predictable starting point for a surface.

#### Implementation guidance
Each `overview.md` should:
- stay short;
- identify what the work surface is;
- explain the most important nearby files;
- link outward to deeper supporting material.

The overview should serve as a router, not a dump of all details.

---

### B3. Prefer explicit `[[links]]` between orientation and anchor files

#### Recommendation
Use explicit links liberally between the pinned orientation file and work-surface anchor files.

#### Why
Explicit links improve both graph navigation and agent navigation. They reduce the amount of inference required to discover relevant context.

#### Implementation guidance
The pinned orientation file should link directly to:
- work-surface `overview.md` files;
- any other especially important schema or convention files.

Where the memory system supports wiki-style links, prefer them for stable navigation paths.

---

### B4. Use stable, human-meaningful slugs in primary retrieval paths

#### Recommendation
Prefer readable, stable slugs over conversation IDs in primary retrieval paths.

#### Why
Conversation/session IDs are weak human and agent anchors. They are poor retrieval handles unless the content is intentionally thread-local and ephemeral.

#### Implementation guidance
Use conversation IDs in:
- provenance metadata;
- logs;
- clearly thread-local artifacts.

Do not make them the main retrieval identity for durable work-surface or schema material unless that material is intentionally throwaway.

---

### B5. Keep noisy operational areas out of pinned orientation

#### Recommendation
Do not pin `scratch/`, `logs/`, or similar low-signal operational areas in orientation context.

#### Why
Pinned context should help the agent orient quickly. Operational byproducts tend to add noise without improving schema understanding.

#### Implementation guidance
Pinned orientation should prefer:
- schema explanations;
- stable conventions;
- anchor summaries;
- work-surface entry points.

Operational areas can remain progressive/on-demand and may need pruning or TTL rules if they accumulate aggressively.

---

### B6. Define multi-writer conflict/ownership rules early if memory is git-backed and shared

#### Recommendation
If multiple harnesses or roles may write to the same git-backed memory repository, define ownership/conflict policy early.

#### Why
A legible schema becomes much less useful if write ownership is ambiguous or if competing agents overwrite each other.

#### Implementation guidance
Choose and document one of the following patterns, or an equivalent:
- strict namespace ownership;
- single-writer regions;
- branch/PR flow for shared areas.

This recommendation is adjacent to the immediate planning issue, but it materially affects the long-term usefulness of the orientation layer.

---

## Suggested implementation sequence

### Phase 1: smallest high-value prompt/compiler change
- add or derive a direct-plan-request rule in `WorkflowPolicy`;
- render it in `[WORKFLOW RULES]` in a capability-aware way;
- add tests for direct plan-request phrases.

### Phase 2: smallest high-value memory/orientation change
- add a pinned orientation file;
- link it explicitly to work-surface `overview.md` anchors;
- ensure the orientation file stays compact and stable.

### Phase 3: cleanup and hardening
- review whether any current prompt prose duplicates the new workflow-policy render;
- reduce duplication where the structured rule now suffices;
- review memory path naming and conversation-ID usage;
- document write-ownership policy if multiple writers are expected.

---

## Acceptance criteria

The recommendations in this document should be considered successfully implemented when:

### Prompt/compiler side
- direct requests such as "make a plan" reliably map to planning-state handling when planning tools are available;
- the environment does not instruct the agent to use unavailable tools;
- active requested plans are not written to durable memory by default;
- the change is represented in structured workflow policy rather than scattered prose reminders.

### Memory/orientation side
- there is a pinned orientation file that explains the schema compactly;
- important work surfaces have compact `overview.md` anchors;
- orientation files use explicit links to those anchors;
- primary retrieval paths favor stable slugs over opaque conversation IDs.

---

## Non-goals

These recommendations do **not** require:
- a broad rewrite of role-environment prompt construction;
- pinning large amounts of role-local memory into the main prompt;
- collapsing role-local and shared memory into a single namespace immediately;
- removing progressive/on-demand memory access patterns.

The goal is a lightweight increase in clarity and discoverability, not a full redesign.

---

## Bottom line

The repository already contains the right ontology and most of the right architecture. The practical improvements now needed are:

1. make direct plan requests map more explicitly to planning-state behavior in the structured workflow-policy layer, while remaining capability-aware; and
2. add a compact pinned orientation file that teaches agents where the meaningful memory/work-surface anchors live.

Together, these changes should improve both runtime plan handling and schema discoverability without making the environment substantially heavier.
