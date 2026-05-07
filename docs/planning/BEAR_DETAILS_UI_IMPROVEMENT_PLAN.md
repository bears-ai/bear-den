# Bear Details UI Improvement Plan

## Summary

The bear details page should evolve from a mostly administrative/debug view into a clear, educational control room for understanding and managing a bear.

The page should help users understand that a bear is shaped by:

- structured instructions
- memory/context
- capabilities/tools
- threads/activity
- access
- advanced runtime/Letta details

The key design constraint is transparency: BEARS may use prompt composition helpers and structured instruction sections, but users should be able to see the instructions the agent will actually see. Prompt composition should be framed as a shortcut for authoring prompt sections, not as an abstraction that hides how the bear works.

## Product principle

BEARS should make users better AI operators. The details page should therefore make the bear legible.

Do not over-humanize the page into an opaque profile/personality editor. Instead, show the user:

- what the bear is instructed to do
- which instruction sections are user-editable
- which sections are platform-managed
- which parts were generated from onboarding/template setup
- what the final composed prompt looks like

Friendly summaries are useful, but they should point back to agent-visible instructions.

## Revised mental model

A bear is not just a chat. A bear is a persistent AI agent configured by a structured instruction bundle plus memory, tools, and threads.

The user-facing mental model should be:

> This bear behaves this way because these are its instructions, context, memory, and capabilities.

The details page should reinforce that model.

## Instruction architecture

The bear details UI should eventually present prompts as structured sections.

Recommended section order:

1. Platform safety instructions
2. BEARS operating model / self-awareness
3. Bear identity
4. Bear purpose
5. Bear personality / working style
6. Capabilities and tool-use guidance
7. Memory and context policy
8. User setup context
9. Template-specific instructions
10. Runtime/thread/task-local instructions, where applicable

Not all sections need to be editable. Not all sections need to be equally prominent. But users should be able to inspect the agent-visible text in each relevant section.

## Section ownership model

Each instruction section should have explicit ownership.

### Platform-owned sections

Examples:

- platform safety instructions
- BEARS operating model
- global tool safety rules

These should usually be read-only for normal users. They can be visible, collapsible, and labeled as managed by BEARS or the operator.

### Template-owned sections

Examples:

- template-specific role defaults
- template memory guidance
- template capability posture

These are generated from the selected template. After bear creation, the materialized text should be visible. Whether future template updates automatically change existing bears is a separate product decision.

### User/admin-owned sections

Examples:

- bear identity
- bear purpose
- working style/personality
- setup context
- custom instructions

These should be directly editable by bear admins.

### Runtime-injected sections

Examples:

- current thread context
- tool availability
- active user/session context
- ephemeral task constraints

These should not necessarily be edited on the details page, but users should understand that runtime context may be added separately from stored bear instructions.

## UI concept: instructions first, helper second

The top of the details page should not hide the prompt behind profile abstractions.

Recommended primary section:

- `Purpose & instructions`

Recommended secondary section:

- `Working style instructions`

Composition helpers should be available, but framed as shortcuts:

- `Use setup helper`
- `Generate prompt changes`
- `Review setup answers`

Direct editing should remain primary:

- `Edit instructions`
- `View composed prompt`
- `Edit section`

## Purpose & instructions section

This replaces an over-humanized "profile" concept.

It should show:

- bear name
- template origin, if any
- short purpose summary
- agent-visible purpose/instruction excerpt
- links/actions to view the composed prompt and edit instructions

Suggested copy:

- `Started from template: Software Product Builder`
- `The template generated initial instructions. You can edit them directly.`
- `This bear's current instructions tell it to...`

The summary should not be treated as the source of truth. The prompt section is the source of truth.

## Working style instructions section

This replaces a vague personality/tuning panel.

It should show prompt clauses such as:

- `Plan before making code changes unless the user asks for a quick patch.`
- `Challenge vague product assumptions before implementation.`
- `Explain important code changes after making them.`

If these clauses were generated from onboarding answers, the UI can optionally show the provenance:

- onboarding question
- user answer
- generated prompt clause

