# Bear Details UI Improvement Plan

## Summary

The bear details page should evolve from a mostly administrative/debug view into a clear, educational control room for understanding and managing a Bear.

A Bear feels like one coherent assistant to the user, but internally it has five specialized roles: `talk`, `pair`, `curate`, `work`, and `watch`. The details UI should respect that architecture.

The page should help users understand that a Bear is shaped by:

- Den baseline instructions
- protected role contracts
- user steering
- Bear context
- capabilities/tools
- memory
- threads/activity
- access
- advanced runtime/Letta details

The key design constraint is transparency: BEARS may use prompt composition helpers and structured context composition, but users should be able to see what the role agents will actually see. Prompt composition should be framed as a shortcut for authoring steering/context, not as an abstraction that hides how the Bear works.

## Product principle

BEARS should make users better AI operators. The details page should therefore make the Bear legible.

Do not over-humanize the page into an opaque profile/personality editor. Instead, show the user:

- what they can steer
- what stable context the Bear has
- which internal role contracts are protected
- which parts were generated from onboarding/template setup
- what composed role prompts look like

Friendly summaries are useful, but they should point back to agent-visible instructions.

## Revised mental model

A Bear is not just a chat. A Bear is a persistent multi-agent system configured by role-aware context composition plus memory, tools, and threads.

The user-facing mental model should be:

> This Bear behaves this way because Den combines protected role contracts, your steering, this Bear's context, and runtime context into the prompt each role sees.

The details page should reinforce that model.

## Role-aware instruction architecture

The details UI should present a simple role-aware model, not a large prompt CMS.

Recommended v1 layers:

1. Den baseline
2. Role contracts
3. User steering
4. Bear context
5. Runtime/thread context

### Den baseline

Platform-level instructions controlled by Den/operators. Usually read-only for normal users.

### Role contracts

Protected structural instructions for `talk`, `pair`, `curate`, `work`, and `watch`.

These define role authority, trust boundaries, and cooperation rules. They should be inspectable but not the normal user editing surface.

### User steering

The main user/admin-editable behavior layer.

This guides preferences, priorities, tone, depth, and workflows without redefining role boundaries.

### Bear context

Stable context this Bear should know. User/admin editable.

### Runtime/thread context

Dynamically injected current task, tool, surface, memory, and thread context. Advanced/debug surface initially.

## UI concept: steering first, role contracts protected

The top of the details page should not hide prompts behind profile abstractions, and it should not invite users to accidentally rewrite role contracts.

Recommended primary editable sections:

- `User steering`
- `Bear context`

Recommended inspectable sections:

- `Role contracts`
- `Composed role prompts`
- `Den baseline`, likely inside composed prompt view or advanced view

Composition helpers should be available, but framed as shortcuts:

- `Use setup helper`
- `Generate steering changes`
- `Review setup answers`

Direct text editing should remain available for user-owned layers:

- `Edit steering`
- `Edit context`

## User steering section

This replaces the earlier over-broad `Purpose & instructions` / `Working style instructions` concept.

It should show the user-editable steering text that influences the Bear across roles without changing the roles themselves.

Suggested copy:

> These instructions steer how your Bear works with you. They guide tone, priorities, and collaboration style without changing the Bear's internal role boundaries.

Example:

```/dev/null/user-steering.md#L1-10
The user prefers concise implementation plans before code changes.
Challenge unclear product assumptions respectfully.
Keep documentation in sync when behavior changes.
Prefer small, reviewable code changes.
When uncertainty matters, call out tradeoffs briefly.
```

Actions:

- Edit steering
- Use setup helper, later
- View composed prompts

## Bear context section

This should show stable context the Bear should know.

Suggested copy:

> This is stable context this Bear should know. It may come from onboarding, later edits, or explicit user-provided setup notes.

Example:

```/dev/null/bear-context.md#L1-8
The user is building BEARS, a system for persistent AI agents called Bears.
They prefer transparent AI workflows where users can inspect and improve agent instructions.
The backend is Rust, the codepool service is TypeScript, and the UI is server-rendered where practical.
```

Actions:

- Edit context
- Use setup helper, later
- View composed prompts

## Role contracts section

This is an inspectable protected section.

Suggested copy:

> Role contracts define what the Bear's internal roles are allowed and expected to do. They preserve trust boundaries.

Show the five roles:

- `talk` — conversational front door
- `pair` — collaborative client/tool role
- `curate` — semantic integration and review role
- `work` — approved outbound executor
- `watch` — inbound observer

Each role should eventually have an expandable contract text view.

Important UI framing:

- Role contracts are not the normal customization surface.
- Users steer the Bear through `User steering` and `Bear context`.
- Advanced/admin editing of role contracts can be considered later.

## Composed role prompt view

The details page should include a way to inspect composed prompts for role agents.

The composed prompt view should be clear that:

> This is the instruction text a role agent sees after Den combines the baseline, role contract, user steering, Bear context, and runtime context.

At minimum, support composed prompt preview for current user-facing roles:

- `talk`
- `pair`

Later, add:

- `curate`
- `work`
- `watch`

Conceptual example:

```/dev/null/composed-role-prompt.md#L1-30
# Den baseline
Managed by Den. Read-only.
...

# Role contract: pair
Protected role contract.
...

# User steering
Editable.
...

# Bear context
Editable.
...

# Runtime/thread context
Injected at runtime.
...
```

Useful actions:

- copy prompt
- switch role prompt
- view role contract source
- edit steering/context

## Composition helper model

Prompt composition helpers should edit user-owned layers, not protected role contracts.

Recommended helper flow:

1. User chooses a target, such as `User steering`.
2. UI asks practical questions.
3. System generates proposed replacement text.
4. User reviews the proposed text.
5. User accepts, edits, or cancels.

This reinforces that helpers generate agent-visible text. Users review the output before saving.

## Relationship to onboarding

First-bear onboarding should produce role-aware context profiles.

Onboarding outputs should include:

- template id/version
- materialized role contracts or role contract version
- user steering
- Bear context
- starter prompts
- first task draft/handoff

The bear details page should expose the user-owned layers and allow inspection of protected role contracts.

This makes onboarding consequential and understandable without making users responsible for maintaining the internal role architecture.

## Relationship to existing `system_prompt`

Today, bears have a `system_prompt` field. The role-aware context model should evolve without breaking existing bears.

Recommended phased approach:

### Phase 1: Preserve legacy prompt behavior

- Existing bears with no `context_profile` continue using `system_prompt`.
- Details UI can show them as legacy/manual prompt bears.
- New role-aware bears store `context_profile` and generate prompt text where existing sync/provisioning paths need it.

### Phase 2: Role-aware details UI

- Show `User steering`, `Bear context`, role contracts, and composed prompts for role-aware bears.
- Keep raw system prompt editing for legacy/manual bears.

### Phase 3: Helper-assisted editing

- Add setup/helper flows that propose changes to steering/context.
- Avoid role contract editing in normal helper flows.

### Phase 4: Advanced role contract management

- Operator/admin role contract editing.
- Template role contract versioning.
- Prompt migration/update helpers.
- Prompt audit/history.

## Candidate data model

Use JSONB first rather than a fully normalized schema.

Potential field:

- `bears.context_profile JSONB`

Conceptual shape:

```/dev/null/context-profile.json#L1-38
{
  "composition_version": 1,
  "template_id": "software_product_builder",
  "template_version": "1",
  "role_contract_version": "1",
  "role_contracts": {
    "talk": "You are the Bear's talk role...",
    "pair": "You are the Bear's pair role...",
    "curate": "You are the Bear's curate role...",
    "work": "You are the Bear's work role...",
    "watch": "You are the Bear's watch role..."
  },
  "user_steering": "The user prefers concise implementation plans before code changes...",
  "bear_context": "The user is building BEARS...",
  "starter_prompts": [
    "Help me turn an app idea into an MVP plan.",
    "Help me understand this codebase."
  ],
  "first_task": "Help me plan the onboarding feature."
}
```

## Page structure recommendation

A future bear details page could be organized like this:

