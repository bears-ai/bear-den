# MemFS, modern Letta agents, and memory UI

This note preserves the architecture decisions from the retired MemFS implementation plans. The implementation now lives in the services themselves; this document records the invariants to keep future changes aligned.

## Naming

Use this vocabulary consistently:

- **Human-readable name:** MemFS Manager
- **Compose service:** `bears-memfs-manager`
- **Internal URL:** `http://bears-memfs-manager:8285`
- **Letta env var:** `LETTA_MEMFS_SERVICE_URL`
- **Canonical on-disk layout:** Letta's `~/.letta/memfs/repository/...` tree on the Letta data volume

Do not introduce a separate `mem-manager` product slug. New docs and UI text should use **MemFS Manager** when referring to the git smart-HTTP service.

## Service responsibilities

### Letta

Letta owns the canonical MemFS integration. In the root compose stack, `bears-letta` receives `LETTA_MEMFS_SERVICE_URL=http://bears-memfs-manager:8285` and proxies git smart-HTTP paths through its `/v1/git/...` API.

Letta itself must have the `git` CLI in its container. Even though MemFS Manager serves the bare repositories, Letta's Context Repository code still performs git operations while handling `/v1/git/{agent_id}/state.git`. The root compose stack therefore builds `bears-letta` from `services/letta/Dockerfile`, which wraps the upstream `letta/letta` image and installs `git`.

### MemFS Manager

MemFS Manager serves Letta's git repository storage. It supports:

- smart-HTTP git operations under `/git/{agent_id}/state.git/...`
- read-only management endpoints under `/v1/management/agents/{agent_id}/...`
- `/health`, diagnostics, and activity reporting

Management reads must be **read-only**: they resolve existing bare repositories and return `404` for missing repositories instead of creating or seeding repos.

### Den

Den is the control plane and read-only operator UI consumer:

- creates/syncs Letta agents using the modern MemFS profile
- stores and sends `BearRuntimePlan` snapshots to Codepool
- reads MemFS Manager for memory health and repository metadata in the bear details/memory UI
- does not write directly to MemFS repositories

### Codepool

Codepool runs Letta Code sessions with `memfs: true`. It must **not** push directly to MemFS Manager. Git writes should route through Letta's `/v1/git/{agent_id}/state.git` endpoint so Letta can update its own memory/block cache.

The root compose stack intentionally does not set `LETTA_MEMFS_SERVICE_URL` on `bears-codepool`. Codepool uses `LETTA_BASE_URL` plus `LETTA_MEMFS_LOCAL=1`, letting Letta Code maintain its local mirror under `$HOME/.letta` and push to `LETTA_BASE_URL/v1/git/{agent_id}/state.git`.

This routing is required for correctness: direct Codepool pushes to MemFS Manager would update the git repository only. They would bypass Letta's `/v1/git/...` proxy and can leave Letta's Postgres-backed memory/block cache stale for later conversations.

## Modern Letta agent profile

Den's Letta create/sync paths should keep these properties:

- `include_base_tools: false`
- `git_enabled: true` when supported by the deployed Letta version
- explicit `tool_ids` selected by Den
- legacy memory mutation tools filtered by Letta tool name, including:
  - `memory_apply_patch`
  - `core_memory_append`
  - `core_memory_replace`
- default `agent_type` of `letta_v1_agent` when no explicit type is stored

Den currently retries create/patch without `git_enabled` if an older Letta server rejects that field with a validation-style response. For a production rollout, check logs or the agent payload if you need to confirm Context Repository support on the deployed Letta build.

## BearRuntimePlan

`bears.runtime_plan` is a versioned JSON snapshot sent from Den to Codepool. The default shape is intentionally narrow and extensible:

- `version`
- `memory.git_remote`
- `memory.git_ref`
- `memory.seed_template`

In the default self-hosted MemFS flow, `memory.git_remote` is normally `null`; canonical memory is on the Letta side and Letta Code handles the local mirror. Non-null git remotes are reserved for uncommon/future overrides.

## Memory UI

Den's bear detail and memory detail pages may show read-only repository health and metadata from MemFS Manager, such as:

- repository health state
- commit counts
- memory file counts
- head date/message
- repository file metadata
- Codepool MemFS reachability checks
- Letta memory block diagnostics

Do not expose blob contents through this operator UI without a separate product/security review.

## Existing-bear rollout

Existing bears created before the modern MemFS flow may need an operational re-sync so their Letta agents and Den rows receive the modern settings and default runtime plan. See `services/den/src/core/bears/rollout.rs` for the implementation note.