This helps teach users how setup choices become durable instructions.

## Composed prompt view

The details page should include a way to inspect the full composed prompt.

The composed prompt view should be clear that:

> This is the full instruction text sent to the agent.

It should show sections in order, likely with headings and ownership labels.

Conceptual example:

```/dev/null/composed-prompt.md#L1-32
# Platform safety instructions
Managed by BEARS. Read-only.
...

# BEARS operating model
Managed by BEARS. Read-only.
...

# Bear identity
Editable.
...

# Bear purpose
Editable.
...

# Working style
Editable.
...

# Capabilities and tool-use guidance
Template/user managed.
...

# Memory and context policy
Template/user managed.
...

# User setup context
Editable.
...
```

The composed prompt view may be implemented as a separate page, modal, or collapsible details panel.

Useful actions:

- copy prompt
- view section sources
- edit editable sections
- compare current prompt with generated proposal

## Composition helper model

Prompt composition helpers should edit sections, not silently mutate the entire bear.

Recommended helper flow:

1. User chooses a section to improve, such as `Working style`.
2. UI asks practical questions.
3. System generates proposed replacement text for that section.
4. User sees a diff or side-by-side comparison.
5. User accepts, edits, or cancels.

This reinforces that helpers generate text. The user reviews the agent-visible output before saving.

## Relationship to onboarding

First-bear onboarding should produce structured instruction sections.

For example, onboarding answers may generate:

- bear identity
- bear purpose
- working style instructions
- setup context
- template-specific instructions
- memory guidance
- capability expectations

The bear details page should expose those sections after creation.

This makes onboarding feel consequential and understandable, rather than a one-time wizard whose output disappears.

## Relationship to existing `system_prompt`

Today, bears have a `system_prompt` field. The structured instruction model should evolve without breaking existing bears.

Recommended phased approach:

### Phase 1: Preserve composed prompt

- Keep `bears.system_prompt` as the composed prompt sent to Letta.
- Add structured metadata, likely JSONB, for instruction sections and onboarding/setup profile.
- Generate `system_prompt` from sections when section-aware editing is used.
- Existing bears continue to work.

### Phase 2: Section editor

- Add UI for editing structured sections.
- Show composed prompt preview.
- Save sections and regenerate `system_prompt`.

### Phase 3: Helper-assisted editing

- Add setup/helper flows that propose changes to specific sections.
- Show prompt diffs before saving.
- Track section provenance where useful.

### Phase 4: Advanced composition

- Operator-level platform sections.
- Template versioning.
- Prompt migration/update helpers.
- Prompt audit/history.

## Candidate data model

A first implementation can use JSONB rather than a fully normalized schema.

Potential field:

- `bears.setup_profile JSONB`
- or `bears.instruction_sections JSONB`

Conceptual shape:

```/dev/null/bear-instruction-sections.json#L1-51
{
  "template_id": "software_product_builder",
  "template_version": "1",
  "sections": {
    "bear_identity": {
      "owner": "user",
      "source": "onboarding",
      "text": "Your name is Product Builder. You are the user's software product-building bear."
    },
    "bear_purpose": {
      "owner": "user",
      "source": "template",
      "text": "Your purpose is to help the user design, build, document, and ship software products."
    },
    "working_style": {
      "owner": "user",
      "source": "onboarding",
      "text": "Be practical, direct, and collaborative. Challenge vague product assumptions respectfully."
    },
    "capabilities_guidance": {
      "owner": "template",
      "source": "template",
      "text": "When code workspace tools are available, inspect files before proposing code changes."
    },
    "memory_policy": {
      "owner": "template",
      "source": "template",
      "text": "Remember stable preferences that help future collaboration. Do not remember secrets."
    },
    "setup_context": {
      "owner": "user",
      "source": "onboarding",
      "text": "Initial setup context: ..."
    }
  }
}
```

## Page structure recommendation

A future bear details page could be organized like this:

```/dev/null/bear-details-layout.md#L1-49
# Product Builder

[Chat] [All threads] [Code with this bear]

## Purpose & instructions
Started from template: Software Product Builder

This bear's current instructions tell it to:
- help with product planning
- help with implementation
- help with documentation
- avoid overengineering
- keep changes reviewable

Instruction excerpt:
> You are a software product-building partner...

[View composed prompt] [Edit instructions] [Use setup helper]

## Working style instructions
These are part of the bear's prompt:
- Plan before coding unless asked for a quick patch.
- Challenge unclear product assumptions.
- Explain important code changes.
- Keep documentation in sync.

[Edit section] [Use helper]

## Current threads
Active and archived threads.

## What this bear remembers
Friendly memory summary plus link to detailed Letta memory.

## Capabilities
Plain-language capability status.

## People
Membership/access management.

## Advanced
System prompt, runtime configuration, Letta sync, tool IDs, diagnostics.
```

## Other details page areas

The prompt transparency guidance mainly affects `Purpose & instructions` and `Working style instructions`.

Other areas can remain more user-friendly and less prompt-centered.

### Memory

Show friendly summary first:

- `What this bear remembers`

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

- raw full prompt
- Letta sync/drift
- model/runtime config
- tool IDs
- diagnostics

## Enabling projects

### Instruction section storage

Add a place to store structured section text and metadata.

Questions:

- JSONB on `bears` vs separate table?
- How to represent section ownership?
- How to represent section source/provenance?

### Prompt assembly service

Create one service responsible for converting sections into composed prompt text.

Responsibilities:

- stable section ordering
- inclusion/exclusion rules
- prompt preview
- saving composed prompt back to existing `system_prompt`

### Section editor UI

Create editing UI for bear-owned sections.

Needs:

- validation
- preview
- save/cancel
- possibly change history later

### Composed prompt view

Add a view/modal that shows the full agent-visible prompt.

Needs:

- section headings
- ownership/source labels
- copy button
- links to edit relevant sections

### Onboarding integration

First-bear onboarding should create section records rather than only a flat prompt.

Needs:

- map template answers to sections
- preserve setup answers
- show onboarding provenance where useful

### Helper-assisted editing

Add helper flows that propose changes to sections.

Needs:

- target section selection
- question forms
- generated prompt section proposal
- diff/review UI
- save/cancel

### Advanced prompt compatibility

Support existing bears and admins who edit raw prompts today.

Questions:

- What happens if a user directly edits `system_prompt`?
- Does that overwrite sections, mark prompt as custom, or store a raw override?
- How do section edits interact with raw full-prompt edits?

### Template versioning

If templates change, decide whether existing bears are updated, offered migrations, or left unchanged.

Likely recommendation:

- Existing bears keep materialized sections.
- UI can show `Started from template X version Y`.
- Later, offer optional template update/migration.

## Open questions

1. Should normal users edit sections only, or should bear admins also be able to edit the full composed prompt directly?
2. Should `system_prompt` remain the canonical stored prompt, or should structured sections become canonical with `system_prompt` as generated output?
3. How should raw prompt edits be reconciled with section-based editing?
4. Which sections should be platform-owned and read-only?
5. Which sections should be template-owned but user-editable after creation?
6. Should onboarding answers be shown as provenance for generated clauses?
7. Should prompt section changes have audit history?
8. Should users be able to copy/export the composed prompt?
9. How much of the platform safety/system instructions should be visible to normal users?
10. How should runtime-injected context be explained without exposing Letta internals?

## Recommended first implementation slice

Start with a minimal structure that improves the details page without requiring a full prompt architecture migration.

Suggested slice:

1. Rename/reframe top prompt area as `Purpose & instructions`.
2. Show a concise prompt excerpt near the top.
3. Add `View full prompt` action.
4. Add a `Working style instructions` section if structured setup data exists; otherwise omit.
5. Move low-level Letta sync/config details into an `Advanced` section.
6. Keep existing prompt editing path available.
7. Add structured instruction storage in parallel with existing `system_prompt`, but do not require all bears to have sections immediately.

This gets the details page moving toward transparency and education while preserving the current prompt model.
