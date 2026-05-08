# Bear Details UI Improvement Plan

## Summary

The Bear details page should become the Bear's workspace home: a vanilla, progressive-enhancement UI that makes one coherent Bear legible while preserving the five-role internal architecture (`talk`, `pair`, `curate`, `work`, `watch`).

The page should not be organized around database fields, Letta ids, or one-agent prompt concepts. It should be organized around the user's mental model of a Bear:

- what this Bear is for
- what it is helping with now
- how the user steers it
- what stable context it has
- how its roles are organized
- what shared memory it has built
- who can access it
- which advanced operations need admin attention

This plan supersedes earlier narrower plans that treated Bear details primarily as a prompt/section editor.

## Product principle

A Bear feels like one assistant, but internally it has five specialized roles. The details UI should preserve that distinction without forcing the user to mentally reassemble role-specific information across many unrelated sections.

The key IA rule is:

> Role-specific capabilities, memory branches, contracts, prompts, and runtime details belong with the role they describe.

Shared Bear-wide concepts remain top-level:

- command center
- current work
- steering
- context
- shared memory
- people/access
- advanced operations

## Progressive enhancement requirement

Implement this as vanilla, progressively enhanced pages.

Baseline behavior:

- every major page and role detail must work as normal server-rendered HTML
- role detail cards should have their own URLs/pages
- no JavaScript should be required to inspect role details or complete admin actions

Enhanced behavior:

- JavaScript may load role detail pages into content panes/tabs within the Bear-level page
- links remain real links
- content panes should preserve accessibility and browser navigation as much as practical
- enhancement should be additive, not a separate SPA

Example route shape:

```/dev/null/routes.md#L1-8
GET /bear/{slug}/details
GET /bear/{slug}/details/roles/{role}
GET /bear/{slug}/details/roles/{role}/prompt
GET /bear/{slug}/details/roles/{role}/memory
GET /bear/{slug}/details/context
GET /bear/{slug}/details/steering
```

Exact route names can change, but role details should be independently addressable.

## Approved three-level IA

### 1. Command center

#### 1.1 Identity

- Bear name
- Slug
- Short description / purpose
- Template origin, if any

#### 1.2 Readiness summary

- Overall Bear status
- Chat readiness
- Pairing readiness
- Memory health summary
- Role provisioning warning summary, if any

#### 1.3 Primary actions

- Chat
- Start new thread
- Pair / Code with this Bear
- All threads
- Tune steering
- Manage access, if permitted

## 2. Current work

### 2.1 Active threads

- Web / `talk` threads
- ACP / `pair` threads
- Last activity
- Open action

### 2.2 Work plans

- Current workboard/work plan status
- In-progress items
- Recently completed items
- Blocked items

### 2.3 Task intents

- Captured requests that may require review
- Source role: `talk` or `pair`
- Status: pending / approved / rejected / dispatched

### 2.4 Background work

- Active `work` runs
- Recent `work` results
- Failed work requiring attention

### 2.5 Observations

- Recent `watch` observations
- Observations awaiting `curate` review

### 2.6 Archive

- Archived thread count
- All threads link

## 3. Steering

### 3.1 User steering text

- Current steering block
- Template-generated defaults, if any
- User edits

### 3.2 Steering helpers

- Use setup helper
- Regenerate steering suggestion
- Review generated change before save

### 3.3 Steering explanation

- Steering affects all roles
- Steering does not change protected role boundaries

## 4. Context

### 4.1 Bear context text

- Stable context block
- Project/user/domain context
- Setup-derived context

### 4.2 Starter context

- First task from onboarding
- Starter prompts
- Template examples

### 4.3 Context helpers

- Edit context
- Use setup helper
- Add project/background notes

## 5. Roles

The Roles section is the home for role-specific details. Do not create separate top-level Memory/Capabilities/Contracts/Prompts sections that repeat the five-role structure independently.

Each role detail follows the same pattern:

```/dev/null/role-detail-pattern.md#L1-38
## {role} — {plain English name}

### Purpose
What this role is responsible for.

### Surfaces
Where this role appears or runs.

### Capabilities
What this role can do.

### Memory
Which branches it reads/writes.

### Contract
Protected role instructions and boundaries.

### Prompt
Composed prompt for this role.

### Runtime status
Provisioning, runtime family, sync, ids, errors.

### Actions
Relevant user/admin actions.
```

