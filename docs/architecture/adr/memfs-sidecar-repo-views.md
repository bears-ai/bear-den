# ADR: MemFS Sidecar Repo Views for Multi-Agent Bear Memory

**Status:** Accepted
**Date:** 2026-05-04
**Deciders:** Hans

## Context

The BEARS multi-agent architecture defines a Bear as one logical assistant backed by role-specific Letta agents: `talk`, `pair`, `curate`, `work`, and `watch`. The memory model in the multi-agent ADR depends on these invariants:

- A Bear has one canonical memory history.
- Each role has an isolated writable branch.
- Role agents cannot directly read or write other roles' private branches.
- `curate` is the only role that can promote durable shared memory into `core/`.
- Git enforcement catches writes outside a role's allowed path before those writes become canonical.

Letta's MemFS / Context Repository integration, however, is naturally agent-oriented. It expects a Git remote per Letta agent, generally with a normal default branch such as `main`, and may make assumptions about clone/checkout/push behavior that are outside Den's control. A direct implementation of one Bear bare repository with role branches exposed to all agents would require betting that Letta and Letta Code can operate cleanly against non-`main` role branches and will continue to do so across upstream releases.

We need a memory layout that preserves the architecture's invariants while avoiding fragile dependence on Letta's branch behavior.

## Decision

BEARS will use **one canonical bare MemFS repository per Bear**, plus **per-agent bare repository views** served by the MemFS sidecar.

For each Bear:

- The canonical Bear repository is the source of truth.
- The canonical repository has role refs/branches for `talk`, `pair`, `curate`, `work`, and `watch`.
- The MemFS sidecar exposes a per-agent Git remote for each Letta role agent.
- Each per-agent remote presents a normal agent-local repository shape, with `main` as the default branch, so Letta can clone/pull/push without needing to understand BEARS role branches.
- Each per-agent remote is backed by `git clone --shared` or equivalent object sharing from the canonical repository.
- Sidecar hooks forward accepted per-agent pushes into the corresponding canonical role branch.
- Canonical hooks enforce the role/path policy before changes become canonical.
- Sidecar reconciliation keeps per-agent remotes and canonical role branches aligned.

Conceptually:

```text
Bear canonical repo
  refs/heads/talk
  refs/heads/pair
  refs/heads/curate
  refs/heads/work
  refs/heads/watch

Per-agent view repos
  /git/{talk_agent_id}/state.git   main -> canonical talk
  /git/{pair_agent_id}/state.git   main -> canonical pair
  /git/{curate_agent_id}/state.git main -> canonical curate
  /git/{work_agent_id}/state.git   main -> canonical work
  /git/{watch_agent_id}/state.git  main -> canonical watch
```

The sidecar is therefore not a passive Git HTTP process. It is a memory-consistency component that maintains the invariant:

> The canonical Bear repo is the source of truth; each per-agent repo is a role-scoped view of exactly one canonical role branch.

## Why this over direct branch routing

A direct branch-routing design would expose the same bare repository to every role agent and rely on Letta checking out/pushing the intended non-`main` branch. That is closer to the wording of the multi-agent ADR, but it is more coupled to Letta's branch behavior.

This decision prioritizes preserving architectural invariants over preserving the simplest possible Git topology.

### 1. Isolation from Letta assumptions

Each role agent sees a normal per-agent repository with `main` as the default branch. Letta does not need to know about `talk`, `pair`, `curate`, `work`, or `watch` branches. The design does not depend on whether Letta can operate against non-`main` branches today or in future releases.

### 2. Defense in depth on visibility

A per-agent view exposes only that role's branch. A role agent cannot accidentally browse or fetch another role's branch through its assigned remote. This is stronger than relying only on canonical `pre-receive` path enforcement.

### 3. Simpler `.letta/config.json` behavior

Each per-agent view has its own agent-local repository shape and config. We avoid coordinating one `.letta/config.json` across several branches of a single repo presented directly to Letta.

### 4. Favorable migration path

If Letta later supports Bear-level shared repos with first-class branch routing, this design can collapse into a simpler direct-branch model by removing sidecar view replication. Moving in the other direction would be a larger architecture change.

## Sidecar responsibilities

The MemFS sidecar must own the following responsibilities explicitly.

### Mapping

The sidecar must resolve `agent_id` to:

- `bear_id`
- `role`
- canonical Bear repo path
- per-agent view repo path
- canonical role branch/ref
- writable path prefixes for that role

The mapping may come from Den, a sidecar cache, or a sidecar-managed manifest, but it must be inspectable by operators.

### View creation

For each role agent, the sidecar creates or repairs a per-agent view repository. The view should use shared object storage where practical (`git clone --shared` or equivalent), but correctness must not depend on object sharing. If object sharing is unsafe in an environment, the sidecar may use full clones.

The role view's default branch is `main`, and `main` corresponds to that agent's canonical role branch.

### Forwarding

When a role agent pushes to its per-agent view:

1. The view accepts the push only if it is locally well-formed.
2. A sidecar hook forwards the new view `main` tip to the canonical role branch.
3. The canonical repository's path policy validates the forwarded commits.
4. If canonical accepts, the sidecar records a successful forward.
5. If canonical rejects, the sidecar records a failed forward and marks the view as needing reconciliation or quarantine, depending on recoverability.

