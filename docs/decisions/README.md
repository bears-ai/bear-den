# Architecture Decision Records

This directory is the canonical home for Bear Den Architecture Decision Records (ADRs).

Use ADRs for cross-cutting product and architecture decisions that are expected to remain useful after a single implementation phase. Use planning documents for sequencing, milestones, checklists, and active delivery plans. Use `docs/architecture/` for conceptual models, stable contracts, and system overviews. Use `docs/guides/` for human-oriented operational and contributor documentation.

## Naming

- ADR files use the form `adr-####-slug.md`.
- `####` is a zero-padded sequential identifier, for example `adr-0001-example.md`.
- The numeric prefix is used for stable ordering in this directory.
- Preserve descriptive slugs after the numeric prefix.

## Status values

Common statuses in this repository include:

- Proposed
- Accepted
- Superseded

## Notes

- Prefer updating an existing ADR over creating near-duplicate decision records.
- Link to ADRs from architecture, guides, and planning docs when those docs depend on a durable decision.
- Avoid scattering ADR files outside `docs/decisions/`.
- Link ADRs to supporting research notes when the decision depends on a longer comparative analysis.
