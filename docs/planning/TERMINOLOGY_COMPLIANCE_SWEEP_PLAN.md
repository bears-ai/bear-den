# Terminology compliance sweep plan

This plan covers a repo-wide compliance sweep for the terminology guidance around **conversation**, **thread**, and **session**.

## Goal

Align code, schemas, prompts, docs, and runtime-facing strings with the following guidance:

- **conversation** = durable, user-visible exchange between a human and a Bear
- **thread** = channel-native reply structure
- **session** = runtime interaction context or client binding

The sweep should also preserve a separate distinction for:

- **role** = Bear operating mode / agent assignment
- **work surface** = durable context of work such as repo, project, service, deployment, or Mission

## Why this matters

These terms describe different layers of the system. Mixing them causes product confusion, schema ambiguity, and architectural drift.

Examples of common mistakes:

- using `session` when the code really means durable chat history,
- using `thread` as the universal term for all Bear interactions,
- using `conversation` for ephemeral client bindings,
- or using transport-specific thread terms as canonical cross-surface identifiers.

## Scope

Sweep all checked-in repository material, including:

- Rust, TypeScript, JavaScript, Python, YAML, JSON, TOML, SQL, and shell files
- prompt/compiler inputs and generated prompt templates if checked in
- docs and ADRs
- tests and fixtures
- config templates and deployment artifacts

If terminology also appears in runtime-generated or external system surfaces not stored in this repo, capture those as follow-up work rather than silently changing repo language to match them.

## Review method

### 1. Inventory terms

Search for and classify usages of:

- `conversation`
- `thread`
- `session`
- related identifiers such as `conversation_id`, `thread_id`, `session_id`
- transport-specific anchors like Slack thread timestamps where relevant

For each usage, classify whether it refers to:

- durable user-visible exchange,
- channel-native thread structure,
- runtime/client binding,
- or something ambiguous/incorrect.

### 2. Decide target term by meaning

Use this replacement guidance:

- replace with **conversation** if the thing is a durable interaction record that may be titled, summarized, resumed, or used as provenance
- replace with **thread** if the thing is specifically a channel-native reply structure
- replace with **session** if the thing is a live client/runtime binding, auth-scoped context, or execution attachment

Do not rename blindly by string match alone.

### 3. Update schemas and naming carefully

When changing identifiers or schema fields:

- prefer preserving external compatibility when required
- add compatibility shims where needed
- update serializers, deserializers, API docs, and tests together
- document migrations when field names change

### 4. Update outward-facing language first

Prioritize correcting:

- product copy
- docs
n- API descriptions
- prompt/instruction text
- logs and operator-facing diagnostics

Then update deeper internal names where feasible and justified.

## Deliverables

1. **Inventory report** of term usages and classifications
2. **Recommended edits** grouped by subsystem
3. **Compatibility notes** for any schema or API renames
4. **Follow-up list** for runtime/external surfaces outside this repo

## Suggested execution order

1. docs and concept language
2. prompt/compiler and runtime-facing strings
3. API/schema definitions
4. implementation code and internal variable names
5. tests, fixtures, and migration notes

## Acceptance criteria

- `conversation`, `thread`, and `session` are used consistently by meaning
- cross-surface durable interaction records are described as **conversations**
- Slack/Discord/forum-style reply chains are described as **threads** only where appropriate
- runtime bindings are described as **sessions**
- any intentional exceptions are documented

## Notes

This plan is about terminology compliance, not forcing every internal identifier to change immediately. Meaning and outward consistency come first; mechanical renames should follow only where they improve clarity and do not create unnecessary churn.
