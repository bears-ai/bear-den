# BEARS Multi-Agent Bear Spec

This is the phase-0 implementation spec for [`../../../docs/architecture/adr/multi-agent-architecture.md`](../../../docs/architecture/adr/multi-agent-architecture.md) and [`../../../docs/planning/MULTI_AGENT_IMPLEMENTATION_PLAN.md`](../../../docs/planning/MULTI_AGENT_IMPLEMENTATION_PLAN.md). It freezes the canonical Den-owned configuration before provisioning code creates the four runtime agents for a Bear.

## Scope

A Bear is a logical assistant backed by four Letta agents with fixed roles:

| Role | Surface | Letta path | Memory branch | Purpose |
|------|---------|------------|---------------|---------|
| `talk` | Slack, web chat, future chat channels | Letta Code harness | `talk` | Synchronous conversation and task-intent authoring. |
| `pair` | ACP clients such as IDEs | Den ACP adapter to Letta API | `pair` | Synchronous client-mediated collaboration and task-intent authoring. |
| `curate` | Den scheduler only | Letta API | `curate` | Reflection, memory integration, task approval, result promotion. |
| `work` | Den task dispatcher only | Letta API | `work` | Scheduled or event-triggered external work execution. |

Legacy `bears.letta_agent_id` remains a migration compatibility field. During rollout Den sets it to the `talk` agent id for newly provisioned multi-agent Bears and for migrated Bears once a `talk` role exists. New code must resolve agents through `bear_agents(role, letta_agent_id)` and must never write a non-`talk` role into `bears.letta_agent_id`.

## Prompt Model

The canonical prompt shape is a shared base prompt plus a role-specific suffix. Den owns the templates and increments `bears.provisioning_version` whenever the base prompt, any role suffix, the canonical tool roster, or provisioning-relevant MemFS/runtime configuration changes.

Den tracks prompt/tool drift using these canonical artifacts:

- `base_prompt_hash` — hash of the shared base prompt template after rendering global variables.
- `role_prompt_hash` — hash of the role-specific suffix after rendering role variables.
- `rendered_prompt_hash` — hash of the final prompt sent to Letta for the specific Bear and role.
- `tool_roster_hash` — hash of the canonical tool names and role policy before resolving environment-specific Letta tool ids.
- `runtime_policy_hash` — hash of MemFS branch/path policy and other non-skill runtime constraints.

Skill roster hashing is intentionally deferred until the separate skills design is finalized.

The shared base prompt must state:

- The assistant identity is the Bear, not the individual Letta agent.
- The agent has one fixed role and must not claim capabilities outside that role.
- Durable cross-role knowledge lives in `core/` and is eventually consistent.
- Secrets must never be written into MemFS task, result, skill, or note files.
- External work is asynchronous and must flow through structured task intents unless the current role is `work` executing an approved task.
- Privileged cross-branch operations are performed by Den tools, not by raw agent writes.

Role suffix requirements:

| Role | Suffix requirements |
|------|---------------------|
| `talk` | Treat Slack/web chat as conversational only. Read `core/` and `talk/`; write only `talk/`. Use `write_task_intent` for external-effect requests. Do not access pair, curate, or work branches. |
| `pair` | Treat ACP tools as client-side, user-gated tools. Read `core/` and `pair/`; write only `pair/`. Use `write_task_intent` for external-effect requests. Do not access talk, curate, or work branches. |
| `curate` | Read all branches. Write directly only to `curate/` and `core/`. Promote durable knowledge to `core/`, approve or reject task intents through privileged Den tools, and promote work summaries. No external communication tools are allowed. |
| `work` | Read only `core/`, the dispatched task definition, and `work/`. Write only `work/`. Use only tools named in the approved task's `allowed_tools`. Never read talk, pair, or curate branches. |

## Letta Agent Tags

Every role agent Den provisions must have these tags:

| Tag | Purpose |
|-----|---------|
| `bear:<bear_id>` | Groups all role agents for one logical Bear. |
| `role:<role>` | Supports role filtering independent of Bear id. |
| `bear:<bear_id>:role:<role>` | Stable reconciliation/discovery tag required by Phase 1 acceptance. |
| `git-memory-enabled` | Marks the agent as using git-backed MemFS memory. |

Den reconciliation uses the database as the source of truth first and Letta tags as secondary discovery/adoption evidence. Tagged Letta agents that are not represented in `bear_agents` must be reported by `reconcile_bear`; adoption vs replacement is an explicit operator decision unless the agent unambiguously matches a missing `(bear_id, role)` row.

## Skill Sync Decision

Skills are tracked separately from this revision of the spec and will be finalized before skill implementation begins. The current architecture still assumes skills are not installed out-of-band by agents.

Current constraints that are already frozen:

- Agents may not attach Letta tools or install out-of-band Letta skills directly.
- Skill-related writes, if enabled, must be constrained to the agent's writable branch or mediated by Den.
- Promotion of reusable skills or preferences to a shared location is a curate/Den responsibility.
- Skill roster hashing and drift details are deferred until the skills design is finalized.

## Tool Roster

