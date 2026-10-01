# BearWire and ACP

**Owner:** Den BearWire + Armature code owners.
**Scope:** Interactive Den–armature sessions and their ACP client projection.
**Current as of:** 2026-10-01; evidence: `services/den/crates/den-bearwire/src/methods/run.rs`, `services/den/crates/den-bearwire/src/methods/tests.rs`, `services/den/crates/den-core/src/client_tools.rs`, `services/den/crates/den-core/src/effective_policy/tests.rs`, `tools/bear-armature/`, and `tests/smoke/test_stack.py`.
**Target:** See the [protocol refinement roadmap](../roadmap/BEARWIRE_V1_PROTOCOL_REFINEMENT_ROADMAP.md) for proposed changes; its draft status is not evidence that any particular step is still open.
**Decisions:** [BearWire as armature wire](../decisions/adr-0034-bearwire-as-den-armature-wire.md), [ACP as edge adapter](../decisions/adr-0043-acp-as-edge-adapter-protocol-agnostic-core.md), [core client obligations](../decisions/adr-0048-core-turn-client-obligation-coordinator.md).

## Current behavior

Den owns run and obligation state; BearWire carries it to the armature, which projects ACP and executes client-owned tools. Den-hosted tools such as `web_fetch` execute in Den, even when the human supplies their permission through ACP. At `run.start`, Den checks the authenticated human, canonical conversation, and live Work-run binding before deriving the typed armature-conversation or authorized-Work-run origin that controls client tool advertisement and the session-mode permission envelope. A client-supplied mode or descriptor list cannot manufacture an armature or Job grant. The verified origin also enters native model-tool merging; descriptor role metadata and prompt/budget selection still use a profile derived from it. Other runtime and execution paths retain profile-based decisions, so this edge is not a completed stance removal. For implementation-level authority and transitions, use the [state-machine inventory](../architecture/den-state-machine-inventory.md) and the [BearWire compatibility guide](../../services/den/docs/guides/bearwire-compatibility.md). For diagnosis, use [ACP troubleshooting](../guides/acp-troubleshooting.md).

## Intended behavior and gaps

The [protocol refinement roadmap](../roadmap/BEARWIRE_V1_PROTOCOL_REFINEMENT_ROADMAP.md) is a draft design, not the description of the deployed wire. The [completed focused-execution plan](../roadmap/pair-execution-authority-and-debug-tracing-plan.md) is delivery history, not a current roadmap. Check code and validation before promoting any planned statement to this page.

## Read deeper

- [Current architecture overview](../architecture/overview.md)
- [BearWire JSON design specification](../architecture/bearwire-json-spec.md) — marked draft; check against the protocol crate before treating a shape as implemented.
- [Den service implementation map](../../services/den/AGENTS.md)
