# First Bear Onboarding Plan

## Summary

After email verification, new users should be sent directly into a guided "first bear" setup flow. The flow should use **heavy templates with simple language**: internally, templates may shape prompts, memory posture, capabilities, initial context, and follow-up workflows; externally, users should feel like they are answering practical questions about how they want their bear to work with them.

The goal is not merely to create a bear record. The goal is to introduce the product model and create a first bear that is immediately useful.

## Product thesis

A bear should not feel like a generic chat window with a name. It should feel like a persistent helper with:

- a role
- a working style
- remembered context
- clear capabilities and boundaries
- starter tasks
- a reason to come back

A guided first-bear workflow is the right place to teach those concepts.

## Template set

Initial onboarding should offer three high-level templates.

### 1. Software Product Builder

A bear for turning software ideas into working products.

It combines:

- coding assistant
- product manager
- documentation writer
- release/support partner

It helps with:

- product requirements
- MVP scoping
- implementation plans
- code changes
- debugging
- tests
- README/API docs/release notes
- product tradeoffs

### 2. Personal Assistant

A bear for everyday work and life logistics.

It combines:

- communications assistant
- scheduling helper
- planning assistant
- follow-up/task organizer
- life admin helper

It helps with:

- drafting emails/messages
- planning weeks/days
- meeting prep
- follow-ups
- scheduling coordination
- task breakdown
- routines
- travel/household/admin checklists

### 3. Research & Writing Partner

A bear for thinking, learning, researching, and creating written work.

It combines:

- knowledge/research assistant
- learning coach
- writing partner
- creative brainstorming partner

It helps with:

- research questions
- article/document summaries
- source comparison
- reading plans
- concept explanations
- brainstorming
- outlines
- drafting and editing
- writing style development

## Design principle: heavy internally, simple experientially

Heavy templates are useful because they can produce meaningfully different bears. But the user should not configure infrastructure.

Do **not** ask users:

- Which Letta agent type do you want?
- Which tool IDs should be enabled?
- Which model/runtime plan should this bear use?
- Which memory block schema do you want?

Instead, ask users:

- How do you want this bear to work with you?
- What should it know to get started?
- What should it help with first?
- Which actions should require confirmation?

The implementation maps these answers to system prompt fragments, stored configuration, memory seeds, capability expectations, and future integrations.

## Onboarding flow

### Entry point

After email verification:

1. Check whether the user already has access to any bear.
2. If not, redirect to `/onboarding/first-bear`.
3. If yes, proceed to the normal post-verification destination.

This flow should be idempotent. A user should not be trapped in onboarding if a bear already exists or if a previous attempt partially succeeded.

### Step 1: Choose template

User-facing question:

> What kind of bear do you want to start with?

Options:

- Software Product Builder
- Personal Assistant
- Research & Writing Partner

Each option should show:

- short description
- a few example tasks
- perhaps a "best for" line

### Step 2: Name and identity

Question:

> What should we call your bear?

Defaults should be template-aware, for example:

- Product Builder
- Personal Assistant
- Research Partner

The name should be editable and skippable through a default.

Optional identity preview:

> This bear will help you build, plan, and document software products.

### Step 3: Working style

Ask template-specific practical questions.

Examples for Software Product Builder:

- Should it plan before coding, or move quickly to patches?
- Should it challenge product assumptions?
- Should it explain code changes in detail?

Examples for Personal Assistant:

- Should communication be concise, warm, or formal?
- Should it proactively suggest follow-ups?
- Should it focus on daily planning, weekly planning, or only when asked?

Examples for Research & Writing Partner:

- Should it be rigorous/source-aware or exploratory/creative?
- Should it ask clarifying questions before drafting?
- Should it focus more on learning, research, or writing output?

These answers should be stored as structured setup answers and incorporated into generated prompt/context.

### Step 4: Context seeding

Ask for useful initial context. This is where templates become much more valuable.

Software Product Builder examples:

- What are you building?
- What tech stack do you prefer?
- Do you have an existing codebase?
- What is the current milestone?

Personal Assistant examples:

- What time zone are you in?
- What does a good week look like?
- What tone do you prefer for messages?
- Are there recurring commitments this bear should know about?

Research & Writing Partner examples:

- What topics are you exploring?
- Are you working on a specific writing project?
- Who is the audience?
- Do you prefer concise notes, detailed explanations, or polished drafts?