Tool ids are environment-specific Letta ids. Den stores canonical tool names in code/config and resolves them to Letta ids during provisioning with the existing tool catalog API.

| Tool | Server-side or client-side | Roles | Requirement |
|------|----------------------------|-------|-------------|
| `memfs_read` | server-side | `talk`, `pair`, `curate`, `work` | Reads only paths allowed for the role. `curate` may read all branches; `work` may not read channel branches. |
| `memfs_write` | server-side | `talk`, `pair`, `curate`, `work` | Writes only paths allowed for the role and commits locally. Raw writes to task/result paths should be blocked in favor of schema tools. |
| `memfs_commit_push` | server-side | `talk`, `pair`, `curate`, `work` | Pushes immediately after committed memory changes. |
| `write_task_intent` | privileged Den tool | `talk`, `pair` | Validates `talk/tasks/<intent-id>.md` or `pair/tasks/<intent-id>.md` against [`../../../docs/architecture/tasks-schema.md`](../../../docs/architecture/tasks-schema.md) before writing. |
| `approve_task_intent` | privileged Den tool | `curate` | Takes a curate decision, validates it, writes `core/tasks/<task-id>.md`, and updates the source intent audit metadata without granting the curate agent raw write access to channel branches. |
| `reject_task_intent` | privileged Den tool | `curate` | Takes a curate decision and updates the source intent to `rejected` with `reviewed_by`, `reviewed_at`, and `rejection_reason` without granting raw channel-branch writes. |
| `write_core_result_summary` | privileged Den tool | `curate` | Writes reviewed summaries under `core/results/`. |
| `write_run_result` | privileged Den tool | `work` | Validates and writes `work/results/<task-id>/<run-id>.md`, commits, and pushes. |
| `reflection` / `defragmentation` | server-side | `curate` | Letta reflection and memory maintenance. No external communication capability. |
| ACP client tool relay | client-side | `pair` | Forwarded through ACP `session/request_permission`; Den does not execute these tools server-side. |
| Integration tools (`http_get`, `slack_post`, `github_*`, etc.) | privileged Den tools / server-side policy wrappers | `work` only | Runtime use must be within the approved task's `allowed_tools` and `scope`, verified by Den-issued run context and tool-side policy checks. |

Denied by default for all roles:

- Tools that attach/detach Letta tools or mutate Letta agent configuration.
- Tools that install out-of-band Letta skills.
- Network egress tools on `talk`, `pair`, or `curate`, except the conversational surface itself and ACP client-mediated tool calls.
- Raw filesystem tools that bypass MemFS path validation.

### Privileged Den Tool Principle

Agents do not receive broad authority just because a task requires a cross-branch or external effect. Instead, Den exposes narrow privileged tools with schema validation, authorization, audit logging, and policy checks.

In particular, `curate` does not raw-write `talk/` or `pair/` intent files. When it approves or rejects an intent, it calls `approve_task_intent` or `reject_task_intent`; Den validates the transition and performs the source-branch audit update as a control-plane operation.

### Work Tool Run Context

Every `work` integration tool call must include or derive a Den-issued run context:

- `bear_id`
- `task_id`
- `run_id`
- `allowed_tools`
- `scope`
- risk/approval state

Tools must reject calls outside the approved task scope even if the underlying Letta agent attempts them. Network egress allowlisting should also be enforced below the agent/tool layer where available. Tool-call logs must include `bear_id`, `task_id`, `run_id`, destination/action, status, byte counts where applicable, and timestamp.

## MemFS Layout

Each Bear has one bare repo, recorded in `bears.memfs_repo_path`, and four branches:

| Branch | Writable paths | Readable paths |
|--------|----------------|----------------|
| `talk` | `talk/` | `talk/`, `core/` |
| `pair` | `pair/` | `pair/`, `core/` |
| `curate` | `curate/`, `core/` | all branches |
| `work` | `work/` | `work/`, `core/` |

Initial directory skeleton:

```text
core/.skills/
core/results/
core/tasks/
talk/.skills/
talk/tasks/
pair/.skills/
pair/tasks/
curate/.skills/
work/.skills/
work/results/
```

The bare repo `pre-receive` hook is authoritative for branch/path write enforcement. Tool-level path checks are defense in depth and should return clearer errors before push time.

### Core Materialization

`core/` is curated shared state. The source of truth for writes to `core/` is the `curate` branch plus Den-mediated privileged tools that write validated `core/` files.

Non-curate role agents may read `core/`, but they do not gain write authority over it. Implementation may materialize `core/` for `talk`, `pair`, and `work` by one of these Den-managed mechanisms:

1. A read-only worktree/mount of the `curate` branch's `core/` directory alongside the role worktree.
2. A Den-managed merge/copy of `core/` into the role's prompt/context construction path without committing `core/` changes to the role branch.
3. A Git merge strategy that imports `core/` into role worktrees while preserving the rule that role agents cannot commit or push `core/` changes.

The implementation must document which mechanism is active before Phase 2 acceptance. Any mechanism that can produce non-fast-forward branch divergence must be Den-managed and must not rely on the agent to resolve conflicts. If a role agent attempts to commit `core/` changes directly, the pre-receive hook rejects the push.

