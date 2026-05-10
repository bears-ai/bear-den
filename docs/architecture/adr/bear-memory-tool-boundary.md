# Bear Memory Tool Boundary — Architecture Decision Record

## Status: Accepted

## Date: 2026-04-29

---

## Context

BEARS uses **Den** as the product control plane and **Letta Code** as the required bear runtime/harness. A **bear** is a Letta-backed primary agent that may be accessed through several surfaces:

- **Den chat UI → Den → Codepool / Letta Code → Letta** for first-party web chat.
- **Letta Code Channels → Letta Code → Letta** for supported external channels such as Slack.
- Future channel/proxy variants where Den may gain additional audit or mediation responsibilities.

Letta Code 0.15+ introduced **MemFS**, also described as a git-backed **context repository**. In the Letta Code version pinned by Codepool (`@letta-ai/letta-code@0.23.8`), MemFS memory is organized as a Markdown file tree under the agent's memory directory, typically:

- `~/.letta/agents/<agent-id>/memory`

MemFS has these important properties:

- Memory files are Markdown (`.md`) files with required frontmatter, including `description`.
- Top-level `system/` files are pinned into the agent's context window.
- Files outside `system/` are visible in the memory tree, but their full contents are omitted until explicitly read.
- Memory edits are saved by committing and pushing to the git-backed memory repository.
- Reflection/dream subagents may edit the memory repository using git worktrees.

Letta Code's native MemFS tools are already tightly integrated with this model. The relevant memory mutation tools include:

- `memory` — convenience tool for memory files in `$MEMORY_DIR`, with automatic commit and push.
- `memory_apply_patch` — apply a Codex-style patch to memory files in `$MEMORY_DIR`, with memory-file guardrails, automatic commit, and push.

The `memory` tool supports file-oriented commands such as:

- `create`
- `str_replace`
- `insert`
- `delete`
- `rename`
- `update_description`

Because a bear accessed through a Letta Code channel can call these native tools directly, wrapping every MemFS tool in Den would add latency and operational complexity without necessarily improving correctness.

At the same time, Den remains the source of truth for BEARS product concepts such as:

- `bear_id` ↔ `letta_agent_id` registry
- users and external identity mapping
- users ↔ bears membership
- roles, permissions, and policy
- Den-managed routines
- skills and MCP catalog attachment/materialization
- Cabinet/shared knowledge permissions
- product-facing audit and operator UX

The architectural question is therefore where to draw the boundary between **Letta Code-native memory tools** and **Den-hosted bear tools**.

---

## Decision

BEARS will **not** wrap Letta Code's native MemFS editing tools in Den by default.

Letta Code remains the preferred low-latency path for a bear's **per-bear, self-managed MemFS memory editing**, including calls to native tools such as `memory` and `memory_apply_patch`.

Den-hosted tools will be used where Den adds product-level semantics, authorization, shared-knowledge access, audit, approval, or cross-system composition.

In short:

> Letta Code owns the bear's native MemFS editing loop. Den owns product policy, identity, membership, shared knowledge, and governed workflows.

### Native Letta Code tools are the default for MemFS edits

A bear should use Letta Code-native tools directly for routine self-memory work, such as:

- creating or editing memory Markdown files;
- inserting notes into existing memory files;
- replacing text inside memory files;
- deleting or renaming memory files;
- updating memory file descriptions;
- applying multi-file patches to `$MEMORY_DIR`;
- committing and pushing memory repository changes;
- reflection/dream subagent memory maintenance.

This avoids unnecessary Den round trips when the bear is already running inside Letta Code.

### Den-hosted tools remain appropriate for product/meta capabilities

Den should expose bear-facing tools for product-level context that Letta Code does not own, such as:

- `about_bear`
- `current_user`
- `list_bear_users`
- `get_bear_user`
- `check_user_permission`
- `list_bear_capabilities`

These tools speak BEARS domain language and are backed by Den's registry, membership, policy, and configuration state.

### Den-hosted tools remain appropriate for Cabinet/shared knowledge

Letta MemFS is per-bear context. BEARS also needs shared, human-editable knowledge surfaces. Those belong behind Den-hosted Cabinet tools, for example:

- `cabinet_search`
- `cabinet_read`
- `cabinet_create`
- `cabinet_update`

Bears should not talk directly to Outline or Cabinet backing stores. Den enforces identity, bear membership, Cabinet permissions, and product policy.

### Den-hosted memory tools require a specific reason

Den may still provide memory-adjacent tools, but only when they add value beyond forwarding a native Letta Code call. Valid reasons include:

1. **Policy enforcement** — user role, membership, workspace, channel, or bear-specific restrictions.
2. **Privacy boundaries** — multi-user bears, user-specific data visibility, redaction, export/delete workflows.
3. **Human approval** — proposed memory changes reviewed in Den UI before application.
4. **Product audit** — durable, user-facing record of who/what caused governed memory changes.
5. **Cross-bear or shared memory** — knowledge that does not belong inside one bear's MemFS repository.
6. **Backend portability** — a deliberately stable product API where multiple storage backends may be composed.
7. **Operational visibility** — read-only memory status, repo health, conflicts, or diagnostics for UI/operator flows.

