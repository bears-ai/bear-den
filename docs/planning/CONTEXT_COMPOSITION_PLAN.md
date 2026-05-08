# Context Composition Plan

## Summary

Before building first-bear onboarding or significantly redesigning the bear details UI, BEARS should define a **role-aware context composition model**.

A Bear feels like one coherent assistant to a user, but internally it has five specialized roles: `talk`, `pair`, `curate`, `work`, and `watch`. Context composition must therefore let users steer their Bear without accidentally breaking the role boundaries that make the Bear useful and safe.

The first implementation should still be simple. Avoid building a prompt CMS too early. Start with five legible layers:

1. **Den baseline**
2. **Role contracts**
3. **User steering**
4. **Bear context**
5. **Runtime/thread context**

The composed role prompt is the agent-visible output for a specific Bear role.

## Core principle

Context composition should be principled, role-aware, and transparent.

BEARS should teach users that Bears are shaped by instructions and context. Users should be able to inspect what each role will see. Prompt composition tools are authoring shortcuts, not hidden abstractions.

The UI should communicate:

> Den combines a baseline, protected role contracts, your steering, and this Bear's context into the prompt each role sees.

## Why role-aware composition is necessary

The earlier simplified model of `Den baseline + Bear instructions + Bear context` is too single-agent. It does not preserve the five-role Bear architecture.

Users should be able to steer preferences such as:

- tone
- answer depth
- planning style
- proactivity
- output format
- preferred workflows
- project priorities

But user steering should not casually override structural role boundaries, such as:

- `talk` should not perform arbitrary autonomous outbound work.
- `pair` should remain client-mediated and user-gated.
- `curate` should remain the semantic integration and review authority.
- `work` should execute only approved outbound tasks in approved scope.
- `watch` should observe inbound events and produce observations, not take outbound action.

The core rule is:

> Users steer priorities and collaboration style; Den preserves role contracts.

## Concept model

### 1. Den baseline

The Den baseline is the platform-level instruction layer applied to Bear roles.

It is controlled by Den/operators, not by normal users.

It may include:

- safety boundaries
- Den/BEARS operating model
- global role-boundary reminders
- global tool-use expectations
- global memory/privacy posture
- confirmation requirements for risky actions
- instruction to avoid claiming unavailable capabilities

Example responsibilities:

```/dev/null/den-baseline.md#L1-10
You are operating as a Bear role in Den.
A Bear feels like one assistant to the user, but internally it has specialized roles.
Preserve role boundaries and do not claim tools or authority unavailable in the current runtime.
Ask before destructive or externally visible actions.
Do not intentionally remember secrets or credentials.
```

### 2. Role contracts

Role contracts are the protected structural instructions for the Bear's internal agents.

They define what each role is for, what it can do, what it must not do, and how it cooperates with other roles.

The five initial role contracts correspond to:

- `talk` — conversational front door
- `pair` — collaborative client/tool role
- `curate` — semantic integration and review role
- `work` — approved outbound executor
- `watch` — inbound observer

Role contracts may be derived from Den defaults plus the selected Bear template. They should be inspectable, but not the normal user editing surface.

Example `pair` contract fragment:

```/dev/null/pair-role-contract.md#L1-10
You are the Bear's pair role: the collaborative agent that works alongside the user inside client tools.
Use client-mediated tools with user approval where appropriate.
When modifying code, inspect relevant context first, prefer small reviewable changes, and report what changed.
Do not perform autonomous outbound work outside the client-mediated permission model.
Do not promote shared memory directly; write durable notes to the pair branch or propose changes for review.
```

### 3. User steering

User steering is the main user/admin-editable behavior layer.

It captures preferences and priorities without redefining role authority.

It may include:

- preferred tone
- planning depth
- proactivity
- output format
- coding/detail preferences
- communication style
- research/writing style
- risk tolerance
- workflow preferences

Templates and onboarding should primarily write to user steering, not directly rewrite role contracts.

Example:

```/dev/null/user-steering.md#L1-12
The user prefers concise implementation plans before code changes.
Challenge unclear product assumptions respectfully.
Keep documentation in sync when behavior changes.
Prefer small, reviewable code changes.
When uncertainty matters, call out tradeoffs briefly.
```

### 4. Bear context

Bear context is stable context this Bear should know.

This is explicit, user-visible context that can be produced during onboarding and edited later.

It may include:

- project description
- user preferences
- time zone
- communication style
- recurring goals
- active research/writing topics
- technology stack
- durable constraints

Example:

```/dev/null/bear-context.md#L1-10
The user is building BEARS, a system for persistent AI agents called Bears.
They prefer transparent AI workflows where users can inspect and improve agent instructions.
The backend is Rust, the codepool service is TypeScript, and the UI is server-rendered where practical.
```

### 5. Runtime/thread context

Runtime/thread context is injected dynamically.

It may include:

- active role (`talk`, `pair`, `curate`, `work`, or `watch`)
- current surface/channel/client
- tool availability
- current thread/task context
- retrieved memory/knowledge
- user/session identity
- task scope and approvals

This context should be visible in debugging/advanced views over time, but it should not be the main v1 editing surface.

## Composed role prompt

The composer should produce a composed prompt for a specific role.

Conceptually:

```/dev/null/composed-role-prompt.md#L1-21
# Den baseline
...

# Role contract: pair
...

# User steering
...

# Bear context
...

# Runtime/thread context
...
```

The same Bear may therefore have multiple composed prompts depending on which internal role is running.

The ordering should be deterministic and tested.

## Why this model is still simple enough for v1

This model adds only one critical layer compared to the earlier simple model: **role contracts**.

It supports heavy templates because templates can materialize role contracts and default steering.

It supports onboarding because onboarding has clear outputs:

- selected template
- role contract version/materialized contracts
- user steering
- bear context
- first task

It supports bear details because the page can show:

- editable user steering
- editable bear context
- inspectable role contracts
- composed prompts per role, or at least for currently available roles

It supports education because users can see the actual instructions each role will see.

It avoids premature complexity by deferring:

- many granular instruction section types
- detailed provenance maps
- prompt diff/audit history
- template migrations
- full raw prompt override reconciliation
- detailed runtime visualization

## Relationship to existing `system_prompt`

Today, bears have a `system_prompt` field. The first role-aware composition implementation should preserve compatibility.

Recommended transitional model:

- For legacy bears, `system_prompt` remains the prompt.
- For role-aware bears, Den stores role contracts, user steering, and bear context in a profile.
- Den generates role-specific prompts from that profile.
- Where existing provisioning/sync paths still require a single `system_prompt`, Den can generate a default visible-role prompt, likely for `talk` or the currently provisioned agent type, until role-specific provisioning is implemented.

Long-term, role-aware context profile should become canonical, with prompt text generated as output.

## Minimal data model

Use JSONB first rather than a normalized role/section schema.

Recommended field:

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

This can later evolve into separate tables or richer sections when usage demands it.

## Bear details UI implications

The details page should eventually show:

### User steering

Primary editable behavior block.

Copy:

> These instructions steer how your Bear works with you. They should guide the Bear without changing its internal role boundaries.

Actions:

- Edit steering
- Use setup helper, later

### Bear context

Primary editable context block.

Copy:

> This is stable context this Bear should know.

Actions:

- Edit context
- Use setup helper, later

### Role contracts

Inspectable protected block.

Copy:

> Role contracts define what the Bear's internal roles are allowed and expected to do. They preserve trust boundaries.

Actions:

- View role contracts
- View composed prompt for role

### Composed prompts

Preview of what role agents see.

Copy:

> Den combines the Den baseline, a role contract, user steering, Bear context, and runtime context into the prompt a role sees.

Actions:

- View `talk` prompt
- View `pair` prompt
- View `curate` prompt
- View `work` prompt
- View `watch` prompt
- Copy prompt

## First-bear onboarding implications

Onboarding should produce:

1. template id/version
2. bear name
3. bear description
4. materialized role contracts or role contract version
5. user steering
6. bear context
7. optional first task

A heavy template workflow can still ask rich questions, but those answers should primarily become **user steering** and **bear context**.

Template defaults supply the role contracts.

## Template implications

Each template should define role contracts for all five Bear roles, even if some are not active in the initial product surface.

### Software Product Builder

Possible role emphasis:

- `talk`: product discussion, synchronous planning, task intent capture
- `pair`: IDE/client collaboration, code changes, debugging, docs updates
- `curate`: product/project memory integration, skill proposal review
- `work`: approved background build/product/release tasks
- `watch`: approved observation of dev/product events

### Personal Assistant

Possible role emphasis:

- `talk`: conversational planning, drafting, follow-up capture
- `pair`: collaboration inside productivity tools, when available
- `curate`: durable preference/contact/routine integration
- `work`: approved scheduling, follow-up, and admin tasks
- `watch`: inbound observations from calendar/email/events, when connected

### Research & Writing Partner

Possible role emphasis:

- `talk`: exploratory conversation, explanations, drafting support
- `pair`: collaboration inside document/editor tools
- `curate`: durable topic/project/style integration
- `work`: approved research or writing tasks
- `watch`: inbound observation of subscribed sources or document changes

## Project plan

### Project 1: Role-aware context composition design

Goal: Lock down terminology and boundaries.

Deliverables:

- define `Den baseline`, `Role contracts`, `User steering`, `Bear context`, `Runtime/thread context`, and `Composed role prompt`
- define how the five role contracts relate to `talk`, `pair`, `curate`, `work`, and `watch`
- define how legacy `system_prompt` maps into this model
- define composition order
- define what is user-visible in v1

### Project 2: Minimal role-aware composition engine

Goal: Implement a small Rust service that composes prompt text for a specific role.

