# Bear Den Multi-Role Runtime Bear Spec

This is the phase-0 implementation spec for [`../../../docs/architecture/adr/multi-role-runtime-architecture.md`](../../../docs/architecture/adr/multi-role-runtime-architecture.md) and [`../../../docs/planning/MULTI_ROLE_RUNTIME_IMPLEMENTATION_PLAN.md`](../../../docs/planning/MULTI_ROLE_RUNTIME_IMPLEMENTATION_PLAN.md). It freezes the canonical Den-owned configuration before provisioning code creates the role runtimes for a Bear.

## Scope

A Bear is a logical assistant with five fixed roles. During the Letta-backed migration era, those roles are realized by Letta-managed runtimes:

| Role | Surface | Letta path | Memory branch | Purpose |
|------|---------|------------|---------------|---------|
| `talk` | Slack, web chat, future chat channels | Letta Code harness | `talk` | Synchronous conversation and task-intent authoring. |
| `pair` | ACP clients such as IDEs | Den ACP adapter to Letta API | `pair` | Synchronous client-mediated collaboration and task-intent authoring. |
| `curate` | Den scheduler only | Letta API | `curate` | Reflection, memory integration, task approval, skill proposal review, watch observation review, result promotion. |
| `work` | Den task dispatcher only | Letta Code harness | `work` | Scheduled or event-triggered outbound external work execution. |
| `watch` | Den subscription/event router only | Letta API | `watch` | Inbound external observation from webhooks, polling results, message queues, and other streams. |

The old single-agent `bears.letta_agent_id` column is dropped from the active schema. Den resolves role runtimes through `bear_agents(role, letta_agent_id)` and sends the selected runtime handle as `role_agent_id` to runtime services.

The watch role runtime may be provisioned later than the first four roles during rollout, but Phase 0 data structures, branch layout, tags, prompts, harness choice, and skill manifest semantics reserve the `watch` role from the start.

## Harness Choice

The five roles use two runtime families:

| Role | Runtime family | Rationale |
|------|----------------|-----------|
| `talk` | Letta Code harness | Conversational channels benefit from Letta Code's tool execution surface, skills loader, and existing channel harness behavior. |
| `pair` | Letta API direct | ACP's multi-session-per-connection model is incompatible with Letta Code harness single-session state. ACP client tools are mediated through ACP, not through the harness. |
| `curate` | Letta API direct | Curate needs a deliberately narrow tool roster, controlled cycle execution, and custom multi-branch read access. Letta Code defaults would be too broad. |
| `work` | Letta Code harness | Work's job is structured tool execution; Den controls dispatch so the per-runtime sequential constraint is acceptable and useful. |
| `watch` | Letta API direct | Watch is a thin inbound event-reception loop and should not get shell/filesystem/tooling breadth from a harness. |

Harness choice is deterministic from role and should be included in each role's `runtime_policy_hash`. Den runs and manages Letta Code harness lifecycle for `talk` and `work`; Den drives `pair`, `curate`, and `watch` through API-direct adapters/runners.

## Prompt Model

The canonical prompt shape is a shared base prompt plus a role-specific suffix. Den owns the templates and increments `bears.provisioning_version` whenever the base prompt, any role suffix, the canonical tool roster, skill manifest, or provisioning-relevant MemFS/runtime configuration changes.

Den tracks prompt/tool/skill/runtime drift using these canonical artifacts:

- `base_prompt_hash` — hash of the shared base prompt template after rendering global variables.
- `role_prompt_hash` — hash of the role-specific suffix after rendering role variables.
- `rendered_prompt_hash` — hash of the final prompt sent to Letta for the specific Bear and role.
- `tool_roster_hash` — hash of the canonical tool names and role policy before resolving environment-specific Letta tool ids.
- `skill_manifest_hash` — hash of the role-relevant slice of `bear_skills_manifest` after resolving skill name, version, source, content hash, and applicability.
- `runtime_policy_hash` — hash of MemFS branch/path policy and other runtime constraints.

The shared base prompt must state:

- The assistant identity is the Bear, not the individual provider-managed runtime.
- The active runtime executes one fixed role and must not claim capabilities outside that role.
- Durable cross-role knowledge lives in `core/` and is eventually consistent.
- Secrets must never be written into MemFS task, result, observation, skill proposal, or note files.
- External outbound work is asynchronous and must flow through structured task intents unless the current role is `work` executing an approved task.
- External inbound events flow through watch observations and curate review before they can cause outbound action.
- Durable skill changes flow through Den's skill proposal and manifest review process.
- Privileged cross-branch operations are performed by Den tools, not by raw runtime writes.

