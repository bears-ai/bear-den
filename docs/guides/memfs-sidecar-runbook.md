# MemFS sidecar operator runbook

This runbook covers recovery for the multi-role MemFS sidecar repo-view design described in [`adr/memfs-sidecar-repo-views.md`](adr/memfs-sidecar-repo-views.md).

## Mental model

Each Bear has one canonical bare repository. Each role runtime has a per-runtime view repository whose `main` branch mirrors exactly one canonical role branch.

```text
runtime view main  -> canonical Bear repo role branch
runtime-talk main  -> refs/heads/talk
runtime-pair main  -> refs/heads/pair
runtime-curate main -> refs/heads/curate
runtime-work main  -> refs/heads/work
runtime-watch main -> refs/heads/watch
```

The sidecar automatically forwards safe view commits to canonical and reconciles views from canonical where possible. When the sidecar cannot safely decide which side should win, it quarantines the affected view rather than risking silent corruption.

## Health endpoints

List all registered views:

```text
GET /v1/management/bears
```

Inspect one Bear/role:

```text
GET /v1/management/bears/{bear_id}/roles/{role}
```

Inspect one runtime's recent sidecar activity:

```text
GET /v1/management/activity?agent_id={agent_id}&limit=100
```

A problematic view health response should include enough information to decide recovery:

```json
{
  "agent_id": "agent-abc",
  "bear_id": "bear-123",
  "role": "talk",
  "state": "quarantined",
  "canonical_tip": "abc123",
  "view_tip": "def456",
  "merge_base": "abc123",
  "view_ahead_by": 1,
  "canonical_ahead_by": 0,
  "drift_count": 1,
  "last_successful_forward_at": 1777875900.456,
  "last_reconciled_at": 1777876000.123,
  "last_reconcile_status": "quarantined",
  "quarantined": true,
  "diagnostic": "canonical would reject role chat path core/nope.md",
  "recommended_action": "Inspect diagnostic, then run reconcile or an operator override."
}
```

## Normal recovery first: reconcile

Before using override endpoints, try normal reconciliation:

```text
POST /v1/management/views/{agent_id}/reconcile
```

To run reconciliation for all registered views immediately:

```text
POST /v1/management/views/reconcile-all
```

The sidecar also runs scheduled reconciliation when `MEMFS_RECONCILE_INTERVAL_SECONDS` is greater than `0` (default: `60`). Set it to `0` to disable the loop for debugging.

Use this when the view may simply be behind canonical, ahead with acceptable commits, or missing/corrupt but canonical is healthy. Reconcile is idempotent.

If reconcile succeeds, no override is needed.

## Override endpoints

All overrides require a non-empty JSON `reason`. The reason is recorded in sidecar activity and registry metadata.

### Canonical wins

```text
POST /v1/management/views/{agent_id}/override/canonical-wins
Content-Type: application/json

{
  "reason": "View committed an invalid core/ path; canonical is correct."
}
```

Effect:

- force-resets the view's `main` branch to the canonical role branch,
- clears quarantine,
- preserves audit metadata in the registry/activity log.

Use when canonical is known-good and view-only commits should be discarded.

### Recreate view

```text
POST /v1/management/views/{agent_id}/override/recreate-view
Content-Type: application/json

{
  "reason": "View repo is corrupt; canonical role branch is healthy."
}
```

Effect:

- moves the old view repo aside with a timestamped suffix,
- recreates the view from canonical,
- clears quarantine.

Use when the view repo is lost, corrupt, or easier to replace than repair.

### View wins

```text
POST /v1/management/views/{agent_id}/override/view-wins
Content-Type: application/json

{
  "confirm": "view-wins",
  "reason": "Canonical role branch was accidentally reset; view has the correct tip."
}
```

Effect:

- force-updates the canonical role branch to the view's `main` tip,
- clears quarantine,
- records old and new tips.

This is dangerous. Use only when you have inspected the view and know it is the correct source of truth.

If the view contains paths outside the role policy, the sidecar refuses unless you also pass:

```json
{
  "confirm": "view-wins",
  "reason": "Policy was incorrect during incident; operator has inspected commit.",
  "allow_policy_violation": true
}
```

Use `allow_policy_violation` only when the policy itself is known wrong. Otherwise prefer manual salvage and `canonical-wins`.

### Clear quarantine

```text
POST /v1/management/views/{agent_id}/override/clear-quarantine
Content-Type: application/json

{
  "reason": "Manual Git repair completed; tips now match."
}
```

Effect:

- removes the quarantine marker only if view and canonical tips already match.

If you must clear quarantine while tips still differ, the endpoint requires force confirmation:

```json
{
  "force": true,
  "confirm": "clear-quarantine-force",
  "reason": "Temporary operator override during incident response."
}
```

Avoid force unless there is a clear incident plan.

## Recommended incident flow

1. Inspect Bear detail in Den or call sidecar health directly.
2. Read the diagnostic. Note Bear id, role, runtime handle, canonical tip, and view tip.
3. Try `POST /reconcile`.
4. If still quarantined, inspect commits manually:
   - canonical: `git --git-dir <canonical_repo> log --oneline <role>`
   - view: `git --git-dir <view_repo> log --oneline main`
   - changed paths: `git --git-dir <view_repo> diff --name-only <canonical_tip> <view_tip>`
5. Choose one recovery strategy:
   - canonical is correct → `canonical-wins`
   - view repo is corrupt → `recreate-view`
   - view is correct and canonical is wrong → `view-wins`
   - manual fix already done → `clear-quarantine`
6. Re-check health.
7. Record the incident in operator notes if data was discarded or force was used.

## What not to do

- Do not clear quarantine without understanding why it was set.
- Do not use `view-wins` just because a runtime recently wrote something.
- Do not manually force-push canonical refs without recording what happened.
- Do not allow role runtimes to call override endpoints.

## Diagnostics operators should expect

Every override and quarantine should log:

- `agent_id`
- `bear_id`
- `role`
- `action`
- `reason`
- old/new canonical tip
- old/new view tip
- archived view path, if any

If these are absent, treat it as an observability bug in the sidecar.
