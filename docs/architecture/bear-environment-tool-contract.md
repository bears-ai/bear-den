# `bear_environment` Tool Contract

This document defines the shared contract for the harness-level `bear_environment` tool.

## Summary

`bear_environment` is a harness-owned Bear Den tool that returns a structured snapshot of the current Bear operating environment as visible to the active runtime.

It should be available across Bear Den runtimes, including non-ACP and ACP-backed sessions.

The tool is intended to help agents and operators answer questions such as:

- what Bear, role, and harness are active;
- what session, conversation, or workspace context is currently attached;
- what tools and services are available;
- what browser capability is active;
- whether the current runtime is healthy, degraded, or missing expected providers;
- when using ACP, what ACP/runtime state is active.

ACP does not own this tool. ACP contributes runtime-specific environment state when present.

## Ownership

`bear_environment` belongs to the harness/runtime layer, not to any single adapter, client, or channel implementation.

That means:

- non-ACP harnesses should expose it;
- ACP-backed harnesses should expose it;
- adapter-local collectors may implement provider logic, but they do not own the contract.

## Top-level shape

The tool should converge on the following top-level structure:

```json
{
  "bear": {},
  "runtime": {},
  "session": {},
  "workspace": {},
  "tools": {},
  "browser": {},
  "services": {},
  "environment_variants": {},
  "diagnostics": {}
}
```

Providers may add fields within these sections, but should preserve the meaning of the top-level keys.

## Section semantics

### `bear`
Trusted identity and role context for the current session/runtime.

Examples:
- bear id
- bear slug
- role name
- role agent id
- human relationship context when appropriate

### `runtime`
Facts about the current harness/runtime.

Examples:
- runtime kind
- runtime family
- version/build when known
- current runtime state

### `session`
Facts about the current active session/turn/conversation binding.

Examples:
- session id
- conversation id
- resolved conversation id
- request id
- active turn summary

### `workspace`
Current workspace or work-surface-adjacent runtime state.

Examples:
- cwd
- workspace roots
- work-surface hints or anchors

### `tools`
Summary of available tool surface.

Examples:
- available tool classes or families
- enabled policy mode
- dynamic tool-source presence
- notable restrictions

### `browser`
User-facing browser capability summary.

Examples:
- whether browser capability is available
- active source
- fallback state
- browser capability warnings

### `services`
Relevant external or backing services.

Examples:
- Den
- memory backing service
- other harness-relevant services

### `environment_variants`
Runtime-specific or provider-specific subtrees.

Examples:
- `acp`
- `adapter`
- future runtime-specific providers

### `diagnostics`
Cross-cutting health summary.

Examples:
- overall status
- warnings
- errors
- likely constraints

## Status vocabulary

Shared status fields should use explicit vocabulary.

Preferred values:

- `ok` — expected and healthy
- `degraded` — partially available, but with important limitation or failure
- `unavailable` — known to be absent or unreachable
- `not_inspected` — not checked in this call/runtime
- `not_applicable` — concept does not apply in this runtime
- `unknown` — provider cannot determine the state

Providers should prefer these values over free-form status strings for machine-relevant status fields.

## Missing-data semantics

The tool must degrade gracefully.

Rules:

- Do not fail the entire tool just because one provider is unavailable.
- Prefer explicit unavailable/not-inspected/not-applicable state over silent omission.
- Omission is acceptable for optional detail fields within a section, but not for the top-level sections themselves.
- Top-level sections should always exist, even if populated minimally.

## ACP variant

When the current runtime is ACP-backed, populate `environment_variants.acp`.

Suggested contents:

```json
{
  "environment_variants": {
    "acp": {
      "status": "ok",
      "session": {
        "acp_session_id": "..."
      },
      "runtime": {
        "active_turn": {},
        "phase": "..."
      },
      "permissions": {}
    }
  }
}
```

If the current runtime is not ACP-backed, prefer:

```json
{
  "environment_variants": {
    "acp": {
      "status": "not_applicable"
    }
  }
}
```

## Adapter variant

When ACP adapter-specific data is available, populate `environment_variants.adapter`.

Suggested contents:

```json
{
  "environment_variants": {
    "adapter": {
      "status": "ok",
      "name": "bears-acp-adapter",
      "version": "0.1.3",
      "browser": {},
      "mcp": {},
      "host_browser_bridge_env": {}
    }
  }
}
```

If the session is ACP-backed but adapter enrichment is unavailable, prefer explicit status such as:

```json
{
  "environment_variants": {
    "adapter": {
      "status": "unavailable"
    }
  }
}
```

## Relationship to status UIs

Where a runtime provides human-facing status surfaces, those should be treated as renderings of the same environment model family.

For ACP-backed runtimes:

- `bear_environment` is the structured snapshot;
- `/status` is a compact human rendering of that environment snapshot family.

Equivalent status UIs in other harnesses should follow the same principle where practical.

## Tool behavior requirements

1. The tool must be safe to call repeatedly.
2. The tool must be read-only.
3. The tool must not require ACP in order to succeed.
4. The tool must not silently pretend ACP/adapter state exists when it does not.
5. The tool should make trust boundaries visible when relevant.

## Initial implementation guidance

The first complete rollout should provide:

- a non-ACP baseline harness implementation;
- ACP variant enrichment from harness-visible runtime state;
- optional adapter enrichment when available;
- explicit degraded behavior when providers are missing.

## Related docs

- [Agent and Bear Environments](AGENT_AND_BEAR_ENVIRONMENTS.md)
- [Harness-Level `bear_environment` Tool ADR](../architecture/adr/harness-bear-environment-tool.md)
- [Implementation plan](../planning/BEAR_ENVIRONMENT_IMPLEMENTATION_PLAN.md)