Context fields should be optional. Defaults should be good enough.

### Step 5: Capability expectations

Use plain-language capability cards or toggles.

Examples:

Software Product Builder:

- Help with code through your editor
- Write and update docs
- Make implementation plans

Personal Assistant:

- Draft messages
- Help plan schedules
- Track follow-ups
- Integrations can be connected later

Research & Writing Partner:

- Summarize pasted text
- Organize notes
- Draft and edit writing
- Use sources/documents when available

This step does not need to enable every integration on day one. It can set expectations, gather consent, and record future intent.

### Step 6: First task

Question:

> What should this bear help you with first?

Offer template-specific prompt chips and a freeform textarea.

The result can either:

- prefill the chat input after creation, or
- submit the first message automatically after explicit confirmation

Default recommendation: prefill first, autosubmit only if the UI clearly says it will.

### Completion

On completion:

1. Create the bear.
2. Apply selected template configuration.
3. Seed initial context/memory as appropriate.
4. Redirect to `/bear/{slug}`.
5. Show a template-specific intro panel or first assistant message.

## High-level implementation architecture

### Main components

#### 1. Onboarding route/controller

New Den web routes, likely under `src/web/onboarding.rs`:

- `GET /onboarding/first-bear`
- `POST /onboarding/first-bear`
- optionally per-step routes if using server-side multi-page forms

Responsibilities:

- enforce login/email verification
- detect whether onboarding is needed
- render setup UI
- validate submitted answers
- call bear creation/provisioning services
- redirect to the new bear

#### 2. Onboarding state model

A workflow may need persistent draft state so users can refresh, go back, or recover from provisioning failures.

Possible table:

- `user_onboarding_flows`
  - `id`
  - `user_id`
  - `flow_type`
  - `status`
  - `current_step`
  - `answers` JSONB
  - `created_bear_id` nullable
  - timestamps

Alternative for v1: avoid persistence and use one POST payload. This is simpler but less robust.

Expected enabling project: decide whether onboarding is single-page or persisted multi-step.

#### 3. Template registry

A server-side registry should define templates in structured form.

Possible location:

- Rust module, e.g. `src/core/bear_templates.rs`
- or data files under `defaults/bear_templates/*.toml|json|yaml|md`

Each template should include:

- stable template id
- display name
- short description
- example tasks
- default bear name
- base system prompt fragment
- setup questions
- memory seed mapping
- capability expectations
- optional runtime/tool hints
- intro message
- first-task prompt chips

Expected enabling project: design template definition format.

#### 4. Prompt assembly service

A service should assemble the final bear prompt from:

- base organization policy / global defaults
- selected template prompt
- structured setup answers
- user-provided context
- safe capability/permission language

This should avoid stringly ad hoc prompt construction in route handlers.

Expected enabling project: define prompt composition rules and test fixtures.

#### 5. Bear provisioning integration

Onboarding should use the existing bear creation/provisioning path where possible.

Responsibilities:

- create Den `bears` row
- attach creator as admin/member
- provision or link Letta agent
- sync Den registry fields to Letta
- handle provisioning errors gracefully

Expected enabling project: extract reusable bear creation service if current create flow is too form-bound.

#### 6. Memory/context seeding

Template setup answers may need to become initial memory or context.

Examples:

- preferred communication tone
- time zone
- product/project description
- writing style preferences
- current learning/research topic

Possible approaches:

- include in system prompt only
- seed Letta memory blocks
- store onboarding answers in Den and expose them to tools/runtime
- create an initial assistant/user exchange that captures the context

Expected enabling project: decide what belongs in prompt vs Letta memory vs Den metadata.

#### 7. Capability configuration

Heavy templates imply different expected capabilities. Some capabilities may exist now; others may be future integrations.

Potential records:

- template-selected capability intents
- enabled tool IDs
- runtime plan hints
- future integration consent/preferences

Expected enabling project: connect templates to the bear capability management model.

#### 8. First task handoff

The first task should flow naturally into chat.

Options:

- redirect with `?draft_message=` or session-backed draft
- create a pending first message in Den
- autosubmit the message after redirect
- render an intro panel with the first task available as a one-click action

Expected enabling project: choose a safe first-message handoff pattern.

#### 9. Onboarding completion tracking

A user should not repeatedly see first-bear onboarding after it is complete.