Role suffix requirements:

| Role | Suffix requirements |
|------|---------------------|
| `talk` | Treat Slack/web chat as conversational only. Read `core/` and `talk/`; write only `talk/`. Use `write_task_intent` for external-effect or subscription requests. Use `propose_skill` for durable skill-learning requests. Do not access pair, curate, work, or watch branches. |
| `pair` | Treat ACP tools as client-side, user-gated tools. Read `core/` and `pair/`; write only `pair/`. Use `write_task_intent` for external-effect or subscription requests. Use `propose_skill` for durable skill-learning requests. Do not access talk, curate, work, or watch branches. |
| `curate` | Read all branches and pending skill proposals. Write directly only to `curate/` and `core/`. Promote durable knowledge to `core/`, approve or reject task intents and skill proposals through privileged Den tools, review watch observations, and promote work summaries. No external communication tools are allowed. |
| `work` | Read only `core/`, the dispatched task definition, and `work/`. Write only `work/`. Use only tools named in the approved task's `allowed_tools` and scoped by the Den-issued run context. Never read talk, pair, curate, or watch branches. Use `propose_skill` for reusable execution procedures rather than installing skills directly. |
| `watch` | Read only `core/`, the delivered subscription event payload, and `watch/`. Write only `watch/`. Use `write_observation` for inbound event records. No outbound action tools are allowed. Never read talk, pair, curate, or work branches. Use `propose_skill` for reusable subscription parsing/handling procedures rather than installing skills directly. |

## Letta Runtime Tags

Every Letta-backed role runtime Den provisions must have these tags:

| Tag | Purpose |
|-----|---------|
| `bear:<bear_id>` | Groups all Letta-backed role runtimes for one logical Bear. |
| `role:<role>` | Supports role filtering independent of Bear id. |
| `bear:<bear_id>:role:<role>` | Stable reconciliation/discovery tag required by Phase 1 acceptance. |
| `git-memory-enabled` | Marks the runtime as using git-backed MemFS memory. |

Valid role tag values are `talk`, `pair`, `curate`, `work`, and `watch`.

Den reconciliation uses the database as the source of truth first and Letta tags as secondary discovery/adoption evidence. Tagged Letta runtimes that are not represented in `bear_agents` must be reported by `reconcile_bear`; adoption vs replacement is an explicit operator decision unless the runtime unambiguously matches a missing `(bear_id, role)` row and passes safety checks.

## Skill Management

Skills are Den-managed Bear-scoped resources with per-role applicability. The manifest is the canonical source of truth; installed skills on individual role runtimes are projections of the manifest.

### Manifest model

Den maintains `bear_skills_manifest` rows with:

- `bear_id`
- `skill_name`
- `skill_version`
- `source` — URL, repository path, local path, or Den-managed source identifier.
- `content_hash` — SHA-256 of the skill contents or normalized skill bundle.
- `applies_to_roles` — set drawn from `talk`, `pair`, `curate`, `work`, `watch`.
- `installed_at`
- `last_verified_at`

The manifest should enforce uniqueness on `(bear_id, skill_name, skill_version)` unless the implementation later needs multiple source variants for the same semantic skill. If multiple versions of a skill may coexist, Den must define role-level conflict resolution before provisioning.

### Installation paths

Den uses two installation mechanisms from one manifest:

| Target role | Installation mechanism |
|-------------|------------------------|
| `talk`, `work` | Den writes role-relevant skills to `~/.letta/agents/{agent_id}/skills/` on the Letta Code harness host before harness start, or restarts the role harness after manifest changes. |
| `pair`, `curate`, `watch` | Den attaches role-relevant skills through the Letta API. |

Skills are not stored in MemFS as the canonical distribution mechanism. Filesystem skill discovery through MemFS would only serve the harness-backed runtimes and would not uniformly reach the API-direct roles.

### Skill proposals

Agents do not get raw, in-place skill installation tools. `/skill` or equivalent learning flows are wrapped as proposals:

1. Agent calls `propose_skill` with proposed content, rationale, desired applicability, and provenance.
2. Den writes a `bear_skill_proposals` row with `status = 'pending_review'`.
3. Curate reviews pending proposals during its cycle.
4. On approval, curate calls privileged Den tooling to create or update the manifest entry with chosen `applies_to_roles`.
5. Den re-provisions or reconciles affected role runtimes so the manifest projection is installed.
6. On rejection, Den records `status = 'rejected'`, `reviewed_at`, and `rejection_reason`.

