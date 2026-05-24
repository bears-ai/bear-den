# Den model registry and Bifrost configuration plan

Status: proposed implementation plan.

This document describes a target architecture in which **Den** owns the canonical model capability registry and **Bifrost** acts as the execution plane. It also describes the current repository state, recommended metadata sources, and a migration path from the current Bifrost-first metadata approach.

Related docs:

- [`../architecture/DEN_ARCHITECTURE.md`](../architecture/DEN_ARCHITECTURE.md)
- [`../deployment/DEPLOYMENT.md`](../deployment/DEPLOYMENT.md)
- [`PLAN.md`](PLAN.md)
- [`PHASE1_DECISIONS.md`](PHASE1_DECISIONS.md)
- [`../../services/bifrost/COOLIFY_DEPLOY.md`](../../services/bifrost/COOLIFY_DEPLOY.md)
- [`../../services/bifrost/config.json`](../../services/bifrost/config.json)
- [`../../services/den/src/core/bifrost.rs`](../../services/den/src/core/bifrost.rs)

---

## Goal

Make **Den** the control-plane owner of the model registry and capability metadata while keeping **Bifrost** as the execution gateway.

The desired steady state is:

```text
Den registry/resolver -> generated Bifrost config -> Letta/runtime execution -> providers
```

More specifically:

1. **Den** owns canonical model identity.
2. **Den** owns model capability metadata such as context window and max output.
3. **Den** owns aliases, lifecycle state, provenance, and confidence.
4. **Bifrost** executes requests against providers using Den-materialized config.
5. **Letta** and BEARS runtime components consume Den/Bifrost-resolved model choices rather than becoming the canonical metadata owner.

---

## Problem statement

The repository already contains a useful bootstrap of this idea, but ownership is currently partially inverted.

### Current state

- `services/bifrost/config.json` contains a BEARS-specific `bears.models` metadata section.
- `services/den/src/core/bifrost.rs` reads Bifrost model metadata and converts it into Den/Letta-facing model options.
- The existing metadata already includes fields such as:
  - `handle`
  - `provider`
  - `model`
  - `display_name`
  - `context_window`
  - `max_output_tokens`
  - `supports_tools`
  - `supports_responses_api`
  - `supports_vision`
  - `enabled`

That is a good bootstrap, but the long-term control-plane boundary should be the opposite:

- **Den decides what models exist and what they mean.**
- **Bifrost executes resolved requests against providers.**

### Why this matters

If Bifrost remains the semantic owner of model metadata, then:

- Den cannot confidently treat model capabilities as control-plane state.
- provenance and confidence become hard to track.
- model aliases and lifecycle management live in the wrong layer.
- migration to additional runtimes becomes harder.
- token budgeting and planning logic must depend on execution-plane artifacts instead of Den-owned state.

---

## Key architectural decision

Treat **model capability metadata** as control-plane state owned by **Den**, and treat **Bifrost configuration** as a materialized execution artifact derived from that state.

### Ownership split

#### Den owns

- canonical model keys
- aliases
- display metadata
- capability metadata
- lifecycle state
- provenance and confidence
- model resolution logic
- future token-budgeting logic
- generation of Bifrost-facing config

#### Bifrost owns

- provider credentials
- provider endpoint wiring
- runtime request execution
- provider routing / failover / weighting where used
- OpenAI-compatible execution surface

This preserves the repo's broader architecture:

- **Den** = control plane
- **Letta** = persistence and agent runtime layer
- **Bifrost** = model execution plane

---

## Canonical naming

Canonical model identifiers should be provider-qualified.

Recommended format:

```text
{provider}/{provider_model_id}
```

Examples:

- `openai/gpt-4.1`
- `openai/gpt-4.1-mini`

This is preferred over bare handles such as:

- `gpt-4.1`
- `gpt-4.1-mini`

### Why provider-qualified keys

1. Avoid collisions across providers.
2. Make registry entries globally unambiguous.
3. Let aliases remain presentation or convenience features instead of identity.
4. Give Den a stable namespace for policy, defaults, and migration.

### Alias policy

Den should maintain aliases separately from canonical identity.

For example, `openai/gpt-4.1` may resolve from aliases like:

- `gpt-4.1`
- `openai-gpt-4.1`

Resolution guidance:

1. exact canonical key match wins
2. unique alias match resolves
3. ambiguous alias match fails loudly
4. deprecated or disabled entries may still resolve internally during migration, but UI exposure should be policy-controlled

---

## Why context window must be first-class

**Context window** should be a first-class field in Den, not just display decoration.

Den needs it for:

1. **Model picker clarity**
   - users and operators need to understand long-context suitability
2. **Token budgeting**
   - Den should be able to estimate prompt fit before execution
3. **Fallback and routing**
   - Den can avoid selecting a model that cannot fit the task
4. **Planning and policy**
   - higher-level logic can intentionally choose between cheaper/smaller and larger-context models
5. **User experience**
   - early warnings are better than late runtime failures

The same control-plane logic applies to:

- `max_output_tokens`
- tool support
- responses API support
- vision support
- future structured-output or reasoning flags

---

