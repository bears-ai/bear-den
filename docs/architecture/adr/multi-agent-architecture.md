# ADR: BEARS Multi-Agent Architecture for Letta-Backed Coding Agents

**Status:** Accepted
**Date:** 2026-05-03
**Deciders:** Hans

## Context

We host stateful coding agents on a server, exposed across multiple interaction surfaces: Slack, a homegrown web chat, IDE integration via the Agent Client Protocol (ACP), and an emerging need for autonomous, scheduled work against external systems. The original architecture used one Letta Code harness per agent identity, routing all surfaces through it.

This produced concurrency failures when ACP — which natively supports multiple sessions per connection — drove tool calls into a harness whose tool executor, permission manager, and cwd were single-session state. A brute-force mitigation (new harness per prompt, reusing the underlying conversation) worked but paid full Letta Code startup costs (skill loading, file indexing, auth handshake, executor setup) on every turn.

Two structural mismatches drove this:

1. **Letta's per-agent sequential processing.** Letta documents that concurrent requests to a single agent are undefined behavior. Parallelism is supposed to come from separate agents or separate Conversations.
2. **Letta Code's harness-coupled state.** Tool execution, permissions, cwd, and environment live in the harness process, not in the Letta agent. The harness is therefore the actual unit of state we need to multiply, not the agent.

We pursued a pre-warmed harness pool with conversation rebinding and hit unmanageable state-leakage between rebindings. We considered a single Letta-API-direct ACP adapter targeting one shared agent but it inherits Letta Code's tool surface assumptions on the ACP path and still hits the per-agent sequential ceiling. We landed on multiple specialized agents per Bear, sharing memory.

A separate but related concern is the **lethal trifecta** for agentic systems: simultaneous access to private data, ability to communicate externally, and ability to modify durable state. An agent holding all three legs is an exfiltration and abuse risk, especially given that private data may include user input that has been subjected to prompt injection. The architecture below distributes the three legs across distinct agents so that no single agent holds all of them.

## Decision

### 1. Bear and Den abstractions

A **Bear** is a logical agent identity from the user's perspective — one coherent assistant with persistent memory and accumulated skills. Internally, a Bear is a cluster of four specialized Letta agents that share memory, prompts, and skills via centralized provisioning.

**Den** is the control plane: it provisions Bears, keeps their constituent agents in sync, owns the MemFS sidecar, schedules curate cycles, manages the work-task queue, and acts as gateway for surfaces that need one (Slack, web chat).

### 2. Four specialized agents per Bear

Every Bear consists of:

- **talk agent** — Backs Letta Code-based conversational channels (Slack, web chat, Discord). Runs behind a Letta Code harness. Tool profile suited to text-in / text-out interaction with optional tool calls executed on the harness server. Holds: private data, durable state (own memory branch). No external comms beyond the channel itself.
- **pair agent** — Backs ACP-direct connections from client-side tools (IDEs, Cowork, Figma plugins, future ACP-speaking apps). Implemented as a Letta-API client that speaks ACP; no Letta Code harness involved. Tool execution forwarded to the client via ACP's native tool-call and `session/request_permission` flow. Holds: private data, durable state (own memory branch). External effects only via the client tool, gated by user approval per call.
- **curate agent** — Handles reflection, defragmentation, cross-branch memory integration, approval of work-task intents, and promotion of work results into shared memory. Server-orchestrated, never user-facing. Has read access to all branches; sole writer to `core/`. Holds: private data (broad — sees everything across the Bear), durable state (writes `core/`, which influences every other agent's system prompt). **No external comms by design.**
- **work agent** — Executes scheduled or event-triggered tasks against external systems (APIs, services, long-running research). Reads only from `core/` and from explicit task definitions Den hands it. **Does not read `talk/` or `pair/` branches.** Holds: external comms, durable state (own memory branch). Sees only curated, post-curate-review private data.

Each agent has its own tool profile. Prompts, skills, and memory are shared via Den-managed sync.

### 3. Trust boundaries and the lethal trifecta

The four-agent split is not arbitrary; it is designed to ensure no single agent holds all three legs of the lethal trifecta:

| Agent | Private data | External comms | Durable state writes |
|---|---|---|---|
| `talk` | yes (channel-scoped) | conversational only | yes (own branch) |
| `pair` | yes (session-scoped) | client-mediated, user-gated | yes (own branch) |
| `curate` | yes (broad) | **no** | yes (own branch + `core/`) |
| `work` | curated only (`core/`) | yes | yes (own branch) |

The trust hierarchy is:

```
talk, pair (raw user input, narrow durable writes)
    ↓
curate (reads all, writes core/, no external comms)
    ↓
work (reads only curated core/, has external comms)
```

Each step does some filtering. Channel agents see raw user input (and potential prompt injections) but cannot act on the world beyond their own conversational surface. The curate agent sees everything but only writes to memory. The work agent acts on the world but only on inputs that have been mediated by the curate agent.

This protects against attacks where a prompt injection in a Slack message or IDE-shared document attempts to induce external action (data exfiltration, unauthorized API calls). The agent that received the injection has no external comms; the agent with external comms has not seen the raw injection.

### 4. Shared memory via MemFS with worktree isolation

- One bare MemFS repo per Bear, served via Letta's git smart HTTP sidecar pattern (`LETTA_MEMFS_SERVICE_URL` pointing at the sidecar).
- Each agent operates on its own git worktree against the bare repo.
- **Branch-per-agent:** `talk`, `pair`, `curate`, `work`. Git's worktree-branch invariant prevents two agents writing the same branch concurrently.
- **Path-per-agent within each branch.** The path access matrix:

| Branch | Writable paths | Readable paths |
|---|---|---|
| `talk` | `talk/` | `talk/`, `core/` |
| `pair` | `pair/` | `pair/`, `core/` |
| `curate` | `curate/`, `core/` | all branches |
| `work` | `work/` | `work/`, `core/` (no `talk/`, `pair/`, `curate/`) |

Enforced via a `pre-receive` hook on the bare repo that inspects branch and changed paths.

- **Push-on-commit:** agents push immediately after each commit rather than waiting for Letta Code's periodic reminders. Other agents fetch and fast-forward merge on each system prompt construction. Fetch cost on long-lived agents is negligible (local-disk fast-forward) and we explicitly accept it without further optimization.
- The curate agent is the sole merge authority. It promotes durable learnings from `talk/` and `pair/` into `core/`. Channel agents never read each other's branches. The work agent never reads channel branches at all.

### 5. Task request flow

External work is requested through the architecture, never invoked directly by channel agents:

1. A user asks the talk or pair agent to do something with external effects ("check deploy status hourly", "post a daily standup summary to #team").
2. The channel agent writes a structured task intent to its own branch (`talk/tasks/<intent-id>.md` or `pair/tasks/<intent-id>.md`).
3. On its next cycle, the curate agent reads pending task intents from channel branches. For each, it decides approve or reject and invokes privileged Den tooling to perform the mutation:
   - Approval validates and writes `core/tasks/<task-id>.md` with appropriate metadata (schedule, scope, allowed tools, risk level), then updates the source intent audit metadata; or
   - Rejection updates the source intent audit metadata with a reason for the channel agent to surface to the user.

The curate agent is not granted raw write access to channel branches; Den tools perform these cross-branch audit updates as controlled operations.
4. Den picks up approved tasks from `core/tasks/` and dispatches them to the work agent on schedule or trigger.
5. The work agent executes, writing logs and results to `work/results/<task-id>/<run-id>.md`.
6. On its next cycle, the curate agent promotes summary results to `core/results/` for visibility to channel agents (so the user can ask "what did you do overnight?" via talk or pair).

For high-risk operations (any task that would be destructive or irreversible), Den implements an additional human-in-the-loop approval queue per run, surfaced in the management UI. Routine, low-risk tasks (read-only checks, idempotent posts) flow through curate-agent approval alone.

The full schema for intent files, approved task files, and result files is specified in the **Bear/Den Tasks Schema** document.

### 6. Den responsibilities

Den must:

- Provision the four agents per Bear with their correct tool profiles, system prompts, runtime policy, and eventually skill rosters once the skills design is finalized.
- Detect and reconcile drift in tool/prompt/runtime policy state across the four agents within a Bear; skill drift detection is deferred to the skills design.
- Run the MemFS sidecar; own conflict resolution policy.
- Trigger curate cycles on appropriate cadence (idle detection, message-count thresholds, manual trigger).
- Manage the work-task queue: pick up approved tasks from `core/tasks/`, dispatch to the work agent on schedule, log results.
- Implement the human-in-the-loop approval queue for high-risk work-task runs.
- Rate-limit the work agent's external calls; alert on novel destinations or unusually high volume.
- Route ACP traffic to the pair agent.
- Route Slack and web-chat traffic to the talk agent via its Letta Code harness.

### 7. Conversations are agent-locked

Conversation history is not shared across the agents within a Bear. Cross-channel learning transfer happens only via the curate agent reading branches and promoting to `core/`. The management UI must surface conversations as agent-locked rather than Bear-global, with a Bear-level view that aggregates without conflating.

### 8. Open: skill sync mechanism

Two viable paths are still on the table:

- **Skills as MemFS content** — store skills inside the memfs repo (in a `.skills/` directory under each agent's path or under `core/`), letting git handle distribution.
- **Den-managed skill installation** — Den owns the skill roster and installs/updates skills on each agent via Letta API, treating skills as out-of-band from MemFS.

Decision remains deferred to the dedicated skills design. Phase 0 freezes the constraint that agent-driven `/skill` learning writes must flow through the selected mechanism or be disabled; agents must not install out-of-band Letta skills directly.

## Consequences

### Positive

- Concurrency issues dissolve: each agent is its own sequential-processing unit. Multiple Bears, and multiple users hitting one Bear via different surfaces, parallelize naturally.
- ACP integration becomes idiomatic — uses the protocol's native tool-call/permission flow rather than fighting Letta Code's tool model.
- Tool surface mismatch between surfaces becomes explicit and managed by design rather than a source of silent drift.
- The lethal trifecta is structurally split. Prompt injections in channel input cannot directly trigger external action.
- Failure isolation: a crashing talk harness doesn't affect pair availability; a failing curate cycle doesn't block user turns; a misbehaving work task doesn't compromise channel agents.
- Reflection and integration workload runs on its own schedule without contending for user-turn capacity.
- Different agents can use different model tiers (e.g., faster model for talk and pair, larger context for curate, narrower-scoped model for work) without configuration warts.

### Negative / Tradeoffs

- More moving parts per Bear (4 agents, central management plane, sidecar, task queue).
- Den must own provisioning sync as a first-class concern; drift between agents within a Bear is a real failure mode requiring monitoring and reconciliation tooling.
- Cross-channel learning transfer is eventually-consistent. A user teaching the talk agent something on Slack does not affect the pair agent's behavior in the IDE until a curate cycle promotes the learning to `core/`.
- Task execution is also eventually-consistent — a user requesting work via talk or pair does not see action until the next curate cycle approves the request. Acceptable for scheduled and background work; not appropriate for urgent operations.
- Conversation history fragmentation requires UI work to be navigable.
- We diverge from Letta's "one stateful agent, many conversations" abstraction. If Letta solves multi-tenant tool execution natively in future versions, our architecture becomes a legacy workaround.

### Operational risks to monitor

- Skill installation drift across the four agents within a Bear.
- Tool profile drift, especially accidental re-attachment by Letta Code on the talk path.
- Memory consistency under concurrent commits across worktrees (mitigated by branch-per-agent + path-per-agent invariants but still worth observing).
- Curate-cycle scheduling colliding with user-turn load.
- Work-agent rate limiting and unauthorized destinations — needs active monitoring and alerting.
- Task intent backlog: if curate cycles fall behind, user-requested work piles up unactioned.
- MemFS itself is a recent feature (Letta Code 0.15+); upstream changes carry risk.

## Alternatives considered

**Pre-warmed harness pool with conversation rebinding.** Pursued; abandoned. State leakage between rebindings (cwd, permission mode, environment, file watchers) was unmanageable.

**Single Letta-API ACP adapter targeting one shared agent (no Letta Code in the loop for ACP).** Cleaner than the pool, but the shared-agent constraint propagates: tool surface still has to satisfy both Slack-via-Letta-Code and ACP-direct, and per-agent sequential processing still bottlenecks parallel users.

**Hybrid: ACP direct to Letta + Slack via Letta Code, both targeting one shared agent.** Considered seriously. Risks identified around tool surface drift, cwd assumptions baked into memory, skill availability mismatch, and provisioning ambiguity outweighed the benefit of preserving "one agent ID per Bear."

**Single shared agent with multi-conversation parallelism.** Letta's recommended pattern. Conversations parallelize at the message level but tool execution context (surface-specific) does not multiplex through them.

**Co-locating curate responsibility with the talk agent.** Considered; rejected. Reflection's tool needs differ from talk's; reflection bursts compete with user-turn slots; merge authority is a global concern that shouldn't sit on a channel agent's view. Also: would have given the talk agent all three legs of the lethal trifecta via its curate powers.

**Three agents (talk, pair, curate) with external work co-located on channel agents.** Considered; rejected on trifecta grounds. Channel agents see raw user input including potential prompt injections; granting them external comms means a successful injection has a direct exfiltration path. Acceptable for narrow, user-confirmed actions but not for autonomous or scheduled execution.

**Three agents with external work as deterministic Den-side jobs (no LLM in the loop for execution).** Considered as a complement to the four-agent design. Useful for highly structured tasks but doesn't cover work that requires reasoning during execution. Adopted as the high-risk-task mode within the work-agent design (Den can choose deterministic execution for tasks that don't need reasoning, agent execution for tasks that do).

## Prior art and pattern alignment

This section situates the design against published work on agentic prompt-injection defenses. The four-agent split was developed from first principles around the trifecta, but most of its load-bearing properties correspond to named patterns in the literature. Naming them makes future review easier and surfaces concrete remediations for the places where this design diverges from the canonical form.

The reference taxonomy is Beurer-Kellner et al., *Design Patterns for Securing LLM Agents against Prompt Injections* (2025), which formalizes six patterns: **Action-Selector**, **Plan-Then-Execute**, **LLM Map-Reduce**, **Dual LLM** (Willison, 2023), **Code-Then-Execute**, and **Context-Minimization**. CaMeL (Debenedetti et al., 2025) is the strongest published instance of Code-Then-Execute and contributes the capability/taint-tracking machinery referenced below. OWASP Top 10 for Agentic Applications (2026) supplies the threat vocabulary, in particular ASI01 (Agent Goal Hijack) and ASI07 (Insecure Inter-Agent Communication).

### Correspondence to the four-agent split

| This ADR | Pattern analog | Notes |
|---|---|---|
| `talk`, `pair` | Quarantined LLM (channel side) of Dual LLM | Diverges in that they hold their own durable writes rather than returning only symbolic references. |
| `curate` | Privileged controller of Dual LLM + Approval Agent (separation-of-duties) | Sees all branches; sole writer to `core/`; no external comms by design. |
| `work` | Executor of Plan-Then-Execute; Action-Selector when dispatched as a deterministic Den job | Reads only `core/` and explicit task definitions; never raw user input. |
| `core/` plus `pre-receive` hook | Coarse capability enforcement, path-and-branch granular | Plays the role CaMeL gives to a typed interpreter at value granularity. |
| Den per-run HITL queue for high-risk tasks | Anthropic Plan-Mode-style intent approval | Approves task intent and individual destructive runs, not each step. |
| Channel agents writing structured task intents (§5) | Context-Minimization on the work-agent path | Work agent receives no free-form user prose. |

The trifecta argument in §3 is the same argument the design-patterns paper makes for Plan-Then-Execute, applied across persistent agents rather than within a single request.

### Departures from the canonical patterns

These are intentional departures or known gaps relative to the strictest form of each pattern. They are recorded here so they remain visible and reviewable; each is a candidate for a follow-up ADR and none change the core decision above.

1. **The `curate` agent is not strictly quarantined.** It reads raw `talk/` and `pair/` branches, which contain potentially-injected content, and it is also the sole writer to `core/`. In strict Dual LLM, only the Q-LLM touches untrusted tokens. The trifecta is still structurally split because curate has no external comms, but an injection-driven promotion to `core/` does compromise the work agent. Tracked mitigations: provenance metadata on every `core/` entry; an LLM Map-Reduce shape for the curate cycle (per-branch quarantined summarizers feeding a reducer that does not see raw content); a non-LLM policy engine validating `core/tasks/<task-id>.md` against an allowlist DSL before Den dispatches.
2. **Inter-agent communication is integrity-checked but not authenticated per agent.** Git push/fetch with `pre-receive` enforces who-writes-what-where but does not cryptographically bind a commit to a specific agent identity. OWASP ASI07 calls this out as a distinct class of risk. Tracked mitigation: per-agent commit signing, with Den verifying signer-vs-branch on each fetch and the `pre-receive` hook rejecting unsigned or mis-signed commits.
3. **Memory poisoning has no first-class rollback story.** `core/` is durable and influences every subsequent system prompt. IBM's A2AS framing recommends data-provenance plus rollback at the memory layer. Git history makes rollback mechanically possible; what is missing is an audit cadence and a documented rollback path. Tracked mitigation: periodic `core/`-audit curate cycles that replay evidence trails for high-impact entries, plus a Den-side procedure for reverting `core/` to a prior known-good commit and reconstructing affected agent contexts.
4. **The `curate` agent is both reviewer and planner.** Strongest-guarantee instances of these patterns place a deterministic policy engine between planner and executor. Today curate plays both roles. Tracked mitigation: split "curate proposes promotion" from "Den policy-engine validates" as a Den responsibility, alongside the existing per-run HITL queue for high-risk tasks. The policy engine is a non-LLM component and applies regardless of how curate was reasoning at the time.

## Naming notes

The four agents are named for the activity they perform:

- `talk` — synchronous conversation through chat-style channels
- `pair` — synchronous collaboration in a client tool (IDE, Figma, etc.)
- `curate` — integrate, reflect, approve; the editorial and gatekeeping role
- `work` — autonomous external action

`curate` was chosen over `dream`, `steward`, `weave`, and `direct` for accessibility and accuracy. It captures both halves of the agent's role: editorial integration of branch content into `core/`, and approval of work-task intents. Internationally legible, not metaphorical, doesn't connote command-and-control (which the agent does not do).

## References

- Letta Code architecture: <https://docs.letta.com/letta-code/how-it-works/>
- Letta Conversations: <https://www.letta.com/blog/conversations>
- Letta MemFS / Context Repositories: <https://www.letta.com/blog/context-repositories>
- ACP overview: <https://agentclientprotocol.com/overview/architecture>
- Letta sequential processing constraint: <https://docs.letta.com/api/python/resources/agents/subresources/messages/methods/stream>
- Lethal trifecta: <https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/>
- Beurer-Kellner et al., *Design Patterns for Securing LLM Agents against Prompt Injections*: <https://arxiv.org/abs/2506.08837>
- Debenedetti et al., *Defeating Prompt Injections by Design* (CaMeL): <https://arxiv.org/abs/2503.18813>
- Willison, *The Dual LLM pattern for building AI assistants that can resist prompt injection*: <https://simonwillison.net/2023/Apr/25/dual-llm-pattern/>
- OWASP Top 10 for Agentic Applications (2026): <https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026/>
- Anthropic, *Trustworthy agents in practice*: <https://www.anthropic.com/research/trustworthy-agents>
- IBM, *Establishing Runtime Security for Agentic AI* (A2AS): <https://www.ibm.com/think/insights/agentic-ai-runtime-security>
- Companion documents: [`../../planning/MULTI_AGENT_IMPLEMENTATION_PLAN.md`](../../planning/MULTI_AGENT_IMPLEMENTATION_PLAN.md), [`../tasks-schema.md`](../tasks-schema.md), [`../../../services/den/docs/bear-spec.md`](../../../services/den/docs/bear-spec.md)