### 5.1 `talk` — conversational front door

#### Purpose

- Handles chat-like conversation
- Captures user intent
- Explains, plans, answers, and routes requests

#### Surfaces

- Web chat
- Future Slack/Discord/etc.

#### Capabilities

- Synchronous conversation
- Task intent capture
- Channel-safe tools
- Work plan updates, if available

#### Memory

- Reads `core/`
- Reads/writes `talk/`
- Does not directly promote to `core/`

#### Contract

- View protected role contract
- Explain role boundaries

#### Prompt

- View composed `talk` prompt
- Copy prompt

#### Runtime status

- Ready / pending / failed
- Runtime family
- Letta agent id
- Last sync
- Provisioning errors

#### Actions

- Open chat
- Start new thread
- View role memory

### 5.2 `pair` — collaborative tool/IDE partner

#### Purpose

- Works alongside the user inside tools
- Helps with code, documents, debugging, design, and active collaboration

#### Surfaces

- ACP clients
- IDEs
- Future design/productivity tools

#### Capabilities

- Client-mediated tool use
- Code/workspace context
- User-gated actions
- File/document collaboration

#### Memory

- Reads `core/`
- Reads/writes `pair/`
- Does not directly promote to `core/`

#### Contract

- View protected role contract
- Explain client-mediated boundary

#### Prompt

- View composed `pair` prompt
- Copy prompt

#### Runtime status

- Ready / pending / failed
- Runtime family
- Letta agent id
- Last sync
- Provisioning errors

#### Actions

- Code with this Bear
- Create/view code token
- View role memory

### 5.3 `curate` — memory and integration reviewer

#### Purpose

- Integrates durable knowledge
- Reviews memories, task intents, observations, results, and skill proposals
- Promotes durable knowledge into shared `core/`

#### Surfaces

- Not directly user-facing
- Internal review/integration role

#### Capabilities

- Memory review
- Task intent review
- Observation review
- Skill proposal review
- Shared memory promotion through Den-controlled mechanisms

#### Memory

- Reads across role branches, subject to policy
- Writes `curate/`
- Promotes to `core/`
- Does not write directly to other role branches

#### Contract

- View protected role contract
- Explain semantic authority / review boundary

#### Prompt

- View composed `curate` prompt
- Copy prompt

#### Runtime status

- Ready / pending / failed
- Runtime family
- Letta agent id
- Last sync
- Provisioning errors

#### Actions

- View review queue, future
- View memory promotions, future
- View role memory

### 5.4 `work` — approved outbound executor

#### Purpose

- Executes approved scheduled/event-triggered/background work
- Performs external actions only within reviewed task scope

#### Surfaces

- Den task dispatch
- Schedules
- Approved background jobs

#### Capabilities

- Approved API calls
- Scheduled tasks
- Event-triggered work
- Run-status reporting

#### Memory

- Reads `core/`
- Reads task definition/run context
- Reads/writes `work/`
- Does not read raw `talk/`, `pair/`, or `watch/` branches directly

#### Contract

- View protected role contract
- Explain approved-scope boundary

#### Prompt

- View composed `work` prompt
- Copy prompt

#### Runtime status

- Ready / pending / failed
- Runtime family
- Letta agent id
- Last sync
- Provisioning errors

#### Actions

- View approved tasks, future
- View recent work results, future
- View role memory

### 5.5 `watch` — inbound observer

#### Purpose

- Receives external events
- Turns webhooks, polling results, and subscriptions into structured observations
- Does not act outward

#### Surfaces

- Webhooks
- Polling
- Queues
- Subscriptions
- Event streams

#### Capabilities

- Inbound event parsing
- Observation creation
- Subscription monitoring
- Event summarization

#### Memory

- Reads `core/`
- Reads delivered event payloads
- Reads/writes `watch/`
- Does not write `core/` directly
- Does not trigger outbound action directly

#### Contract

- View protected role contract
- Explain inbound-only boundary

#### Prompt

- View composed `watch` prompt
- Copy prompt

#### Runtime status

- Ready / pending / failed
- Runtime family
- Letta agent id
- Last sync
- Provisioning errors

