# Implementation Plan: Bear/Den Multi-Agent Architecture

This plan implements the architecture described in `multi-agent-architecture` ADR. Each phase has explicit acceptance criteria. Phases are ordered for safe incremental rollout — earlier phases do not depend on later ones, and phase 8 (migration) only runs after phases 1–7 have been validated on a test Bear.

## Glossary (read first)

- **Bear** — a logical agent identity. Implemented as a triplet of Letta agents (chat, control, dream) sharing memory.
- **Den** — our existing control plane service. Provisions and manages Bears.
- **MemFS** — Letta's git-backed memory filesystem. Stored as a bare repo per Bear; agents access it via worktrees.
- **Sidecar** — the git smart HTTP service that fronts the bare repos. Configured via `LETTA_MEMFS_SERVICE_URL`.
- **Channel agent** — chat or control. The two user-facing agents.
- **Worktree** — a git working directory tied to one branch. Each agent gets one worktree per Bear.

## Prerequisites

- Den service is running and able to talk to the Letta server.
- Letta server has MemFS support enabled (`LETTA_MEMFS_SERVICE_URL` set to the sidecar URL or `local`).
- One existing Bear available for end-to-end testing. Do not use a Bear with real user data until phase 8.

---

## Phase 0 — Specification freeze

**Goal:** lock down the canonical Bear configuration before any code changes.

### Tasks

1. Write a spec doc (`bear-spec.md` in the Den repo) covering:
   - Tool roster for each of the three agent types. List every tool by name, server-side vs client-side, and which agent it's attached to.
   - System prompt template. Decide whether the three agents share a base prompt with role-specific suffixes, or have fully distinct prompts. Recommendation: shared base, distinct role suffixes.
   - Skill roster: which skills go to which agents. Some skills (e.g., reflection-related) are dream-only; coding skills are chat and control; some user-preference skills go to all three.
   - MemFS directory layout (see phase 2).
2. Resolve the open ADR question on **skill sync mechanism**. Pick one of:
   - **(A)** Skills live in MemFS under `shared/.skills/` and `<agent>/.skills/`. Distribution is via git.
   - **(B)** Den owns skill installation via Letta API; skills are not in MemFS. Agent-driven `/skill` learning writes go through a Den webhook.
   - Recommendation: (A), because it makes skill propagation a side effect of memory sync we're already paying for. Disable agent-driven skill writes outside `<own-branch>/.skills/`.
3. Define the Bear data model in Den's database:
   ```
   bears
     id (uuid)
     name
     created_at
     memfs_repo_path
     provisioning_version (int — bumped on prompt/skill/tool roster changes)

   bear_agents
     bear_id (fk)
     role (chat | control | dream)
     letta_agent_id
     last_provisioned_version
     last_synced_at
   ```

### Acceptance

- Spec doc reviewed by team and merged to main.
- Skill sync mechanism decided and documented.
- Database migration written but not yet applied.

---

## Phase 1 — Den data model and provisioning API

**Goal:** Den knows what a Bear is and can talk about its three constituent agents.

### Tasks

1. Apply the database migration from phase 0.
2. Add the following Den endpoints (or internal methods if Den is library-shaped, not service-shaped):
   - `create_bear(name) -> bear_id` — creates the bear record. Does not yet provision agents.
   - `provision_bear(bear_id)` — creates/updates the three Letta agents to match the current canonical config.
   - `reconcile_bear(bear_id)` — checks each agent's actual tool/prompt/skill state against canonical, reports drift. Idempotent fix-up.
   - `get_bear(bear_id)` — returns the bear with its three agent IDs and roles.
3. Write integration tests for these endpoints against a test Letta server.

### Acceptance

- `create_bear` followed by `provision_bear` produces three agents on the Letta server with the correct tags (`bear:<id>:role:chat`, etc.) and tool profiles.
- `reconcile_bear` returns no drift immediately after provisioning.
- `reconcile_bear` detects and corrects drift after manually mutating an agent's tool roster via the Letta API.

---

## Phase 2 — MemFS layout and enforcement

**Goal:** the bare repo per Bear has the right structure, and the path-per-agent invariant is enforced server-side.

### Tasks

