# Implementation Plan: BEARS Multi-Agent Architecture

This plan implements the architecture described in the `multi-agent-architecture` ADR. The task-queue specifics referenced in phases 4–8 are detailed in `tasks-schema.md`.

Each phase has explicit acceptance criteria. Phases are ordered for safe incremental rollout. Phase 10 (migration) only runs after phases 1–9 have been validated on a test Bear.

## Glossary (read first)

- **Bear** — a logical agent identity. Implemented as a quartet of Letta agents (talk, pair, curate, work) sharing memory.
- **Den** — our existing control plane service. Provisions and manages Bears.
- **MemFS** — Letta's git-backed memory filesystem. Stored as a bare repo per Bear; agents access it via worktrees.
- **Sidecar** — the git smart HTTP service that fronts the bare repos. Configured via `LETTA_MEMFS_SERVICE_URL`.
- **Channel agent** — talk or pair. The two user-facing agents.
- **Curate agent** — the integrator. Reflects on memory, approves task intents, promotes results to `core/`.
- **Work agent** — the executor. Runs approved external-action tasks.
- **Worktree** — a git working directory tied to one branch. Each agent gets one worktree per Bear.

## Prerequisites

- Den service is running and able to talk to the Letta server.
- Letta server has MemFS support enabled (`LETTA_MEMFS_SERVICE_URL` set to the sidecar URL or `local`).
- One existing Bear available for end-to-end testing. Do not use a Bear with real user data until phase 10.

---

## Phase 0 — Specification freeze

**Goal:** lock down the canonical Bear configuration before any code changes.

### Tasks

1. Write a spec doc (`bear-spec.md` in the Den repo) covering:
   - Tool roster for each of the four agent types. List every tool by name, server-side vs client-side, and which agent it's attached to.
   - System prompt template. Decide whether the four agents share a base prompt with role-specific suffixes, or have fully distinct prompts. Recommendation: shared base, distinct role suffixes.
   - Skill roster: which skills go to which agents. Coding skills are talk and pair; reflection skills are curate; integration tools (HTTP clients, posting tools, etc.) are work; some user-preference skills go to all four.
   - MemFS directory layout (see phase 2).
2. Resolve the open ADR question on **skill sync mechanism**. Pick one of:
   - **(A)** Skills live in MemFS under `core/.skills/` and `<agent>/.skills/`. Distribution is via git.
   - **(B)** Den owns skill installation via Letta API; skills are not in MemFS. Agent-driven `/skill` learning writes go through a Den webhook.
   - Recommendation: (A), because it makes skill propagation a side effect of memory sync we're already paying for. Disable agent-driven skill writes outside `<own-branch>/.skills/`.
3. Define the Bear data model in Den's database:
   ```
   bears
     id (uuid)
     name
     created_at
     memfs_repo_path
     provisioning_version (int — bumped on prompt/tool/runtime changes; skills deferred)

   bear_agents
     bear_id (fk)
     role (talk | pair | curate | work)
     letta_agent_id (nullable until provisioning succeeds)
     provisioning_status (pending | provisioning | ready | drifted | failed)
     last_provisioned_version
     last_synced_at
     last_provisioning_error
     config_hash
   ```
4. Read `tasks-schema.md` and confirm tool requirements derived from it (e.g., channel agents need a privileged Den tool that can write structured intent files; the curate agent needs privileged Den tools for approval/rejection and validated writes to `core/tasks/`; the work agent needs structured input handling and scoped result-writing tools for task definitions).

### Acceptance

- Spec doc reviewed by team and merged to main.
- Skill sync mechanism scope decided for this phase: direct out-of-band skill installation is denied, while detailed skill storage/runtime behavior is deferred to the dedicated skills discussion/design.
- Database migration written but not yet applied.
- Tool requirements from the tasks schema reviewed and folded into the per-agent tool roster.

---

## Phase 1 — Den data model and provisioning API

**Goal:** Den knows what a Bear is and can talk about its four constituent agents.

### Tasks

