# Den Model Registry Schema and Sync Spec

Status: proposed implementation spec.

## Objective

Define the first concrete implementation contract for a Den-owned model registry that becomes the canonical source of truth for model capabilities, aliases, and execution metadata, while continuing to materialize a Bifrost-compatible configuration artifact for gateway routing.

This document is narrower and more concrete than `DEN_MODEL_REGISTRY_AND_BIFROST_CONFIG_PLAN.md`. That planning document explains the project shape and migration strategy. This spec defines the data model, source hierarchy, sync behavior, and the Den→Bifrost materialization boundary.

## Related docs

- `docs/planning/DEN_MODEL_REGISTRY_AND_BIFROST_CONFIG_PLAN.md`
- `docs/architecture/DEN_ARCHITECTURE.md`
- `services/den/src/core/bifrost.rs`
- `services/bifrost/config.json`
- `services/bifrost/COOLIFY_DEPLOY.md`

---

## Current repo-grounded baseline

Today, effective model metadata is stored in `services/bifrost/config.json` under the custom `bears.models` section.

That metadata currently includes fields such as:
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

Den currently consumes that metadata through `services/den/src/core/bifrost.rs`, which:
- fetches a JSON payload from `BIFROST_METADATA_URL`
- deserializes `models: Vec<BifrostModelMetadata>`
- filters to enabled models
- sorts models for presentation
- converts each record into a Letta-facing `LettaModelOption`

So the current architecture is effectively:
1. Bifrost config is the source of truth.
2. Bifrost exposes model metadata.
3. Den reads Bifrost’s metadata projection.
4. Den presents a simplified model list to clients.

The desired architecture in this spec inverts that ownership:
1. Den owns the canonical registry.
2. Den resolves aliases, metadata, and execution selection.
3. Den materializes Bifrost config input from that registry.
4. Bifrost serves as execution gateway, not metadata authority.

---

## Design goals

1. Make Den the canonical authority for model identity and capability metadata.
2. Separate canonical model identity from deployment-specific gateway configuration.
3. Preserve enough provenance to distinguish observed, documented, inferred, and manually curated values.
4. Support multiple naming layers:
   - canonical provider-qualified key
   - provider-native model id
   - human-friendly display label
   - local aliases
   - legacy handles
5. Allow Den to expose stable user-facing model options even when Bifrost provider config changes.
6. Allow Bifrost config generation to remain deterministic and auditable.
7. Support future providers beyond OpenAI without changing the schema shape.
8. Keep the first implementation simple enough to ship incrementally.

---

## Canonical identity model

### Canonical key

Each model registry entry should have a canonical provider-qualified key:

- `openai/gpt-4.1`
- `openai/gpt-4.1-mini`
- `openai/gpt-4o`
- `openai/gpt-4o-mini`

This key is the stable Den-side identifier for the conceptual model entry.

### Identity layers

Each model can have several distinct identifiers:

- `key`: canonical Den identifier, provider-qualified
- `provider`: logical provider namespace such as `openai`
- `provider_model_id`: upstream provider’s model id used for execution
- `gateway_handle`: optional Bifrost-facing handle when different from the provider model id
- `display_name`: user-facing label
- `aliases`: alternative Den-resolvable names
- `legacy_handles`: historical names preserved for migration compatibility

These layers should not be conflated.

In the current repo state, `handle` and `model` are usually the same string. This spec treats that as an implementation convenience, not a permanent schema assumption.

---

## Core schema

## `DenModelRegistryEntry`

This is the canonical persisted registry record.