Forwarding must be idempotent. Re-running the forwarder for the same view and commit should produce the same canonical result or the same diagnostic.

### Reconciliation

The sidecar must periodically compare every per-agent view with its canonical role branch.

Cases:

- **View behind canonical:** fast-forward the view from canonical. This is the normal path for `core/` or role branch updates reaching role views where applicable.
- **View ahead of canonical and commits are acceptable:** forward the missing commits to canonical.
- **View and canonical equal:** no-op.
- **View and canonical diverged:** attempt only safe recovery. If no safe fast-forward/forward exists, quarantine the role view.
- **View missing or corrupted:** recreate the view from canonical.
- **Canonical missing:** fail closed and report that Bear memory is not initialized or canonical storage is unavailable.

### Quarantine

If the sidecar cannot reconcile a role view without risking silent corruption, it must quarantine that view.

Quarantine means:

- block further pushes from that role agent,
- preserve the view repo for inspection,
- expose a clear health status and diagnostic,
- require an operator override to resolve.

The sidecar must prefer stopping one role agent over allowing silent divergence from canonical memory.

### Diagnostics

Every rejection, failed forward, quarantine, self-healing action, and operator override must be logged with:

- Bear id,
- role,
- Letta agent id,
- per-agent view repo path,
- canonical repo path,
- old and new commit SHAs where applicable,
- reason,
- whether the sidecar self-healed,
- recommended operator action.

Diagnostics must be understandable without reading sidecar source code.

### Health API

The sidecar must expose per-Bear and per-role health including:

- canonical repo exists / usable,
- view repo exists / usable,
- canonical branch tip,
- view branch tip,
- last successful forward timestamp,
- last reconciliation timestamp,
- drift count,
- quarantine status,
- last diagnostic / recommended action.

Den consumes this for admin UI and alerting.

### Operator overrides

The sidecar must ship with an operator runbook and admin commands/endpoints for irrecoverable drift. At minimum, operators need procedures to:

- inspect canonical and view tips,
- choose canonical-wins,
- choose view-wins,
- re-run forwarding,
- recreate a corrupted view from canonical,
- clear quarantine after resolution.

The runbook must exist before this sidecar design is used in production.

## Failure modes accepted

This decision accepts additional sidecar responsibility and explicitly accounts for these failure modes:

- A push to a per-agent view succeeds locally but fails to forward to canonical.
- Canonical path enforcement rejects a forwarded commit after the role agent thinks its push succeeded.
- A per-agent view and canonical diverge because of forwarder bugs or manual operator changes.
- A per-agent view is lost or corrupted after node/storage failure.
- Concurrent forwards contend for canonical refs. This should be rare because branches are role-scoped, but the sidecar must still handle it.

The mitigation is not to assume these cases never happen; the mitigation is self-healing, reconciliation, quarantine, diagnostics, health reporting, and operator override paths.

## Consequences

### Positive

- Preserves the multi-agent memory invariants while keeping Letta on a conventional per-agent `main` branch repo.
- Reduces coupling to Letta's branch checkout and push behavior.
- Provides stronger visibility isolation than direct branch routing.
- Gives operators per-role health and recovery surfaces.
- Keeps a canonical per-Bear memory history.

### Negative / tradeoffs

- The MemFS sidecar becomes a stateful consistency component, not merely a Git HTTP proxy.
- Sidecar implementation is more complex than direct branch routing.
- Forwarding and reconciliation bugs can affect memory availability.
- Operator runbooks and health monitoring are mandatory, not optional.
- Some agent-visible push success states may later be quarantined if canonical rejects the forwarded commit.

## Alternatives considered

### Direct branch routing on one bare repository

This would expose the canonical Bear repo directly to all role agents and depend on each agent using its assigned role branch. It is simpler and closest to the original wording of the multi-agent ADR, but it couples BEARS to Letta's current and future branch behavior and gives each agent broader Git-level visibility.

Rejected in favor of per-agent views.

### Per-agent repos plus Den-maintained canonical copy

This would keep Letta's per-agent repos as runtime truth and have Den periodically copy/import/export memory into a canonical Bear repo. It avoids sidecar view complexity but creates duplicate sources of truth and throwaway replication machinery that does not move us toward the desired invariant.

Rejected as an interim path.

### One repo per Letta agent only

This matches Letta's native shape but does not provide a canonical per-Bear memory history or Git-level role/path enforcement across the Bear.

Rejected because it weakens the core multi-agent memory invariants.

## Open work

- Specify the sidecar mapping API / manifest format.
- Specify exact per-role view repo layout on disk.
- Specify canonical hook implementation and view post-receive forwarding algorithm.
- Specify reconciliation schedule and locking.
- Specify quarantine states and health response schemas.
- Keep the operator override runbook current as override behavior evolves.
- Run an early sanity check that Letta does not actively fight the per-agent view presentation, even though non-`main` branch behavior is no longer gating.

## References

- [BEARS Multi-Agent Architecture for Letta-Backed Coding Agents](multi-agent-architecture.md)
- [BEARS Multi-Agent Bear Spec](../../../services/den/docs/bear-spec.md)
- [Implementation Plan: BEARS Multi-Agent Architecture](../../planning/MULTI_AGENT_IMPLEMENTATION_PLAN.md)
- [MemFS sidecar operator runbook](../MEMFS_SIDECAR_RUNBOOK.md)