Curate should use the proposing runtime's role as a default applicability hint, not as an automatic rule. For example, a work-proposed integration procedure normally applies to `work`, while a coding convention may apply to both `talk` and `pair`.

### Skill reconciliation

`reconcile_bear` compares each runtime's actual installed skills against the manifest's role-relevant slice:

- For `talk` and `work`, Den lists files/bundles under `~/.letta/agents/{agent_id}/skills/`, computes normalized content hashes, and compares them to the manifest.
- For `pair`, `curate`, and `watch`, Den lists attached skills through the Letta API and compares them to the manifest.
- Missing skills are installed.
- Extra out-of-manifest skills are removed unless an operator explicitly marks them as tolerated during migration.
- Skill changes update the role's `skill_manifest_hash` in `bear_agents.config_hash` and may require harness restart for `talk` or `work`.

## Tool Roster

Tool ids are environment-specific Letta ids. Den stores canonical tool names in code/config and resolves them to Letta ids during provisioning with the existing tool catalog API.

| Tool | Server-side or client-side | Roles | Requirement |
|------|----------------------------|-------|-------------|
| `memfs_read` | server-side | `talk`, `pair`, `curate`, `work`, `watch` | Reads only paths allowed for the role. `curate` may read all branches; `work` may not read channel, curate, or watch branches; `watch` may not read channel, curate, or work branches. |
| `memfs_write` | server-side | `talk`, `pair`, `curate`, `work`, `watch` | Writes only paths allowed for the role and commits locally. Raw writes to task/result/observation paths should be blocked in favor of schema tools. |
| `memfs_commit_push` | server-side | `talk`, `pair`, `curate`, `work`, `watch` | Pushes immediately after committed memory changes. |
| `write_task_intent` | privileged Den tool | `talk`, `pair` | Validates `talk/tasks/<intent-id>.md` or `pair/tasks/<intent-id>.md` against [`../../../docs/architecture/tasks-schema.md`](../../../docs/architecture/tasks-schema.md) before writing. Used for outbound work requests and subscription requests. |
| `approve_task_intent` | privileged Den tool | `curate` | Takes a curate decision, validates it, writes `core/tasks/<task-id>.md`, and updates the source intent audit metadata without granting the curate agent raw write access to channel branches. |
| `reject_task_intent` | privileged Den tool | `curate` | Takes a curate decision and updates the source intent to `rejected` with `reviewed_by`, `reviewed_at`, and `rejection_reason` without granting raw channel-branch writes. |
| `write_core_result_summary` | privileged Den tool | `curate` | Writes reviewed summaries under `core/results/`. |
| `write_run_result` | privileged Den tool | `work` | Validates and writes `work/results/<task-id>/<run-id>.md`, commits, and pushes. |
| `write_observation` | privileged Den tool | `watch` | Validates and writes `watch/observations/<observation-id>.md`, commits, and pushes. Includes source subscription, timestamp, payload summary or raw payload reference, and salience hint. |
| `propose_skill` | privileged Den tool | `talk`, `pair`, `curate`, `work`, `watch` | Captures proposed durable skill content into `bear_skill_proposals`; does not install the skill directly. |
| `approve_skill_proposal` | privileged Den tool | `curate` | Adds or updates `bear_skills_manifest`, closes the proposal as approved, and triggers re-provisioning/reconciliation for affected roles. |
| `reject_skill_proposal` | privileged Den tool | `curate` | Closes a pending skill proposal with reviewer metadata and rejection reason. |
| `reflection` / `defragmentation` | server-side | `curate` | Letta reflection and memory maintenance. No external communication capability. |
| ACP client tool relay | client-side | `pair` | Forwarded through ACP `session/request_permission`; Den does not execute these tools server-side. ACP prompt streaming is API-direct to the pair role runtime, not Codepool. |
| Integration tools (`http_get`, `slack_post`, `github_*`, etc.) | Letta Code harness tools plus privileged Den policy wrappers | `work` only | Runtime use must be within the approved task's `allowed_tools` and `scope`, verified by Den-issued run context and tool-side policy checks. Harness-level tools must be scoped or wrapped so the work runtime cannot bypass Den policy. |
| Subscription/event delivery | Den control-plane operation | `watch` | Den registers durable subscriptions after curate-approved intents, validates inbound signatures, performs polling where required, and delivers structured event prompts to watch. This is not generic outbound network access for watch. |

