# Docket and task execution

**Owner:** Den Docket code owners.
**Scope:** Den-owned tasks, jobs, session attachments, and background work boundaries.
**Current as of:** 2026-09-26; evidence: `services/den/crates/den-docket/`, `services/den/migrations/`, and `services/den/docs/testing/docket.md`.
**Target:** The [Docket implementation plan](../roadmap/DOCKET_IMPLEMENTATION_PLAN.md) contains both landed work and proposed UX; verify each remaining item before treating it as an active commitment.
**Decisions:** [Jobs and tasks](../decisions/adr-0034-jobs-and-tasks-work-management.md), [session task lists and checkout](../decisions/adr-0045-session-task-lists-and-docket-checkout.md), [Docket-driven turn routing](../decisions/adr-0056-docket-driven-turn-routing.md).

## Current behavior

Docket owns canonical jobs and tasks in Den Postgres. Client-session task attachments are distinct from the `pair` stance; execution authority is bound to tasks, attempts, and runs rather than to that label. Start with [tasks and autonomy](../architecture/tasks-and-autonomy.md) for the conceptual model and [the state-machine inventory](../architecture/den-state-machine-inventory.md) for authority boundaries. The `den-docket` crate owns persistence and the service API.

## Intended behavior and gaps

The [implementation plan](../roadmap/DOCKET_IMPLEMENTATION_PLAN.md) contains historical sequencing alongside remaining ideas; it does not override the current code or this page. Reconcile its status before using an item as delivery guidance. The [focused-execution completion record](../roadmap/pair-execution-authority-and-debug-tracing-plan.md) documents delivered stabilization and compatibility seams.

## Read deeper

- [Task schema overview](../architecture/task-schema.md) — architectural shape, not a database schema reference.
- [Docket test regime](../../services/den/docs/testing/docket.md)
- [Docket execution-attempt design](../../services/den/docs/design/docket-execution-attempts.md) — design/history; check current implementation.
