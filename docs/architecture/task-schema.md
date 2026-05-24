# BEARS Tasks Schema

This document specifies the file formats and lifecycle for the task-management subsystem described in the `multi-role-runtime-architecture` ADR (section 5, "Task request flow") and operationalized by `MULTI_ROLE_RUNTIME_IMPLEMENTATION_PLAN.md` (phases 4–8).

There are three distinct file types, each living on a different branch of the Bear's MemFS repo. They form a pipeline: **intent → approved task → result**, mediated by the `curate` role and Den.

## Pipeline overview

```
                          ┌──────────────────┐
  user request via talk   │  talk/tasks/     │  intent files
  user request via pair → │  pair/tasks/     │  written by channel roles
                          └────────┬─────────┘
                                   │  curate run reviews,
                                   │  approves or rejects
                                   ▼
                          ┌──────────────────┐
                          │  core/tasks/   │  approved task definitions
                          │                  │  written by the `curate` role via Den
                          └────────┬─────────┘
                                   │  Den dispatches per
                                   │  schedule or trigger
                                   ▼
                          ┌──────────────────┐
                          │  work/results/   │  per-run execution results
                          │  <task-id>/      │  written by the `work` role
                          │    <run-id>.md   │
                          └────────┬─────────┘
                                   │  curate run promotes
                                   │  summaries to core/
                                   ▼
                          ┌──────────────────┐
                          │  core/results/ │  cross-channel-visible
                          │                  │  result summaries
                          └──────────────────┘
```

All files are markdown with YAML frontmatter. This matches the format of skills and Letta Code memory files, keeping the LLM's authoring familiar.

---

## File type 1: Intent (channel branch)

**Location:** `talk/tasks/<intent-id>.md` or `pair/tasks/<intent-id>.md`

**Written by:** the channel agent (talk or pair) when a user requests work with external effects.

**Read by:** the curate agent during its cycle.

**Lifecycle:** `pending_review` → `approved` (intent file remains as audit record; canonical state moves to `core/tasks/`) or `rejected` (intent file updated with rejection reason).

The approval/rejection update is performed by a privileged Den tool, not by granting the curate agent raw write access to the channel branch. The curate agent decides approve/reject during its cycle; Den validates the transition and writes the source-branch audit metadata as a control-plane operation.

### Schema

```yaml
---
id: intent-2026-05-03-001              # unique within branch; suggest <yyyy-mm-dd>-<seq>
schema_version: 1
status: pending_review                  # pending_review | approved | rejected
requested_by: "user@example.com"        # the user who asked
requested_at: 2026-05-03T14:23:00Z
requesting_channel: slack               # slack | webchat | discord | acp | ...

# What the user wants:
proposed_type: scheduled                # oneshot | scheduled | event_triggered
proposed_schedule: "0 9 * * *"          # if scheduled, a cron expression in UTC
proposed_tools: [http_get, slack_post]  # tools the agent thinks it'll need
proposed_scope:                         # destinations the agent thinks it'll touch
  http_endpoints: ["api.example.com/deploys"]
  slack_channels: ["#team-alerts"]
proposed_risk: low                      # low | high (agent's best guess)

# Outcome of curate review (populated when status changes):
reviewed_by: null                       # agent-id of the curate agent that reviewed
reviewed_at: null
rejection_reason: null                  # populated only when status: rejected
approved_task_id: null                  # populated only when status: approved
                                        # references the file in core/tasks/
---

# <human-readable title>

<2-4 sentence description of what the task does and why the user requested it.
Should include enough detail for the curate agent to evaluate the request and
for the work agent to execute it later. Avoid encoding secrets or credentials.>

## User context

<Optional. Any context from the conversation that informs the task —
preferences, constraints, prior related work. The curate agent uses this
when deciding whether to approve.>
```

### Validation rules

- `id` matches the regex `^intent-\d{4}-\d{2}-\d{2}-\d{3,}$` and is unique within the branch.
- `schema_version` is `1`.
- `status` starts at `pending_review`. May only transition to `approved` or `rejected`.
- `proposed_schedule` is a valid 5-field cron expression if `proposed_type: scheduled`, otherwise null.
- `proposed_tools` is non-empty.
- `proposed_scope` lists at least one destination; entries must be specific (no wildcards like `*.example.com`).
- The body has at least 50 characters of human-readable description.

