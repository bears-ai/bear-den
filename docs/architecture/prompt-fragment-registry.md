# Prompt Fragment Registry

This document describes the target architecture for file-backed prompt fragments, runtime-authored prompt content, and compiled runtime prompt assembly in Den.

**Current implementation note (working-tree WIP):** Managed compilation emits bound Bear-wide base and platform interaction modes only, parsed independently of legacy role contracts/metadata. Ordinary turn assembly selects the current canonical real-hat identity; no zero-hat legacy prompt fallback exists. No production inference selects a role prompt or role contract. Dedicated Curate briefing uses direct source-verified tool-free inference with repository Markdown instructions; worker briefings consume direct verified `MemorySource`. Source admission precedes all inference/continuations, and direct tools recheck it. Historical profile bundles/contracts below illustrate the original design, not configurable runtime stances or live model/loop overrides. Initialization never creates/refreshes a profile registry. See [the maintained topic](../topics/bear-memory-hats.md) for current boundaries and latest-image validation limits.

**Decision source of truth:** [ADR-0046 — File-backed prompt fragments and compiled runtime prompts](../decisions/adr-0046-file-backed-prompt-fragments-and-compiled-runtime-prompts.md)

## Goals

- Move long-lived instruction text out of Rust source and into reviewable prompt files.
- Preserve Den's hot-path performance invariant: turns read compiled prompt material, not raw source.
- Support Bear Admin-entered prompt content without allowing arbitrary runtime template execution on every turn.
- Keep prompt layering explicit so context provenance remains debuggable.

## Source classes

Den prompt source comes from three classes.

### 1. Repository-authored fragments

- Stored in Git.
- Format: Markdown body with YAML frontmatter.
- Loaded into a startup-built prompt registry.
- Source of truth for shared stance/policy/runtime prose.

Example layout:

```text
services/den/prompts/
  fragments/
    base/
    stances/
    policies/
    runtime/
  bundles/
    pair.yaml
    chat.yaml
    curate.yaml
```

### 2. Runtime-authored fragments

- Stored in Den Postgres.
- Includes Bear Admin-authored prompt content and managed Bear prompt fields.
- May use compile-time variables only.
- Recompiled into `bear_compiled_configs` on write/reconcile.

### 3. Dynamic turn supplements

- Produced by Rust code during turn assembly.
- Includes key memory projection, derived recall, prompt-memory blocks, compaction blocks, runtime/channel reminders, and tool-surface summaries.
- Not stored as raw file-backed prompt source.

## Fragment format

Fragments use Markdown plus YAML frontmatter.

```md
---
id: stance_pair
layer: stance
templating_phase: compile
applies_to: [pair]
order: 200
vars: [bear_name, bear_slug]
---

You are the Pair stance for **{{ bear_name }}** (`{{ bear_slug }}`).
```

Suggested frontmatter fields:

- `id` — stable fragment identifier
- `layer` — `base | stance | policy | runtime | tooling`
- `templating_phase` — `compile | turn`
- `applies_to` — role/profile applicability
- `order` — stable assembly ordering inside a layer
- `vars` — declared template variables
- `description` — optional human-oriented note

## Templating contract

Den supports two templating phases.

### Compile-time templating

Allowed in:

- repository-authored fragments
- runtime-authored fragments

Typical variables:

- `bear_name`
- `bear_slug`
- `user_steering`
- `bear_context`
- role/profile labels
- compile-time feature/config flags

Compile-time rendering feeds `bear_compiled_configs`.

### Turn-time templating

Allowed only in:

- repository-authored fragments explicitly marked `templating_phase: turn`

Typical variables:

- current date/time
- context budget reminders
- channel/runtime mode reminders
- other per-turn controlled values

Turn-time templating is intentionally restricted to Git-backed prompt source so Den does not execute arbitrary user-authored prompt templates in the hot path.

## Prompt registry

At startup, Den builds an immutable prompt registry:

1. load repository fragment files,
2. split frontmatter/body,
3. parse metadata with `serde_yaml`,
4. register MiniJinja templates,
5. compute a prompt source version/hash,
6. expose typed bundle/fragment lookup.

The registry should be an `Arc<PromptRegistry>` shared across the process.

The hot path must not:

- glob for files,
- read prompt files from disk,
- parse YAML frontmatter,
- compile MiniJinja templates.

## Bundle manifests

Bundles define role/profile composition from fragments.

Example:

```yaml
id: pair
fragments:
  - base/identity
  - base/platform_contract
  - stances/pair
  - policies/memory
  - policies/tools
  - runtime/tool_surface
```

Bundles are repository-authored and versioned with code.

## Compilation pipeline

Current managed bound compilation:

```text
repo fragments
+ runtime-authored fragments / context_profile / managed bindings
=> compile and validate
=> rendered bound Bear-wide base and platform modes (no legacy-contract dependency)
=> store in bear_compiled_configs
=> turn assembler appends dynamic layers
```

This preserves the existing Den-native runtime invariant that the loop reads compiled prompt material, not raw source.

## Turn assembly relationship

This architecture changes the source of prompt text, not the overall layer ordering.

Turn assembly remains:

1. compiled bound Bear-wide base/platform mode plus current verified hat identity
2. key memory projection
3. derived recall
4. prompt-memory blocks
5. runtime/channel/tool/compaction supplements

The compiled prompt base becomes file-backed and data-backed source compiled into `bear_compiled_configs`.

## Performance model

### Startup work

- load and validate repository fragments
- compile template registry
- compute registry version/hash

### Mutation-time work

- validate runtime-authored fragments on write
- recompile affected Bear prompt output
- update `bear_compiled_configs`

### Turn-time work

- load compiled prompt text
- append dynamic runtime layers
- optionally render a small number of repo-owned turn-time fragments

Turn-time work must stay append-oriented and bounded.

## Cache tiers

### Prompt source version

Single hash over repository fragment metadata/bodies and bundle manifests.

### Compiled Bear config

`bear_compiled_configs` remains the cacheable persistent compiled output keyed by Bear/config hashes.

### Turn-local dynamic caches

Dynamic turn layers may continue to use specialized caches such as key-memory projection cache tokens.

## Authoring rules

- Use Markdown for readable prompt prose.
- Keep YAML frontmatter small and typed.
- Use MiniJinja for interpolation, not prompt-side business logic.
- Keep runtime-authored templates compile-time-only.
- Keep generated structural text in Rust when it is primarily derived data, not prose.
- Do not hardcode prompt text in Rust source. Add prompt prose as repository context fragments, or store runtime-configured prompt prose in the database with explicit defaults alongside the fragment/configuration model.
- Do not ask the model to choose among branches when Den already has the state needed to choose. Render only the applicable instruction whenever runtime state such as permission mode, governance, budget, or active execution state is known before the model call.

### Deterministic instruction selection

Prompt fragments may contain MiniJinja conditionals, but those conditionals should be used to **select rendered text before inference**, not to delegate policy branching to the model.

Prefer this shape:

```jinja
{% if execution.acp_permission_mode == "Write" %}
Work the current Docket task and only mark it done after the work is performed or verified.
{% else %}
Docket execution is active, but the session must be switched to Write mode before execution can proceed.
{% endif %}
```

Avoid this shape when `execution.acp_permission_mode` is already known:

```text
If the ACP permission mode is Ask or Plan, do not continue execution.
If the ACP permission mode is Write, continue execution.
```

The first form gives the model one applicable instruction. The second form asks the model to perform a policy decision that Den can make deterministically.

## What stays in Rust

Small deterministic structural helpers may remain code-generated:

- descriptor-derived tool surface blurbs,
- compact structured diagnostics,
- simple dynamic supplements that are mostly data formatting.

Long-lived instruction prose and stance/policy text should move to prompt fragments.