## Proposed conceptual data model

The final implementation can vary, but the architecture benefits from three conceptual objects.

### `DenModelRegistryEntry`

A canonical registry entry for one logical model.

Illustrative shape:

```ts
type DenModelRegistryEntry = {
  key: string;                     // "openai/gpt-4.1"
  provider: string;                // "openai"
  provider_model_id: string;       // "gpt-4.1"
  display_name: string;            // "OpenAI GPT-4.1"

  aliases: string[];

  capabilities: {
    context_window: number | null;
    max_output_tokens: number | null;
    supports_tools: boolean | null;
    supports_responses_api: boolean | null;
    supports_vision: boolean | null;
    supports_json_mode: boolean | null;
    supports_reasoning_controls: boolean | null;
    supports_streaming: boolean | null;
  };

  lifecycle: {
    enabled: boolean;
    deprecated: boolean;
    hidden: boolean;
  };

  provenance: {
    sources: Array<{
      source_type: "provider_docs" | "provider_api" | "litellm" | "manual";
      source_ref: string;
      observed_at: string;
      fields: string[];
      confidence: "low" | "medium" | "high";
    }>;
  };
};
```

This is the control-plane answer to:

- what model is this?
- what aliases resolve to it?
- what capabilities does it have?
- how certain are we?
- should it be exposed?

### `DenResolvedExecutionSpec`

A Den-side resolved execution object derived from registry state and policy.

Illustrative shape:

```ts
type DenResolvedExecutionSpec = {
  registry_key: string;            // "openai/gpt-4.1"
  execution_backend: "bifrost";
  provider: "openai";
  provider_model_id: "gpt-4.1";

  capabilities: {
    context_window: number | null;
    max_output_tokens: number | null;
    supports_tools: boolean | null;
    supports_responses_api: boolean | null;
    supports_vision: boolean | null;
  };

  bifrost: {
    handle: string;                // eventually "openai/gpt-4.1"
    provider_key: string;
    model: string;
  };
};
```

This is the bridge between Den’s semantic registry and Bifrost’s runtime format.

### `BifrostExecutionRequest`

A runtime request shape used when execution is routed through Bifrost.

Illustrative shape:

```ts
type BifrostExecutionRequest = {
  model: string;
  messages: unknown[];
  tools?: unknown[];
  stream?: boolean;
  max_tokens?: number;
  temperature?: number;
  response_format?: unknown;
};
```

Bifrost does not need to understand Den’s full registry schema. It only needs a resolved runtime handle and provider mapping.

---

## Source-of-truth and data sourcing strategy

The registry needs a practical sourcing hierarchy.

### Recommended precedence

Use this order of trust where possible:

1. **provider API** when it exposes authoritative capability data
2. **provider documentation** as the main human-auditable source
3. **LiteLLM** as a broad bootstrap and gap-filler
4. **manual Den curation** for incomplete, ambiguous, or contradictory cases

### LiteLLM role

LiteLLM is useful because it:

- aggregates many providers
- normalizes many model names
- often includes context-window-style metadata
- is practical for bootstrap seeding

Recommended usage:

- use LiteLLM to seed candidate entries
- keep provenance showing fields originated from LiteLLM
- treat LiteLLM-derived values as lower confidence than provider-native confirmation unless independently verified

### Provider docs role

Provider docs are often the best auditable source for:

- context window
- max output
- multimodal support
- tool/function-calling support
- deprecation state

### Manual override role

Some providers publish incomplete or inconsistent information. Den therefore needs explicit manual curation with provenance such as:

- source type `manual`
- operator rationale
- timestamp
- fields overridden

Guidance: prefer explicit unknowns (`null`) over invented certainty.

---

## Den -> Bifrost materialization

### Core principle

Bifrost config should be a **materialized view** of Den-owned state, not the canonical origin.

Operational flow:

1. operators or sync jobs update Den registry state
2. Den validates identity, aliases, and capabilities
3. Den resolves the active execution mapping
4. Den generates Bifrost-facing config
5. Bifrost loads that config and executes requests

### Current repository fit

Today `services/bifrost/config.json` contains both:

- provider execution config
- a BEARS-specific `bears.models` metadata block

In the target design, that `bears.models` block becomes generated output.

Illustrative generated entry:

```json
{
  "handle": "openai/gpt-4.1",
  "provider": "openai",
  "model": "gpt-4.1",
  "display_name": "OpenAI GPT-4.1",
  "context_window": 1047576,
  "max_output_tokens": 32768,
  "supports_tools": true,
  "supports_responses_api": true,
  "supports_vision": true,
  "enabled": true
}
```

Long-term guidance: the Bifrost-visible `handle` should match the Den canonical key.

### What Den should generate

Den should generate, conceptually:

1. **execution-facing provider config**
   - provider key mapping
   - provider model ids
   - routing/weights if used
2. **metadata-facing model config**
   - canonical handle
   - display name
   - capability fields
   - enabled state

This may still be emitted as one JSON artifact for Bifrost, but ownership remains Den-side.

---

## Current repository baseline

The existing repository already supports a first bootstrap of the target design.

### Den-side metadata consumer