Responsibilities:

- load Den baseline text/config
- load role contract for requested role
- load user steering and bear context when present
- accept optional runtime/thread context
- fall back to legacy `system_prompt` when role-aware context is absent
- render composed prompt in stable order
- expose structured representation for UI/debugging

Possible API shape:

```/dev/null/context-composer.rs#L1-24
#[derive(Clone)]
struct BearContextProfile {
    template_id: Option<String>,
    template_version: Option<String>,
    role_contract_version: Option<String>,
    role_contracts: RoleContracts,
    user_steering: String,
    bear_context: String,
    starter_prompts: Vec<String>,
    first_task: Option<String>,
}

struct RoleContracts {
    talk: String,
    pair: String,
    curate: String,
    work: String,
    watch: String,
}

struct ComposedRoleContext {
    role: BearRole,
    den_baseline: String,
    role_contract: String,
    user_steering: Option<String>,
    bear_context: Option<String>,
    runtime_context: Option<String>,
    composed_prompt: String,
}
```

### Project 3: Storage migration

Goal: Add storage for the role-aware context profile.

Likely migration:

- add `context_profile JSONB` to `bears`

### Project 4: Legacy compatibility

Goal: Make current bears work without migration.

Behavior:

- If `context_profile` is absent, treat `system_prompt` as legacy manual prompt.
- Details UI can show `Legacy prompt` or current prompt block.
- New role-aware bears still write generated text into `system_prompt` where existing Letta sync paths need it.

### Project 5: Template role contract materialization

Goal: Define the three onboarding templates as generators of role-aware context profiles.

Each template should provide:

- template id/version
- default bear name
- default description
- role contracts or role contract references for `talk`, `pair`, `curate`, `work`, `watch`
- question set
- default user steering generation
- default bear context generation
- starter prompts

For v1, templates can be hardcoded Rust definitions or data files. Avoid a dynamic admin-editable template system initially.

### Project 6: Bear details UI v1

Goal: Add transparency without a complete page redesign.

Changes:

- show `User steering` if `context_profile` exists
- show `Bear context` if `context_profile` exists
- add `View role contracts`
- add `View composed prompt` for at least `talk` and `pair`, as available
- keep existing system prompt editing path for legacy/manual prompts

### Project 7: First-bear onboarding v1

Goal: Create first bear from a heavy template workflow using the role-aware context profile model.

Flow outputs:

- name
- description
- template id/version
- role contract version/materialized contracts
- user steering
- bear context
- starter prompts
- first task draft or handoff

The onboarding controller creates the bear, generates necessary prompt output for current provisioning, syncs/provisions Letta, and redirects to chat.

### Project 8: Helper-assisted editing

Goal: Add composition helpers as shortcuts for editing user-owned layers.

Initial helper behavior:

- edit `User steering`
- edit `Bear context`
- ask practical questions
- generate replacement text for one block
- show proposed text before saving

Helpers should not casually rewrite role contracts.

### Project 9: Future richer model

Goal: Split the simple role-aware model only when real needs appear.

Potential future splits:

- `User steering` -> style, priorities, workflow preferences, role-specific steering
- `Bear context` -> setup context, project context, user preferences
- `Role contracts` -> platform base role contracts + template overlays
- Den baseline -> platform safety, Den operating model, global tool rules

Do not build this until the simple role-aware model becomes insufficient.

## Adjusted answers to prior open questions

### Should users edit sections or full prompt?

For v1, users edit:

- User steering
- Bear context

They inspect:

- Role contracts
- Composed role prompts

### Should `system_prompt` remain canonical?

Short term: yes, for compatibility.

For role-aware bears, generated prompt text may still be written into `system_prompt` for existing provisioning/sync paths.

Long term: `context_profile` should become canonical.

### How should raw prompt edits work?

For v1, avoid solving full reconciliation.

Options:

- legacy bears keep raw prompt editing
- role-aware bears prefer steering/context editing
- if raw prompt edit is used on a role-aware bear, mark it as manual/legacy or require explicit conversion

### Which instructions are protected?

In v1:

- Den baseline
- Role contracts

### Should onboarding answers be shown as provenance?

Not in v1. Store answers if useful, but do not build provenance UI yet.

### Should prompt changes have audit history?

Not in v1.

### Should users copy/export composed prompts?

Yes, especially for `talk` and `pair`, because it supports education and debugging.

## Recommended first slice

Implement the foundation in this order:

1. Add role-aware context composition service with Den baseline + legacy prompt fallback.
2. Add `context_profile` storage.
3. Add composed prompt preview to bear details for at least current user-facing roles.
4. Add editable User steering / Bear context blocks for role-aware bears.
5. Add inspectable role contracts.
6. Update bear creation/provisioning to write generated prompt text where current paths require `system_prompt`.
7. Build first-bear onboarding on top.

This keeps the model simple enough for v1 while respecting the multi-agent Bear architecture.
