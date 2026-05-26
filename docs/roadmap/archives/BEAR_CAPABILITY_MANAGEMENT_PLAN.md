# Bear capability management in Den

For the canonical role model and current role names, see [bear roles](../../architecture/bear-roles.md).
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

- `search_memory`
- `remember_memory`
- `cabinet_search`
- `cabinet_read`
- `create_artifact`
- `update_artifact`
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
- By default, Den should manage MCP passthrough: catalog/attachment/configuration in Den, raw provider tools exposed by the runtime, and discovered tools recorded in the effective capability manifest.
- Den-brokered wrapper tools should be added only when raw passthrough is insufficient for policy, audit, stability, or usability.

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
  - `search_memory`
  - `remember_memory`
  - `cabinet_search`
  - `cabinet_read`
- client tools:
  - `zed_read_file`
  - `zed_apply_patch`
  - `zed_run_tests`
- connected app tools:
  - `github_search_issues`
  - `linear_create_comment`
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

## Formal roadmap

This roadmap assumes ACP basic chat is already working. The goal is to make capability management useful incrementally: first prove one BEARS-native capability end-to-end, then generalize to per-bear management, then add progressively more dynamic capability sources.

### Roadmap principles

1. **Start with one BEARS-specific tool.** Do not build an abstract capability platform before at least one real BEARS tool works through the full path.
2. **Den remains the authority.** Capability assignment, authorization, policy, approval, and audit stay in Den even when execution happens in Letta Code, Letta, an ACP client, or an MCP server.
3. **Prove multiple execution targets early.** The first managed capability set should include at least one Den tool, one Letta Code tool, and one Letta-native tool so the model does not accidentally assume a single runtime.
4. **Separate configured capabilities from runtime-discovered capabilities.** ACP local tools and some MCP tools are only known after connection or discovery. The registry must support both configured catalog entries and session-scoped discovered descriptors.
5. **Keep the user model simple.** Users manage “Capabilities”; advanced views can explain Den, Letta Code, Letta, ACP, and MCP execution details.
6. **Skills come after tools.** Skills should depend on normalized capabilities. They should not be the first abstraction implemented.

### My read on the likely priority order

This is the practical execution order across capability management and adjacent runtime work. It complements the formal phases below: the phases define the capability-management product roadmap, while this list captures the likely engineering priority order when ACP, `bear_channel`, Cabinet, runtime events, and observability are considered together.

1. **ACP basic chat stabilization**
   - Validate ACP with real clients such as Zed and OpenCode.
   - Harden authentication, session start/resume, streaming deltas, reconnect/disconnect behavior, error propagation, and bear membership enforcement.
   - Package `bears-acp-adapter` for source/dev install first, then prebuilt CLI releases, optional npm wrapper, and eventual ACP Registry submission.

2. **Capability descriptor model**
   - Normalize built-in tools, Letta Code runtime tools, Letta-native tools, ACP local tools, connected apps, and skills into one Den-owned descriptor model.
   - Keep execution target, provider, scope, permissions, approval policy, availability, and audit policy explicit.
   - Prove the model with at least one real BEARS-native capability instead of designing only in the abstract.

3. **Runtime capability context into `bear_channel`**
   - Den computes the effective capability set for the bear, user, channel, and session.
   - Den passes only authorized capabilities into `bear_channel`.
   - Codepool treats the Den-provided context as authoritative and does not invoke undeclared capabilities.

4. **ACP client tool relay**
   - Ingest ACP client-advertised tools at connection/session time.
   - Normalize them as session-scoped `client_tool` capabilities.
   - Add pending call persistence, timeouts, disconnect handling, cancellation semantics, result forwarding, and audit.
   - Keep local ACP tools unavailable outside the active client/session unless a future explicit background model is designed.

5. **Server tools over `bear_channel`**
   - Enable Den-controlled server tools such as bear introspection, memory, Cabinet, artifacts, and future reflection/subagent tools.
   - Keep BEARS server tools behind Den policy instead of exposing them as ACP client tools by default.
   - Ensure every server tool call is authorized and audited by Den.

6. **Richer runtime events**
   - Extend runtime events beyond assistant text and reasoning deltas.
   - Candidate events include server tool started/finished, client tool requested/completed, memory update proposed/recorded, artifact created, subagent started/finished, and skill invoked/completed.
   - Preserve browser chat compatibility by dropping, translating, or gently surfacing richer events until the Den UI is ready for them.

7. **Capability management UI**
   - Add `Bear → Capabilities` so operators can understand and control what a bear can do.
   - Start with Den, Letta Code, and Letta capabilities, then add ACP local tools, connected apps, and skills as those sources become available.
   - Keep protocol names in advanced/developer views; primary UI should use user-facing concepts like Built-in tools, Runtime tools, Memory tools, Local app tools, Connected apps, and Skills.