Denied by default for all roles:

- Tools that attach/detach Letta tools or mutate Letta runtime configuration.
- Tools that install out-of-band Letta skills.
- Network egress tools on `talk`, `pair`, `curate`, or `watch`, except the conversational surface itself and ACP client-mediated tool calls.
- Raw filesystem tools that bypass MemFS path validation.
- Outbound action tools on `watch`.

### Privileged Den Tool Principle

Role runtimes do not receive broad authority just because a task requires a cross-branch, skill-manifest, observation, or external effect. Instead, Den exposes narrow privileged tools with schema validation, authorization, audit logging, and policy checks.

In particular:

- `curate` does not raw-write `talk/` or `pair/` intent files. When it approves or rejects an intent, it calls `approve_task_intent` or `reject_task_intent`; Den validates the transition and performs the source-branch audit update as a control-plane operation.
- Agents do not install durable skills directly. They call `propose_skill`; curate and Den decide whether to update the manifest.
- `watch` does not register arbitrary outbound polls by itself. Den owns the subscription registry and polling jobs; watch records observations from Den-delivered events.

### Work Tool Run Context

Every `work` integration tool call, whether executed by the Letta Code harness directly or routed through a Den wrapper, must include or derive a Den-issued run context:

- `bear_id`
- `task_id`
- `run_id`
- `allowed_tools`
- `scope`
- risk/approval state

Tools must reject calls outside the approved task scope even if the underlying Letta runtime attempts them. Because `work` runs behind Letta Code, Den must ensure the harness tool surface for work is either restricted to approved wrappers or configured so broad tools such as shell/filesystem/network clients cannot bypass task policy. Network egress allowlisting should also be enforced below the runtime/tool layer where available. Tool-call logs must include `bear_id`, `task_id`, `run_id`, destination/action, status, byte counts where applicable, and timestamp.

### Watch Event Context

Every watch event prompt/tool call must include or derive a Den-issued event context:

- `bear_id`
- `subscription_id`
- `event_id`
- source type and source identifier
- signature/auth validation status where applicable
- delivery timestamp
- payload reference or bounded payload excerpt

Watch tools must reject observations that do not have a valid Den-issued event context. Watch logs must include `bear_id`, `subscription_id`, `event_id`, salience hint, observation id, and eventual curate decision when available.

## MemFS Layout

MemFS topology is defined by [`../../../docs/architecture/adr/memfs-sidecar-repo-views.md`](../../../docs/architecture/adr/memfs-sidecar-repo-views.md).

Each Bear has one canonical bare repo, recorded in `bears.memfs_repo_path`, and five canonical role branches:

| Branch | Writable paths | Readable paths |
|--------|----------------|----------------|
| `talk` | `talk/` | `talk/`, `core/` |
| `pair` | `pair/` | `pair/`, `core/` |
| `curate` | `curate/`, `core/` | all branches |
| `work` | `work/` | `work/`, `core/` |
| `watch` | `watch/` | `watch/`, `core/` |

Initial directory skeleton:

```text
core/results/
core/tasks/
talk/tasks/
pair/tasks/
curate/
work/results/
watch/observations/
watch/subscriptions/
```

The canonical bare repo `pre-receive` hook is authoritative for branch/path write enforcement. Tool-level path checks are defense in depth and should return clearer errors before push time.

The MemFS sidecar presents each Letta role agent with a per-agent view repo whose default branch is `main`. The view repo maps to exactly one canonical role branch and forwards accepted pushes to canonical. The sidecar owns view creation, forwarding, reconciliation, quarantine, diagnostics, and per-role health reporting.

### Core Materialization

`core/` is curated shared state. The source of truth for writes to `core/` is the canonical `curate` branch plus Den-mediated privileged tools that write validated `core/` files.

Non-curate role agents may read curated `core/` context, but they do not gain write authority over it. Under the sidecar-view design, any `core/` materialization into non-curate view repos must be sidecar-managed and must remain read-only from the role agent's effective write path. If a role agent attempts to commit `core/` changes directly, the canonical hook rejects the forwarded push and the sidecar records a diagnostic or quarantine according to the MemFS sidecar ADR.

## Data Model

Phase-0 migration introduces the additive schema needed by Phase 1:

```sql
bears
  memfs_repo_path text null
  provisioning_version integer not null default 1

bear_agents
  bear_id uuid references bears(id) on delete cascade
  role text check (role in ('talk', 'pair', 'curate', 'work', 'watch'))
  letta_agent_id text null
  provisioning_status text not null default 'pending'
  last_provisioned_version integer not null default 0
  last_synced_at timestamptz null
  last_provisioning_error text null
  config_hash jsonb null
  created_at timestamptz not null default now()
  updated_at timestamptz not null default now()
  primary key (bear_id, role)

bear_skills_manifest
  bear_id uuid references bears(id) on delete cascade
  skill_name text not null
  skill_version text not null
  source text not null
  content_hash text not null
  applies_to_roles text[] not null
  installed_at timestamptz null
  last_verified_at timestamptz null
  created_at timestamptz not null default now()
  updated_at timestamptz not null default now()

bear_skill_proposals
  bear_id uuid references bears(id) on delete cascade
  id uuid primary key
  proposed_by_agent_id text not null
  proposed_at timestamptz not null default now()
  skill_payload jsonb not null
  status text not null default 'pending_review'
  reviewed_at timestamptz null
  rejection_reason text null
  resulting_manifest_bear_id uuid null
  resulting_manifest_skill_name text null
  resulting_manifest_skill_version text null
  updated_at timestamptz not null default now()
```

`provisioning_status` values are:

- `pending` — desired row exists, but no Letta agent has been created yet.
- `provisioning` — Den is actively attempting to create or patch the role agent.
- `ready` — the role agent exists and matches the last recorded provisioning version/config hash.
- `drifted` — Den detected a mismatch requiring reconciliation.
- `failed` — the most recent provisioning/reconciliation attempt failed; see `last_provisioning_error`.

`bear_skill_proposals.status` values are:

- `pending_review`
- `approved`
- `rejected`

Indexes/constraints:

- `bear_agents` primary key: `(bear_id, role)`.
- Partial unique index on `bear_agents.letta_agent_id` where `letta_agent_id IS NOT NULL AND btrim(letta_agent_id) <> ''`.
- Index on `bear_agents(role)` for operator discovery and role-wide audits.
- Optional index on `bear_agents(provisioning_status)` for management UI filters.
- `bear_skills_manifest` unique index on `(bear_id, skill_name, skill_version)`.
- `bear_skills_manifest.applies_to_roles` must be non-empty and contain only valid role values.
- `bear_skill_proposals` index on `(bear_id, status, proposed_at)` for curate review queues.
- Optional composite foreign key from `bear_skill_proposals(resulting_manifest_bear_id, resulting_manifest_skill_name, resulting_manifest_skill_version)` to the manifest unique key once approved.

`last_provisioned_version` is compared to `bears.provisioning_version` for coarse drift detection. `config_hash` stores detailed artifact hashes such as rendered prompt, tool roster, role-relevant skill manifest slice, and runtime policy. `last_synced_at` records the most recent successful Den reconciliation for that role.

Den update queries are responsible for setting `updated_at = NOW()` explicitly whenever a row changes; no trigger is required for Phase 1.

## Provisioning and Reconciliation Rules

`create_bear` creates only the logical Bear row. `provision_bear` creates or reconciles one desired `bear_agents` row per role, applies the role's deterministic harness choice, and projects the role-relevant skill manifest to each agent.

Rules:

- A missing role row is inserted with `provisioning_status = 'pending'` before Letta creation begins.
- While Den is creating or patching the Letta agent, the row is set to `provisioning`.
- After successful creation, Den stores `letta_agent_id`, expected config hashes, `last_provisioned_version`, clears `last_provisioning_error`, sets `last_synced_at`, and marks the row `ready`.
- Re-running `provision_bear` with no canonical changes is a no-op aside from optional health/drift checks.
- If a row exists and the Letta agent exists but differs from canonical config, Den patches/reconciles it and records the outcome.
- If a row exists but the Letta agent is missing, Den marks it `failed` or creates a replacement depending on operator policy; automatic replacement must be logged.
- If Letta contains tagged agents not present in the database, `reconcile_bear` reports them. Den may adopt a tagged agent only when it unambiguously matches a missing `(bear_id, role)` and passes safety checks.
- Newly provisioned or reconciled Bears store role agents only in `bear_agents(role, letta_agent_id)` and send the selected value as `role_agent_id`.
- Skill manifest changes trigger reconciliation for all roles in the changed entry's `applies_to_roles`; harness restart is required when `talk` or `work` skill projections change.