Possible rules:

- If user has at least one bear membership, onboarding is complete.
- Or track explicit `first_bear_onboarding_completed_at` on user/account.
- Or both: membership check plus completion timestamp.

Expected enabling project: decide whether completion is inferred or explicit.

## Data model candidates

### Bear template definitions

Conceptual shape:

```/dev/null/bear_template.json#L1-46
{
  "id": "software_product_builder",
  "name": "Software Product Builder",
  "default_bear_name": "Product Builder",
  "description": "Build software with a partner who can plan, code, and document.",
  "prompt_fragments": {
    "base": "...",
    "memory_policy": "...",
    "permissions": "..."
  },
  "questions": [
    {
      "id": "working_style",
      "type": "single_select",
      "label": "How should this bear work with you?",
      "options": ["Plan first", "Move quickly", "Ask me each time"]
    }
  ],
  "capability_intents": ["code_workspace", "docs", "implementation_planning"],
  "starter_prompts": [
    "Help me turn an app idea into an MVP plan.",
    "Help me understand this codebase."
  ]
}
```

### Onboarding flow record

Conceptual shape:

```/dev/null/onboarding_flow.sql#L1-12
CREATE TABLE user_onboarding_flows (
    id UUID PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id),
    flow_type TEXT NOT NULL,
    status TEXT NOT NULL,
    current_step TEXT NOT NULL,
    answers JSONB NOT NULL DEFAULT '{}',
    created_bear_id UUID NULL REFERENCES bears(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ NULL
);
```

## Enabling projects likely needed

This effort will probably uncover several smaller projects.

### Onboarding routing and redirect policy

- Decide where email verification redirects.
- Add first-bear-needed detection.
- Avoid loops if provisioning fails.

### Template registry

- Define template schema.
- Choose code vs data-file representation.
- Add validation/tests for templates.

### Prompt composition

- Build a prompt assembly layer.
- Snapshot expected prompts in tests.
- Keep operator defaults and template fragments composable.

### Bear creation service extraction

- Ensure onboarding and existing `/bears/new` share creation/provisioning logic.
- Avoid duplicating validation and Letta provisioning.

### Memory seeding strategy

- Decide which setup answers become durable memory.
- Add safe memory defaults and user-visible explanation.
- Avoid storing secrets/sensitive data accidentally.

### Capability mapping

- Map template capability intents to real Den/Letta capabilities.
- Keep future integrations representable without pretending they exist.
- Possibly use the existing capability management plan as the long-term target.

### First task handoff

- Decide draft vs autosubmit.
- Support safe carryover from onboarding to chat.
- Make the first chat moment feel intentional.

### Recovery/idempotency

- Handle partially created bears.
- Handle Letta provisioning failures.
- Let users retry or create with reduced capability.

### Admin/operator controls

- Operators may need to edit default templates.
- Templates may need deployment-specific model/tool compatibility.
- Some templates may be disabled if dependencies are not configured.

### Analytics/observability

- Track template chosen.
- Track onboarding completion.
- Track first-message submission.
- Track provisioning failures.

## Open questions

1. Should onboarding be one page, multi-step server-rendered pages, or progressive-enhanced JavaScript?
2. Should first task be prefilled or automatically sent?
3. Should template definitions live in Rust code or external data files?
4. Should setup answers be stored permanently, or only compiled into prompt/memory?
5. Which setup answers are appropriate to seed into Letta memory?
6. How much template customization should operators have?
7. Should templates control actual tools/runtime from v1, or only intent/prompt/context?
8. How should users later revisit or rerun template setup for an existing bear?
9. Should a user be allowed to skip first-bear onboarding entirely?
10. What is the fallback if Letta is unavailable immediately after verification?

## Recommended first implementation slice

Start with a medium-heavy implementation that is architecturally ready for heavier behavior.

Suggested first slice:

1. Add `/onboarding/first-bear` after email verification.
2. Implement the three template definitions in a server-side registry.
3. Use a single-page form with sections:
   - template
   - name
   - working style
   - initial context
   - first task
4. Assemble a template-specific system prompt.
5. Create the bear using existing provisioning logic.
6. Store onboarding answers in Den JSON for future use if cheap to add.
7. Redirect to chat with the first task prefilled or shown as a prominent starter action.

This gives users a meaningful guided workflow while avoiding a large multi-step state machine as the very first implementation.
