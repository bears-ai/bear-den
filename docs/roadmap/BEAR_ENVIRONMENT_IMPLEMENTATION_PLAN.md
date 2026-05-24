# `bear_environment` Harness Rollout Implementation Plan

**Status:** Draft  
**Date:** 2026-05-22  
**Related ADR:** `docs/architecture/adr/harness-bear-environment-tool.md`

## Goal

Implement `bear_environment` as a harness-level BEARS tool that is available across runtimes, with ACP-aware variants when using ACP and optional adapter/browser/MCP enrichment when an ACP adapter is present.

## Scope

This plan covers:

- shared contract stabilization;
- harness-level ownership and exposure;
- provider-based collection;
- ACP-aware integration;
- migration from current adapter-local implementation;
- alignment of `/status`-style surfaces with the shared environment model.

This plan does **not** require all optional enrichment to ship in the first slice.

## Phase 1: Shared contract and vocabulary

### Tasks

- Create a durable shared contract/spec for `bear_environment` in `docs/concepts/` or equivalent.
- Define stable top-level sections:
  - `bear`
  - `runtime`
  - `session`
  - `workspace`
  - `tools`
  - `browser`
  - `services`
  - `environment_variants`
  - `diagnostics`
- Define shared status vocabulary, for example:
  - `ok`
  - `degraded`
  - `unavailable`
  - `not_inspected`
  - `not_applicable`
- Define missing/unavailable semantics explicitly.

### Acceptance criteria

- A documented contract exists independent of ACP adapter implementation.
- All future providers can target the same schema.

## Phase 2: Harness ownership and exposure

### Tasks

- Identify the canonical harness/tool exposure layer for:
  - api-direct sessions
  - ACP-backed sessions
  - future non-ACP harnesses
- Implement `bear_environment` as a harness-owned tool in that layer.
- Ensure the tool is exposed to agents in non-ACP sessions.
- Ensure the ACP adapter is no longer the only location where the capability exists.

### Acceptance criteria

- `bear_environment` is callable in at least one non-ACP harness.
- ACP and non-ACP harnesses share the same tool name.

## Phase 3: Baseline provider implementation

### Tasks

- Implement a baseline environment collector that can run without ACP.
- Populate baseline fields from harness-visible state:
  - bear identity/role
  - runtime kind/state
  - session identifiers
  - workspace root/cwd when known
  - available tool families
  - high-level diagnostics
- Add tests for baseline non-ACP output.

### Acceptance criteria

- Non-ACP sessions return a meaningful `bear_environment` snapshot.
- The tool does not depend on ACP-specific services to function.

## Phase 4: ACP provider integration

### Tasks

- Add an ACP provider that contributes:
  - ACP session id
  - conversation resolution/binding
  - active-turn/runtime phase
  - ACP-specific permission/protocol context when appropriate
- Map ACP provider output into `environment_variants.acp`.
- Ensure missing ACP state results in explicit unavailable fields rather than hard failure.

### Acceptance criteria

- ACP-backed runs enrich `bear_environment` with ACP-specific data.
- Non-ACP runs remain unaffected.

## Phase 5: Adapter provider integration

### Tasks

- Define the adapter provider contract for optional enrichment.
- Decide transport strategy for adapter enrichment:
  - push to Den,
  - query on demand,
  - or hybrid.
- Recommendation: implement hybrid behavior.
- Populate adapter provider fields such as:
  - adapter version/build
  - browser active source
  - MCP registration/discovery state
  - host browser bridge env summary
  - local fallback/browser capability state
- Preserve useful existing adapter-side collector code by converting it into the provider implementation.

### Acceptance criteria

- ACP adapter-backed runs include adapter enrichment when available.
- Lack of adapter enrichment does not prevent `bear_environment` from succeeding.

## Phase 6: Status surface alignment

### Tasks

- Align ACP `/status` with the harness-level environment model family.
- Ensure `/status` remains a human rendering of the same environment snapshot family used by `bear_environment`.
- Evaluate whether other status/doctor/runtime surfaces should also use the shared collector or a shared rendering layer.

### Acceptance criteria

- `/status` and `bear_environment` do not diverge semantically.
- Degraded backends still produce useful status output.

## Phase 7: Migration and cleanup

### Tasks

- Reclassify current ACP adapter `bear_environment` implementation as adapter-provider code.
- Remove or reduce duplicate environment/status assembly paths where practical.
- Update docs across concepts, architecture, and troubleshooting.
- Add integration tests for:
  - non-ACP baseline behavior
  - ACP behavior without adapter enrichment
  - ACP behavior with adapter enrichment
  - degraded Den or service reachability

### Acceptance criteria

- The final architecture matches the ADR.
- Duplicate environment logic is minimized.
- Documentation reflects harness ownership and ACP-aware variants.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Tool schema drifts across runtimes | Establish contract/spec first and test provider outputs against it |
| ACP adapter remains the accidental source of truth | Move ownership to harness exposure layer early |
| Adapter enrichment becomes tightly coupled to Den internals | Use explicit provider contract and hybrid enrichment approach |
| Non-ACP implementation lags and leaves the tool ACP-only in practice | Require a non-ACP baseline slice before calling the work complete |
| Status rendering diverges again | Treat `/status` as a rendering of the environment model family and test rendered semantics |

## Recommended task breakdown

1. Write shared contract/spec.
2. Identify harness tool exposure layer.
3. Implement non-ACP baseline collector and exposure.
4. Add ACP provider.
5. Add adapter enrichment provider.
6. Unify status rendering around the same environment model family.
7. Expand tests and update docs.

## Summary

This rollout should move `bear_environment` from:

- adapter-local diagnostic implementation

into:

- harness-level BEARS capability with ACP-aware and adapter-enriched variants.

That preserves the useful ACP work already done while aligning the capability with the broader BEARS runtime architecture.