### Authoring

The channel agent uses `write_task_intent`, a privileged Den tool that takes structured inputs, validates them, and writes the file. The agent should not write intent files directly via raw filesystem operations; the tool exists to enforce schema and avoid malformed entries that would be rejected during review.

Approval and rejection are similarly performed through privileged Den tools (`approve_task_intent` and `reject_task_intent`). These tools update source intent audit fields without giving the curate agent raw write access to `talk/` or `pair/` paths.

### Examples

A scheduled task:

```yaml
---
id: intent-2026-05-03-001
schema_version: 1
status: pending_review
requested_by: hans@quillbot.com
requested_at: 2026-05-03T14:23:00Z
requesting_channel: slack
proposed_type: scheduled
proposed_schedule: "0 9 * * 1-5"
proposed_tools: [http_get, slack_post]
proposed_scope:
  http_endpoints: ["api.deploy-tracker.internal/v1/status"]
  slack_channels: ["#team-deploy-alerts"]
proposed_risk: low
reviewed_by: null
reviewed_at: null
rejection_reason: null
approved_task_id: null
---

# Daily deploy status check

Check the internal deploy tracker each weekday morning at 9am UTC. Post a
summary to #team-deploy-alerts if any deploys in the previous 24h have
failed. If everything is green, post nothing.

## User context

Hans's team has been getting blindsided by silent deploy failures over
weekends. He asked for a Monday-morning roundup of any failures from the
weekend, plus a daily check during the week.
```

A oneshot task:

```yaml
---
id: intent-2026-05-03-002
schema_version: 1
status: pending_review
requested_by: hans@quillbot.com
requested_at: 2026-05-03T15:10:00Z
requesting_channel: acp
proposed_type: oneshot
proposed_schedule: null
proposed_tools: [github_search, github_read_file]
proposed_scope:
  github_repos: ["quillbot/letta-experiments"]
proposed_risk: low
reviewed_by: null
reviewed_at: null
rejection_reason: null
approved_task_id: null
---

# Survey repo for telemetry library usage

Search the quillbot/letta-experiments repo for any usage of OpenTelemetry,
Datadog, or Prometheus libraries. Produce a list of files and the libraries
they use, grouped by library.

## User context

Hans is auditing observability tooling across experiment repos before
choosing a standard.
```

---

## File type 2: Approved task (shared branch)

**Location:** `core/tasks/<task-id>.md`

**Written by:** privileged Den tooling after the curate agent reviews and approves an intent. The curate agent supplies the decision and refined task definition; Den validates and writes the file.

**Read by:** Den (for scheduling and dispatch); the work agent (when Den dispatches a run).

**Lifecycle:** `approved` → `active` (Den has scheduled it) → `completed` | `paused` | `failed` | `expired`.

### Schema

```yaml
---
id: task-2026-05-03-001                 # unique within Bear; suggest <yyyy-mm-dd>-<seq>
schema_version: 1
status: approved                         # approved | active | paused | completed | failed | expired

# Origin:
parent_intent: talk/tasks/intent-2026-05-03-001.md
created_by: talk                         # which channel originated the request
approved_by: curate                      # always curate (named for completeness)
approved_at: 2026-05-03T16:00:00Z

# Definition:
type: scheduled                          # oneshot | scheduled | event_triggered
schedule: "0 9 * * 1-5"                  # cron in UTC, only when type: scheduled
event_trigger: null                      # webhook spec, only when type: event_triggered

allowed_tools: [http_get, slack_post]    # work agent must use only these
scope:
  http_endpoints: ["api.deploy-tracker.internal/v1/status"]
  slack_channels: ["#team-deploy-alerts"]

risk: low                                # low | high
                                         # high triggers Den's HITL approval per run

# Limits:
max_runtime_seconds: 300                 # work agent must complete a run within this
max_runs_per_day: 5                      # rate limit
expires_at: 2026-12-31T00:00:00Z         # task auto-completes after this date

# Bookkeeping (Den updates these):
last_run_id: null
last_run_at: null
next_run_at: 2026-05-04T09:00:00Z
consecutive_failures: 0
---

# Daily deploy status check

<The curate agent should propose a clean, executable description of the task
based on the intent and any context the agent has. Den validates and writes
this description through `approve_task_intent`. This is what the work agent
will see when dispatched. Write it in the second person, addressing the work
agent directly.>

When triggered:

1. Call `http_get` against `api.deploy-tracker.internal/v1/status?window=24h`.
2. Parse the response. Identify any deploys with `status: failed`.
3. If there are zero failures, write a result file with `status: success`
   and body "no failures in window" and exit. Do not post to Slack.
4. If there are one or more failures, post a summary to `#team-deploy-alerts`
   using `slack_post`. Format:
       Daily deploy check: <N> failure(s) in last 24h.
       <bulleted list of <service> at <timestamp>: <error summary>>
