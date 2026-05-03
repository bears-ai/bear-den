# ADR: Bear/Den Multi-Agent Architecture for Letta-Backed Coding Agents

**Status:** Accepted
**Date:** 2026-05-03
**Deciders:** Hans

## Context

We host stateful coding agents on a server, exposed across multiple interaction channels: Slack, a homegrown web chat, and IDE integration via the Agent Client Protocol (ACP). The original architecture used one Letta Code harness per agent identity, routing all channels through it.

This produced concurrency failures when ACP — which natively supports multiple sessions per connection — drove tool calls into a harness whose tool executor, permission manager, and cwd were single-session state. A brute-force mitigation (new harness per prompt, reusing the underlying conversation) worked but paid full Letta Code startup costs (skill loading, file indexing, auth handshake, executor setup) on every turn.

Two structural mismatches drove this:

1. **Letta's per-agent sequential processing.** Letta documents that concurrent requests to a single agent are undefined behavior. Parallelism is supposed to come from separate agents or separate Conversations.
2. **Letta Code's harness-coupled state.** Tool execution, permissions, cwd, and environment live in the harness process, not in the Letta agent. The harness is therefore the actual unit of state we need to multiply, not the agent.

We pursued a pre-warmed harness pool with conversation rebinding (the "Approach 3" route) and hit unmanageable state-leakage between rebindings. We considered a single Letta-API-direct ACP adapter targeting one shared agent (Approach 1+2), but it inherits Letta Code's tool surface assumptions on the ACP path and still hits the per-agent sequential ceiling. We landed on channel-locked agents sharing memory.

## Decision

### 1. Bear and Den abstractions

A **Bear** is a logical agent identity from the user's perspective — one coherent assistant with persistent memory and accumulated skills. Internally, a Bear is a cluster of three specialized Letta agents that share memory, prompts, and skills via centralized provisioning.

**Den** is the control plane: it provisions Bears, keeps their constituent agents in sync, owns the MemFS sidecar, schedules dream cycles, and acts as gateway for channels that need one (Slack, web chat).

### 2. Three specialized agents per Bear

Every Bear consists of:

- **chat agent** — Backs Letta Code-based channels (Slack, web chat). Runs behind a Letta Code harness. Tool profile suited to conversational interaction with file/repo access on the harness server.
- **control agent** — Backs ACP-direct connections from IDEs. Implemented as a Letta-API client that speaks ACP; no Letta Code harness involved. Tool execution forwarded to the IDE via ACP's native tool-call and `session/request_permission` flow.
- **dream agent** — Handles reflection, defragmentation, and cross-channel memory integration. Server-orchestrated, never user-facing. Reads from all channel branches; sole writer to the `shared/` memory path.

Each agent has its own tool profile (because tool semantics differ across channels). Prompts, skills, and memory are shared via Den-managed sync.

### 3. Shared memory via MemFS with worktree isolation

- One bare MemFS repo per Bear, served via Letta's git smart HTTP sidecar pattern (`LETTA_MEMFS_SERVICE_URL` pointing at the sidecar).
- Each agent operates on its own git worktree against the bare repo.
- **Branch-per-agent:** `chat`, `control`, `dream`. Git's worktree-branch invariant prevents two agents writing the same branch concurrently.
- **Path-per-agent within each branch:** agents only write to their own subdirectory. The `shared/` path is read-only for chat and control agents and writable by the dream agent only. Enforced via a `pre-receive` hook on the bare repo.
- **Push-on-commit:** agents push immediately after each commit rather than waiting for Letta Code's periodic reminders. Other agents fetch and fast-forward merge on each system prompt construction. Fetch cost on long-lived agents is negligible (local-disk fast-forward) and we explicitly accept it without further optimization.
- The dream agent is the sole merge authority. It promotes durable learnings from `chat/` and `control/` into `shared/`. Channel agents never read each other's branches.

### 4. Den responsibilities

Den must:

- Provision the three agents per Bear with their correct tool profiles, system prompts, and skill rosters.
- Detect and reconcile drift in tool/prompt/skill state across the three agents within a Bear.
- Run the MemFS sidecar; own conflict resolution policy.
- Trigger dream cycles on appropriate cadence (idle detection, message-count thresholds, manual trigger).
- Route ACP traffic to the control agent.
- Route Slack and web-chat traffic to the chat agent via its Letta Code harness.

