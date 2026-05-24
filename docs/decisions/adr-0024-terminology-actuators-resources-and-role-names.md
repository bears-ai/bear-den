# Terminology: Actuators, Resources, and Role Names — Architecture Decision Record

## Status: Proposed

## Date: 2026-05-23

---

## Context

BEARS terminology has accumulated legacy language from earlier architecture phases, including Letta-centered framing, ACP-specific naming, and role labels that no longer match the desired product model.

Several terms now create avoidable confusion:

1. **"Adapter" is too generic for the ACP-connected execution layer.** The ACP component does more than adapt APIs; it is the execution surface that performs file, process, terminal, browser, and other tool actions on behalf of a role.
2. **"Work surface" is too specialized and competes with other concepts.** The concept is intended to name the canonical target of work, memory, planning, and observation, but the terminology should be broader and more uniform.
3. **Role naming still reflects older framing.** Existing names such as `pair` and `talk` are tied to earlier metaphors and Letta-era usage rather than the desired role model.
4. **"Agent" language is overused.** BEARS should emphasize roles operating within a Bear runtime, rather than centering the older mental model of many loosely named agents.

This ADR establishes the terminology direction for the execution layer, the canonical target-of-work concept, and role naming.

---

## Decision

### 1. ACP "adapter" becomes "actuator"

The ACP-connected execution layer should be called an **actuator**.

Meaning:
- an actuator is the execution surface that performs actions on behalf of a role
- it may expose file, process, terminal, browser, and related tool capabilities
- it may also report environment state, but its defining characteristic is that it can act

Implications:
- references to ACP adapters should be migrated toward actuator terminology
- user-facing, architectural, and implementation language should prefer **actuator** over **adapter**

### 2. "Work surface" becomes "resource"

The canonical target of work, memory, planning, and observation should be called a **resource**.

Meaning:
- a resource is the canonical thing a role is acting on, reasoning over, or organizing memory around
- examples may include a repo, service, subsystem, conversation, artifact set, external system boundary, or other durable work target

Implications:
- references to work surfaces should migrate toward resource terminology
- pathing, schema, orientation, and planning language should use **resource** as the general term
- where precision is needed, more specific phrases such as `repository resource` or `conversation resource` may be used

### 3. Roles are roles, not agent-branded personas

BEARS should prefer **role** language over casual or default **agent** language when describing runtime responsibilities.

This does not forbid the term `agent` in implementation details where it remains technically required, but the system's primary conceptual framing should be:
- a Bear runtime
- with named roles
- acting through actuators
- on resources

### 4. Role names are updated

The role set is renamed as follows:

| Previous | New |
|---|---|
| `pair` | `code` |
| `talk` | `chat` |
| `work` | `work` |
| `watch` | `watch` |
| `curate` | `review` |

### 5. Role intent under the new names

#### `code`
Interactive implementation-oriented role for editing, building, testing, debugging, and related technical changes in a workspace or other execution environment.

#### `chat`
Conversational role for interactive discussion, clarification, explanation, and front-door user interaction.

#### `work`
Autonomous or semi-autonomous task-execution role for carrying out work that is not primarily framed as interactive chat or local coding collaboration.

#### `watch`
Observation and monitoring role for subscriptions, events, audits, and ongoing awareness of system or resource state.

#### `review`
Judgment and consolidation role for evaluating, synthesizing, reviewing, and governing durable shared knowledge and related cross-role outputs.

---

## Consequences

### Positive

- Terminology becomes less tied to Letta-era metaphors.
- `actuator` better reflects the action-taking role of the ACP execution layer.
- `resource` provides a broader and more reusable term than `work surface`.
- role names become plainer and more legible to operators and users.
- `review` better communicates governance and synthesis responsibilities than `curate`.

### Tradeoffs

- `resource` is broader and more overloaded than `work surface`, so docs must define it carefully.
- implementation symbols, API payloads, tool descriptions, and docs will need coordinated migration.
- some older references to `agent`, `adapter`, and `work surface` will persist temporarily during transition.
- backward compatibility may require temporary aliasing in code and documentation.

---

## Migration direction

### 1. Documentation

- update architecture, planning, and operator-facing docs to prefer the new terms
- treat this ADR as the source of truth for terminology migration
- revise related ADRs where the old terminology is now misleading

### 2. Semantic memory and resource model

- update the semantic memory ADR and implementation plan to replace `work surface` with `resource`
- update any proposed paths, schema fields, and scaffold language accordingly
- prefer `resources` over `work_surfaces` in new documentation and design work

### 3. ACP and execution terminology

- rename ACP adapter references to actuator where feasible
- update UI and operator-facing descriptions first
- retain implementation aliases temporarily where migration cost is high

### 4. Role naming migration

- migrate docs, tool descriptions, and product copy from old role names to new role names
- plan code-level migration for enums, payload fields, and persistent values carefully to avoid breakage
- use aliases or compatibility layers during transition as needed

### 5. Agent language reduction

- reduce casual use of `agent` where `role` or `Bear` is more precise
- keep `agent` only where required by technical constraints, third-party APIs, or code-level compatibility

---

## Recommended implementation posture

- perform terminology migration deliberately, not piecemeal
- update contracts and schema names before or alongside implementation changes that depend on them
- prefer compatibility layers and staged migration where persistence or external integrations are involved
- avoid introducing new docs or APIs using deprecated terminology unless temporary compatibility requires it

---

## Short policy statement

- ACP adapters are actuators.
- Work surfaces are resources.
- Runtime responsibilities are roles.
- The canonical role names are `code`, `work`, `chat`, `watch`, and `review`.
