# Environment Affordance and Resource Boundaries — Architecture Decision Record

## Status: Proposed

## Date: 2026-05-25

---

## Context

BEARS agents operate in environments that expose multiple adjacent but distinct resource domains through one conversation:

- workspace files and directories under local workspace roots;
- Bear memory entries and memory-backed artifacts;
- live activity/workboard records;
- workplan approval gates and submitted plan artifacts;
- execution capabilities such as process, browser, and mutation tools.

Each domain is individually reasonable, but the environment often presents them with similar interaction shapes:

- path-like strings;
- ids and handles embedded in prose;
- broad write-mode affordances;
- tool outputs that mix state, artifacts, and capabilities in one turn.

This creates a recurring affordance problem: the locally plausible action can still target the wrong substrate.

A recent example is enough to generalize the pattern. A cancelled plan-mode gate returned a referenced artifact path like `pair/plans/mem_...md`. In adjacent turns, the environment also exposed `/workspace/...` filesystem tools and broad session write enablement. Because the artifact looked path-shaped and file-like, it was easy to treat it as a repo-relative file path even though it belonged to a different storage domain. The failure mode was not primarily missing diligence; it was that the environment made cross-domain confusion easy.

This problem also appears in nearby areas:

- workplan artifacts can look too similar to memory documents;
- broad write-mode wording can suggest mutation is available everywhere when it is only available for some substrates;
- the same conceptual object may appear as a live state record, a durable artifact, a memory summary, and a local file reference;
- tool families may be explicit individually while the returned objects are not.

Existing ADRs already establish parts of the solution:

- ADR 0011 defines a harness-level `bear_environment` contract for structured environment state.
- ADR 0016 defines `session_info` as the canonical orientation tool and requires scope/side-effect semantics in descriptors.
- ADR 0025 defines provider-safe naming, canonical names, and execution location strategy.
- ADR 0027 defines the ontology boundary between workplan, activity, memory, and execution and notes that the environment should prevent category mistakes structurally when possible.

What is still missing is a general design rule for **environment affordances**: how the runtime should represent resources, capabilities, and returned objects so that agents and operators do not have to rely on constant manual category discipline.

---

## Decision

BEARS will adopt an explicit **environment affordance design rule**:

> Resource domains, capability domains, and workflow domains must be visually and structurally distinct at point of use.

The environment must not rely on agents to infer substrate boundaries from context alone when the returned object, tool input, or capability state can encode those boundaries directly.

This decision applies across:

- tool descriptors;
- tool input schemas;
- tool return payloads;
- environment/orientation tools;
- UI/state rendering;
- workflow reminders and approval surfaces.

---

## Design principles

### 1. Domain identity must be explicit

Any durable or actionable object surfaced to an agent or operator should carry explicit domain identity.

Examples:

- `domain: workspace`
- `domain: memory`
- `domain: activity`
- `domain: workplan`
- `domain: execution`

Where helpful, returned objects should also carry a more specific storage or ownership marker such as:

- `storage_domain: workspace_fs`
- `storage_domain: bear_memory`
- `storage_domain: den_db`
- `storage_domain: garage`

A path-like string alone is not enough to identify the resource domain.

### 2. Resource identifiers should encode substrate when possible

The environment should prefer resource identifiers that make substrate boundaries obvious.

Preferred patterns include:

- typed ids;
- URI-like schemes;
- explicit descriptor fields;
- tool-specific opaque handles.

Examples:

- `ws:///workspace/services/den/src/main.rs`
- `mem://pair/plans/mem_abc123.md`
- `workplan://8394c0c3-eaad-471a-9e34-97c51fdb9604`
- `activity://f16c36ca-19e3-4ae3-bff1-d43a0dd635a6`

Exact syntax may vary by subsystem, but BEARS should avoid presenting objects from different substrates as visually interchangeable bare paths when those objects might later be mutated, deleted, or passed between tools.

### 3. Tool schemas should enforce resource boundaries

Mutating and retrieval tools should accept resource identifiers native to their domain.

Examples:

- workspace filesystem tools should accept absolute workspace paths only;
- memory tools should accept memory paths or memory ids only;
- workplan tools should accept workplan ids only;
- activity tools should accept activity/plan ids only.

When possible, tools should prefer typed handles over free-form strings that can be mistaken for another domain.

### 4. Capability state must be substrate-specific

Broad session mode labels such as `Write` are useful but insufficient.

The environment should expose capability state in a more granular way at point of use, such as:

- workspace mutation: enabled/disabled
- memory note writes: enabled/disabled
- memory artifact deletion: enabled/disabled/unavailable
- workplan mutation: enabled/disabled
- activity mutation: enabled/disabled
- execution/process: enabled/disabled
- browser interaction: enabled/disabled

This prevents a true-but-coarse capability statement from being misread as universal write authority.

### 5. Returned objects should carry actionability metadata

When tools return objects that may be acted on later, those objects should include enough metadata to make safe next actions obvious.

Preferred fields include:

- `domain`
- `storage_domain`
- `id`
- `path` or `uri` when relevant
- `writable_by_current_agent`
- `deletable_by_current_agent`
- `supported_mutations`
- `owning_tool_family`
- `provenance`

An agent should not have to remember from prior prose whether a referenced object is editable, deletable, or only informational.

### 6. The environment should fail early on cross-domain misuse

When a tool is called with an identifier that appears to belong to another substrate, the environment should reject early with a domain-aware explanation.

Preferred style:

- “This identifier refers to a Bear memory artifact, not a workspace file. Workspace filesystem tools only operate on absolute workspace paths.”

Avoid late failures that merely report missing files or generic invalid input when the more important issue is domain mismatch.

### 7. Distinguish object representations from implementation details

A workplan artifact may temporarily be stored using memory-backed infrastructure, but it should still present primarily as a workplan object if that is its user-facing role.

Likewise:

- semantic memory should not masquerade as active planning state;
- plan artifacts should not masquerade as generic notes;
- execution capabilities should not masquerade as durable objects;
- observed workspace anchors should not masquerade as canonical work-surface identity.

The surfaced representation should match the object's conceptual role, not merely its storage implementation.

### 8. Shared path metaphors need stronger namespace markers

If BEARS continues to use path-like notation across multiple domains, those paths must carry stronger namespace cues.

Examples:

- reserve bare absolute paths for workspace filesystem paths;
- reserve role-relative memory paths for memory tools only and label them explicitly as memory paths;
- prefer URI-like prefixes or explicit labels in mixed-domain tool outputs.

In mixed-domain responses, unlabeled path strings should be treated as a design smell.

---

## Consequences

### Positive

- Reduces category-confusion failures between memory, filesystem, activity, and workplan domains.
- Makes tool misuse easier to catch structurally rather than through prompt discipline.
- Improves both agent and human operator understanding of what an object is and what can be done with it.
- Gives future UI and API work a shared standard for rendering resources and capabilities.
- Complements the turn-state ontology by making domain boundaries visible at the object and capability level.

### Tradeoffs

- Tool schemas and return payloads may become slightly more verbose.
- Existing path-oriented outputs may need compatibility layers or staged migration.
- Some systems will need adapters that translate internal storage details into clearer external identities.
- UI and prompt surfaces will need consistent vocabulary to avoid replacing one ambiguity with another.

---

## Implementation guidance

Near-term changes should prefer incremental, high-value clarity improvements:

1. Add explicit `domain` and, where relevant, `storage_domain` fields to returned objects.
2. Strengthen `session_info` / `bear_environment` capability summaries so mutation authority is broken down by substrate, not only by coarse mode.
3. Review workplan and memory artifact outputs to ensure they do not present primarily as interchangeable file paths.
4. Add early domain-mismatch validation and clearer error messages to tool handlers where practical.
5. Prefer typed/URI-like identifiers for new cross-domain object surfaces.
6. Document mixed-domain rendering rules for UI and API consumers.

---

## Alternatives considered

### Rely on better prompt instructions and agent discipline

Rejected as a primary strategy. Prompt guidance helps, but the same cross-domain confusion will recur when the environment presents distinct resources with similar shapes and incomplete actionability metadata.

### Flatten all durable objects into one filesystem-like namespace

Rejected. This hides important ontology and capability differences rather than clarifying them.

### Keep current identifiers but improve training/examples only

Rejected as insufficient. Examples can reduce confusion temporarily, but the environment should encode key distinctions directly in object shapes and tool contracts.

---

## Related documents

- [ADR 0011: Harness-Level `bear_environment` Tool](./adr-0011-harness-bear-environment-tool.md)
- [ADR 0016: Pair Tool Discovery and Scope Orientation](./adr-0016-pair-tool-discovery-and-scope-orientation.md)
- [ADR 0025: Tool Naming and Execution Strategy](./adr-0025-tool-naming-and-execution-strategy.md)
- [ADR 0027: Workflow State Ontology](./adr-0027-workflow-state-ontology.md)
- [Memory model](../architecture/memory-model.md)
- [Planning in Bear Den](../architecture/planning.md)
- [Bear environment tool contract](../architecture/bear-environment-tool-contract.md)
