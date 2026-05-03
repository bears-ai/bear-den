# Architecture Decision Records

This directory is the canonical home for BEARS Architecture Decision Records (ADRs).

Use ADRs for cross-cutting product and architecture decisions that are expected to remain useful after a single implementation phase. Use `docs/planning/` for sequencing, milestones, checklists, and active delivery plans. Use `docs/architecture/` for current system descriptions and stable contracts.

## Index

| ADR | Status | Topic |
|-----|--------|-------|
| [acp-boring-waiters.md](acp-boring-waiters.md) | Accepted | Codepool-owned process-local ACP client-tool waiters; Den as stateless result proxy |
| [acp-session-bindings.md](acp-session-bindings.md) | Accepted | ACP sessions as protocol bindings, cwd/list/load/cancel/MCP/auth semantics |
| [artifacts-garage.md](artifacts-garage.md) | Proposed | Artifacts bucket, Garage/S3 storage, Cabinet attachment separation, GC policy |
| [bear-memory-tool-boundary.md](bear-memory-tool-boundary.md) | Accepted | Boundary between Letta Code-native MemFS tools and Den-hosted bear tools |
| [cabinet-reading-pipeline.md](cabinet-reading-pipeline.md) | Proposed | Cabinet document ingestion and reading pipeline |
| [dynamic-skills-subagents.md](dynamic-skills-subagents.md) | Proposed | Dynamic skills, reflection subagents, bear-authored capability growth |
| [multi-user-memory.md](multi-user-memory.md) | Proposed | Multi-user memory model and Letta-native memory visibility |
| [routines-automation.md](routines-automation.md) | Proposed | Den-managed routines, scheduling, output handling, learning constraints |

## Naming

- File names should be descriptive and stable, for example `artifacts-garage.md`.
- Avoid scattering `*-adr.md` files outside this directory.
- Link from plans or architecture docs to `../architecture/adr/<name>.md` or `adr/<name>.md` depending on location.