1. Apply the database migration from phase 0.
2. Add the following Den endpoints (or internal methods if Den is library-shaped, not service-shaped):
   - `create_bear(name) -> bear_id` — creates the bear record. Does not yet provision agents.
   - `provision_bear(bear_id)` — creates/updates the four Letta agents to match the current canonical config.
   - `reconcile_bear(bear_id)` — checks each agent's actual tool/prompt/skill state against canonical, reports drift. Idempotent fix-up.
   - `get_bear(bear_id)` — returns the bear with its four agent IDs and roles.
3. Write integration tests for these endpoints against a test Letta server.

### Acceptance

- `create_bear` followed by `provision_bear` creates or updates four `bear_agents` rows and produces four agents on the Letta server with the correct tags (`bear:<id>`, `role:<role>`, `bear:<id>:role:talk`, `bear:<id>:role:pair`, `bear:<id>:role:curate`, `bear:<id>:role:work`, `git-memory-enabled`) and tool profiles.
- `reconcile_bear` returns no drift immediately after provisioning.
- `reconcile_bear` detects and corrects drift after manually mutating an agent's tool roster via the Letta API.

---

## Phase 2 — MemFS layout and enforcement

**Goal:** the bare repo per Bear has the right structure, and the path-per-agent invariant is enforced server-side.

### Tasks

1. Define the canonical layout. For each Bear, the bare repo has these branches:
   - `talk` — writable by talk agent. Contents: `talk/` directory, including `talk/tasks/` for intent files.
   - `pair` — writable by pair agent. Contents: `pair/` directory, including `pair/tasks/` for intent files.
   - `curate` — writable by curate agent. Contents: `curate/` directory and `core/` directory (which contains `core/tasks/` and `core/results/`).
   - `work` — writable by work agent. Contents: `work/` directory, including `work/results/<task-id>/<run-id>.md`.
   - `core/` is curated shared state. The implementation must document how `core/` is materialized read-only for `talk`, `pair`, and `work` before Phase 2 acceptance (for example read-only worktree/mount, Den-managed prompt/context copy, or Den-managed merge strategy).
2. Write a `pre-receive` hook for the bare repo that:
   - Identifies the branch being pushed.
   - Walks the changed file list.
   - Rejects the push if any changed path is outside that branch's allowed paths. Error message must be clear (e.g., "branch 'talk' attempted to write to 'core/notes.md'; only 'talk/' paths are allowed").
   - Preserves the invariant that privileged Den tools, not raw agent pushes, perform approved cross-branch audit updates such as task intent approval/rejection metadata.
3. Write an initialization script `den/scripts/init_bear_repo.sh <bear_id>` that:
   - Creates the bare repo at the configured path.
   - Installs the `pre-receive` hook.
   - Creates the four branches with appropriate empty directory structure (a `.gitkeep` in each, plus the `tasks/` and `results/` subdirectories where applicable).
4. Add a "memfs sidecar healthcheck" to Den's existing health endpoints — confirms the sidecar is reachable and can serve at least one repo.

### Acceptance

- Initializing a new Bear produces a bare repo with all four branches and the hook installed.
- Manual test: clone the `talk` branch, add a file under `core/`, push — push is rejected with the clear error.
- Manual test: clone the `curate` branch, add a file under `core/`, push — push succeeds.
- Manual test: clone the `work` branch, add a file under `core/`, push — push is rejected.
- Sidecar healthcheck passes.
- The active `core/` materialization strategy is documented and does not give `talk`, `pair`, or `work` raw push authority over `core/`.

---

## Phase 3 — Agent provisioning logic

**Goal:** `provision_bear` and `reconcile_bear` correctly configure each of the four agents.

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

- `provision_bear` on a fresh Bear produces four correctly configured agents.
- `reconcile_bear` detects each implemented kind of drift in isolation (tool added, tool removed, prompt edited, runtime policy/config hash mismatch) and corrects it. Skill drift detection is deferred until the skills design is finalized.
- Re-running `provision_bear` on an already-provisioned Bear is a no-op (idempotent).

---

## Phase 4 — Talk agent integration (Letta Code path)

**Goal:** Slack and web chat route to the Bear's talk agent via Letta Code, with no concurrency regressions and with the ability to write task intents.

### Tasks

1. Update Den's Slack/web-chat router to look up the talk agent ID from `bear_agents` rather than the legacy single agent ID.
2. Configure the Letta Code harness for each talk agent:
   - Working directory: per-Bear path (existing convention).
   - MemFS auto-clone: confirm Letta Code clones the `talk` branch on startup.
   - Push-on-commit: configure the harness to push immediately after committing, not on the periodic reminder. Verify by triggering a memory edit and confirming the push happens within seconds.