5. Write a result file with `status: success`, body containing the same
   summary plus the raw API response.

If the API call fails, write a result file with `status: failed` and the
error details. Do not post to Slack.
```

### Validation rules

- `id` matches `^task-\d{4}-\d{2}-\d{2}-\d{3,}$` and is unique within the Bear.
- `schema_version` is `1`.
- `status` starts at `approved`; transitions managed by Den.
- `parent_intent` references a real file in `talk/tasks/` or `pair/tasks/` whose `status` is `approved` and whose `approved_task_id` matches this file's `id`.
- `schedule` is a valid 5-field cron expression iff `type: scheduled`.
- `event_trigger` is non-null iff `type: event_triggered` (out of scope for MVP per phase 7).
- `allowed_tools` is non-empty and is a subset of the work agent's available tool roster.
- `scope` has at least one entry. Wildcards (`*`) are forbidden.
- `risk` is `high` if `allowed_tools` includes any tool that performs writes against external systems by default (e.g., `slack_post`, `github_create_issue`, anything matching a configurable destructive-tool list). Curate may downgrade to `low` only if scope is sufficiently narrow (specific channels, specific repos) — the policy for this is documented in `bear-spec.md`.
- `max_runtime_seconds` is between 30 and 3600.
- `expires_at` is in the future at time of writing.
- The body is structured as instructions to the work agent.

### Authoring

The curate agent reads the corresponding intent, evaluates it, and calls privileged Den tooling (`approve_task_intent`) with the intent path, a refined task definition, and validation parameters. The tool:

- Validates the result against this schema.
- Writes the file to `core/tasks/`.
- Updates the source intent audit metadata as a Den control-plane operation: `status: approved`, `reviewed_by`, `reviewed_at`, `approved_task_id`.

For rejections, a parallel privileged Den tool (`reject_task_intent`) updates the source intent audit metadata as a Den control-plane operation: `status: rejected`, `reviewed_by`, `reviewed_at`, `rejection_reason`.

Neither tool grants the curate agent raw write access to `talk/` or `pair/` paths.

### State transitions (managed by Den)

- `approved` → `active`: Den has indexed the task and scheduled it (or, for oneshot, queued it for first dispatch).
- `active` → `completed`: oneshot task completed successfully, or `expires_at` reached.
- `active` → `paused`: `consecutive_failures` exceeded threshold (configurable, default 5). Surfaces in UI for manual review.
- `active` → `failed`: terminal failure (e.g., schema validation persistently failing, target system gone).
- `paused` → `active`: manual unpause via UI.

---

## File type 3: Run result (work branch)

**Location:** `work/results/<task-id>/<run-id>.md`

**Written by:** the work agent at the end of each task run.

**Read by:** the curate agent during its cycle (to decide whether to surface a summary in `core/results/`); Den (for logging); the user (via UI).

**Lifecycle:** terminal — once written, never modified.

### Schema

```yaml
---
id: run-2026-05-04-001                  # unique within task; suggest <yyyy-mm-dd>-<seq>
schema_version: 1
task_id: task-2026-05-03-001
run_status: success                      # success | failed | timeout | aborted
started_at: 2026-05-04T09:00:00Z
ended_at: 2026-05-04T09:00:14Z
duration_seconds: 14

# What was done:
external_calls:
  - tool: http_get
    target: "api.deploy-tracker.internal/v1/status"
    status_code: 200
    response_bytes: 4821
    timestamp: 2026-05-04T09:00:02Z
  - tool: slack_post
    target: "#team-deploy-alerts"
    status_code: 200
    timestamp: 2026-05-04T09:00:13Z

