# Bear Capability Configuration, Connections, and Portability Implementation Plan

**Status:** Proposed

**Decision source of truth:** [ADR-0055 — Bear Capability Configuration, Connections, and Portability](../decisions/adr-0055-bear-capability-configuration-connections-and-portability.md)

**Related plans:**

- [Capability discovery and Code Mode](CAPABILITY_DISCOVERY_AND_CODE_MODE_IMPLEMENTATION_PLAN.md)
- [Skills implementation](SKILLS_IMPLEMENTATION_PLAN.md)
- [Phase 1 native product debt](PHASE1_NATIVE_PRODUCT_DEBT_PLAN.md)
- [Den-BearWire-armature-ACP hardening](DEN_BEARWIRE_ARMATURE_ACP_HARDENING_PLAN.md)
- [Host browser MCP bridge](HOST_BROWSER_MCP_BRIDGE_IMPLEMENTATION_PLAN.md)

## Goal

Make Bears portable while keeping authority local to each Bear. A Bear added to Den should bring its bundled skills and capability preferences with it, but should not bring secrets, live sessions, or authority that belongs to another Bear or environment.

The intended product behavior is:

- skills bundled with a Bear just work for that Bear after import/add;
- bundled skills do not automatically affect other Bears;
- connections are per Bear;
- capability configuration, including enablement, is per Bear;
- stances may override the Bear's default capability configuration;
- capability discovery reports what exists, what is enabled, what is available now, and what is missing.

## Non-goals

- Do not create a global skill install that changes all Bears.
- Do not export secrets, OAuth tokens, local paths as authority, live armature sessions, or MCP process state with portable Bears.
- Do not make catalog presence imply invocation authority.
- Do not build a rich policy language before exact refs, simple patterns, and stance overrides prove insufficient.
- Do not solve all connection-provider UX in the first slice.

## Design constraints

- Separate capability definitions, capability instances, Bear capability configuration, and stance overrides.
- Dispatcher authorization remains the enforcement point.
- Capability discovery is explanatory; it is not a grant mechanism.
- Connections are Bear-scoped authority bindings.
- Bear-bundled skills are Bear-scoped behavior/procedure capabilities, not global Den behavior.
- Stance overrides should restrict or tune Bear capability configuration by default. They should not silently grant authority the Bear does not have.
- Session-local armature/browser/MCP capabilities are runtime instances, not portable Bear-owned abilities.

## Phases

### Phase 0 — Schema and config sketch

1. Define minimal records for:
   - `bear_capability_config`,
   - `bear_stance_capability_overrides`,
   - `bear_connections`,
   - `bear_connection_requirements`,
   - `bear_skills` or the equivalent Bear-scoped skill attachment table.
2. Decide whether first-slice config stores exact refs only or exact refs plus simple wildcard/tag patterns.
3. Define common fields:
   - Bear id,
   - stance where applicable,
   - capability ref/pattern/abstract requirement,
   - enabled/disabled,
   - approval requirement,
   - scope/surface hints,
   - provider preference,
   - created/updated metadata.
4. Add Rust model types before wiring runtime behavior.

**Exit criteria:** checked-in schema/model sketch exists and names the persistence boundaries without changing authorization behavior.

### Phase 1 — Bear-bundled skills import and attachment

1. Extend the Bear add/import path to accept bundled skills by ref/content/hash/version.
2. Import or deduplicate bundled skill content inside Den.
3. Attach imported skills to the new Bear only.
4. Mark bundled skills enabled according to the Bear definition.
5. Ensure no other Bear sees behavior changes from the import.
6. Add simple validation for malformed skill bundles.

**Exit criteria:** adding a Bear with bundled skills makes those skills available to that Bear without prompting and without affecting other Bears.

### Phase 2 — Per-Bear capability configuration

1. Persist Bear-level capability enablement and approval posture.
2. Add basic read/update UI or API surfaces for Bear capability config.
3. Teach capability discovery to include Bear-level status:
   - enabled,
   - disabled,
   - approval required,
   - not configured.
4. Keep dispatcher enforcement conservative: if config is missing or disabled for a risky/non-default capability, do not allow invocation.
5. Add tests proving catalog presence does not grant use.

**Exit criteria:** each Bear can have independent capability settings, and discovery reflects those settings.

### Phase 3 — Stance overrides

1. Add per-Bear, per-stance override persistence.
2. Implement merge semantics:
   - Bear default config first,
   - stance override second,
   - runtime availability last,
   - turn-local approval gates still apply.
3. Enforce the first-slice safety rule: stance overrides may restrict/tune, but may not silently grant authority absent at Bear level.
4. Display effective capability state for a Bear+stance.
5. Add tests for `chat`, `pair`, `work`, and `curate` divergence.

**Exit criteria:** stance-specific behavior can differ without cloning Bear configs and without accidental grants.

### Phase 4 — Bear-scoped connections and requirements