3. Update the conversation lifecycle: each new chat session creates a Letta Conversation against the talk agent.
4. Verify the talk agent has access to the privileged Den tool needed to write task intents under `talk/tasks/` per [`../architecture/tasks-schema.md`](../architecture/tasks-schema.md). The tool should validate the intent against the schema before writing.
5. Confirm tool execution path: tools execute on the harness server (existing behavior) and don't interact with `pair/`, `curate/`, `work/`, or `core/` paths in MemFS.

### Acceptance

- A Slack message routes correctly to the talk agent and gets a response.
- Sending two Slack messages back-to-back to the same Bear produces correctly serialized responses (existing per-agent sequential guarantee holds).
- A memory edit during a chat turn results in a push to the bare repo's `talk` branch within a few seconds.
- The push contains only `talk/` paths.
- A user can request a scheduled task ("check the deploy API every morning") and the agent writes a valid intent file to `talk/tasks/`. The intent file passes schema validation.

---

## Phase 5 — Pair agent (ACP adapter)

**Goal:** IDEs can connect to the pair agent over ACP. No Letta Code in this path. Pair agent can also write task intents.

### Tasks

1. Create a new service `den-acp` (or module within Den) that implements the ACP server side. References:
   - <https://agentclientprotocol.com/get-started/introduction>
   - <https://github.com/zed-industries/agent-client-protocol>
2. Implement the required ACP methods. Minimum viable set:
   - `initialize` — protocol version negotiation, capability advertisement.
   - `session/new` — creates a Letta Conversation against the pair agent. Returns the session ID.
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
6. Logging and tracing: every ACP session should emit structured logs tagged with the bear ID, pair agent ID, and ACP session ID.
7. Verify the pair agent has the tool needed to write task intents under `pair/tasks/` per the schema. Same validation as talk agent.

### Acceptance

- Zed's `dev: open acp logs` shows a clean handshake with the den-acp service.
- A simple prompt that doesn't invoke tools round-trips correctly: prompt in Zed, response streams back.
- A prompt that invokes a tool round-trips: tool call appears in Zed, permission prompt shows, on approval the tool runs in the IDE, result returns to the agent, agent continues.
- Disconnecting Zed mid-stream and reconnecting (`session/load`) resumes the in-progress turn.
- Two concurrent Zed sessions to the same pair agent are correctly serialized by the agent's sequential processing.
- A user can request a scheduled task in the IDE and the agent writes a valid intent file to `pair/tasks/`.

---

## Phase 6 — Curate agent and orchestration

**Goal:** curate agent runs reflection, integrates learnings, reviews task intents, and promotes work results.

### Tasks

1. Implement the curate agent's tool profile per `bear-spec.md`. It needs read access to all branches (via additional read-only worktrees) and write access to its own branch and `core/`. Reuse Letta's existing reflection and defragmentation skills.
2. Implement the curate agent's worktree setup. The curate agent reads from `talk/`, `pair/`, and `work/` and writes to `curate/` and `core/`. Mount additional read-only worktrees for the other branches alongside the curate worktree:
   ```
   /work/curate-branch/      (read-write, current branch: curate)
   /work/talk-readonly/      (read-only, branch: talk)
   /work/pair-readonly/      (read-only, branch: pair)
   /work/work-readonly/      (read-only, branch: work)
   ```
3. Implement curate cycle triggering in Den:
   - Idle trigger: after N minutes of no activity on talk or pair agents.
   - Volume trigger: after M new messages across talk and pair since last cycle.
   - Pending intent trigger: when one or more files in `talk/tasks/` or `pair/tasks/` have status `pending_review`.
   - Pending result trigger: when new files have appeared in `work/results/` since last cycle.
   - Manual trigger: an `admin/trigger_curate/<bear_id>` endpoint for testing.
