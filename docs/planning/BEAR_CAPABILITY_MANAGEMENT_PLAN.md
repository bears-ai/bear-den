# Bear capability management in Den

This document defines the product and implementation plan for managing bear capabilities in Den.

The goal is to give users a simple, consistent way to understand what a bear can do, while giving the runtime a precise model for built-in server tools, connected apps, local ACP client tools, and skills.

## User-facing mental model

A bear has **capabilities**.

> Capabilities are everything a bear is allowed to use to help the user.

Capabilities include:

1. **Built-in tools**
2. **Connected apps**
3. **Local app tools**
4. **Skills**

Users should not need to understand ACP, MCP, Den server boundaries, Codepool execution, or protocol-specific tool routing in the primary UI. Those details can appear in advanced/developer views.

The default user-facing explanation should be:

> Your bear has capabilities: things it can use to help you. Some are built into Bears, like memory and Cabinet search. Some come from connected apps, like GitHub or Linear. Some come from the app you are currently using, like an editor that can let the bear read or edit files. And some are skills — reusable ways of working, like reviewing a PR or triaging bugs. You can choose which capabilities each bear has, what they are allowed to do, and when they must ask first.

## Capability buckets

### 1. Built-in tools

Built-in tools are capabilities provided by BEARS itself, typically owned and governed by Den.

Examples:

- `memory.search`
- `memory.remember`
- `cabinet.search`
- `cabinet.read`
- `artifact.create`
- `artifact.update`
- server-side reflection/subagent tools

User-facing language:

> Built-in tools are native abilities your bear can use inside Bears.

Implementation notes:

- Built-in tools are server-side capabilities.
- Den remains the policy and authorization owner.
- Codepool may request or orchestrate these tools through trusted Den-controlled contracts.
- Built-in tools should not be exposed as ACP client tools by default.

### 2. Connected apps

Connected apps are external capability providers. Internally, many of these may be MCP servers or other connector types.

Examples:

- GitHub
- Linear
- Slack
- filesystem connector
- database connector
- browser automation connector

User-facing language:

> Connected apps let your bear work with external services you authorize.

Implementation notes:

- The normal UI should say "Connected app" or "Connector", not "MCP server".
- MCP details belong in advanced settings and developer docs.
- Connected apps may be configured globally, per workspace, per user, per bear, or per environment.
- Den should normalize connected app tools into capability descriptors before making them available to a bear.

### 3. Local app tools

Local app tools are capabilities exposed by the active ACP client or another session-scoped client.

Examples from an editor client:

- read open file
- list workspace files
- apply patch
- open diff
- run tests
- show notification
- ask the user for confirmation

User-facing language:

> Local app tools are temporary abilities provided by the app you are currently using.

Implementation notes:

- Local app tools are usually scoped to a client connection or session.
- A bear in Zed may be able to edit local files while the same bear in web chat cannot.
- A bear in a mobile client may expose a different set of local tools.
- Den should show these as "Provided by Zed", "Available from this client", or "Only available in this session".
- Local tools must not be available after the client disconnects unless a future explicit background capability model is designed.

### 4. Skills

Skills are reusable workflows, behaviors, or playbooks. They are not usually atomic tools. They often depend on other capabilities.

Examples:

- review a PR
- triage bugs
- write release notes
- investigate a failing test
- prepare a sprint summary
- run a release checklist

User-facing language:

> Skills are reusable ways your bear knows how to work.

Important distinction:

| Type | User mental model |
|------|-------------------|
| Tool | An action the bear can perform |
| Skill | A way the bear knows how to work |

A skill may use built-in tools, connected apps, local app tools, other skills, memory, Cabinet, artifacts, or subagents.

For example, a "Review PR" skill may depend on:

- repository read access
- pull request read access
- optional pull request comment/write access
- optional local editor read tools
- optional test execution tools
- memory search
- artifact creation

## Capability requirements and skill dependencies

Skills should declare requirements in terms of generic capabilities where possible, not hardcoded providers.

Prefer:

- requires ability to read repository code
- requires ability to read pull request metadata
- optionally can write review comments
- optionally can run tests

Avoid, unless truly provider-specific:

- requires Zed
- requires GitHub
- requires one exact MCP server implementation

This allows the same skill to work with multiple providers.

For example, repository read access might be satisfied by:

- Cabinet
- GitHub connector
- filesystem connector
- local ACP editor tools
- future code index service

Skill availability states should be understandable:

| State | Meaning |
|-------|---------|
| Ready | All required capabilities are available |
| Partially available | Required capabilities are available, but optional enhancements are missing |
| Needs setup | One or more required capabilities are missing or unauthenticated |
| Unavailable in this client | Required local app tools are not exposed by the current client/session |
| Disabled | The user, bear, workspace, or policy has disabled the skill |

Example user-facing copy:

> The PR Review skill needs access to pull requests. Connect GitHub or choose a bear that already has pull request access.

## Internal normalized model

Den should normalize all capability sources into a single descriptor model.

Conceptual shape:

- `id`
- `name`
- `description`
- `kind`
  - `server_tool`
  - `connected_app_tool`
  - `client_tool`
  - `skill`
- `provider`
  - `bears`
  - `den`
  - `codepool`
  - `acp_client`
  - `mcp_server`
  - `skill_catalog`
  - external provider id
- `scope`
  - `global`
  - `workspace`
  - `user`
  - `bear`
  - `session`
  - `client_connection`
- `availability`
  - `available`
  - `disabled`
  - `requires_auth`
  - `requires_setup`
  - `requires_client`
  - `unavailable`
- `execution_target`
  - `den`
  - `codepool`
  - `acp_client`
  - `mcp_gateway`
  - `skill_runner`
  - `external_connector`
- `permissions`
  - `read`
  - `write`
  - `network`
  - `filesystem`
  - `shell`
  - `memory_read`
  - `memory_write`
  - `cabinet_read`
  - `artifact_write`
  - `external_service_read`
  - `external_service_write`
- `approval_policy`
  - `never`
  - `on_write`
  - `on_sensitive_action`
  - `always`
  - `inherited`
- `input_schema`
- `output_schema`
- `dependencies`
  - required capability requirements
  - optional capability requirements
- `metadata`
  - labels
  - icon
  - documentation URL
  - provider-specific data
- `audit_policy`
- `last_used_at`
- `created_at`
- `updated_at`

The important implementation principle:

> Users see "Capabilities". Den sees normalized descriptors, scopes, providers, permissions, dependencies, and execution targets.

## Per-bear capability sets

Every bear should have a Den-managed capability set.

A bear's effective capability set is assembled from multiple sources:

1. Default capabilities
   - Baseline BEARS abilities such as memory, chat, Cabinet access, and artifacts depending on policy.
2. Workspace/team capabilities
   - Organization-approved built-in tools, connected apps, and skills.
3. User capabilities
   - User-authorized connectors and preferences.
4. Per-bear capabilities
   - Capabilities explicitly enabled or disabled for this bear.
5. Session/client capabilities
   - ACP client tools and other session-scoped abilities.
6. Runtime/discovered capabilities
   - Capabilities discovered from connected MCP servers, authenticated external providers, or active clients.

Den should compute an effective set for each runtime request.

## Catalogs

Catalogs are libraries of capabilities that can be added to a bear.

Potential catalogs:

| Catalog | Contains |
|---------|----------|
| Built-in catalog | Native BEARS tools |
| Connected app catalog | External connectors and MCP-backed providers |
| Local app catalog | Capabilities exposed by active ACP clients |
| Skill catalog | Reusable workflows and behaviors |
| Team catalog | Organization-approved capabilities |
| Bear capability set | Capabilities currently enabled for one bear |

User-facing language:

> Catalogs are libraries of capabilities you can add to a bear.

Common actions:

- browse catalog
- add to bear
- configure
- enable/disable
- test
- review permissions
- view audit history

## Den UI recommendation

Add a bear capability management area:

> Bear → Capabilities

Primary sections:

1. Built-in
2. Connected Apps
3. Local App Tools
4. Skills

Each capability card should show:

- name
- short description
- type
- provider
- scope
- runs in
- status
- approval requirement
- permissions summary
- enabled/disabled state
- configure/test action where applicable

Recommended fields:

| Field | Example |
|-------|---------|
| Name | Search Cabinet |
| Type | Built-in tool |
| Provider | Bears |
| Available to | This bear |
| Runs in | Bears server |
| Requires approval | On write |
| Status | Enabled |

Advanced/developer view may show:

- protocol: server, ACP, MCP, skill
- provider id
- raw tool ids
- JSON schemas
- scopes
- auth state
- audit policy
- last used
- failure history
- raw descriptor metadata

## Mapping to `bear_channel`

`bear_channel` should receive the trusted runtime capability context for a request.

Conceptual shape:

- `capabilities.server_tools`
  - Den/BEARS-controlled built-in tools
- `capabilities.client_tools`
  - ACP client/session tools that Den has authorized for this request
- `capabilities.mcp_tools`
  - connected app tools normalized by Den
- `capabilities.skills`
  - skills enabled for the bear and available in this runtime context