8. **Cabinet Phase 2/3 integration**
   - Introduce Cabinet as a Den-controlled capability/tool surface once the basic capability framework exists.
   - Start with Cabinet abstraction tools such as `cabinet_search`, `cabinet_read`, `cabinet_create`, and `cabinet_update`.
   - Later back Cabinet with Outline and expose Cabinet read/write permissions through the same per-bear capability policy model.

9. **Observability, audit, and policy hardening**
   - Add comprehensive audit and metrics after multiple capability sources exist.
   - Track capability usage by bear, user, provider, execution target, result, latency, denial reason, and failure mode.
   - Add admin views for recent usage, denied requests, high-risk grants, tool failures, and approval decisions.

### Phase 1: BEARS-native tool vertical slice

Implement one BEARS-specific tool end-to-end before building broad UI or catalogs.

Recommended first tool:

- `list_bear_members`
  - Purpose: let an authorized user or bear inspect who has access to the bear and what roles/memberships apply.
  - Execution target: `den`.
  - Permissions: membership read / admin-sensitive read.
  - Approval policy: likely `never` for admins/operators, policy-controlled for normal members.
  - Audit: record tool invocation, requesting user, target bear, and whether sensitive fields were included.

Deliverables:

- canonical descriptor schema sufficient for one tool
- hardcoded or seeded built-in descriptor for `list_bear_members`
- Den execution endpoint for the tool
- authorization policy for who may invoke it
- audit record for invocation
- minimal runtime path so a bear can call or request the tool through the existing Den/Codepool boundary
- test proving non-members cannot introspect a bear

Exit criteria:

- A member/admin can ask a bear who has access to it, and the answer is produced through a Den-authorized tool call.
- Den denies the same tool for an unauthorized user.
- The capability appears in an internal/developer listing, even if the user-facing UI is not finished.

### Phase 2: Capability descriptor model and registry foundation

Generalize from the first tool into the Den-owned registry.

Deliverables:

- descriptor schema/types
- capability kind taxonomy
- provider model
- execution target model: `den`, `codepool`, `letta`, `acp_client`, `mcp_gateway`, `external_connector`, `skill_runner`
- scope model: global, workspace/team, user, bear, session, client connection
- permission model
- approval policy model
- availability model
- dependency/requirement model, initially minimal
- registry storage for built-in descriptors
- descriptor validation
- admin/developer API for listing registered descriptors

Exit criteria:

- Den can list known capabilities from the registry.
- `list_bear_members` is represented by the same descriptor model future tools will use.
- The model can represent Den, Letta Code, and Letta execution targets without special-casing them in UI code.

### Phase 3: Prove Den, Letta Code, and Letta tool sources

Add at least one capability from each core BEARS runtime source.

Candidate examples:

| Source | Example capability | Execution target | Purpose |
|--------|--------------------|------------------|---------|
| Den | `list_bear_members` | `den` | Inspect bear membership and role policy |
| Letta Code | `runtime_session_status` or `runtime_cancel_session` | `codepool` | Inspect or control the active runtime session |
| Letta | `memory_read_summary` or `search_memory` | `letta` | Read Letta-native bear memory/state through policy |

Implementation notes:

- Den should normalize all three as capabilities, even if their execution adapters differ.
- Letta-native tools must still be governed through Den policy when surfaced as BEARS capabilities.
- Letta Code capabilities should expose runtime/harness behavior, not become a general bypass around Den.

Deliverables:

- execution adapter interface or equivalent service layer
- Den adapter for `list_bear_members`
- Codepool/Letta Code adapter for one runtime capability
- Letta adapter for one memory/state capability
- tests for descriptor validation and authorization across all three execution targets

Exit criteria:

- A single registry/API can show capabilities from Den, Letta Code, and Letta.
- A per-bear effective capability set can include one capability from each source.
- Den remains the authorization and audit point for all three.

### Phase 4: Per-bear capability access model

Allow each bear to have an explicit configured capability set.

Deliverables:

- bear capability assignment table/model
- enable/disable support per bear
- inherited defaults for baseline capabilities
- effective capability resolution for a bear
- API endpoints for reading/updating bear capabilities
- policy checks that runtime requests can only use enabled capabilities
- audit records for capability grant/revoke/configuration changes

Exit criteria:

- Operators can enable or disable the initial Den, Letta Code, and Letta capabilities per bear through APIs.
- Runtime requests receive only the capabilities enabled for the target bear and allowed for the requesting user/session.
- Disabled capabilities cannot be invoked even if the model/runtime asks for them.

### Phase 5: Bear capability management UI

Build the user-facing management surface once per-bear APIs exist.

Primary location:

> Bear → Capabilities

Initial sections:

1. Built-in tools
2. Runtime tools
3. Memory / Letta tools
4. Advanced details

Deliverables:

- capability list for a bear
- enabled/disabled controls
- status badges
- provider/source display using user-friendly labels
- permissions summary
- approval policy display
- configure/test action where applicable
- advanced/developer view showing execution target, raw descriptor id, schemas, scopes, and audit policy

