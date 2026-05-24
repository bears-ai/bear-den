# Work-Surface Memory Scaffolding Plan

## Purpose

This document defines the first implementation slice for making BEARS memory easier to use across multiple **work surfaces** within a single Bear and Workplace.

A **Workplace** is the role-scoped memory surface such as `pair`, `talk`, `curate`, `work`, or `watch`. A **work surface** is the durable Bear-level work setting described in `docs/architecture/adr/bear-workplaces.md`. Examples include repositories, services, deployments, Missions, projects, and other coherent long-running scopes of work.

The problem this plan addresses is simple: memory is Bear-scoped, but user questions are often work-surface-specific. Without predictable work-surface anchors, an agent may search all Bear memory and answer from the wrong slice of knowledge.

## Goals

- make local grounding work-surface-first instead of Bear-flat,
- give any agent/model predictable anchor paths,
- separate Bear-global memory from work-surface-local memory,
- reduce dependence on ad hoc memory search,
- and prepare for future trusted work-surface hints and orientation helpers.

## Canonical memory topology

### Pinned orientation entrypoint

To make this topology reliably legible to agents that otherwise discover `pair/...` progressively and on demand, add a small pinned orientation file such as:

```text
system/memory-map.md
```

or:

```text
system/work-surfaces.md
```

That file should stay compact and explain:

- the major memory regions and which are pinned versus progressive/on-demand,
- where work-surface anchors live,
- which files are the preferred starting points for local understanding,
- and how role-local memory relates to shared `core/` orientation.

Where supported, the pinned orientation file should link explicitly to canonical anchors with wiki-style links such as `[[core/work_surfaces/<work_surface_slug>/overview.md]]`.

### Bear-global shared anchors

```text
core/bear-overview.md
core/bear-glossary.md
core/shared-conventions.md
core/work_surfaces/index.md
```

### Work-surface-shared canonical anchors

```text
core/work_surfaces/<work_surface_slug>/index.md
core/work_surfaces/<work_surface_slug>/overview.md
core/work_surfaces/<work_surface_slug>/glossary.md
core/work_surfaces/<work_surface_slug>/architecture.md
core/work_surfaces/<work_surface_slug>/decisions.md
core/work_surfaces/<work_surface_slug>/conventions.md
```

`overview.md` should be treated as the primary anchor for a work surface: compact, link-oriented, and suitable as the first file an agent reads before following deeper links.

### Role-local work-surface working memory

```text
pair/work_surfaces/<work_surface_slug>/current-understanding.md
pair/work_surfaces/<work_surface_slug>/recent-findings.md
pair/work_surfaces/<work_surface_slug>/open-questions.md
```

## Minimal scaffold for a new work surface

The first implementation should keep the mandatory scaffold small:

```text
core/work_surfaces/<work_surface_slug>/index.md
core/work_surfaces/<work_surface_slug>/overview.md
core/work_surfaces/<work_surface_slug>/glossary.md
pair/work_surfaces/<work_surface_slug>/current-understanding.md
```

Optional files such as `architecture.md`, `decisions.md`, `conventions.md`, `recent-findings.md`, and `open-questions.md` can be added immediately or later.

## Retrieval policy

For questions about the current project, service, repository, architecture, terminology, or prior local decisions, agents should prefer this order:

1. current conversation and trusted situation/session briefing,
2. current Workplace and current work-surface hints,
3. current work-surface canonical anchors,
4. current work-surface role-local working memory,
5. Bear-global shared anchors,
6. broader Bear memory search,
7. local workspace artifacts,
8. general world knowledge.

## Prompt/runtime guidance goals

ACP and related prompts should explicitly remind agents that:

- memory is Bear-scoped across Workplaces and may span multiple work surfaces,
- local-understanding questions should be grounded in the current work surface within the current Workplace,
- and canonical work-surface anchors should be preferred over generic memory search when available.

## Initial implementation slices

### Slice A
- update docs to make the Workplace vs work-surface distinction explicit,
- add ACP reminder text for work-surface-first retrieval,
- add tests that lock in the new prompt guidance.

### Slice B
- add a deterministic scaffold creation path for new work surfaces,
- register new work surfaces in `core/work_surfaces/index.md`.

### Slice C
- add trusted session/situation hints for the current work surface when available.

### Slice D
- add a read-only work-surface orientation affordance or helper tool.

## Success criteria

This plan succeeds when:

- a Bear can represent multiple Workplaces and multiple work surfaces clearly,
- agents can find local understanding through stable work-surface anchor paths,
- prompt guidance teaches work-surface-first retrieval within the current Workplace,
- a compact pinned orientation file points agents toward work-surface anchors rather than relying on lucky subtree discovery,
- `overview.md` files serve as reliable first-read entrypoints for each work surface,
- and future implementation slices can add scaffold creation and runtime hints without changing the conceptual model.