# For UI display and curate review:
summary: "Found 2 deploy failures in last 24h, posted summary to #team-deploy-alerts."
surfaceable: true                        # curate uses this as a hint;
                                         # true means "worth promoting to core/results/"

# For debugging:
error: null                              # populated when run_status != success
---

# Run output

<Free-form markdown. The work agent writes whatever is useful for a human
reading later: the data it found, the decisions it made, anything anomalous.
Should not include credentials or secrets.>

## Findings

- payment-service deploy at 2026-05-03T22:14Z: failed with "DB migration timeout"
- search-service deploy at 2026-05-04T03:08Z: failed with "health check failed after 5 retries"

## Slack message posted

> Daily deploy check: 2 failure(s) in last 24h.
> - payment-service at 2026-05-03 22:14 UTC: DB migration timeout
> - search-service at 2026-05-04 03:08 UTC: health check failed after 5 retries
```

### Validation rules

- `id` matches `^run-\d{4}-\d{2}-\d{2}-\d{3,}$` and is unique within the task's results directory.
- `task_id` references an existing file in `core/tasks/`.
- `run_status` is one of the allowed values.
- `external_calls` is an array; can be empty if the run made no external calls (e.g., a no-op result).
- All `external_calls[].target` values must be within the parent task's `scope`. Den's auditor cross-checks this and alerts on mismatches.
- `summary` is a single sentence (≤ 200 chars).
- `error` is non-null iff `run_status != success`.

### Authoring

The work agent uses a tool (`write_run_result`) that takes structured inputs and writes the file. The tool also commits and pushes immediately so Den can react to it via `post-receive` hook (faster than polling).

---

## Cross-cutting concerns

### Identifiers

- Intent IDs and task IDs and run IDs all use `<prefix>-<yyyy-mm-dd>-<seq>` format, where `<seq>` is a zero-padded counter (3+ digits).
- The mapping between intents and tasks is captured by `parent_intent` (on the task) and `approved_task_id` (on the intent). Both directions are populated for easy navigation.
- The mapping between tasks and runs is captured by `task_id` (on the run) and the directory structure (`work/results/<task-id>/...`).

### Schema versioning

All three file types include `schema_version`. We start at `1`. Future schema changes:

- **Backward-compatible additions** (adding optional fields): no version bump required. Validators must accept and ignore unknown fields.
- **Breaking changes**: bump `schema_version`. Den must support reading prior versions for at least one major release window.

### Secrets

None of these files may contain secrets, credentials, tokens, or other sensitive material. Tools used by the work agent obtain credentials from a Den-managed secret store at execution time. The result file may reference *that the agent used a credential* but never the credential value.

### Validation enforcement

Validation runs at three points:

1. **Authoring tools** — the tools that channel/curate/work agents use to create these files validate before writing. Bad inputs fail fast.
2. **Pre-receive hook** — for files in paths Den cares about, the bare repo's `pre-receive` hook can optionally re-validate (recommended for `core/tasks/` since it's the dispatch source of truth). The hook also enforces branch/path write policy; Den-mediated privileged tools are responsible for any approved cross-branch audit updates.
3. **Den's polling/index** — when Den picks up a new approved task, it validates again before scheduling. Failures are logged and the task is not dispatched.

### Open design questions

These are not blockers for phase 0–8 implementation but should be settled before broader rollout:

1. **Event-triggered tasks.** The schema reserves `type: event_triggered` and an `event_trigger` slot, but the trigger format and webhook auth are TBD. Defer to a follow-up ADR.
2. **Task supersession.** If a user creates a new intent that conflicts with an existing approved task ("change the schedule to weekly"), how is the prior task replaced? Current model: curate writes a new task and Den retires the old one (`status: completed`, with a note). Worth confirming this UX with users before locking it in.
3. **Result retention.** Run result files accumulate over time. Need a retention policy (e.g., keep last N runs per task, plus all `failed` runs for 90 days). Implement in phase 11 monitoring or as a separate cleanup job.
4. **Cross-Bear task delegation.** Out of scope. If it becomes relevant later, schema needs a `delegated_to_bear` field or similar.