Exit criteria:

- An operator can manage per-bear access for the initial Den, Letta Code, and Letta capabilities from the browser.
- The UI does not require users to understand ACP or MCP terminology.
- Changes in the UI affect the runtime capability context used by `bear_channel`.

### Phase 6: Runtime capability context for `bear_channel`

Pass the effective capability context into `bear_channel` for every request.

Deliverables:

- Den runtime capability resolver
- `bear_channel` capability payload mapping
- tests for request construction
- Codepool handling of declared server/runtime capabilities
- server tool event surfacing
- denial path when Codepool or the bear requests undeclared capabilities

Exit criteria:

- Codepool receives a trusted capability context assembled by Den.
- Codepool can only request or orchestrate capabilities declared in that context.
- Browser chat remains stable while richer capability events are introduced behind compatible event handling.

### Phase 7: Local app tools via ACP

Add ACP local tool capabilities after basic ACP chat and server-side capability context are stable.

Important constraint:

- The exact local tool set usually cannot be known until an ACP client connects and advertises its capabilities. Den therefore needs a two-layer model:
  - **catalog/policy templates** for known client families or tool classes, such as editor read, editor write, test execution, shell execution, and user confirmation
  - **session-scoped discovered descriptors** for the actual tools exposed by a specific ACP client connection

Deliverables:

- ACP client capability ingestion
- mapping from advertised ACP tools to normalized `client_tool` descriptors
- policy templates for local tool classes
- per-bear/per-user allow rules for local tool classes or specific client tools
- authorized `client_tools` declaration in `bear_channel`
- `client_tool_request` handling
- pending call persistence
- timeout and disconnect handling
- cancellation semantics
- client tool result forwarding
- audit records for request and result

Exit criteria:

- A bear in an ACP editor session can use an authorized local client tool.
- The same bear in web chat does not see unavailable local tools.
- A disconnected ACP client cannot be used for background local tool execution.
- Den can show local tools as session-scoped capabilities, for example “Provided by Zed in this session.”

### Phase 8: Connected apps via managed MCP passthrough

Enable connected apps as Den-managed MCP-backed capability providers.

Deliverables:

- connected app catalog model
- MCP server registration/import flow
- per-bear connected app enable/disable and configuration
- secret reference model; no hardcoded credentials
- toolset, explicit tool allowlist, and read-only/write-mode settings where supported by the MCP provider
- health/test flow for a connected app
- runtime materialization into Letta/Letta Code MCP configuration
- discovery records for raw MCP tools exposed by the runtime
- capability manifest entries with raw MCP tool name, source server, provider, scope, and policy metadata
- drift detection when the runtime-discovered tool set differs from Den's recorded expectation
- coarse audit/diagnostics for connected app tool usage where available

Implementation notes:

- The primary UI should say “Connected app” or “Connector”; MCP details belong in advanced/developer views.
- Den catalogs and authorizes connected app attachment. Coolify or the deployment platform may still run the actual MCP server process.
- Raw MCP tools may be exposed to ordinary bears when the connector is enabled for that bear.
- Den-brokered wrapper tools are exceptional and should be introduced only for specific policy, audit, stability, or usability needs.
- Connected apps should support both globally configured connectors and user-authorized connectors where appropriate.

Exit criteria:

- At least one MCP-backed connected app can be enabled for a bear.
- The connected app’s raw MCP tools appear in the effective capability manifest with provenance.
- Den can show connector configuration, discovered tools, and drift status.
- Den enforces per-bear connector attachment and coarse policy.

### Phase 9: Skills

Add skills after underlying tool/capability sources are normalized.

Deliverables:

- skill descriptor model
- skill catalog
- required/optional capability requirements
- dependency resolution
- availability states: ready, partially available, needs setup, unavailable in this client, disabled
- user-facing setup guidance
- provider-agnostic requirement matching
- per-bear skill enable/disable
- runtime mapping into `bear_channel.capabilities.skills`
- skill invocation audit records

Exit criteria:

- A skill can declare dependencies on generic capabilities instead of hardcoding a provider.
- Den can show whether a skill is ready for a bear/session and explain missing requirements.
- A skill can use a mix of built-in tools, connected apps, Letta tools, and ACP local tools when available and authorized.

### Phase 10: Observability, audit, and policy hardening

Add production-grade controls across all capability kinds.

Deliverables:

- capability usage audit log
- tool call audit log
- skill invocation audit log
- memory write audit log
- approval records
- metrics by capability kind/provider/bear/user
- failure and timeout tracking
- policy enforcement tests
- admin views for recent usage, denied requests, failures, and high-risk capability grants

Exit criteria:

- Operators can answer: which bear used which capability, on behalf of which user, against which target, and with what result.
- Policy denials and runtime failures are visible and debuggable.
- Sensitive capability classes have explicit approval/audit behavior.

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
