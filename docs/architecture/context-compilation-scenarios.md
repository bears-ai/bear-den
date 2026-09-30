# Context Compilation Scenarios

Den context compilation is the process that decides what the model sees for one turn.

This document is both:

- an implementer map for debugging and extending prompt/context assembly;
- a foundation for a future new-user explainer about how a Bear receives context without replaying everything it knows.

Related docs:

- [Den runtime](den-runtime.md#turn-context-assembly)
- [Prompt fragment registry](prompt-fragment-registry.md)
- [Bear memory guide](../guides/bear-memory.md)
- [Context compaction contract](den-context-compaction-contract.md)
- [ADR-0046 — File-backed prompt fragments and compiled runtime prompts](../decisions/adr-0046-file-backed-prompt-fragments-and-compiled-runtime-prompts.md)

## One-line mental model

Den does not hand the model “everything.” It builds a bounded **turn context** from explicit layers: a verified Bear/hat identity plus execution-mode instructions (legacy per-stance prompts for no-hat/internal roles), selected memory, retrieval, runtime state, compaction state, current transcript, and tool descriptors.

## Layer Model

| Layer | Timing | Source | Model-visible? | Notes |
|-------|--------|--------|----------------|-------|
| Bound Bear/hat identity and mode (legacy stance prompt for no-hat/internal roles) | Compile time + turn selection | `bear_compiled_configs`, canonical `bear_hats`, repo fragments | Yes | Bear-wide base and platform mode are compiled; the selected hat identity is rendered from a repo-owned fragment using the verified binding. Hat text does not grant tools. |
| Key memory projection | Turn-time | Per-Bear SQLite | Yes | Bounded proactive memory anchors; not the whole memory store. |
| Derived recall | Turn-time | Qdrant + passage metadata | Yes, when available | Rebuildable index over canonical sources. |
| Prompt-memory blocks | Turn-time selection, DB-authored content | Den Postgres | Yes | Editable standing context, scoped by Bear/stance/session/work surface. |
| Runtime fragments | Turn-time | Repo fragments + typed runtime state | Yes | Examples: Docket execution active, date/budget reminders. |
| Compaction state | Turn-time | Den Postgres compaction artifacts | Yes | Derived summary/state, not raw transcript. |
| Transcript | Turn-time | Canonical conversation messages | Yes | Projected according to model transcript rules. |
| Tool descriptors | Turn-time | Descriptor registry + client capabilities | Yes | Model-facing tool surface and schemas. |

## Source-of-Truth Rules

- Long-lived prompt prose belongs in prompt fragments or runtime-configured data, not Rust literals.
- Runtime-authored **templates** are compile-time only. Current hat identity text is a literal value rendered by a repository-owned turn fragment; Den never treats that database text as a template.
- Turn-time templating is repository-owned only.
- If Den knows a runtime condition, Den should render the applicable instruction before inference instead of asking the model to choose.
- Dynamic records may be formatted in Rust when they are primarily structured data, but standing behavioral prose should be a fragment.

Exception, current ACP adapter direct-tool descriptors:

- ACP `direct_tools` and `adapter.direct_tools` currently include short model-facing affordance hints in adapter-generated descriptor objects, for example "prefer this instead of `rg`/`grep`".
- This is a narrow exception to improve tool discoverability for armature-local tools until the context compilation system owns the canonical text for those hints.
- Treat these strings as temporary descriptor text, not a precedent for moving standing prompt prose out of compiled/runtime fragment assembly.

## Scenario Summary

| Scenario | Pre-turn compiled | Turn-time rendered | Dynamic data | Deterministic branch? | Main regression risk |
|----------|-------------------|--------------------|--------------|-----------------------|----------------------|
| Normal web chat | Bear-wide base and platform chat mode for a bound hat; legacy stance prompt without hats | Verified hat identity, optional runtime fragments | transcript, memory, recall | Usually no | hidden prompt drift or over-including memory |
| ACP/pair with tools | Bear-wide base and platform pair mode for a bound hat; legacy stance prompt without hats | Verified hat identity, tool/runtime reminders | client tools, Den tools, prompt memory | Yes, by tool policy | hiding/revealing tools by heuristics |
| Docket execution active | Bear-wide base and platform Work mode for a bound Job hat | Verified hat identity and Docket runtime fragment | canonical execution-attempt state | Yes, permission mode | asking model to decide Write vs Ask/Plan |
| Prompt memory block present | Stance prompt | prompt-memory block context | selected blocks | Selection done by Den | stale or overbroad block inclusion |
| Key memory projection | Stance prompt | projection block | SQLite latest heads | Selection done by Den | exposing raw branches or too much history |
| Derived recall | Stance prompt | recall block | vector/keyword hits | Selection done by Den | treating recall as canonical memory |
| Compaction active | Stance prompt | compaction block | compacted summary | Policy decides inclusion | flattening summary into raw transcript |
| Overflow retry | Recompiled same base | compaction recovery block | compacted transcript state | Yes, one retry | losing tool/approval continuity |
| Admin prompt edit | Recompile required | none from raw DB template | compiled output only | Compile-time validation | rendering DB templates per turn |
| Repo prompt edit | Rebuild/restart or registry reload | possible turn fragments | prompt source hash | Registry versioning | stale compiled prompt hashes |

## Scenario Walkthroughs

### Scenario 1 — Normal Web Chat Turn

What the user experiences:

> “I ask my Bear a question in the web chat.”

What Den does:

1. Loads the Bear and current stance configuration.
2. For a configured Bear, checks the conversation's canonical hat binding and composes the compiled Bear-wide base and chat mode with that hat's current identity text. A Bear with no hats retains its compiled stance prompt or legacy `bears.system_prompt`.
3. Projects a small memory slice from per-Bear SQLite.
4. Optionally adds derived recall passages.
5. Adds selected prompt-memory blocks.
6. Replays the model transcript projection from canonical conversation storage.
7. Sends tool descriptors appropriate to the current surface.

Model-visible result:

```text
system: Bear-wide base + verified hat identity + platform interaction mode
      + projected memory
      + derived recall if available
      + prompt-memory blocks
messages: projected transcript + current user message
tools: Den-hosted tools available to this surface
```

Explainer version:

> The Bear does not reread every memory or every prior chat. Den gives it a concise working packet: who it is, what it should remember now, relevant recalled passages, recent conversation, and tools it can use.

Implementation checks:

- The turn path must not parse prompt files.
- The turn path must not render runtime-authored DB templates.
- Recall output must be labeled as recall, not canonical memory.

### Scenario 2 — ACP Pair Session With Client Tools

What the user experiences:

> “I am pairing with my Bear inside an editor or armature.”

What Den does:

1. Uses the verified conversation's hat identity, compiled Bear-wide base, and platform pair-mode instructions; only no-hat Bears retain the compiled `pair` stance prompt.
2. Reads trusted human/Bear identity from the ACP token/session, not chat text.
3. Receives client capability context from the armature.
4. Builds a stable tool surface from Den-hosted tools plus client-local tools.
5. Adds runtime reminders about tool scope, memory scope, workspace roots, and approval policy.

Model-visible result:

```text
system: Bear-wide base + verified hat identity + platform pair mode
      + memory/retrieval layers
      + runtime supplement describing trusted session/tool/work-surface boundaries
messages: projected transcript
tools: Den tools + armature-local tools
```

Explainer version:

> In an editor, the Bear can see a trusted work surface. Den tells the model which tools exist and what their boundaries are, instead of letting it assume local filesystem access.

Implementation checks:

- Do not hide filesystem/git/terminal tools based on prompt heuristics.
- Route Den-hosted tools server-side and armature tools through the armature.
- Runtime reminders should be deterministic fragments or structured renderers, not scattered string literals.

### Scenario 3 — Docket Execution Active

What the user experiences:

> “The Bear is executing a Docket job or task.”

What Den does:

1. Looks up the active canonical Docket execution attempt.
2. Supplies execution state to a repository-owned runtime fragment.
3. Renders exactly one permission-mode instruction before inference.

If the current work surface is writable:

```text
Work the current Docket task and only mark it done after the work is performed or verified.
```

If the current work surface is not writable:

```text
Docket execution is active, but the current work surface must be switched to Write mode before execution can proceed.
```

Explainer version:

> The Bear is not asked to decide whether it may act. Den already knows whether the work surface is writable and gives the model only the instruction that applies.

Implementation checks:

- Do not render both Ask/Plan and Write instructions and ask the model to choose.
- Keep the Docket execution prose in `runtime` fragments.
- Keep execution state structured and typed before rendering.

### Scenario 4 — Prompt Memory Block Present

What the user experiences:

> “An operator or prior process has added standing context for this Bear/session.”

What Den does:

1. Selects prompt-memory blocks by Bear, stance/profile, session, and work surface.
2. Applies precedence and budget rules.
3. Renders included blocks as explicit prompt-memory context.

Explainer version:

> Prompt memory is editable standing context. It is different from long-term Bear memory and different from the conversation transcript.

Implementation checks:

- Do not flatten prompt memory into transcript.
- Do not include archived/superseded blocks by default.
- Emit diagnostics showing which blocks were included or omitted.

### Scenario 5 — Key Memory Projection

What the user experiences:

> “The Bear remembers relevant stable facts without being explicitly asked to search memory.”

What Den does:

1. Reads latest-head records from per-Bear SQLite.
2. Applies stance scope and work-surface gating.
3. Selects bounded anchor paths and recent highlights.
4. Appends a projected memory block.

Explainer version:

> The Bear gets a small memory briefing, not the whole memory database.

Implementation checks:

- Latest head only.
- Do not include raw unpromoted proposals.
- Do not expose another stance’s raw local branch through projection.
- Keep Docket/task state out of Bear memory projection.

### Scenario 6 — Derived Recall Present

What the user experiences:

> “The Bear brings in semantically relevant older material.”

What Den does:

1. Queries the derived recall index when configured.
2. Combines vector and keyword signals where available.
3. Appends bounded recall passages.

Explainer version:

> Recall is a search result over memory, not memory itself. If the index is rebuilt, the source of truth remains the Bear’s canonical memory and other canonical records.

Implementation checks:

- Recall must be rebuildable.
- Do not treat Qdrant vectors as canonical memory.
- Deduplicate passages already present in anchors where possible.

### Scenario 7 — Compaction Active

What the user experiences:

> “A long-running conversation still continues coherently without replaying everything.”

What Den does:

1. Groups transcript into semantic segments.
2. Produces or reads compacted derived state.
3. Cuts transcript replay according to compaction policy.
4. Injects compacted state as explicit context.

Explainer version:

> Den summarizes older working state but keeps that summary separate from transcript and memory. The model can continue without being handed every old message.

Implementation checks:

- Compacted state is derived context, not raw transcript.
- Preserve active goals, constraints, tool/approval state, and workflow continuity.
- Do not hide unresolved approvals or active tool obligations.

### Scenario 8 — Overflow Retry After Compaction

What the user experiences:

> “The model rejects a too-large prompt, but Den recovers by shrinking context and retrying once.”

What Den does:

1. Detects provider context overflow.
2. Runs emergency compaction if enabled.
3. Reassembles the prompt from persisted compiled prompt + compacted state + preserved active spans.
4. Retries once.

Explainer version:

> If the prompt becomes too large, Den can compress older context and try again, while preserving the parts needed to continue safely.

Implementation checks:

- Only one overflow retry per step.
- Preserve in-session tool results appended during the current step.
- Do not use compaction to resolve unresolved approvals.

### Scenario 9 — Admin Prompt Edit

What the user experiences:

> “An admin edits Bear instructions.”

What Den does:

1. Stores runtime-authored prompt content in Den data stores.
2. Allows only compile-time variables in that content.
3. Validates MiniJinja at save/reconcile time.
4. Recompiles and stores per-stance output in `bear_compiled_configs`.

Explainer version:

> Admin-edited instructions are compiled before use. They are not interpreted as live code every time the Bear answers.

Implementation checks:

- Unknown compile-time variables should fail clearly.
- Turn-time variables such as current date or active budget are not allowed in admin-authored templates.
- Runtime should read compiled output only.

### Scenario 10 — Repository Prompt Fragment Change

What the developer experiences:

> “A prompt fragment changes in Git.”

What Den does:

1. Loads repository fragments into the prompt registry at startup or rebuild time.
2. Computes a prompt source version hash.
3. Uses that hash in compiled config invalidation.
4. Recompiles affected Bear prompt output when needed.

Explainer version:

> Product-authored default instructions live in versioned files. A change to those files creates a new prompt source version that can trigger recompilation.

Implementation checks:

- Do not read prompt files during turns.
- Keep bundle/fragment validation strict.
- Prompt source hash should change when relevant fragment or bundle content changes.

## Scenario Design Checklist

When adding a new context scenario, answer these questions:

1. What human-facing situation does this scenario represent?
2. What canonical state decides the scenario?
3. Is the text pre-turn compiled, turn-time rendered, or structural data formatting?
4. If there is a branch, can Den choose before inference?
5. Which source owns the prose: repo fragment, DB content, or Rust structural formatter?
6. What should the model see?
7. What must the model not see?
8. What regression test proves the scenario still compiles correctly?

## Candidate User-Explainer Illustrations

Good candidates for a user-facing explanation:

- Normal web chat: “a concise working packet, not everything the Bear knows.”
- Key memory projection: “stable memory anchors, not the whole database.”
- Docket execution: “Den knows whether execution is allowed before the model acts.”
- Compaction: “older context is summarized but kept separate from transcript and memory.”
- Admin prompt edit: “your Bear’s standing instructions are compiled before use.”

Avoid exposing implementation details such as table names or exact fragment ids in the user explainer unless the audience is technical.