4. Implement curate cycle execution. The cycle has three responsibilities, executed in this order:
   - **Memory integration:** review new content in `talk/` and `pair/` since last cycle. Promote durable learnings to `core/` through allowed curate writes or privileged Den tools.
   - **Task intent review:** for each pending intent file, decide approve or reject. If approved, call privileged Den tooling to validate and write a corresponding file to `core/tasks/<task-id>.md` per the schema and update the source intent audit metadata. If rejected, call privileged Den tooling to update the source intent with rejection metadata. The curate agent must not receive raw write access to `talk/` or `pair/`.
   - **Result promotion:** for each new file in `work/results/`, decide whether to surface a summary to channel agents. If yes, write a summary to `core/results/<task-id>.md` through allowed curate writes or privileged Den tools.
5. Den records the cycle (start time, end time, branches' commit SHAs at start, what was integrated, what intents were approved/rejected, what results were promoted).
6. Ensure curate cycles don't run concurrently for the same Bear (Den-level lock).

### Acceptance

- Triggering a curate cycle on a test Bear produces commits to the `curate` branch touching `curate/` and `core/`, with a clear commit message.
- After a curate cycle, the next system prompt construction on the talk agent reflects content from `core/` (verify by inspecting the agent's loaded context).
- A pending intent file in `talk/tasks/` is reviewed during the next cycle; either an approval lands in `core/tasks/` and the source intent audit metadata is updated by Den, or a rejection audit update is written by Den to the source intent. The curate agent never raw-writes the channel branch.
- A new result file in `work/results/` produces a summary in `core/results/` after the next cycle.
- Two near-simultaneous curate triggers result in only one cycle running; the second is rejected or queued.
- A failure mid-cycle leaves the repo in a clean state.

---

## Phase 7 — Task queue infrastructure (Den-side)

**Goal:** Den can read approved tasks from `core/tasks/`, schedule them, dispatch to the work agent, and record results.

### Tasks

1. Implement a task index in Den's database, populated by polling `core/tasks/` (or, better, reacting to `post-receive` hooks on the bare repo when curate pushes):
   ```
   bear_tasks
     bear_id (fk)
     task_id (matches frontmatter id)
     file_path
     status (active | paused | completed | failed)
     type (oneshot | scheduled | event_triggered)
     schedule (nullable cron string)
     risk (low | high)
     last_run_at
     next_run_at
     created_at
   ```
2. Implement the scheduler:
   - For `type: scheduled` tasks, parse the cron string and compute `next_run_at`. Use a job runner that polls and dispatches.
   - For `type: oneshot` tasks, dispatch immediately on first index, then mark `completed` after first successful run.
   - For `type: event_triggered`, expose a webhook endpoint that fires the task when called (out of scope for MVP; document the slot).
3. Implement the dispatcher. For each task run:
   - Generate a `run_id`.
   - If `risk: high`, enqueue in the human-in-the-loop approval queue. Block dispatch until approved or rejected. Surface in the management UI per phase 9.
   - On approval (or for `risk: low`), construct a structured prompt for the work agent containing the task definition, run ID, and any input parameters.
   - Send the prompt to the work agent via Letta API. Use a fresh Letta Conversation per run (to keep run histories isolated; the work agent's persistent memory still tracks higher-level patterns).
   - Wait for completion (with a timeout per task type). On completion, the work agent will have written `work/results/<task-id>/<run-id>.md`.
4. Implement rate limiting:
   - Per-Bear limit on outbound HTTP requests per minute.
   - Per-domain limit (alert on novel destinations).
   - Per-task-type limit (e.g., no more than N runs per hour for a given task).
5. Implement task lifecycle handling:
   - On schema-validation failure of a `core/tasks/` file, log and skip; alert if persistent.
   - On `expires_at` reached, mark task `completed` and stop scheduling.
   - On repeated failures (configurable threshold), pause the task and alert.

### Acceptance

- A `core/tasks/` file with `type: oneshot` and `risk: low` is dispatched to the work agent within one polling interval.
- A `type: scheduled` task with a cron string runs at the expected time.
- A `risk: high` task does not dispatch until the HITL queue has been approved.
- Rate limiting blocks excessive requests and logs the rejection.
- A task with `expires_at` in the past is not dispatched.

---

## Phase 8 — Work agent

**Goal:** the work agent executes dispatched tasks against external systems with appropriate sandboxing and observability.

### Tasks

1. Implement the work agent's tool profile per `bear-spec.md`. Includes HTTP client, third-party API tools (Slack post, GitHub, etc.), result writer. **No tools that read `talk/`, `pair/`, or `curate/` branches.** Den enforces this by attaching only the approved set.
2. Implement the work agent's MemFS configuration:
   - Worktree on the `work` branch.
   - Read-only mount of `core/` so the work agent can consult curated context (prefs, project info) when executing tasks.
3. Implement the work agent's input handling. Den dispatches a structured prompt of the form:
   ```
   You are executing task <task-id>, run <run-id>. Definition:
   <task-frontmatter>
   <task-body>

   Inputs (if any): <inputs>

   Write your final result to work/results/<task-id>/<run-id>.md per the schema.
   Use only the tools listed in `allowed_tools`.
   ```
4. Implement result writing. The work agent must:
   - Call privileged Den tooling to write a result file matching [`../architecture/tasks-schema.md`](../architecture/tasks-schema.md).
   - Commit and push through the validated result-writing path.
   - Return a short summary in its response to Den (so Den can log without re-reading the file).
5. Implement sandboxing. The work agent should run with:
   - Network egress filtered to allowed destinations (HTTP allowlist enforced at network layer, not just at agent layer).
   - No access to other Bears' MemFS repos (already enforced by sidecar auth, but verify).
   - Read-only on its own filesystem outside the worktree.
6. Implement observability:
   - Every HTTP call from the work agent is logged with destination, method, status code, byte counts.
   - Every result file is checksummed and indexed.
   - Anomalies (novel destinations, large response bodies, long execution time) raise alerts.

### Acceptance

- A dispatched low-risk task executes and produces a valid result file.
- A dispatched high-risk task only executes after HITL approval.
- An attempt to access a non-allowlisted destination fails at the network layer (verified by attempting to call a domain not in `allowed_tools` scope).
- An attempt by the work agent to read `talk/`, `pair/`, or `curate/` paths fails (no read-only mount of those, plus tool roster doesn't include readers for them).
- Result files validate against the schema.
- All external calls appear in logs with full metadata.

---

## Phase 8.5 — Documentation and operator UI: retire implicit 1:1 bear ↔ Letta agent

**Goal:** Every human-facing document, operator template, and user-visible error string reflects that a **bear** is a **logical assistant** (one coherent product identity) backed by **one or more Letta agents** with explicit **roles** (`talk` \| `pair` \| `curate` \| `work` per the [`multi-agent-architecture` ADR](../architecture/adr/multi-agent-architecture.md)). Legacy `bears.letta_agent_id` remains valid only as a **transitional** or **migration** concept until Phase 10 completes; it must not be the only story in UI or docs.

**Prerequisite:** `bear_agents` (or equivalent) exists and at least the **talk** row is populated for newly provisioned bears (Phases 0–3). Work can **start in parallel with Phase 4+** once that is true.

**Ordering:** Baseline doc + list/detail/harness template updates from this phase **must ship before Phase 11 cutover** so no production operator relies on obsolete 1:1 copy. **Phase 9** advanced views (unified timeline, MemFS browser, HITL) **assume** this phase has updated the **bear list**, **bear detail**, **create/link bear**, and **Letta Code harness** surfaces so they already speak in roles or show the quartet.

### Tasks — documentation

1. **`docs/planning/PLAN.md`** — Revise **Terminology** and any §1–§2 bullets that describe Den as storing a single `bear_id` ↔ `letta_agent_id` map or routing web/Slack to “the” Letta agent without naming **talk** (and ACP → **pair**, etc.).
2. **`docs/architecture/DEN_ARCHITECTURE.md`** — Harness binding, skills materialization paths, Den meta tools: document **per-role** Letta agent ids and which **surface** uses which role.
3. **`docs/architecture/ARCHITECTURE_NOTES.md`** — Stack diagram and component tables: one bear → **cluster** of Letta agents where the architecture is live.
4. **`docs/planning/PHASE1_BOOTSTRAP.md`**, **`PHASE1_DECISIONS.md`** — Public JSON stays **`bear_id`**-centric; internal and harness artifacts document **role → `letta_agent_id`**; clarify any transition where legacy `bears.letta_agent_id` mirrors **talk** only.
5. **`docs/planning/DEN_SPECIFIC_TOOLS_PLAN.md`**, **`docs/architecture/BEAR_CHANNEL_AND_ACP.md`**, repo-root **`FAQ.md`** (if present) — JSON examples and narratives: use **talk** (or explicit role) in payloads; ACP sections name **pair**.
6. **`services/den/migrations/README.md`** — When `bear_agents` lands, document it next to legacy `bears.letta_agent_id` semantics.
7. **`services/den/docs/`** (e.g. **`concepts-overview.md`**, **`src/web/ROUTES.md`**) — Provisioning and admin flows: no “one agent per bear” without qualification.
8. **`README.md`** (repo root) — If the one-liner implies a single Letta agent per bear, align wording with **logical bear** vs **Letta runtime identities**.

### Tasks — operator UI, templates, and copy

Audit and update so operators **never** see a bare `letta_agent_id` without **role** or **legacy / pre-migration** context (except deliberate migration tooling).

| Area | Indicative paths | Expected change |
|------|------------------|-----------------|
| Admin bear list | `services/den/src/web/templates/admin/bears/list.html` | Replace a single “Letta id” column with **roles** (e.g. talk / pair / curate / work), **partial** provisioning, or **legacy single-agent** badge. |
| Admin bear detail | `services/den/src/web/templates/admin/bears/detail.html` | Per-role Letta summary (or tabs); SSE / API hints must name **talk** agent, not a generic “the agent”. |
| Create / attach bear | `admin/bears/new.html`, `bear/new.html` | Copy for **attach existing Letta agent**: clarify **legacy single-agent link** vs **quartet** provision; forms may need role-specific attach later. |
| Letta Code harness admin | `admin/letta_code_harness.html` | Rows or copy must not assume one Letta row per bear without role. |
| Unlinked Letta agents | `admin/bears/unlinked_letta_agents.html` | Consider ids referenced only in **`bear_agents`** as linked; not only `bears.letta_agent_id`. |
| End-user bear pages | `bear/details.html`, `bear/memory.html`, `bear/edit_configuration.html` | Diagnostics: show **which** Letta agent is summarized (default **talk** for operator/e2e “bear health”). |

**Rust / API surfaces:** Audit `services/den/src/web/bear_management.rs`, `services/den/src/web/v1/mod.rs`, `services/den/src/api/acp.rs`, and related modules for template context and JSON field names — external docs and OpenAPI-style comments should match. Prefer structured logs with **`bear_id` + `role` + `letta_agent_id`**.

**Codepool:** `services/codepool/` — Document in README or inline types which **role** Den supplies for harness / Den-tool calls (typically **talk** until multi-role payloads exist).

### Acceptance

- A new engineer can read **only** updated docs and correctly explain why **Slack/web** and **ACP** may use **different** Letta agents for the same `bear_id`.
- Operator **bear list** and **bear detail** do not imply a single Letta agent unless labeled **legacy** or **pre-migration**.
- Checklist (manual or scripted grep): remaining UI that binds **only** `bears.letta_agent_id` without `bear_agents` / role is listed and tracked to zero before Phase 11.

---

## Phase 9 — Management UI updates

**Goal:** the Den UI surfaces conversations as agent-locked, makes the task lifecycle visible, and provides the HITL approval queue. **Depends on:** Phase **8.5** for list/detail/harness and doc baseline so Phase 9 is not built on 1:1 assumptions.

### Tasks

1. Bear-level view: lists the four agents with status, last activity, last curate cycle, last reconcile.
2. Conversations view: scoped per agent. A Bear's conversations across all four agents can be aggregated into a unified timeline, but each conversation entry must clearly show which agent it belongs to.
3. Tasks view: shows pending intents, approved tasks, paused/failed tasks, and recent run history. Allows manual approval / rejection of intents (override of curate decisions if needed) and manual triggering of `oneshot` tasks.
4. HITL approval queue: dedicated view for high-risk task runs awaiting approval. Each entry shows the task definition, the run-specific inputs, and approve/reject buttons. Approval should be cryptographically signed (so the audit log records who approved what).
5. MemFS browser: read-only view of each branch's content. Useful for debugging.
6. Drift indicator: surface `reconcile_bear` results with an actionable button to fix.
7. Curate cycle log: history of cycles per Bear with timing and what was integrated/approved/promoted.
8. Work agent activity: outbound request log, rate-limit status, alerting summary.

### Acceptance

- Stakeholder review of the updated UI confirms the agent-locked nature is clear and not confusing.
- A user can answer "what did Slack-me teach the Bear last week?" by looking at the talk agent's conversations.
- A user can answer "what does the Bear durably know about me?" by reading `core/`.
- A user can see all pending and approved tasks for a Bear in one place.
- A user can approve or reject a high-risk task run from the UI.

---

## Phase 10 — Migration of existing Bears

**Goal:** existing single-agent Bears are converted to quartets without data loss.

### Tasks

1. Write a migration script `den/scripts/migrate_bear.py <legacy_bear_id>`:
   - Read the legacy Bear's MemFS content.
   - Initialize the new bare repo with the four-branch layout.
   - Write the legacy memory content into `core/` on the `curate` branch (treating all existing memory as "already promoted").
   - Provision the four new agents.
   - Create new `bear_agents` records pointing to the new quartet.
   - Mark the legacy agent as deprecated (do not delete; we want rollback).
2. Test the migration on a cloned production Bear in a staging environment. Verify:
   - The talk agent has access to the migrated content via `core/`.
   - The pair agent likewise.
   - The work agent has read access to the migrated content via `core/`.
   - Conversation history from the legacy agent is **not** carried over (this is the agent-locked tradeoff; document in user-facing release notes).
3. Define a rollback path: if a migrated Bear misbehaves, point Den's router back at the legacy agent until investigation completes.

### Acceptance

- A staging Bear migrates successfully and serves Slack, ACP, and a simple low-risk task correctly.
- Rollback works: pointing back at the legacy agent restores prior behavior.
- Migration is idempotent: re-running on an already-migrated Bear is a no-op.

---

## Phase 11 — Cutover and monitoring

**Goal:** production Bears are migrated; observability confirms healthy operation.

### Tasks

1. Migrate Bears in waves. Suggested order: internal/test users first, then small external groups, then full rollout. At each wave, hold for at least 48 hours of monitoring before proceeding.
2. Monitoring to put in place before wave 1:
   - **Drift detection:** scheduled `reconcile_bear` run every hour per Bear; alert if drift detected.
   - **Sidecar health:** alert on healthcheck failures.
   - **Pre-receive rejections:** alert on any rejected push (indicates an agent or harness misbehaving).
   - **Curate cycle success rate:** alert if cycles fail repeatedly for the same Bear.
   - **ACP error rate:** alert if `den-acp` returns errors above a threshold.
   - **Concurrent-request anomalies:** log every Letta API call with conversation ID; alert on overlapping calls to the same agent (would indicate a regression of the original concurrency bug).
   - **Task queue health:** backlog size, dispatch latency, rate-limit rejection rate.
   - **Work agent egress:** novel destinations, unusual byte volumes, error spikes.
   - **HITL queue depth:** pending high-risk approvals per Bear.
3. Document operational runbooks: how to manually fix drift, how to recover from a failed curate cycle, how to roll back a Bear, how to inspect MemFS state for debugging, how to handle a stuck or failing task, how to revoke a misbehaving work-agent capability.

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
| Curate cycle conflicts with user load | Idle-triggered by default; volume-triggered with backoff during active hours. |
| Conversation history fragmentation confuses users | UI work in phase 9 + explicit release notes. |
| Letta upstream changes break our assumptions | Pin Letta and Letta Code versions; subscribe to changelog; integration tests in CI against pinned versions. |
| Work agent exfiltration via prompt injection | Trifecta split (work has no read access to channel branches); network egress allowlist; HITL on high-risk tasks; rate limiting. |
| Task intent backlog | Curate trigger on pending intents; backlog size alert; manual override in UI. |

## What is explicitly out of scope

- Real-time cross-channel coherence ("user says X in Slack, IDE agent immediately knows"). Cross-channel transfer is curate-mediated and eventually consistent.
- Sharing conversation history across the agents within a Bear. Distillation only.
- Synchronous external work execution. Tasks are inherently async; users requesting work are told to expect a delay until the next curate cycle approves.
- Migration to a future Letta-native multi-tenant tool execution model. Will be its own ADR if/when that lands upstream.