1. Define the canonical layout. For each Bear, the bare repo has these branches:
   - `chat` — writable by chat agent. Contents: `chat/` directory only (plus optionally `.skills/` if skill sync mechanism A was chosen).
   - `control` — writable by control agent. Contents: `control/` directory only.
   - `dream` — writable by dream agent. Contents: `dream/` directory and `shared/` directory.
2. Write a `pre-receive` hook for the bare repo that:
   - Identifies the branch being pushed.
   - Walks the changed file list.
   - Rejects the push if any changed path is outside that branch's allowed paths. Error message must be clear (e.g., "branch 'chat' attempted to write to 'shared/notes.md'; only 'chat/' paths are allowed").
3. Write an initialization script `den/scripts/init_bear_repo.sh <bear_id>` that:
   - Creates the bare repo at the configured path.
   - Installs the `pre-receive` hook.
   - Creates the three branches with appropriate empty directory structure (a `.gitkeep` in each).
4. Add a "memfs sidecar healthcheck" to Den's existing health endpoints — confirms the sidecar is reachable and can serve at least one repo.

### Acceptance

- Initializing a new Bear produces a bare repo with all three branches and the hook installed.
- Manual test: clone the `chat` branch, add a file under `shared/`, push — push is rejected with the clear error.
- Manual test: clone the `dream` branch, add a file under `shared/`, push — push succeeds.
- Sidecar healthcheck passes.

---

## Phase 3 — Agent provisioning logic

**Goal:** `provision_bear` and `reconcile_bear` correctly configure each of the three agents.

### Tasks

1. Implement provisioning per role. For each role:
   - Create the Letta agent with the role's tool profile, system prompt, and `git-memory-enabled` tag.
   - Configure MemFS to point at the Bear's bare repo on the correct branch.
   - Apply the role's skill roster (mechanism depends on phase 0 decision).
   - Tag with `bear:<id>` and `role:<role>` for discovery.