#### Actions

- View subscriptions, future
- View observations, future
- View role memory

## 6. Shared memory

Shared memory is Bear-wide memory, not role-specific branch memory. Role-specific memory belongs inside the role detail pages/cards.

### 6.1 `core/` memory

- Durable shared Bear memory
- Promoted knowledge
- Stable preferences
- Important context

### 6.2 Memory health

- Repo status
- Commit count
- Memory file count
- Recent memory manager events
- Warnings/errors

### 6.3 Recent memory changes

- Recent promotions into `core/`
- Recent updates by `curate`
- Recent role branch changes, summarized

### 6.4 Memory details

- Link to full memory browser
- Link to runtime memory blocks
- Advanced tree/file view

## 7. People & access

### 7.1 Members

- Users with access
- Admin/member role labels

### 7.2 Access management

- Add member
- Remove member
- Change role

### 7.3 Sharing/invites, future

- Invite links
- Team/household/workspace access

## 8. Advanced operations

### 8.1 Model and runtime configuration

- Default model
- Agent type
- Runtime plan
- Role runtime families

### 8.2 Tool and skill configuration

- Tool IDs
- Skill manifest
- Skill proposals
- Role applicability

### 8.3 Letta sync and diagnostics

- Per-role sync status
- Drift status
- Force sync
- Recompile status
- Raw Letta agent state links/details

### 8.4 Prompt compatibility output

- Generated prompt snapshots
- Legacy migration notes
- Raw `system_prompt` compatibility field, if still present internally

### 8.5 Danger zone

- Delete Bear
- Reset/reprovision roles
- Clear/rebuild derived state, future

## Placement of technical details

Technical details should appear in the context that gives them meaning:

- Letta agent IDs → role detail runtime status
- Runtime family → role detail runtime status
- Role provisioning status → role details and command-center health summary
- MemFS role view status → role detail memory status
- Shared MemFS health → shared memory
- Tool IDs → capabilities/advanced operations
- Prompt snapshots → role prompt views / advanced prompt compatibility
- Letta sync/drift → advanced operations, summarized in command center if unhealthy
- Members → people & access
- Conversations → current work as threads

## Relationship to context composition

This IA depends on role-aware context composition but should not be reduced to it.

Context composition answers:

> What text does each role see?

The Bear details IA answers:

> What is this Bear doing, how do I steer it, what does it know, what can each role do, and what needs attention?

## Implementation plan

### Project 1: Restructure Bear details into the approved IA

- Command center
- Current work
- Steering
- Context
- Roles
- Shared memory
- People & access
- Advanced operations

### Project 2: Role detail pages

- Add server-rendered pages for `/bear/{slug}/details/roles/{role}`.
- Each role detail page follows the shared role detail pattern.
- Include role contract, composed prompt, memory branch status, capabilities, and runtime status.

### Project 3: Progressive role panes

- Enhance the Bear details page with JS that loads role detail pages into content panes.
- Keep links functional without JS.
- Prefer semantic links/buttons and preserve accessible focus behavior.

### Project 4: Shared memory redesign

- Present `core/` memory separately from role branch memory.
- Move role branch memory status into role detail pages/cards.
- Keep detailed memory browser links.

### Project 5: Capabilities in role context

- Move role-specific capabilities into role details.
- Add command-center capability summary.
- Keep raw tools/skills in advanced operations.

### Project 6: Advanced operations cleanup

- Consolidate model/config, Letta sync, raw tool IDs, runtime diagnostics, and danger-zone actions.
- Remove legacy one-agent prompt framing from primary UI.

## Completed MVP slice

Current implementation has pieces of the future IA but not the final organization:

- user steering display
- Bear context display
- role agents table
- composed `talk`/`pair` prompt previews
- memory health/details
- active/all threads
- advanced prompt/config blocks

## Remaining near-term work

1. Reorder Bear details page around the approved IA.
2. Add role detail pages for all five roles.
3. Move composed prompts into role detail pages/panes.
4. Move role-specific memory/capabilities into role detail pages/panes.
5. Present shared `core/` memory separately from role branches.
6. Remove legacy single-agent prompt framing from primary UI.
7. Add progressive enhancement for role content panes.