## Data Model

Phase-0 migration introduces the additive schema needed by Phase 1:

```sql
bears
  memfs_repo_path text null
  provisioning_version integer not null default 1

bear_agents
  bear_id uuid references bears(id) on delete cascade
  role text check (role in ('talk', 'pair', 'curate', 'work'))
  letta_agent_id text null
  provisioning_status text not null default 'pending'
  last_provisioned_version integer not null default 0
  last_synced_at timestamptz null
  last_provisioning_error text null
  config_hash jsonb null
  created_at timestamptz not null default now()
  updated_at timestamptz not null default now()
  primary key (bear_id, role)
```

`provisioning_status` values are:

- `pending` — desired row exists, but no Letta agent has been created yet.
- `provisioning` — Den is actively attempting to create or patch the role agent.
- `ready` — the role agent exists and matches the last recorded provisioning version/config hash.
- `drifted` — Den detected a mismatch requiring reconciliation.
- `failed` — the most recent provisioning/reconciliation attempt failed; see `last_provisioning_error`.

Indexes/constraints:

- Primary key: `(bear_id, role)`.
- Partial unique index on `letta_agent_id` where `letta_agent_id IS NOT NULL AND btrim(letta_agent_id) <> ''`.
- Index on `(role)` for operator discovery and role-wide audits.
- Optional index on `(provisioning_status)` for management UI filters.

`last_provisioned_version` is compared to `bears.provisioning_version` for coarse drift detection. `config_hash` stores detailed artifact hashes such as rendered prompt, tool roster, and runtime policy. `last_synced_at` records the most recent successful Den reconciliation for that role.

Den update queries are responsible for setting `updated_at = NOW()` explicitly whenever a row changes; no trigger is required for Phase 1.

## Provisioning and Reconciliation Rules

`create_bear` creates only the logical Bear row. `provision_bear` creates or reconciles one desired `bear_agents` row per role.

Rules:

- A missing role row is inserted with `provisioning_status = 'pending'` before Letta creation begins.
- While Den is creating or patching the Letta agent, the row is set to `provisioning`.
- After successful creation, Den stores `letta_agent_id`, expected config hashes, `last_provisioned_version`, clears `last_provisioning_error`, sets `last_synced_at`, and marks the row `ready`.
- Re-running `provision_bear` with no canonical changes is a no-op aside from optional health/drift checks.
- If a row exists and the Letta agent exists but differs from canonical config, Den patches/reconciles it and records the outcome.
- If a row exists but the Letta agent is missing, Den marks it `failed` or creates a replacement depending on operator policy; automatic replacement must be logged.
- If Letta contains tagged agents not present in the database, `reconcile_bear` reports them. Den may adopt a tagged agent only when it unambiguously matches a missing `(bear_id, role)` and passes safety checks.
- Newly provisioned or migrated Bears mirror the `talk` role into `bears.letta_agent_id` for legacy callers until those callers are migrated.

## Task Schema Tool Requirements

[`../../../docs/architecture/tasks-schema.md`](../../../docs/architecture/tasks-schema.md) requires structured authoring tools rather than raw file writes:

| Schema file | Authoring tool | Required validator behavior |
|-------------|----------------|-----------------------------|
| `talk/tasks/<intent-id>.md`, `pair/tasks/<intent-id>.md` | `write_task_intent` | Enforce id format, lifecycle starts at `pending_review`, cron validity, non-empty tools and scope, no wildcard scope, and body length. |
| `core/tasks/<task-id>.md` | `approve_task_intent` | Enforce task id format, parent intent linkage, work-agent tool subset, risk policy, limits, future `expires_at`, and structured body. |
| Source intent approval/rejection audit update | `approve_task_intent`, `reject_task_intent` | Den-mediated cross-branch metadata update only. Enforce valid lifecycle transition, reviewer metadata, and non-empty rejection reason when rejected. |
| `work/results/<task-id>/<run-id>.md` | `write_run_result` | Enforce run id format, known parent task, terminal status, scoped external calls, summary length, and error/status consistency. |

Den must validate `core/tasks/` again before indexing or dispatching. The `pre-receive` hook should re-validate `core/tasks/` once the validator exists because this path is the dispatch source of truth.

## Conversation Locking

Conversation history belongs to a specific Bear role agent, not to the logical Bear as a whole. Bear-level views may aggregate conversations, but they must preserve and display the role/agent source.

Migration rules:

- Legacy conversation views that read `bears.letta_agent_id` should be treated as `talk` conversations.
- ACP conversation lists/history must migrate to the `pair` agent id.
- Web chat and Slack conversation routing must migrate to the `talk` agent id.
- Management UI filters should support role-specific conversation inspection before Phase 8.5 is considered complete.

## Phase 1 Implementation Notes

- `create_bear` keeps creating only the logical Bear row.
- `provision_bear` creates or reconciles one `bear_agents` row per role.
- New Slack/web chat routing must resolve role `talk`; ACP routing must resolve role `pair`.
- Existing legacy paths should continue to read `bears.letta_agent_id` until their call sites are explicitly migrated, but new code should not add additional dependencies on that column.