1. Add Bear-scoped connection records.
2. Add connection requirement records to portable Bear definitions.
3. Model connections as satisfying capability requirements or provider-backed capability instances.
4. Start with minimal providers needed soonest, likely MCP server bindings and common external services.
5. Ensure credentials/secrets are stored in the existing secret-management path or provider-specific secure storage, never in Bear export blobs. Credential-backed tools must follow the active hats plan's [runtime-mediated credential and external key-management boundary](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#runtime-mediated-credentials-and-external-key-management-planned): models request operations, trusted runtime adapters resolve and inject credentials after live authorization, and raw keys never become model-visible tool inputs/results or ambient credentials for model-controlled processes. That active gate owns backend selection, migration, rotation/revocation and exposure tests; this proposed plan must not create a parallel credential authority.
6. Add import UX/API output that reports missing required/recommended connections.

**Exit criteria:** a Bear can declare it needs a provider connection; import restores the requirement but not the secret, and capability discovery can report missing connection state.

### Phase 5 — MCP catalog and Bear/stance attachments

This phase absorbs the former standalone Phase 1 MCP catalog/attachment slice. It owns the general product model for MCP servers; provider-specific implementation, including the host-browser bridge, remains in its own plan.

1. Inventory existing MCP configuration and discovery data from armatures, Den descriptors, and Bear configuration.
2. Define the smallest MCP catalog record that reuses capability definitions where possible: name, description, source kind, transport, version/pin, required secret/config fields (never values), stance/surface compatibility, risk class, descriptors, and diagnostics.
3. Add Bear/stance attachment records with configuration references and policy. Keep catalog record, Bear attachment, and runtime-discovered tool instance distinct.
4. Add operator catalog/detail and attachment-status UI/API: configured, secrets missing, unreachable, discovery failed, no tools, or tools discovered.
5. Feed only configured, discovered, stance- and surface-compatible attachments into descriptor routing. A web turn must not inherit an armature-local MCP tool merely because it is attached for `pair` or `work`.
6. Keep MCP attachments subject to normal stance policy and dispatcher authorization; attachment is not an authority grant.
7. Add a smoke check proving a web chat turn does not receive an armature-local MCP tool solely because of a pair/work attachment.

**Exit criteria:** operators can attach an MCP catalog entry to a Bear/stance, see actionable configuration and discovery diagnostics, and receive only compatible descriptor projections.

### Phase 6 — Capability discovery integration

1. Update `capability_search` and `capability_describe` to merge:
   - catalog definition,
   - Bear capability config,
   - stance override,
   - Bear-scoped connections,
   - current runtime instances.
2. Include concise status fields:
   - `catalog_status`,
   - `bear_config_status`,
   - `stance_effective_status`,
   - `availability_status`,
   - `missing_requirements`.
3. Keep the result short enough for model use.
4. Add examples for:
   - bundled skill available to this Bear,
   - MCP tool exists but connection missing,
   - armature-local tool available only in current pair session,
   - capability disabled in `chat` but enabled in `pair`.

**Exit criteria:** discovery can explain why a capability can or cannot be used by the current Bear in the current stance.

### Phase 7 — Bear portability export/import

1. Define portable Bear export shape for:
   - bundled skills,
   - capability configuration,
   - stance overrides,
   - connection requirements,
   - optional/recommended capability requirements.
2. Explicitly exclude:
   - secrets,
   - live connections,
   - live MCP server state,
   - local paths as durable authority,
   - armature/browser/session instances.
3. Add import validation and clear degraded-state reporting.
4. Add round-trip tests.

**Exit criteria:** a Bear can be exported/imported with skills and capability posture intact, while external authorities must be reconnected per Bear.

## Minimal first slice

The smallest useful implementation is:

1. Bear-scoped skill attachment/import for bundled skills;
2. a simple per-Bear capability config table keyed by exact capability ref;
3. a simple per-Bear, per-stance override table;
4. discovery status text that reports enabled/disabled/missing connection for the current Bear+stance;
5. tests proving importing a Bear with skills affects only that Bear.

`ponytail:` The first slice can use exact refs and simple JSON config blobs instead of a generalized policy engine. The ceiling is awkward matching for abstract capabilities like `capability.issue_tracker.create`. The upgrade path is explicit requirement resolution and provider predicates after MCP and skills use cases are concrete.

## Documentation and model experience

For every delivered phase, review and update [`MODEL_EXPERIENCE.md`](../../MODEL_EXPERIENCE.md) when the model-facing discovery, descriptor, authority, or failure behavior changes. Create or update the relevant operator and user documentation under [`docs/guides`](../guides), including connection setup, MCP attachment, portability/import degradation, and diagnostics. Documentation must distinguish catalog presence, Bear attachment, runtime availability, and invocation authority.

## Verification strategy

- Unit tests for config merge semantics.
- Import tests proving bundled skills attach only to the imported Bear.
- Authorization tests proving catalog presence is not enough to invoke a capability.
- Stance tests proving `chat` can disable a capability that `pair` may use.
- Connection tests proving one Bear's connection is not visible to another Bear.
- Portability round-trip tests proving secrets/live instances are excluded.
- Discovery tests for enabled, disabled, missing connection, and session-local availability states.

## Open questions

- Should first-slice capability config use SQL columns, JSON blobs, or both?
- Which connection providers should be implemented first: MCP, GitHub/Linear, or armature workspaces?
- Should stance-specific positive grants ever be allowed, or should Bear-level config always be the authority ceiling?
- How should UI phrase missing optional connection requirements without making import feel failed?
