# Pair Role: Collaborative Coding Agent

The `pair` role is the Bear's collaborative coding and client-tool role. It is the agent a user works with inside ACP-speaking tools such as IDEs, design tools, and future productivity clients.

This note preserves product and implementation thinking for the `pair` role: what it should do inline, when it should delegate to other roles, what tools it should have, and how it should treat memory.

## Job description

`pair` works side-by-side with the user in an active workspace.

It should feel like a capable collaborator who can:

- reason about the user's code and working context,
- use client-mediated tools with user approval,
- look up narrow external documentation when needed,
- write role-local notes to help future pair sessions,
- propose durable skills or conventions,
- and create reviewed work requests when the user asks for broader autonomous research or external action.

It should not feel like an autonomous background worker. It is present with the user, in the user's tool, helping the current work move forward.

## Runtime and trust boundary

`pair` is API-direct, not Letta Code-backed.

Reasons:

- ACP clients can have multiple sessions per connection; Letta Code harness state is too single-session-oriented for this role.
- Client tools are mediated through ACP and should remain user-gated.
- The `pair` tool surface should be narrow and exact, not inherited from a full harness.

Trust posture:

| Capability | Pair posture |
|------------|--------------|
| Private/raw context | Sees user workspace/session context and client-approved tool results. |
| External communication | Client-mediated tools only, plus Den-mediated read-only retrieval tools. |
| Durable state | Writes only its own `pair/` memory branch or Den-mediated structured records. |
| Shared memory | Cannot write `core/` directly. Curate promotes durable shared knowledge. |
| Autonomous work | Cannot execute directly. Creates task intents for review. |

## Common use cases

### Coding help

Examples:

- "Explain this function."
- "Refactor this file."
- "Why is this test failing?"
- "Use the IDE's read file tool to inspect the module."

Expected behavior:

- Use ACP client tools where appropriate.
- Ask for permission through ACP for client-side operations.
- Keep changes and analysis scoped to the active user session.

### Inline docs lookup

Examples:

- "Look up the Axum 0.8 docs for extractors."
- "Check the SQLx offline-mode documentation."
- "Fetch this API reference and use it to fix the code."

Expected behavior:

```text
user asks for narrow docs lookup
  -> pair calls Den-mediated search/fetch tool
  -> Den returns bounded, quoted, cached snippets with source metadata
  -> pair uses snippets in the current response
```

This should be immediate and inline. The result is context for the current coding turn, not automatically shared Bear memory.

### Durable local learning

Examples:

- "Remember that this repo uses Axum 0.8."
- "Note that migrations go through sqlx and must not edit old files."
- "For this codebase, prefer these naming conventions."

Expected behavior:

```text
pair writes a note under pair/
  -> curate later reviews
  -> if broadly useful, curate promotes distilled knowledge to core/
```

Pair can write role-local notes, but cannot unilaterally write `core/`.

### Durable skill learning

Examples:

- "Learn this checklist for reviewing API routes."
- "Remember this debugging procedure as a reusable skill."

Expected behavior:

```text
pair proposes skill
  -> bear_skill_proposals
  -> curate reviews
  -> Den updates skill manifest if approved
  -> affected roles are re-provisioned/reconciled
```

Pair should not install skills directly.

### Broad research / opinion formation

Examples:

- "Research industry approaches to multi-agent memory isolation and recommend one."
- "Survey how frameworks handle agent tool permissions."
- "Compare five options and prepare a report."

Expected behavior:

```text
pair writes a task intent
  -> curate reviews and approves/rejects
  -> Den dispatches approved work to work
  -> work writes a result/report
  -> curate promotes summary/report to core/results or core knowledge
  -> pair can surface it to the user
```

This is not inline docs lookup. It is background work requiring synthesis, auditability, scoping, and possibly rate limiting.

## Inline lookup vs delegated work

Use inline `pair` retrieval when:

- the user needs a specific fact or docs page now,
- scope is narrow,
- result can fit in a few snippets,
- it supports the current coding turn,
- it is read-only and low risk,
- the user is actively waiting.

Delegate to `work` when:

- the user asks for research, comparison, survey, or report,
- the problem needs multi-source synthesis,
- it may take minutes,
- it requires repeated external calls,
- the result should be auditable,
- the output may become durable Bear knowledge,
- it is not necessary for the immediate edit loop.

When ambiguous, pair should ask:

> I can do a quick docs lookup now, or create a background research task for a deeper report. Which do you want?

## Tool profile

The `pair` role should have a deliberately narrow tool profile.

### Client-mediated ACP tools

These come from the user's ACP client and are governed by the client's permission model.

Examples:

- read file,
- inspect workspace,
- proposed edits,
- run client-supported operations if approved.

Den should preserve ACP-native approval semantics rather than pretending these are server-side tools.

### Den-mediated retrieval tools

These support inline docs lookup.

Candidate tools:

- `web_search`
- `web_fetch`
- `docs_search`
- `docs_fetch`

Current implementation:

- `den.web.fetch` is implemented with SSRF guards, timeouts, redirect limits, and bounded content extraction.
- `den.web.search` supports Brave Search when configured:

```bash
DEN_SEARCH_PROVIDER=brave
BRAVE_SEARCH_API_KEY=...
DEN_SEARCH_MAX_RESULTS=5
```

If these variables are not set, `den.web.search` returns a clear configuration error and `pair` should ask the user for a direct URL or explain that search is unavailable.

Policy expectations:

- read-only,
- bounded result count,
- bounded content size,
- domain allow/deny controls,
- source/citation metadata,
- cacheable,
- result text framed as untrusted external content,
- no cookies or user secrets,
- clear diagnostics for blocked domains or fetch failures.

### Role-local memory tools

Current tool:

- `memory_write_entry`

`memory_write_entry` is intentionally role-aware. Den resolves the caller's Bear role and writes to that role's allowed memory location. It supports semantic kinds such as `note`, `log`, `decision`, `reflection`, `scratch`, and `summary`.

For `pair`, it writes only under pair-local paths, for example:

```text
pair/notes/<entry-id>.md
pair/logs/<entry-id>.md
pair/decisions/<entry-id>.md
```

It must not write `core/` directly, Cabinet, tasks, observations, or run results.

### Memory review tools

Future tool:

- `memory_request_review`

`memory_request_review` is the producer-side Reflection tool for asking `curate` to review pair-local memory. It creates a memory proposal row for the `memory_curate` lane; it does not write `core/`, Cabinet, skills, tasks, observations, or run results.

Pair should use this when local memory may matter beyond future pair sessions. The request can include a `suggested_action`, such as `summarize_into_core`, `promote_to_core`, `cabinet_update`, `skill_review`, `retain_role_local`, `delete_after_review`, `human_review`, or `unspecified`. `curate` decides the final outcome.

Pair should not request review for every local note. Most tactical notes can remain pair-local forever.

### Structured delegation tools

Future tool once Docket exists:

- `write_task_intent`

Pair uses this for external-effect requests and broad research tasks.

Until Docket exists, pair should explain that background task creation is not yet available or should use a temporary Den-managed intent implementation if we choose to bridge it.

### Skill proposal tools

Skill learning belongs to Reflection's adaptation lane. Pair should not directly install durable skills.

When pair discovers a reusable procedure, repeated failure mode, or user-requested behavior change, it should first write appropriate pair-local memory and then use `memory_request_review` with `suggested_action: skill_review` once that tool exists. Future dedicated skill tools may be added under Den's skill namespace, but they should remain proposal-and-review based.

## `memory_write_entry` naming

A shared tool name like `memory_write_entry` is a better fit than `write_note` because role-local memory includes more than notes.

Benefits:

- The agent-facing action is explicit: write a typed memory entry.
- The policy remains centralized in Den.
- Different roles can use the same affordance without gaining the same filesystem authority.
- UI and logs can show a common action shape with role-specific outcomes and `kind` metadata.

Required behavior:

```text
memory_write_entry(context, args)
  -> Den authenticates caller context
  -> Den resolves Bear + role
  -> Den validates kind/title/body/refs/lifecycle/provenance
  -> Den selects allowed destination for that role and kind
  -> Den writes only to that role's memory area
  -> Den records audit metadata
```

For example:

| Role | `memory_write_entry` destinations |
|------|---------------------------------------|
| `talk` | `talk/notes/`, `talk/logs/`, `talk/decisions/`, ... |
| `pair` | `pair/notes/`, `pair/logs/`, `pair/decisions/`, ... |
| `curate` | `curate/notes/`, `curate/reflections/`, `curate/decisions/`, ... |
| `work` | task/run-bound `work/logs/`, `work/decisions/`, `work/summaries/` |
| `watch` | `watch/logs/` and summaries; observations should use observation tooling |

The first implemented slice is `pair` only.

## Memory edge: explicit content vs durable memory

Pair should distinguish between:

1. **temporary context** — web/docs search snippets used in the current turn;
2. **role-local memory** — notes under `pair/` useful for future pair sessions;
3. **shared Bear memory** — curated content promoted to `core/`;
4. **reports/results** — explicit outputs from `work`, usually under `core/results/` after curate review;
5. **skills** — reviewed durable procedures in the Bear skill manifest.

Pair should not treat every docs lookup as memory. It should only write notes when the information is likely useful beyond the current turn.

Pair's memory decision ladder:

1. **Use only in the current turn** when the information is temporary or already available from source files/docs.
2. **Write pair-local memory** when the information is durable for future pair sessions.
3. **Request Reflection review** when the information may matter across roles, belongs in `core/`, suggests a Cabinet update, or indicates a reusable skill/procedure.
4. **Never directly mutate shared memory or behavior** from pair.

## Recommended first implementation order

1. Implement Den-mediated `web_search` / `web_fetch` or docs-oriented equivalents for pair.
2. Implement role-aware `memory_write_entry` for pair-local memory.
3. Expose web, situation, and memory tools to the pair prompt/tool profile.
4. Add diagnostics and tests showing pair can write to `pair/notes/` but cannot write `core/`.
5. Implement `memory_request_review` so pair can request Reflection curation without writing shared memory.
6. Implement Docket-backed `write_task_intent` later for broad research delegation.

## Good pair behavior examples

### Narrow docs lookup

User:

> Look up the official docs for this function.

Pair:

> I found the relevant docs. The important constraint is ... [citation]. I'll use that in the change below.

No durable memory is written unless the user asks or the fact is clearly project-stable.

### Durable project note

User:

> Remember that this repo uses SQLx offline mode and we must run prepare after query changes.

Pair:

> Noted for future pair sessions. I wrote this to pair-local memory. If you want, I can also request Reflection review so `curate` can decide whether it belongs in shared Bear memory.

### Broad research

User:

> Research approaches to policy engines for task approval and recommend one.

Pair:

> This is broader than a quick docs lookup. I should create a background research request for the work role once Docket task intents are available. For now, I can either do a quick inline scan or draft the task intent for review.
