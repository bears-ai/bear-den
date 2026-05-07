# Architecture Decision Records

This directory is the canonical home for BEARS Architecture Decision Records (ADRs).

Use ADRs for cross-cutting product and architecture decisions that are expected to remain useful after a single implementation phase. Use `docs/planning/` for sequencing, milestones, checklists, and active delivery plans. Use `docs/architecture/` for current system descriptions and stable contracts.

## Index

| ADR | Status | Topic |
|-----|--------|-------|
| [acp-boring-waiters.md](acp-boring-waiters.md) | Superseded for ACP direct mode | Historical Codepool-owned ACP client-tool waiters; replaced by direct Den ⇄ adapter local tool runtime |
| [acp-conversation-resolver.md](acp-conversation-resolver.md) | Accepted | Typed ACP conversation/session routing decisions and Letta target boundaries |
| [acp-session-bindings.md](acp-session-bindings.md) | Accepted | ACP sessions as protocol bindings, cwd/list/load/cancel/MCP/auth semantics |
| [artifacts-garage.md](artifacts-garage.md) | Proposed | Artifacts bucket, Garage/S3 storage, Cabinet attachment separation, GC policy |
| [bear-memory-tool-boundary.md](bear-memory-tool-boundary.md) | Accepted | Boundary between Letta Code-native MemFS tools and Den-hosted bear tools |
| [cabinet-reading-pipeline.md](cabinet-reading-pipeline.md) | Proposed | Cabinet document ingestion and reading pipeline |
| [dynamic-skills-subagents.md](dynamic-skills-subagents.md) | Proposed | Dynamic skills, reflection subagents, bear-authored capability growth |
| [memfs-sidecar-repo-views.md](memfs-sidecar-repo-views.md) | Accepted | Canonical Bear MemFS repo plus per-agent sidecar repo views |
| [multi-user-memory.md](multi-user-memory.md) | Proposed | Multi-user memory model and Letta-native memory visibility |
| [provider-safe-tool-naming.md](provider-safe-tool-naming.md) | Accepted | Provider-safe tool names with scoped canonical BEARS tool identities |
| [routines-automation.md](routines-automation.md) | Proposed | Den-managed routines, scheduling, output handling, learning constraints |
| [schema-first-path-strategy.md](schema-first-path-strategy.md) | Accepted | Conservative schema-first path ownership and Den-generated durable artifact paths |
| [semantic-bear-memory.md](semantic-bear-memory.md) | Accepted | Semantic memory model: locality, kind, references, lifecycle, Cabinet spaces, and situation briefings |

## Naming

- File names should be descriptive and stable, for example `artifacts-garage.md`.
- Avoid scattering `*-adr.md` files outside this directory.
- Link from plans or architecture docs to `../architecture/adr/<name>.md` or `adr/<name>.md` depending on location.