```json
{
  "key": "openai/gpt-4.1",
  "provider": "openai",
  "provider_model_id": "gpt-4.1",
  "gateway": {
    "bifrost": {
      "handle": "gpt-4.1",
      "enabled": true
    }
  },
  "display_name": "OpenAI GPT-4.1",
  "family": "gpt-4.1",
  "release_channel": "general",
  "aliases": ["gpt-4.1", "openai:gpt-4.1"],
  "legacy_handles": [],
  "capabilities": {
    "context_window": {
      "value": 1047576,
      "provenance": "provider_docs",
      "confidence": "high"
    },
    "max_output_tokens": {
      "value": 32768,
      "provenance": "provider_docs",
      "confidence": "high"
    },
    "supports_tools": {
      "value": true,
      "provenance": "manual_curated",
      "confidence": "medium"
    },
    "supports_responses_api": {
      "value": true,
      "provenance": "manual_curated",
      "confidence": "medium"
    },
    "supports_vision": {
      "value": true,
      "provenance": "manual_curated",
      "confidence": "medium"
    }
  },
  "status": {
    "enabled": true,
    "selectable": true,
    "deprecated": false
  },
  "sources": [
    {
      "kind": "provider_docs",
      "ref": "https://platform.openai.com/docs/models",
      "observed_at": "2026-05-19T00:00:00Z"
    },
    {
      "kind": "manual_curated",
      "ref": "repo bootstrap from services/bifrost/config.json",
      "observed_at": "2026-05-19T00:00:00Z"
    }
  ],
  "notes": null
}
```

### Required top-level fields

- `key: string`
- `provider: string`
- `provider_model_id: string`
- `display_name: string`
- `capabilities: object`
- `status: object`

### Recommended optional fields

- `gateway.bifrost.handle: string`
- `family: string`
- `release_channel: string`
- `aliases: string[]`
- `legacy_handles: string[]`
- `sources: SourceAttribution[]`
- `notes: string | null`

---

## Capability value envelope

Capability values should not be stored as naked scalars in the canonical registry when they are sourced from outside Den. Instead, each tracked capability should use a small envelope.

## `CapabilityValue<T>`

```json
{
  "value": 32768,
  "provenance": "provider_docs",
  "confidence": "high",
  "observed_at": "2026-05-19T00:00:00Z",
  "source_ref": "https://platform.openai.com/docs/models"
}
```

### Fields

- `value`: typed capability value
- `provenance`: one of
  - `provider_api`
  - `provider_docs`
  - `litellm_bootstrap`
  - `manual_curated`
  - `inferred`
- `confidence`: one of
  - `high`
  - `medium`
  - `low`
- `observed_at`: optional timestamp
- `source_ref`: optional string reference

### Why this exists

This envelope allows Den to:
- merge multiple sources without losing origin
- expose confidence to operators
- prefer documented or directly observed values over inherited guesses
- re-run sync and identify stale values later

For the first implementation, not every field must be populated. But the shape should exist from the beginning so migration does not require schema churn.

---

## Source attribution model

## `SourceAttribution`

```json
{
  "kind": "provider_docs",
  "ref": "https://platform.openai.com/docs/models",
  "observed_at": "2026-05-19T00:00:00Z",
  "details": "context window and output token limits recorded manually"
}
```

### Fields

- `kind`
- `ref`
- `observed_at`
- `details` optional

This should exist both:
- at the entry level for broad lineage
- optionally inside individual capability envelopes for precise field-level attribution

---

## Resolved execution shape

The registry entry is canonical storage. Execution requires a resolved runtime shape.

## `DenResolvedExecutionSpec`

This is the Den-side runtime object produced after alias resolution, policy filtering, and gateway selection.

```json
{
  "requested_name": "gpt-4.1",
  "resolved_key": "openai/gpt-4.1",
  "provider": "openai",
  "provider_model_id": "gpt-4.1",
  "display_name": "OpenAI GPT-4.1",
  "gateway_target": {
    "kind": "bifrost",
    "handle": "gpt-4.1"
  },
  "capabilities": {
    "context_window": 1047576,
    "max_output_tokens": 32768,
    "supports_tools": true,
    "supports_responses_api": true,
    "supports_vision": true
  },
  "selection_metadata": {
    "resolved_via": "alias",
    "confidence": "high"
  }
}
```

### Notes

This object intentionally flattens capability envelopes into executable values.

Execution code should not need to reason about provenance on the hot path. Provenance belongs in the canonical registry and operator/debug surfaces.

### Required fields