`services/den/src/core/bifrost.rs` currently defines a `BifrostModelMetadata` struct with fields including:

- `handle`
- `provider`
- `model`
- `display_name`
- `context_window`
- `max_output_tokens`
- `enabled`
- `supports_tools`
- `supports_responses_api`
- `supports_vision`

It also converts that metadata into Den/Letta-facing model options for presentation.

### Bifrost-side metadata producer

`services/bifrost/config.json` already stores BEARS model metadata for entries such as:

- `gpt-4o-mini`
- `gpt-4o`
- `gpt-4.1-mini`
- `gpt-4.1`

That config is useful as a seed source for the Den registry, but it should not remain the long-term semantic owner.

---

## Migration plan

Use a staged migration rather than a flag day.

### Phase 0: accept current bootstrap

Keep the current flow working:

- Bifrost exposes metadata
- Den reads it
- Den uses it for model-selection UX

This is an acceptable temporary state.

### Phase 1: add a Den-owned registry

Introduce a Den-side registry table or config source with:

- canonical key
- aliases
- capabilities
- lifecycle state
- provenance and confidence

Seed it from:

- `services/bifrost/config.json`
- LiteLLM-derived metadata where helpful
- provider docs and manual review

During this phase, Den may still compare against Bifrost metadata for compatibility checks.

### Phase 2: adopt canonical handles

Move from Bifrost handles like:

- `gpt-4.1`

To canonical handles like:

- `openai/gpt-4.1`

Maintain temporary alias compatibility for older references.

### Phase 3: generate Bifrost metadata from Den

Make the BEARS-specific `bears.models` section in Bifrost generated from Den-owned registry state.

Possible operational forms:

- generated file committed by workflow
- generated artifact in deploy pipeline
- admin export / config materialization job

### Phase 4: resolve through `DenResolvedExecutionSpec`

Introduce a Den resolver that transforms:

- requested canonical key or alias
- bear policy or defaults
- feature requirements

Into:

- `DenResolvedExecutionSpec`

Use that spec as the source for:

- UI display
- provisioning choices
- execution references
- Bifrost materialization
- future token-budgeting logic

### Phase 5: treat Den as authoritative

At this stage:

- Bifrost metadata is generated only
- Den is the canonical registry source
- mismatches between Den and Bifrost are treated as drift or deployment errors

---

## Operational guidance

### Keep provenance with the field values

Capability data should always be traceable to a source, such as:

- provider docs URL
- provider API reference
- LiteLLM source ref
- manual operator note

### Prefer `null` over guesses

If a field is not known, store `null` instead of inventing a value.

Examples:

- `max_output_tokens: null`
- `supports_reasoning_controls: null`

### Validate generated config before publish

Before Den publishes Bifrost config, validate:

- unique canonical keys
- no ambiguous aliases
- required provider/model mappings exist
- no disabled entries are selected as active defaults
- generated handles are stable and deterministic

### Surface drift

If Bifrost exposes runtime metadata, Den should compare generated expectations with observed runtime state and surface drift in logs or operator views.

### Treat display labels as presentation only

`display_name` is for UI. Identity should be based on:

- canonical key
- provider model id
- resolved execution mapping

---

## Relationship to Letta and BEARS runtime

Letta should not become the canonical owner of model capability metadata for BEARS.

Instead:

- Den owns the canonical registry and resolution logic
- Letta and related runtime components consume the resolved model choice
- Bifrost remains the execution gateway under the runtime path

This keeps the control-plane / runtime / execution split consistent with the rest of the repository architecture.

---

## Open questions

1. Should the Bifrost `handle` always equal the Den canonical key?
   - recommended long-term answer: yes
2. Should pricing metadata live in the same registry?
   - probably later, not required for the first milestone
3. Should policy be attachable globally, per bear, and per role?
   - likely yes over time
4. Should LiteLLM ingestion be scheduled automatically?
   - useful later, not required for the first implementation
5. Should provider APIs be polled for drift where available?
   - valuable, but not necessary for the first cut

---

## Recommended immediate next steps

1. define a Den-side canonical registry schema
2. seed it from `services/bifrost/config.json`
3. normalize on canonical keys such as `openai/gpt-4.1`
4. attach provenance and confidence to capability fields
5. generate Bifrost metadata from Den-owned state
6. introduce a resolver that emits `DenResolvedExecutionSpec`
7. later, use the same resolver for token budgeting, defaults, and execution routing policy

---

## Summary

The recommended architecture is:

- **Den owns the canonical model registry**
  - context window
  - max output
  - capability flags
  - aliases
  - provenance
  - confidence
  - lifecycle state
- **Bifrost is the execution plane**
  - provider auth
  - provider routing
  - request execution
- **Den materializes Bifrost configuration**
  - including canonical handles such as `openai/gpt-4.1`
- **LiteLLM is a bootstrap source**
  - useful for broad initial coverage
  - not the final authority
- **provider docs and provider APIs are authoritative where possible**
  - with manual Den curation when necessary

This keeps the control-plane / execution-plane boundary clean and sets up BEARS for better model selection, token budgeting, auditability, and future runtime flexibility.