### 5. Conversations are agent-locked

Conversation history is not shared across the three agents within a Bear. Cross-channel learning transfer happens only via the dream agent reading branches and promoting to `shared/`. The management UI must surface conversations as agent-locked rather than Bear-global, with a Bear-level view that aggregates without conflating.

### 6. Open: skill sync mechanism

Two viable paths are still on the table:

- **Skills as MemFS content** — store skills inside the memfs repo (in a `.skills/` directory under each agent's path or under `shared/`), letting git handle distribution.
- **Den-managed skill installation** — Den owns the skill roster and installs/updates skills on each agent via Letta API, treating skills as out-of-band from MemFS.

Decision deferred to implementation phase 0. Constraints: agent-driven `/skill` learning writes must flow through whichever mechanism we pick, or be disabled.

## Consequences

### Positive

- Concurrency issues dissolve: each agent is its own sequential-processing unit. Multiple Bears, and multiple users hitting one Bear via different channels, parallelize naturally.
- ACP integration becomes idiomatic — uses the protocol's native tool-call/permission flow rather than fighting Letta Code's tool model.
- Tool surface mismatch between channels becomes explicit and managed by design rather than a source of silent drift.
- Failure isolation: a crashing chat harness doesn't affect ACP availability; a failing dream cycle doesn't block user turns.
- Reflection workload runs on its own schedule without contending for user-turn capacity.
- Different channels can use different model tiers (e.g., faster model for chat, larger context for dream) without configuration warts.

### Negative / Tradeoffs

- More moving parts per Bear (3 agents, central management plane, sidecar).
- Den must own provisioning sync as a first-class concern; drift between agents within a Bear is a real failure mode requiring monitoring and reconciliation tooling.
- Cross-channel learning transfer is eventually-consistent. A user teaching the chat agent something on Slack does not affect the control agent's behavior in the IDE until a dream cycle promotes the learning to `shared/`.
- Conversation history fragmentation requires UI work to be navigable.
- We diverge from Letta's "one stateful agent, many conversations" abstraction. If Letta solves multi-tenant tool execution natively in future versions, our architecture becomes a legacy workaround.

### Operational risks to monitor

- Skill installation drift across the three agents within a Bear.
- Tool profile drift, especially accidental re-attachment by Letta Code on the chat path.
- Memory consistency under concurrent commits across worktrees (mitigated by branch-per-agent + path-per-agent invariants but still worth observing).
- Dream-cycle scheduling colliding with user-turn load.
- MemFS itself is a recent feature (Letta Code 0.15+); upstream changes carry risk.

## Alternatives considered

**Pre-warmed harness pool with conversation rebinding.** Pursued; abandoned. State leakage between rebindings (cwd, permission mode, environment, file watchers) was unmanageable.

**Single Letta-API ACP adapter targeting one shared agent (no Letta Code in the loop for ACP).** Cleaner than the pool, but the shared-agent constraint propagates: tool surface still has to satisfy both Slack-via-Letta-Code and ACP-direct, and per-agent sequential processing still bottlenecks parallel users.

**Hybrid: ACP direct to Letta + Slack via Letta Code, both targeting one shared agent.** Closer to current design, considered seriously. Risks identified around tool surface drift, cwd assumptions baked into memory, skill availability mismatch, and provisioning ambiguity outweighed the benefit of preserving "one agent ID per Bear."

**Single shared agent with multi-conversation parallelism.** Letta's recommended pattern. Conversations parallelize at the message level but tool execution context (channel-specific) does not multiplex through them. Unsolved.

**Co-locating dreamer responsibility with the chat agent.** Considered; rejected. Reflection's tool needs differ from chat's; reflection bursts compete with user-turn slots; merge authority is a global concern that shouldn't sit on a channel agent's view.

## References

- Letta Code architecture: <https://docs.letta.com/letta-code/how-it-works/>
- Letta Conversations: <https://www.letta.com/blog/conversations>
- Letta MemFS / Context Repositories: <https://www.letta.com/blog/context-repositories>
- ACP overview: <https://agentclientprotocol.com/overview/architecture>
- Letta sequential processing constraint: <https://docs.letta.com/api/python/resources/agents/subresources/messages/methods/stream>