```/dev/null/bear-details-layout.md#L1-55
# Product Builder

[Chat] [All threads] [Code with this bear]

## User steering
These instructions steer how your Bear works with you without changing internal role boundaries.

The user prefers concise implementation plans before code changes...

[Edit steering] [Use setup helper]

## Bear context
This is stable context this Bear should know.

The user is building BEARS...

[Edit context]

## Role contracts
A Bear feels like one assistant, but internally it has five specialized roles.

- talk: conversational front door
- pair: collaborative client/tool role
- curate: semantic integration and review role
- work: approved outbound executor
- watch: inbound observer

[View role contracts]

## Composed prompts
See what each role sees.

[View talk prompt] [View pair prompt]

## Current threads
Active and archived threads.

## What this Bear remembers
Friendly memory summary plus link to detailed Letta memory.

## Capabilities
Plain-language capability status.

## People
Membership/access management.

## Advanced
Legacy prompt, runtime configuration, Letta sync, tool IDs, diagnostics.
```

## Other details page areas

### Memory

Show friendly summary first:

- `What this Bear remembers`

Then link to technical memory details.

### Capabilities

Show plain-language capability cards:

- available
- not connected
- not supported
- advanced details

Do not expose Letta tool IDs up front.

### Threads

Use `threads`, not `conversations`, in user-facing UI.

Show:

- active threads
- archived threads count
- all threads link

### Access

Keep member/access management straightforward.

### Advanced

Keep low-level details available but visually separated:

- legacy/manual prompt
- Letta sync/drift
- model/runtime config
- tool IDs
- diagnostics

## Enabling projects

### Role-aware context profile storage

Add storage for `context_profile`.

Questions:

- JSONB on `bears` vs separate table?
- How to represent materialized vs referenced role contracts?
- How to mark legacy/manual prompt mode?

### Role-aware prompt assembly service

Create one service responsible for composing role prompts.

Responsibilities:

- stable layer ordering
- role selection
- Den baseline loading
- role contract loading
- user steering and Bear context inclusion
- runtime context inclusion
- legacy fallback

### Steering/context editor UI

Create editing UI for user-owned layers.

Needs:

- validation
- preview
- save/cancel

### Role contracts view

Add UI for inspecting role contracts.

Needs:

- list all five roles
- expandable contract text
- clear protected/structural labeling

### Composed role prompt view

Add a view/modal that shows the full agent-visible prompt for a role.

Needs:

- role selector
- layer headings
- copy button
- links to edit steering/context

### Onboarding integration

First-bear onboarding should create role-aware context profiles.

Needs:

- map template to role contracts
- map setup answers to steering/context
- preserve starter prompts and first task handoff

### Helper-assisted editing

Add helper flows that propose changes to user-owned layers.

Needs:

- target steering or context
- question forms
- generated text proposal
- save/cancel

### Legacy/manual prompt compatibility

Support existing bears and admins who edit raw prompts today.

Questions:

- What happens if a user directly edits `system_prompt` on a role-aware Bear?
- Does that switch it to manual mode?
- Can it be converted back later?

### Template role contract versioning

If templates change, decide whether existing Bears are updated, offered migrations, or left unchanged.

Likely recommendation:

- Existing Bears keep materialized contracts or role contract version references.
- UI can show `Started from template X version Y`.
- Later, offer optional template update/migration.

## Open questions

1. Which roles should have composed prompt previews in the first UI slice?
2. Should role contracts be materialized per Bear, referenced by version, or both?
3. Should normal bear admins ever edit role contracts directly?
4. How should raw `system_prompt` editing interact with role-aware Bears?
5. Should `context_profile` become canonical immediately for new Bears?
6. How should runtime/thread context be explained without exposing Letta internals?
7. Should onboarding answers be shown as provenance for steering/context?
8. Should role prompt changes have audit history?
9. Should users be able to copy/export role prompts?
10. How should role-aware composition map onto current Letta provisioning if only one linked agent exists today?

## Recommended first implementation slice

Start with a minimal role-aware foundation that improves the details page without requiring the full five-agent runtime to be complete.

Suggested slice:

1. Add `context_profile` storage.
2. Add a role-aware composer with Den baseline, role contract, user steering, Bear context, and legacy fallback.
3. Support composed prompt preview for `talk` and `pair` first.
4. Show/edit `User steering` and `Bear context` for role-aware Bears.
5. Show inspectable role contracts.
6. Keep existing prompt editing path for legacy/manual Bears.
7. Defer helper-assisted editing and role contract editing.

This gets the details page moving toward transparency and education while respecting the multi-agent Bear architecture.
