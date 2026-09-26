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

- Correct typos and links in existing ADRs; for a genuinely changed decision, add a dated amendment or a superseding ADR instead of rewriting historical rationale or creating a near-duplicate.
- Link to ADRs from architecture, guides, and planning docs when those docs depend on a durable decision.
- Avoid scattering ADR files outside `docs/decisions/`.
- Link ADRs to supporting research notes when the decision depends on a longer comparative analysis.
- A decision's `Accepted` status records intent and rationale, **not** implementation. Link to the [topic map](../README.md) and its current behavior and plan separately.
- New ADRs need a unique number, a `**Topic:**` link to a maintained topic page, and a link from this index. Historical number collisions exist at ADR-0029 and ADR-0034; use full filename/slug when citing those records, and do not renumber them in an unrelated cleanup. New collisions are blocked by `scripts/check-docs.py`.

## New decisions

Add new ADR links here when created. Older decisions remain discoverable by filename and topic references while their statuses and identifiers are reconciled.