Example categories:

- server tools:
  - `memory.search`
  - `memory.remember`
  - `cabinet.search`
  - `cabinet.read`
- client tools:
  - `zed.read_file`
  - `zed.apply_patch`
  - `zed.run_tests`
- connected app tools:
  - `github.search_issues`
  - `linear.create_comment`
- skills:
  - `pr_review`
  - `bug_triage`
  - `release_notes`

Runtime rules:

- Codepool may only emit `client_tool_request` for client tools declared in the trusted `bear_channel` context.
- Den persists and audits pending client tool calls.
- Den translates runtime tool requests into ACP client tool calls.
- Den forwards results back to Codepool through the agreed continuation/result contract.
- Den remains the owner of policy, authorization, approval, and audit.
- Built-in server tools stay behind Den policy and should not be exposed as ACP client tools by default.

## Implementation phases

### Phase 1: Capability descriptor model

Define the canonical Den descriptor model.

Deliverables:

- descriptor schema/types
- capability kind taxonomy
- provider model
- scope model
- permission model
- approval policy model
- availability model
- dependency/requirement model

### Phase 2: Den capability registry

Build a Den-owned registry for known capabilities.

Deliverables:

- registry storage
- built-in tool descriptors
- connected app descriptors
- skill descriptors
- descriptor validation
- basic admin/developer inspection

### Phase 3: Per-bear capability sets

Allow each bear to have an explicit configured capability set.

Deliverables:

- bear capability assignment model
- enable/disable support
- inherited default/team/user capability handling
- effective capability resolution
- API endpoints for reading/updating bear capabilities

### Phase 4: Catalog sources

Add catalog/import sources.

Deliverables:

- built-in catalog
- skill catalog
- connected app catalog
- MCP discovery/import path
- ACP client runtime discovery path
- team-approved capability catalog

### Phase 5: Skill dependency resolution

Make skills depend on capability requirements.

Deliverables:

- required/optional capability requirements
- dependency resolution
- ready/partial/needs setup/unavailable states
- user-facing setup guidance
- provider-agnostic requirement matching

### Phase 6: Runtime capability context for `bear_channel`

Pass the effective capability context into `bear_channel`.

Deliverables:

- Den runtime capability resolver
- `bear_channel` capability payload mapping
- tests for request construction
- Codepool handling of declared capabilities
- server tool event surfacing
- skill event surfacing

### Phase 7: ACP client tool relay

After ACP basic chat and runtime capability context are stable, add local client tool execution.

Deliverables:

- ACP client capability ingestion
- authorized `client_tools` declaration in `bear_channel`
- `client_tool_request` handling
- pending call persistence
- timeout and disconnect handling
- cancellation semantics
- client tool result forwarding
- audit records

### Phase 8: Den UI

Build the user-facing capability management UI.

Deliverables:

- Bear → Capabilities page
- sections for Built-in, Connected Apps, Local App Tools, Skills
- status badges
- permission and approval display
- setup flows
- test/configure actions
- advanced/developer details view

### Phase 9: Observability, audit, and policy hardening

Add production-grade controls.

Deliverables:

- capability usage audit log
- tool call audit log
- skill invocation audit log
- memory write audit log
- approval records
- metrics by capability kind/provider/bear/user
- failure and timeout tracking
- policy enforcement tests

## Guardrails and non-goals

Initial guardrails:

- Do not expose BEARS server tools as ACP client tools by default.
- Do not allow local client tools to be used after the client disconnects.
- Do not let Codepool call undeclared client tools.
- Do not hardcode provider-specific skill dependencies when a generic capability requirement can be used.
- Do not make users understand ACP or MCP in the primary UI.
- Do not bypass Den for authorization, policy, approval, or audit.
- Do not allow skills to silently gain new sensitive permissions without surfacing them through capability policy.

Non-goals for the first implementation:

- Full arbitrary background execution using local client tools.
- Complex marketplace billing or commercial catalog flows.
- Cross-organization public capability sharing.
- Automatic installation of external connectors without user/admin authorization.
- Replacing the existing `bear_channel` migration plan.

## Success criteria

This work is successful when:

1. Users can open a bear and understand what it can do.
2. Users can distinguish built-in tools, connected apps, local app tools, and skills without learning protocol names.
3. Den can compute a trusted effective capability set for each bear/session.
4. Skills can declare dependencies and show clear availability/setup states.
5. `bear_channel` receives only authorized capability context.
6. ACP client tools are relayed only when declared, authorized, auditable, and connected.
7. Den remains the policy, approval, and audit owner.
