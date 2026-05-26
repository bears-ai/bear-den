# Role Vocabulary Note

## Purpose

This note proposes a terminology shift for Bear Den as the system migrates away from Letta.

Canonical framing:

> **Bear Den uses a multi-role runtime.** A Bear has one durable identity and charter, and may execute under role-scoped contexts such as `chat`, `pair`, `work`, `review`, and `watch`. Roles are Den-owned descriptors that define tools, memory scope, autonomy policy, surfaces, and audit behavior. They are not distinct provider-managed agents.

The central idea is:

- **Bear** remains the primary assistant identity
- **role** becomes the primary behavioral and policy boundary
- **runtime instance** becomes the execution/provisioning concept
- **agent** becomes an implementation-specific or historical compatibility term rather than the primary architectural concept

This note is not yet a full ADR. It is intended to guide ongoing schema, architecture, and documentation work while Letta-dependent concepts are being removed.

## Why this matters now

Under Letta, Bear Den naturally inherited a multi-agent framing because the implementation literally provisioned multiple Letta agents.

As Bear Den moves toward a Den-owned runtime, that framing becomes less accurate.

What the architecture appears to actually need is:

- one logical Bear identity
- several strongly bounded operating roles
- one or more runtime implementations that execute those roles

In that world, the role is the durable product/domain concept, while the runtime instance is the technical execution binding that realizes a role under a particular provider or runtime family.

## Recommended terminology

### Bear

The **Bear** is the durable assistant identity.

A Bear is:

- what the human is interacting with
- the owner of canonical memory domains and responsibilities
- the conceptual assistant across all surfaces

A Bear is **not** merely a wrapper around several provider-created agents.

### Role

A **role** is the primary operating boundary inside a Bear.

A role defines:

- behavioral stance
- prompt contract
- policy boundary
- tool permissions
- memory read/write scope
- runtime surface expectations

Current roles include:

- `chat`
- `pair`
- `review`
- `work`
- `watch`

Whether these names remain unchanged is a separate decision. The key point is that **role** should remain the primary concept.

### Runtime instance

A **runtime instance** is the concrete execution binding for a Bear role.

Examples:

- a Letta-backed role runtime
- a Codepool-backed harness runtime
- a future Den-native runtime

A runtime instance may have:

- provider handles
- provisioning status
- runtime family
- diagnostics
- config hashes

But it should not be treated as the fundamental product identity.

### Agent

The term **agent** should be treated carefully.

Recommended usage:

- acceptable as a provider-specific term, such as “Letta agent id”
- acceptable in historical migration notes when describing prior architecture
- avoid as the primary term for current domain concepts once migration progresses

Why:

- it overfits the Letta implementation model
- it implies stronger ontological separation than the product may actually want
- it confuses identity with execution substrate

## Recommended architectural framing

Instead of describing Bear Den as:

> a logical assistant backed by five agents

prefer language such as:

> a Bear is one assistant identity with several fixed roles

or:

> a Bear executes through role-specific runtime contexts

or:

> a Bear has multiple operational roles, each with distinct policy, memory, and runtime constraints

This better reflects a post-Letta architecture.

## Implications for schema design

This terminology shift suggests the following modeling choices.

### Prefer role-centric schema names

Good examples:

- `bear_roles`
- `runtime_instances`
- `conversations`
- `conversation_runs`
- `conversation_tool_calls`

Less ideal once migration progresses:

- `agents`
- `agent_threads`
- `agent_runs`

### Separate role identity from runtime implementation

Schema should distinguish between:

- the Bear and role as durable domain concepts
- the runtime instance as implementation detail
- provider references as compatibility metadata

This aligns with the recommendation that Letta ids should not be canonical identity.

### Avoid locking vocabulary to the migration shim

It is acceptable to keep provider-specific compatibility fields during migration, such as:

- `letta_agent_id`
- `provider_run_ref`
- `provider_conversation_ref`

But new canonical abstractions should not be named around those fields.

## Implications for docs and code

### Documentation

Recommended direction:

- describe the architecture as **multi-role**, not primarily **multi-agent**
- reserve “agent” for provider-specific or legacy discussion
- emphasize that role is the policy and memory boundary

### Code

Recommended direction:

- keep current provider-specific fields where needed for compatibility
- introduce provider-neutral types such as `RuntimeInstance`, `Conversation`, `ConversationRun`
- avoid spreading new core abstractions that use “agent” as the default noun unless they truly mean provider agent objects

### UI and operator language

Recommended direction:

- surface Bear identity first
- present roles as operating modes/boundaries
- avoid exposing implementation-specific “agent” identity unless it helps debugging or migration support

## Suggested wording conventions

### Prefer

- Bear
- role
- runtime instance
- runtime family
- conversation
- run
- event log
- tool call
- approval

### Use carefully

- agent
- agent id
- agent provisioning

### Acceptable migration phrasing

- “legacy Letta agent id”
- “Letta-backed runtime instance”
- “historical multi-agent implementation”

## Open naming question: role labels themselves

This note does **not** attempt to settle whether the current role names should change.

However, it does suggest that role renaming can now be considered without being constrained by Letta terminology confusion.

In particular, if some role names are revisited, the decision should optimize for:

- clarity of operating stance
- user/operator legibility
- policy meaning
- continuity cost

rather than compatibility with the language of separate provider-created agents.

## Recommended next steps

1. Update architecture docs to say **Bear + roles** rather than **Bear + agents** where possible.
2. Keep provider-specific “agent” terms only where they refer to real Letta compatibility fields.
3. Prefer provider-neutral schema and type names in new work.
4. Revisit role label names separately, if desired, after the conceptual shift is accepted.

## Bottom line

Post-Letta, Bear Den is better described as a **single Bear identity with multiple roles** than as a set of distinct agents.

That means:

- **role** should be the primary architectural term
- **runtime instance** should name the execution/provisioning layer
- **agent** should gradually become a legacy/provider-specific term rather than the dominant conceptual model
