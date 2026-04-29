# Multi-User Memory Architecture — Architecture Decision Record

## Status: Proposed

## Date: 2026-04-14

---

## Context

A **bear** (one Letta agent) may be shared by multiple **users** (Den's `user_bear` many-to-many). The bear must:

1. Know who it is talking to at any moment.
2. Read and write **person-specific** memories (preferences, facts, history) independently per user.
3. Maintain its own **identity and shared knowledge** (persona, policies, domain knowledge) across all users.
4. Handle the case where **multiple people appear in the same chat session** (e.g. a Slack channel or WhatsApp group via Letta Code).

Letta provides three primitives that together solve this:

- **Memory blocks** — labeled, always-in-context, agent-editable text sections (`human`, `persona`, custom labels). Can be shared across agents, or private.
- **Conversations API** — isolated message threads on a single agent, each with its own context window. Memory blocks are shared across conversations by default, but specific blocks can be **isolated per-conversation** via `isolated_block_labels`.
- **Archival memory** — a vector-searchable store the agent can query via tools; good for overflow facts that exceed block size limits.

References:

- [Letta memory blocks](https://docs.letta.com/guides/core-concepts/memory/memory-blocks)
- [Letta shared memory](https://docs.letta.com/guides/core-concepts/memory/shared-memory/)
- [Letta Conversations API](https://docs.letta.com/guides/agents/conversations/)
- [DEN_ARCHITECTURE.md](../DEN_ARCHITECTURE.md) — Den + self-hosted Letta
- [PLAN.md](../../planning/PLAN.md) — roadmap and contracts
- [PLAN.md § Shared memory blocks and concurrency](../../planning/PLAN.md#shared-memory-blocks-and-concurrency-letta) — write-path races, read staleness (e.g. LET-7893), memfs caveats (LET-8217, LET-8133), and team-oriented mitigations when **multiple agents** touch **shared** blocks

---

## Decision

**Relationship to Phase 1:** [Phase 1 planning](../../planning/PHASE1_BOOTSTRAP.md) is **web-first** and **1:1**-oriented; **Scenario A** below matches that near-term shape (per-user conversations and isolated `human` memory). **Scenario B** (multi-person group threads, `person:{name}` blocks, `group_context`, Den-managed per-person block lifecycle) is a **future / post–Phase 1 design target**—useful to decide early, **not** a commitment that Phase 1 will ship full group semantics or Den channel identity mapping. Where Phase 1 UI touches memory (for example a memory dashboard), it should stay aligned with **Letta-native** state for 1:1 flows and avoid implying group-mode completeness.

**Phase 1 product promise (blocks vs archival):** End-user and operator copy should reflect **curated, bounded memory blocks** as “always in mind” and **archival memory** (Letta-native, typically tool-mediated retrieval) as “findable when needed” — not a second store in Den. See [PLAN.md](../../planning/PLAN.md) § Phase 1 memory model.

**Memory dashboard metric:** Phase 1 does **not** expose an aggregate memory score; the dashboard shows Letta-native **`human`** memory for member bears. Operators get the full per-bear **state** summary in **bear detail**. See [PHASE1_DECISIONS.md](../../planning/PHASE1_DECISIONS.md) decision 8.

**Skills and subagents (separate ADR):** Procedural **skills** lifecycle (catalog, bear-created, reflection subagents) is covered in [dynamic-skills-subagents.md](dynamic-skills-subagents.md), not in this ADR.

### Two Distinct Scenarios

#### Scenario A: One Person Per Session (the common case)

This covers **Den web chat**, and **1:1 DMs** via messaging channels (Slack DM today via [Letta Code Channels](https://docs.letta.com/letta-code/channels/); WhatsApp when available). Each message comes from exactly one identified user.

```
                  ┌──────────────────────────────────────────────────┐
  Den chat ─────┐   │           Letta Agent (one bear)                 │
  Slack DM ─┘       │  ┌────────────────┐  ┌────────────────────────┐ │
       │           │  │ persona block   │  │ org_policy block       │ │
       ▼           │  │ (shared)        │  │ (shared, read-only)    │ │
  Den router ──────│  └────────┬───────┘  └───────────┬────────────┘ │
  (membership      │           │                      │              │
   check)          │  ┌────────┴──────────────────────┴────────┐     │
       │           │  │              │                │         │     │
       ▼           │  │  Conversation A (Alice)   Conversation B │   │
  create/reuse     │  │  ┌──────────────────┐  ┌──────────────┐│    │
  conversation     │  │  │ human block      │  │ human block  ││    │
  with isolated    │  │  │ (isolated copy   │  │ (isolated    ││    │
  human block      │  │  │  for Alice)      │  │  for Bob)    ││    │
       │           │  │  └──────────────────┘  └──────────────┘│    │
       ▼           │  └────────────────────────────────────────┘     │
  POST /v1/        │                                                  │
  conversations/   └──────────────────────────────────────────────────┘
  {id}/messages
```

**How it works:**

1. User sends a message through any surface. Den verifies `user_bear` membership.
2. Den looks up or creates a **Letta Conversation** for this `(user_id, bear_id)` pair, using `isolated_block_labels: ["human"]`. This tells Letta to **copy** the agent's `human` block into a conversation-specific instance.
3. Messages are sent to `POST /v1/conversations/{conv_id}/messages`. The agent reads/writes the **isolated** `human` block, which stores facts about that specific person. The `persona` block (and any org-policy block) remains shared.
4. Den stores the mapping `(user_id, bear_id) → conversation_id` in its database so the same conversation is reused across sessions.

**What Den tracks (new or extended tables):**

- `bear_conversations` table: `(user_id, bear_id, letta_conversation_id, channel, created_at)`.
- Optionally, a `channel` column distinguishes per-surface conversations if the operator wants separate threads for Slack vs. web.

**What the bear's system prompt needs:**

No special multi-user instructions. The standard `human`/`persona` pattern works unchanged — the agent thinks it is always talking to one person (which it is, per conversation).

#### Scenario B: Multiple People in One Chat Session (the hard case)

**Implementation status:** **Post–Phase 1** target design (see note under [Decision](#decision)); not required for v1 web chat delivery.

This covers **Letta Code group chats**: a Slack channel, a WhatsApp group, or a Slack thread where multiple people interact with the bear simultaneously.

One Letta Conversation maps to the group thread, but the bear must distinguish speakers and maintain per-person memory.

```
                     ┌──────────────────────────────────────────────┐
  Slack channel      │           Letta Agent (one bear)              │
  ┌───────────┐      │                                               │
  │ Alice     │      │  ┌────────────┐  ┌─────────────────────────┐ │
  │ Bob       │──┐   │  │ persona    │  │ group_context block     │ │
  │ Carol     │  │   │  │ (shared)   │  │ (participants, channel) │ │
  └───────────┘  │   │  └──────┬─────┘  └────────────┬────────────┘ │
                 │   │         │                     │              │
  Letta Code       │   │  ┌──────┴─────────────────────┴────────┐     │
  (prefixes      ▼   │  │         Group Conversation           │     │
   messages      ────►│  │                                     │     │
   with sender)      │  │  ┌──────────┐ ┌────────┐ ┌───────┐ │     │
                      │  │  │person:   │ │person: │ │person:│ │     │
                      │  │  │alice     │ │bob     │ │carol  │ │     │
                      │  │  │block     │ │block   │ │block  │ │     │
                      │  │  └──────────┘ └────────┘ └───────┘ │     │
                      │  └────────────────────────────────────┘     │
                      └──────────────────────────────────────────────┘
```

**How it works:**

1. Letta Code (or Den as proxy) **prefixes** each message with the sender's identity: `[Alice]: Can you check my schedule?`. This is the simplest reliable way for the agent to know who is talking.
2. Instead of a single `human` block, the bear has **per-person blocks** with labels like `person:alice`, `person:bob`. These are standard read-write memory blocks the agent can update with `memory_insert` / `memory_replace`.
3. An optional `group_context` block stores shared group state (e.g. "This is the #project-alpha Slack channel. Participants: Alice, Bob, Carol.").
4. The **system prompt** instructs the bear:
   - Messages are prefixed with `[Name]`. Always note who is speaking.
   - Use the `person:{name}` blocks to store/recall facts about each individual.
   - Use the `group_context` block for shared group state.
   - When replying, you may address individuals by name.

**What Den manages:**

- When a user is added to a bear's membership (or first appears in a group channel), Den:
  1. Creates a `person:{username}` block via `POST /v1/blocks` with a description like "Facts and preferences about Alice."
  2. Attaches it to the bear's agent via `POST /v1/agents/{agent_id}/blocks/attach`.
  3. Optionally updates the `group_context` block to list the new participant.
- When a user is removed, Den detaches (and optionally archives) the person block.
- Den stores the mapping between its `user_id` and the Letta `block_id` for that person block, so it can manage the lifecycle.

**New Den data:**

- `user_bear_blocks` table: `(user_id, bear_id, letta_block_id, block_label, created_at)`.
- Or extend `user_bear` with a nullable `letta_person_block_id`.

### Hybrid: A Bear That Does Both

Most bears will handle **both** scenarios: 1:1 chats via Den chat/web and group chats via Letta Code. The approaches compose cleanly:

- **1:1 conversations** use `isolated_block_labels: ["human"]` — person-specific memory is automatic via the Conversations API.
- **Group conversations** use explicit `person:{name}` blocks — person-specific memory is managed by Den and the system prompt.
- The agent's **persona** block and any **org_policy** blocks are shared everywhere.

The key design question is whether a bear should also have `person:{name}` blocks for 1:1 users (for consistency), or rely on isolated `human` blocks. The recommendation:

- **Phase 1:** Use isolated `human` blocks for 1:1 (simpler, Letta handles it natively). Group chat is out of scope per [PHASE1_BOOTSTRAP.md](../../planning/PHASE1_BOOTSTRAP.md).
- **Phase 2:** When group chat support lands, add `person:{name}` blocks. If the bear needs to cross-reference person knowledge between 1:1 and group contexts (e.g. "Alice told me in our private chat that she prefers dark mode"), consider migrating 1:1 to also use `person:{name}` blocks with a shared conversation instead.

### Memory Block Layout

| Block label | Scope | Writable by agent | Purpose |
|---|---|---|---|
| `persona` | Per bear (shared across all convos) | Yes | Bear's identity, personality, skills |
| `org_policy` | Per deployment (shared, read-only) | No | Org-wide rules, policies |
| `human` (isolated) | Per conversation (one per user in 1:1) | Yes | Facts about the person (1:1 mode) |
| `person:{name}` | Per bear (attached when user joins) | Yes | Facts about a specific person (group mode) |
| `group_context` | Per group conversation | Yes | Shared state for a group thread |

### What Letta Code Needs to Do

For multi-person chats, Letta Code (or Den as proxy) must:

1. **Identify the sender** in every message. The simplest approach: prefix the message content with `[username]:` before sending to Letta. Letta Code already knows who sent the Slack/WhatsApp message.
2. **Map external IDs to Den users.** Harness config (generated by Den) maps Slack user IDs and WhatsApp numbers to Den usernames. Den already generates allowlists / `letta-code.yaml`; extend this to include a `name`/`displayName` mapping.
3. **Route through Den (later).** In v1, the harness talks to Letta directly. When a channel-only Den proxy lands, Den handles the sender prefix injection and person-block provisioning automatically.

---

## Consequences

- **1:1 chat** gains true per-person memory with minimal implementation effort — Letta's `isolated_block_labels` on the Conversations API handles it natively.
- **Group chat** requires Den to manage per-person block lifecycle (create, attach, detach) and system prompt instrumentation, adding complexity to the provisioning layer.
- The hybrid approach means a bear's knowledge about a person may live in two places (an isolated `human` block for 1:1, a `person:{name}` block for groups). Cross-context sync is deferred to Phase 3 as an explicit trade-off.
- Per-person blocks consume context window space proportional to the number of participants. For large groups, this may require archival overflow or selective block loading — a scaling concern to revisit when real usage data exists.