Absent one of these reasons, Den should not wrap `memory`, `memory_apply_patch`, or equivalent native MemFS edit operations.

### Den may observe native memory activity asynchronously

Den does not need to be in the synchronous path for every memory edit in order to provide visibility.

A preferred pattern is:

1. The bear edits MemFS using Letta Code-native tools.
2. Letta Code commits and pushes memory changes through the normal MemFS path.
3. Den reads metadata, diagnostics, logs, or MemFS Manager read-only endpoints asynchronously.
4. Den updates operator UX, memory dashboards, health views, or audit summaries where appropriate.

This preserves the fast native edit path while still allowing Den to provide product visibility.

---

## Capability Boundary

| Capability | Preferred implementation | Rationale |
|---|---|---|
| Edit a bear's own MemFS files | Letta Code native `memory` / `memory_apply_patch` | Lowest latency; already integrated with `$MEMORY_DIR`, git, commit/push, and reflection workflows. |
| Search/read local MemFS context during agent work | Letta Code native read/search/file tools | The bear is already running near the local memory checkout. |
| Show memory repo health in Den UI | Den read-only metadata/diagnostics | Operator UX and transparency; no need to mediate edits. |
| Identify current Den user | Den-hosted tool | Den owns external identity mapping and membership. |
| List users with bear access | Den-hosted tool | Den owns users ↔ bears authorization. |
| Check whether user may perform an action | Den-hosted tool | Den owns product policy. |
| Access shared Cabinet knowledge | Den-hosted Cabinet tools | Den enforces Cabinet ACLs and hides backing implementation details. |
| Governed memory proposal/approval workflow | Den-hosted workflow, applied through approved runtime path | Den adds human review, audit, and policy. |
| Cross-bear shared knowledge | Cabinet/Den, not per-bear MemFS | Per-bear MemFS should not become the shared knowledge database. |

---

## Tool Naming Guidance

Avoid presenting Letta MemFS as a generic fact store in Den-facing names unless the tool is intentionally a high-level workflow.

Preferred names for low-level MemFS-aligned capabilities, if Den ever exposes them, are file/repository-shaped:

- `memory_browse`
- `read_memory_file`
- `search_memory_files`
- `memory_repo_status`
- `propose_memory_patch`

Avoid default Den wrappers named like these unless they provide policy/approval/composition:

- `remember`
- `forget_memory`
- `edit_memory_file`
- `patch_memory_files`

If semantic tools such as `remember` are introduced later, they should be documented as **workflow tools** that result in curated MemFS Markdown edits or Cabinet writes, not as a separate Den-owned memory store.

---

## Consequences

### Positive

- Keeps low-latency channel interactions fast by avoiding unnecessary Den round trips.
- Avoids duplicating Letta Code's MemFS logic, git handling, commit/push behavior, and memory guardrails.
- Preserves Letta Code's native reflection/dream subagent workflows.
- Keeps Den focused on durable BEARS product responsibilities: users, bears, membership, policy, Cabinet, routines, skills/MCP catalog, audit, and UX.
- Reduces risk of split-brain memory semantics between Den and Letta Code.
- Makes Den-hosted tools meaningful instead of thin pass-through wrappers.

### Negative / Trade-offs

- Den will not automatically see every native MemFS edit synchronously.
- Product-level audit of all memory changes may require asynchronous log/metadata ingestion or future runtime hooks.
- Some memory operations may have different observability depending on whether they occur through Den web chat or direct Letta Code channels.
- Governed memory workflows will require explicit design rather than relying on native `memory` calls.

### Mitigations

- Use read-only MemFS Manager and Letta diagnostics for Den memory UI and health views.
- Prefer asynchronous observation for native memory changes where full synchronous mediation is unnecessary.
- Introduce Den-mediated memory proposal/approval tools only for workflows that actually require governance.
- Keep Cabinet separate from per-bear MemFS for shared, human-editable knowledge.
- Document whether a tool is **native memory editing**, **Den product metadata**, **Cabinet shared knowledge**, or **governed memory workflow**.

---

## Non-Goals

This ADR does not decide:

- the exact Cabinet tool schema;
- the final Den operator UI for memory dashboards;
- how to ingest or normalize Letta Code memory edit logs;
- whether future channel traffic should be proxied through Den for full audit;
- how to implement group-chat per-person memory beyond the existing multi-user memory ADR;
- whether Letta's future native memory APIs should replace or augment current MemFS tooling.

---

## Related Documents

- [multi-user-memory.md](multi-user-memory.md) — multi-user memory model and Letta-native memory visibility.
- [dynamic-skills-subagents.md](dynamic-skills-subagents.md) — dynamic skills, reflection subagents, and bear-authored capability growth.
- [routines-automation.md](routines-automation.md) — Den-managed routines and learning constraints.
- [../MEMFS_AND_MEMORY_UI.md](../MEMFS_AND_MEMORY_UI.md) — MemFS Manager responsibilities and Den read-only memory UI behavior.
- [../../planning/PLAN.md](../../planning/PLAN.md) — overall roadmap and Phase 1 memory model.