- `requested_name`
- `resolved_key`
- `provider`
- `provider_model_id`
- `gateway_target`
- `capabilities`

---

## Bifrost materialization shape

Den should not persist raw Bifrost config as its canonical model state. Instead, it should derive a Bifrost-specific projection.

## `BifrostExecutionRequest`

This is the minimal runtime request Den effectively needs in order to call Bifrost.

```json
{
  "model": "gpt-4.1",
  "provider": "openai",
  "canonical_key": "openai/gpt-4.1"
}
```

In practice, chat/completions payloads will include many additional fields, but for model-resolution purposes the important point is that Bifrost receives a Bifrost-visible model handle, not the full canonical Den registry entry.

## `BifrostMaterializedModelConfig`

This is the generated config fragment Den would emit into `services/bifrost/config.json` or an equivalent generated artifact.

```json
{
  "handle": "gpt-4.1",
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

This shape is intentionally close to the current `bears.models[]` entries already consumed by Den and served via Bifrost metadata.

That reduces migration cost while still shifting authority upstream into Den.

---

## Alias resolution behavior

Den should resolve model requests using a deterministic precedence order.

### Resolution order

1. Exact canonical key match
   - example: `openai/gpt-4.1`
2. Exact alias match
   - example: `gpt-4.1`
   - example: `openai:gpt-4.1`
3. Exact legacy handle match
4. Optional future policy-based defaulting
   - example: family alias like `gpt-4.1-default`

### Constraints

- Alias collisions must be rejected at registry validation time.
- A canonical key cannot also be a different entry’s alias.
- Deprecated aliases may continue to resolve, but the resolution result should record that a deprecated identifier was used.

---

## Data sourcing hierarchy

The canonical registry may combine several sources, but their trust ranking should be explicit.

### Preferred precedence for capability values

1. `provider_api`
2. `provider_docs`
3. `litellm_bootstrap`
4. `manual_curated`
5. `inferred`

### Interpretation

#### `provider_api`
Best when a provider exposes a machine-readable authoritative models endpoint including capability metadata.

#### `provider_docs`
Preferred fallback when docs provide clearer or more current limits than APIs.

#### `litellm_bootstrap`
Useful for broad initial coverage or backfilling many provider/model pairs quickly, but should usually not outrank direct provider information.

#### `manual_curated`
Useful for repo bootstrap, operator corrections, or temporary overrides when upstream data is incomplete.

#### `inferred`
Allowed only for soft assumptions and should not silently override better sources.

### First implementation recommendation

Bootstrap from the repo’s existing `services/bifrost/config.json` values as `manual_curated`, then selectively upgrade fields as better evidence is gathered.

That is the fastest path to invert source-of-truth ownership without blocking on a perfect discovery pipeline.

---

## Validation rules

The registry compiler should reject invalid state before materializing any Bifrost config.

### Entry validation

Each entry must satisfy:
- non-empty `key`
- non-empty `provider`
- non-empty `provider_model_id`
- `key` prefix matches `provider/`
- `display_name` non-empty
- `context_window` positive if present
- `max_output_tokens` positive if present

### Global validation

Across the whole registry:
- canonical keys are unique
- aliases are globally unique
- legacy handles are globally unique unless explicitly tombstoned
- no generated Bifrost handle collisions
- provider references used by generated models exist in the Bifrost provider section

### Materialization validation

Before emitting Bifrost config:
- every enabled generated model has a Bifrost handle
- every generated model maps to a provider declared in the generated or static provider config
- provider secret references are available by name, even if their values live only in environment configuration

---

## Sync and materialization pipeline

The intended pipeline is:

1. Read canonical Den registry source.
2. Merge data-source overlays if configured.
3. Validate canonical entries and alias uniqueness.
4. Produce runtime index for Den resolution.
5. Materialize Bifrost-facing model config.
6. Write generated artifact or expose generated payload for deployment tooling.
7. Optionally expose Den-native registry read APIs.

### Phase 1 recommended implementation split

#### Den-owned source artifact
A checked-in Den-side registry file, for example:
- `services/den/model_registry.json`
- or `services/den/config/model_registry.json`

#### Generated Bifrost artifact
A generated file, for example:
- `services/bifrost/config.generated.json`
- or regeneration of `services/bifrost/config.json` with a documented ownership boundary

#### Compiler step
A small Den-side tool or script that:
- reads canonical registry JSON
- reads static provider-secret mapping configuration
- emits Bifrost-compatible model metadata projection

### Recommended ownership split

The cleanest medium-term split is:
- Den owns model metadata and selection policy.
- Bifrost config owns provider credentials, provider routing config, and gateway-local operational flags.
- Generated output bridges the two.

---

## Relationship to existing Bifrost config

The current `services/bifrost/config.json` mixes at least three concerns:

1. gateway operational settings
   - `client.disable_db_pings_in_health`
2. provider credential/routing settings
   - `providers.openai.keys[]`
3. model metadata presented to Den and users
   - `bears.models[]`

This spec proposes that only the third concern moves under Den canonical ownership first.

That means phase 1 does not need to redesign all Bifrost config generation.
It only needs to make `bears.models[]` derived from the canonical Den registry.

A later phase may also generate more of the provider mapping layer, but that is not required to establish the new architecture.

---

## Den API behavior

Once the registry exists, Den should stop treating Bifrost metadata as authoritative for model-selection UX.

### Desired behavior

Den should be able to:
- list canonical/selectable models from its own registry
- resolve aliases locally without making a Bifrost metadata request
- optionally compare local registry state with Bifrost-exposed metadata for drift detection

### Compatibility path

During migration, Den may continue supporting the existing Bifrost metadata fetch path as a fallback or validation mechanism, but canonical model selection should move toward local registry reads.

---

## Drift detection

Once Den owns the registry, Bifrost metadata can still be useful as a verification surface.

Examples of drift checks:
- generated Bifrost handle missing from `/bears/models`
- context window mismatch between generated artifact and Bifrost-served metadata
- enabled model missing from gateway-visible list

This is especially useful because `services/bifrost/COOLIFY_DEPLOY.md` documents a file-based GitOps deployment model where config files are mounted into the Bifrost container. Generated artifacts can drift from deployed state if deploys are partial or stale.

---

## Migration path

### Phase 1: schema introduction

- Add canonical Den registry file.
- Mirror the existing four OpenAI entries from `services/bifrost/config.json`.
- Mark imported values primarily as `manual_curated` with repo-source references.

### Phase 2: compiler and generated projection

- Add a generator that emits Bifrost `bears.models[]` entries.
- Keep provider sections hand-managed.
- Validate that generated output is semantically equivalent to the current checked-in config.

### Phase 3: Den local resolution

- Update Den model-listing logic to read the canonical registry directly.
- Keep Bifrost metadata fetch available for verification or temporary fallback.

### Phase 4: richer sourcing

- Add provider docs/API harvesting where available.
- Upgrade field provenance and confidence.
- Add drift reporting and stale-data detection.

---

## Open questions

1. Should canonical registry storage live in Den service code, shared config, or a docs/config area with generation into both services?
2. Should Bifrost handles be required to equal provider model ids in phase 1, or merely default to them?
3. How much provider-specific execution metadata belongs in the canonical registry versus gateway/provider config?
4. Should Den expose provenance/confidence to end users, operators only, or not at all initially?
5. Should aliases be globally unique across all providers forever, or only within a provider namespace unless explicitly promoted?

---

## Recommended first concrete implementation

Implement the smallest useful slice:

1. Create `DenModelRegistryEntry` JSON for the current four OpenAI models.
2. Preserve current capability values and map them as imported manual curation.
3. Add a generator for Bifrost `bears.models[]`.
4. Keep current provider config untouched.
5. Add a Den-side resolver that can map:
   - canonical key
   - alias
   - legacy handle
6. Change Den model listing to prefer the local registry.

This yields the architecture change that matters most: Den becomes the authority for model identity and metadata, while Bifrost remains the gateway executor.