## Task, Observation, and Skill Tool Requirements

[`../../../docs/architecture/tasks-schema.md`](../../../docs/architecture/tasks-schema.md) requires structured authoring tools rather than raw file writes:

| Schema/object | Authoring tool | Required validator behavior |
|---------------|----------------|-----------------------------|
| `talk/tasks/<intent-id>.md`, `pair/tasks/<intent-id>.md` | `write_task_intent` | Enforce id format, lifecycle starts at `pending_review`, cron/subscription validity, non-empty tools and scope, no wildcard scope, and body length. |
| `core/tasks/<task-id>.md` from channel intent | `approve_task_intent` | Enforce task id format, parent intent linkage, work-agent or subscription tool subset, risk policy, limits, future `expires_at`, and structured body. |
| `core/tasks/<task-id>.md` from watch observation | privileged curate/Den task creation | Enforce task id format, parent observation linkage, work-agent tool subset, risk policy, limits, and structured body. This requires the task schema to permit observation-origin parents. |
| Source intent approval/rejection audit update | `approve_task_intent`, `reject_task_intent` | Den-mediated cross-branch metadata update only. Enforce valid lifecycle transition, reviewer metadata, and non-empty rejection reason when rejected. |
| `work/results/<task-id>/<run-id>.md` | `write_run_result` | Enforce run id format, known parent task, terminal status, scoped external calls, summary length, and error/status consistency. |
| `watch/observations/<observation-id>.md` | `write_observation` | Enforce observation id format, known subscription id, Den-issued event context, bounded payload summary or safe raw payload reference, salience hint, and no secrets. |
| `bear_skill_proposals` row | `propose_skill` | Enforce non-empty skill payload, provenance, proposing agent id, proposed applicability, content hash, and pending lifecycle start. |
| `bear_skills_manifest` row | `approve_skill_proposal` | Enforce valid proposal, unique skill identity/version, role applicability, content hash, source, and audit linkage. |

Den must validate `core/tasks/` again before indexing or dispatching. The `pre-receive` hook should re-validate `core/tasks/` once the validator exists because this path is the dispatch source of truth.

## Conversation Locking

Conversation history belongs to a specific Bear role agent, not to the logical Bear as a whole. Bear-level views may aggregate conversations, but they must preserve and display the role/agent source.

Migration rules:

- Active conversation views must resolve the relevant role through `bear_agents` and preserve the role/agent source.
- ACP conversation lists/history and prompt streaming use `bear_agents(role='pair')` strictly. ACP must not fall back to `talk`; missing `pair` is an operator-remediable provisioning error.
- Web chat and Slack conversation routing use the `talk` role agent id.
- Curate, work, and watch conversations/runs are internal and should be surfaced as role-scoped operational history rather than user-facing chat history. Work runs are Letta Code harness-backed operational sessions, not API-direct user conversations.
- Management UI filters should support role-specific conversation inspection before Phase 8.5 is considered complete.

## Subscription Registry

Den owns durable watch subscriptions. User-requested subscriptions enter through the same task-intent and curate-approval path as other external effects.

Phase 0 reserves the following registry concept for the watch rollout:

```sql
bear_subscriptions
  bear_id uuid references bears(id) on delete cascade
  subscription_id text not null
  source_type text not null
  source_config jsonb not null
  status text not null default 'active'
  approved_task_id text null
  created_at timestamptz not null default now()
  updated_at timestamptz not null default now()
  last_fire_at timestamptz null
  error_count integer not null default 0
  primary key (bear_id, subscription_id)
```

Polling subscriptions are executed by Den and delivered to watch as structured events. Webhook subscriptions are received by Den, signature-validated where applicable, and then delivered to watch. Watch records observations; curate decides whether observations become `core/` knowledge or derived tasks.

## Phase 1 Implementation Notes

- `create_bear` keeps creating only the logical Bear row.
- `provision_bear` creates or reconciles one `bear_agents` row per role. The implementation may leave `watch` in `pending`/reserved state until the watch rollout if the chosen deployment phase provisions only four runtime agents initially.
- The migration sequence imports historical single-agent ids into `bear_agents(role='talk')`, then drops `bears.letta_agent_id`; admin detail tooling may provision only missing roles for a Bear.
- Slack/web chat routing must resolve role `talk`; ACP routing must resolve role `pair` strictly.
- Active runtime paths must not read or write a bear-level Letta agent id.