2. Implement drift detection in `reconcile_bear`:
   - Tool roster: list attached tools on the agent, diff against canonical.
   - System prompt: hash compare.
   - Skills: list (via filesystem read of `.skills/` if mechanism A, or via Den's installation log if mechanism B), diff against canonical.
   - MemFS branch: confirm the agent is configured against the right branch.
3. Implement drift correction. Order matters: detach removed tools before attaching new ones; update prompt last (so any in-flight turn finishes on the old config rather than mid-mutation).

### Acceptance

- `provision_bear` on a fresh Bear produces three correctly configured agents.
- `reconcile_bear` detects each kind of drift in isolation (tool added, tool removed, prompt edited, skill missing) and corrects it.
- Re-running `provision_bear` on an already-provisioned Bear is a no-op (idempotent).

---

## Phase 4 — Chat agent integration (Letta Code path)

**Goal:** Slack and web chat route to the Bear's chat agent via Letta Code, with no concurrency regressions.

### Tasks

1. Update Den's Slack/web-chat router to look up the chat agent ID from `bear_agents` rather than the legacy single agent ID.
2. Configure the Letta Code harness for each chat agent:
   - Working directory: per-Bear path (existing convention).
   - MemFS auto-clone: confirm Letta Code clones the `chat` branch on startup.
   - Push-on-commit: configure the harness to push immediately after committing, not on the periodic reminder. Verify by triggering a memory edit and confirming the push happens within seconds.
3. Update the conversation lifecycle: each new chat session creates a Letta Conversation against the chat agent (existing pattern; just confirm it works against the new agent ID).
4. Confirm tool execution path: tools execute on the harness server (existing behavior) and don't interact with `control/` or `dream/` paths in MemFS.

### Acceptance

- A Slack message routes correctly to the chat agent and gets a response.
- Sending two Slack messages back-to-back to the same Bear produces correctly serialized responses (existing per-agent sequential guarantee holds).
- A memory edit during a chat turn results in a push to the bare repo's `chat` branch within a few seconds.
- The push contains only `chat/` paths (the `pre-receive` hook would have rejected otherwise).

---

## Phase 5 — Control agent (ACP adapter)

**Goal:** IDEs can connect to the control agent over ACP. No Letta Code in this path.

### Tasks

1. Create a new service `den-acp` (or module within Den) that implements the ACP server side. References:
   - <https://agentclientprotocol.com/get-started/introduction>
   - <https://github.com/zed-industries/agent-client-protocol>
2. Implement the required ACP methods. Minimum viable set:
   - `initialize` — protocol version negotiation, capability advertisement.
   - `session/new` — creates a Letta Conversation against the control agent. Returns the session ID.
   - `session/prompt` — calls `client.agents.messages.stream(agentId, { conversationId, input })` on the Letta API. Translates the streamed reasoning, messages, and tool calls into ACP `session/update` notifications.
   - `session/cancel` — issues cancel against the Letta stream.
   - `session/load` — resumes an existing Letta Conversation (Letta supports background streaming so this should work cleanly).
3. Implement tool forwarding. When the Letta agent emits a client-side tool call:
   - Translate to ACP `session/update` of kind `tool_call` (status: pending).
   - Translate to ACP `session/request_permission` for the IDE to approve/reject.
   - On approval, the IDE executes the tool; the IDE's response comes back as an ACP message.
   - Forward the result back to Letta as the tool result, allowing the agent to continue.
   - On rejection, emit the appropriate refusal back to Letta.
4. Authenticate against the Letta server using a service token bound to the Bear (so the ACP adapter can only see/touch agents in its scope).
5. Background streaming: use Letta's background mode so a dropped ACP connection doesn't kill the agent's in-progress work. On reconnect, resume the stream.
6. Logging and tracing: every ACP session should emit structured logs tagged with the bear ID, control agent ID, and ACP session ID.

### Acceptance

- Zed's `dev: open acp logs` shows a clean handshake with the den-acp service.
- A simple prompt that doesn't invoke tools round-trips correctly: prompt in Zed, response streams back.
- A prompt that invokes a tool round-trips: tool call appears in Zed, permission prompt shows, on approval the tool runs in the IDE, result returns to the agent, agent continues.
- Disconnecting Zed mid-stream and reconnecting (`session/load`) resumes the in-progress turn.
- Two concurrent Zed sessions to the same control agent are correctly serialized by the agent's sequential processing — test that they don't interleave tool calls.

---

## Phase 6 — Dream agent and orchestration

**Goal:** dream agent runs reflection and integrates learnings to `shared/`.

### Tasks

1. Implement the dream agent's tool profile: read access via filesystem to all branches, write access only to its own branch and `shared/`. Reuse Letta's existing reflection and defragmentation skills.
2. Implement the dreamer's worktree setup. The dreamer needs to *read* `chat/` and `control/` content. Two options:
   - Mount additional read-only worktrees for the chat and control branches alongside the dream branch in the same agent's filesystem. The dreamer then has paths like `/work/dream-branch/`, `/work/chat-readonly/`, `/work/control-readonly/`.
   - Have the dreamer fetch each branch via git and read from `git show <branch>:<path>`.
   - Recommendation: the first option. Simpler reasoning for the agent.
3. Implement dream cycle triggering in Den:
   - Idle trigger: after N minutes of no activity on chat or control agents.
   - Volume trigger: after M new messages across chat and control since last dream cycle.
   - Manual trigger: an `admin/trigger_dream/<bear_id>` endpoint for testing.
4. Implement dream cycle execution:
   - Den sends a structured prompt to the dream agent: "Review new content in chat/ and control/ since last cycle. Integrate durable learnings into shared/."
   - Dream agent reads, decides what to promote, writes to `dream/` and `shared/`, commits, pushes.
   - Den records the cycle (start time, end time, branches' commit SHAs at start, agents involved).
5. Ensure dream cycles don't run concurrently for the same Bear (Den-level lock).

### Acceptance

- Triggering a dream cycle on a test Bear produces commits to `dream` branch touching `dream/` and `shared/`, with a clear commit message.
- After a dream cycle, the next system prompt construction on the chat agent reflects content from `shared/` (verify by inspecting the agent's loaded context).
- Two near-simultaneous dream triggers result in only one cycle running; the second is rejected or queued.
- A failure mid-cycle leaves the repo in a clean state (the worktree-local commits are recoverable, and the `shared/` content reflects either the prior or new state, never partial).

---

## Phase 7 — Management UI updates

**Goal:** the Den UI surfaces conversations as agent-locked but presents Bears as coherent entities.

### Tasks

1. Bear-level view: lists the three agents with status, last activity, last dream cycle, last reconcile.
2. Conversations view: scoped per agent. A Bear's conversations across all three agents can be aggregated into a unified timeline, but each conversation entry must clearly show which agent it belongs to.
3. MemFS browser: read-only view of each branch's `<role>/` and `shared/` content. Useful for debugging.
4. Drift indicator: surface `reconcile_bear` results with an actionable button to fix.
5. Dream cycle log: history of cycles per Bear with timing and what was promoted.

### Acceptance

- Stakeholder review of the updated UI confirms the agent-locked nature is clear and not confusing.
- A user can answer "what did Slack-me teach the Bear last week?" by looking at the chat agent's conversations.
- A user can answer "what does the Bear durably know about me?" by reading `shared/`.

---

## Phase 8 — Migration of existing Bears

**Goal:** existing single-agent Bears are converted to triplets without data loss.

### Tasks

1. Write a migration script `den/scripts/migrate_bear.py <legacy_bear_id>`:
   - Read the legacy Bear's MemFS content.
   - Initialize the new bare repo with the three-branch layout.
   - Write the legacy memory content into `shared/` on the `dream` branch (treating all existing memory as "already promoted").
   - Provision the three new agents.
   - Create new `bear_agents` records pointing to the new triplet.
   - Mark the legacy agent as deprecated (do not delete; we want rollback).
2. Test the migration on a cloned production Bear in a staging environment. Verify:
   - The chat agent has access to the migrated content via `shared/`.
   - The control agent likewise.
   - Conversation history from the legacy agent is **not** carried over (this is the agent-locked tradeoff; document in user-facing release notes).
3. Define a rollback path: if a migrated Bear misbehaves, point Den's router back at the legacy agent until investigation completes.

### Acceptance

- A staging Bear migrates successfully and serves both Slack and ACP traffic correctly.
- Rollback works: pointing back at the legacy agent restores prior behavior.
- Migration is idempotent: re-running on an already-migrated Bear is a no-op.

---

## Phase 9 — Cutover and monitoring

**Goal:** production Bears are migrated; observability confirms healthy operation.

### Tasks

1. Migrate Bears in waves. Suggested order: internal/test users first, then small external groups, then full rollout. At each wave, hold for at least 48 hours of monitoring before proceeding.
2. Monitoring to put in place before wave 1:
   - **Drift detection:** scheduled `reconcile_bear` run every hour per Bear; alert if drift detected.
   - **Sidecar health:** alert on healthcheck failures.
   - **Per-receive rejections:** alert on any rejected push (indicates an agent or harness misbehaving).
   - **Dream cycle success rate:** alert if cycles fail repeatedly for the same Bear.
   - **ACP error rate:** alert if `den-acp` returns errors above a threshold.
   - **Concurrent-request anomalies:** log every Letta API call with conversation ID; alert on overlapping calls to the same agent (would indicate a regression of the original concurrency bug).
3. Document operational runbooks: how to manually fix drift, how to recover from a failed dream cycle, how to roll back a Bear, how to inspect MemFS state for debugging.

### Acceptance

- All production Bears migrated.
- Two weeks of clean monitoring with no concurrency-related incidents and drift detection consistently reporting clean.
- Runbooks merged and accessible to on-call.

---

## Risks and mitigations summary

| Risk | Mitigation |
|---|---|
| Drift between agents in a Bear | Hourly `reconcile_bear`, alerting, fix-up tooling. |
| Agent self-modifies tools/skills out-of-band | Agents do not have tools to attach/detach tools or install skills outside their branch's `.skills/`. Confirm in tool roster review (phase 0). |
| Concurrent commits across worktrees | Branch-per-agent + path-per-agent enforced by `pre-receive`. |
| Dream cycle conflicts with user load | Idle-triggered by default; volume-triggered with backoff during active hours. |
| Conversation history fragmentation confuses users | UI work in phase 7 + explicit release notes. |
| Letta upstream changes break our assumptions | Pin Letta and Letta Code versions; subscribe to changelog; integration tests in CI against pinned versions. |

## What is explicitly out of scope

- Real-time cross-channel coherence ("user says X in Slack, IDE agent immediately knows"). Cross-channel transfer is dream-mediated and eventually consistent.
- Sharing conversation history across the three agents within a Bear. Distillation only.
- Migration to a future Letta-native multi-tenant tool execution model. Will be its own ADR if/when that lands upstream.
