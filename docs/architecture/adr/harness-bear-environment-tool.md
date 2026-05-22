# Harness-Level `bear_environment` Tool — Architecture Decision Record

## Status: Proposed

## Date: 2026-05-22

---

## Context

BEARS runs agents through multiple harnesses and runtime environments, including:

- ACP-backed `pair` sessions via `bears-acp-adapter`;
- API-direct / non-ACP sessions;
- browser/chat-facing and future task/work runtimes;
- future background, watch, and service-executed runtimes.

A recurring need is a reliable, agent-callable summary of the Bear's current operating environment. That summary should answer questions such as:

- what Bear and role is active;
- what runtime or harness is serving this session;
- what session/workspace/tool context is active;
- what services are reachable or degraded;
- what browser capability is currently available;
- when using ACP, what ACP/runtime state is present;
- when using an ACP adapter, what adapter/browser/MCP details are available.

An initial implementation added `bear_environment`-shaped diagnostics inside the ACP adapter and refactored ACP `/status` to render from a shared adapter-local environment snapshot. This proved useful, but also highlighted an architectural problem:

- adapter-local diagnostics may not automatically appear as model-callable tools in every session;
- non-ACP harnesses also need the same capability;
- ACP is a runtime variant, not the owner of the concept.

BEARS therefore needs a harness-level decision about the ownership and shape of `bear_environment`.

---

## Decision

BEARS will treat `bear_environment` as a **harness-owned tool contract**.

This means:

- every BEARS harness should expose a callable `bear_environment` tool;
- the tool contract is runtime-agnostic at the top level;
- ACP-specific facts are included as an environment variant when the current harness/runtime is ACP-backed;
- ACP adapter details are optional enrichment, not the base ownership layer of the tool.

### Stable tool ownership

The canonical ownership of `bear_environment` belongs to the BEARS harness/runtime layer, not to:

- the ACP adapter specifically;
- Den specifically;
- one channel or client surface.

### Stable contract shape

The tool should converge on a stable top-level structure such as:

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

The exact contents may vary by harness, but the semantics of the top-level sections should remain stable.

### ACP-aware variant rule

When the current harness/runtime is ACP-backed, `bear_environment` should include ACP-aware state in an explicit variant subtree, for example:

```json
{
  "environment_variants": {
    "acp": {
      "session": {},
      "runtime": {},
      "permissions": {}
    }
  }
}
```

ACP does not own the tool. ACP contributes a variant.

### Adapter enrichment rule

When ACP adapter-specific data is available, it should be included as optional enrichment, for example:

```json
{
  "environment_variants": {
    "adapter": {
      "name": "bears-acp-adapter",
      "version": "0.1.3",
      "browser": {},
      "mcp": {},
      "host_browser_bridge_env": {}
    }
  }
}
```

If adapter enrichment is not available, `bear_environment` must still return a meaningful result.

### Relationship to `/status`

Status UIs should be derived from the same environment model family.

For ACP-backed surfaces:

- `bear_environment` is the structured, machine-readable environment snapshot;
- `/status` is a compact human rendering of that same environment snapshot family.

This avoids parallel, diverging status implementations.

---

## Provider model

`bear_environment` should be implemented through a harness-owned collection pipeline that merges provider outputs.

### Baseline harness provider

Every harness should provide baseline environment facts it already owns, such as:

- active Bear and role;
- harness/runtime identity;
- session identity;
- workspace/work-surface facts when known;
- callable tool families;
- diagnostics and policy context available at the harness layer.

### ACP provider

ACP-backed harnesses should add an ACP provider that contributes:

- ACP session id;
- conversation resolution/binding;
- ACP runtime phase and active-turn state;
- ACP-specific permission or protocol state when relevant.

### Adapter provider

When an ACP adapter exists and adapter state is accessible, an adapter provider may contribute:

- adapter version/build metadata;
- host browser bridge env visibility;
- browser active-source selection;
- MCP registration/discovery status;
- local fallback/browser capability detection.

### Service providers

Additional provider modules may contribute service health, for example:

- Den reachability/runtime summaries;
- memory service health;
- other runtime backends.

---

## Adapter awareness in Den and harnesses

Den and BEARS harnesses may be aware of ACP adapter state, but only through intentional, bounded interfaces.

Den can already be ACP-aware with respect to:

- ACP session identity;
- runtime state;
- active turns;
- conversation bindings;
- authenticated human and policy context.

Den should not require direct access to arbitrary adapter in-memory implementation details. Adapter facts should be treated as optional provider data exposed through an explicit contract.

### Preferred integration style

Use a **hybrid model**:

- harness-visible baseline state is always available;
- ACP runtime facts come from Den/harness-visible sources when present;
- adapter-specific enrichment is added only when available;
- missing adapter enrichment is explicit and should not break the tool.

---

## Consequences

### Positive

- `bear_environment` becomes a durable capability available across BEARS runtimes.
- ACP-specific details fit naturally as variants rather than owning the concept.
- Non-ACP sessions can still return meaningful environment snapshots.
- Status surfaces can converge on one environment model family.
- Adapter-local diagnostics remain useful as one provider implementation.

### Tradeoffs

- BEARS must define and maintain a shared contract/schema.
- Harnesses will need provider-specific collectors and merge rules.
- Adapter enrichment may involve additional plumbing or reporting interfaces.
- Some sections will be unavailable in some runtimes and must be represented explicitly.

### Rejected alternative

**Make `bear_environment` an ACP adapter tool only.**

Rejected because:
- not all BEARS sessions are ACP-backed;
- adapter-local tool exposure is not guaranteed in every session;
- ACP-specific ownership would distort the conceptual model.

---

## Acceptance criteria

1. Every BEARS harness can expose a callable `bear_environment` tool.
2. Non-ACP sessions return a meaningful baseline environment snapshot.
3. ACP-backed sessions include ACP-aware environment variants.
4. ACP adapter-backed sessions include adapter enrichment when available.
5. Missing adapter enrichment is explicit and non-fatal.
6. ACP `/status` and related status surfaces are rendered from the same environment model family rather than a separate parallel implementation.

---

## Follow-up work

1. Define the shared `bear_environment` contract and status vocabulary in a durable concept/spec document.
2. Identify the canonical harness layer that should own tool exposure in api-direct and other non-ACP runtimes.
3. Design the provider interface and merge semantics for baseline, ACP, adapter, and service providers.
4. Decide how adapter enrichment reaches the harness-level collector.
5. Align existing ACP adapter implementation with the shared contract as a provider implementation.
6. Create a rollout plan covering harness implementation, ACP integration, migration, and documentation.
