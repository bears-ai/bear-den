# Capabilities, skills, and Den brokering plan

For the canonical role model and current role names, see [bear roles](../../architecture/bear-roles.md).
This plan formalizes the long-term model discussed for BEARS capabilities, skills, connected apps, managed MCP passthrough, and selective Den-brokered tool execution.

It complements:

- [BEAR_CAPABILITY_MANAGEMENT_PLAN.md](BEAR_CAPABILITY_MANAGEMENT_PLAN.md)
- [DEN_SPECIFIC_TOOLS_PLAN.md](DEN_SPECIFIC_TOOLS_PLAN.md)
- [DEN_ARCHITECTURE.md](../../architecture/DEN_ARCHITECTURE.md)
- [bear-memory-tool-boundary.md](../../architecture/adr/bear-memory-tool-boundary.md)

## Decisions

### 1. Runtime tool names must be model-safe

Do not use dotted runtime tool names. Many model APIs reject dots in tool names or silently sanitize them.

Runtime-visible tool names should use lowercase snake case, for example:

| Capability | Runtime tool name |
|---|---|
| Current bear profile | `get_current_bear` |
| Current user profile | `get_current_user` |
| Bear membership list | `list_bear_members` |
| Bear capability list | `list_bear_capabilities` |
| Channel/session context | `get_channel_context` |
| Current policy summary | `get_current_policy` |
| Cabinet search | `cabinet_search` |
| Optional BEARS-owned GitHub PR read | `github_get_pull_request` |
| Optional BEARS-owned GitHub PR files | `github_list_pull_request_files` |

Provider/provenance is metadata, not necessarily a prefix in the runtime tool name.

### 2. Den is the policy and brokering layer

Den owns product policy and capability state. This includes:

- user and bear identity;
- users ↔ bears membership;
- per-bear capability assignment;
- connected app configuration;
- credential selection;
- approval policy;
- audit policy;
- skill availability;
- runtime capability context sent to Codepool.

Codepool/Letta Code remains the harness that runs the agent loop and registers runtime tools. Letta remains persistence/runtime state. Den remains the source of truth for BEARS product capability policy.

### 3. Use Letta Code skills for workflows, not enforcement

Skills are reusable workflows and guidance. They are not security boundaries.

BEARS should use skills for:

- PR review workflows;
- issue triage;
- release notes;
- incident summaries;
- coding workflows;
- provider-specific playbooks.

Privileged actions should go through Den-brokered capabilities where Den can enforce policy.

### 4. Connected apps use managed MCP passthrough by default

For connected apps such as GitHub, the default long-term pattern is **managed passthrough**:

1. Den owns the connector catalog, bear attachment, configuration, secret references, and coarse policy.
2. The runtime exposes the provider's raw MCP tools to the bear.
3. Den records discovered tools and includes them in the effective capability manifest.
4. Den detects tool drift and surfaces diagnostics.
5. Approval and tool allow/deny policy are applied where supported by Letta, Letta Code, or the MCP backend.

This matches how most agent harnesses use MCP and avoids unnecessary wrapper work.

### 5. Den-brokered connected app tools are exceptional

Den-brokered provider tools are used only when managed passthrough is insufficient, for example:

- per-user credential selection is required at call time;
- a write action needs stronger approval/audit semantics;
- raw MCP schemas or names are too unstable or confusing;
- BEARS needs a stable cross-backend tool name;
- policy must restrict resources more precisely than the MCP backend supports.

In those cases, Den exposes a BEARS-owned tool such as `github_get_pull_request` and may implement it using a direct API client, MCP behind Den, or another backend.

### 6. Letta is not the long-term source of truth for connected app config

Letta may be a runtime attachment point for MCP tools, but Den is the product source of truth for connected app configuration and bear attachment.

Manual Letta-side tool/MCP changes should be treated as runtime drift unless synchronized from Den.

### 7. Imported skills require adaptation

The public skills ecosystem is not uniformly abstract or portable. Skills may reference:

- concrete tool names;
- MCP tools;
- CLI commands;
- host-specific paths;
- shell access;
- network access;
- sandbox-specific behavior.

External skills are not automatically BEARS-ready. They must be imported, analyzed, mapped, and approved before becoming bear skills.

## Skill classes

### Generic skills

Generic skills avoid concrete tool names. They describe workflows and abstract requirements.

They may say:

- read pull request metadata;
- list changed files;
- search the web;
- create an artifact;
- post a comment if write capability is available and approved.

They should not hardcode BEARS tool names.

Generic skills are the most portable across BEARS, Claude Code, and other hosts.

### Bear skills

Bear skills are BEARS-ready. They may reference concrete tools from the BEARS capability manifest.

Examples:

- raw MCP tools present in the bear's effective manifest, such as `get_me`, `search_users`, or `pull_request_read`
- BEARS-owned tools present in the manifest, such as `cabinet_search` or `get_current_bear`
- exceptional Den-brokered provider tools if defined, such as `github_get_pull_request`
- `get_current_bear`
- `create_artifact`

Bear skills are allowed to be product-specific because their dependencies are explicit and governed by Den.

### Imported skills

Imported skills come from external catalogs or repositories. They become bear skills only after import processing.

Import processing should:

1. parse skill metadata and content;
2. detect concrete tools, CLIs, paths, MCP names, and network assumptions;
3. map dependencies to BEARS capabilities where possible;
4. identify unresolved requirements;
5. produce a compatibility report;
6. optionally rewrite or attach a BEARS binding;
7. require operator approval before enabling.

## Capability manifest

BEARS needs a capability manifest with model-safe runtime tool names.

Minimum fields:

- `name` — model-safe runtime tool name;
- `display_name`;
- `description`;
- `kind` — `server_tool`, `connected_app_tool`, `client_tool`, `skill`;
- `provider` — `bears`, `github`, `cabinet`, `acp_client`, etc.;
- `execution_target` — `den`, `codepool`, `acp_client`, `letta`, `mcp_backend`, etc.;
- `scope` — `global`, `workspace`, `user`, `bear`, `session`, `client_connection`;
- `permissions`;
- `approval_policy`;
- `input_schema`;
- `output_schema`;
- `audit_policy`;
- `availability`;
- optional `backend_mapping` for Den-internal adapters;
- optional `source_tool_name` and `source_server` for raw MCP passthrough tools.

Only tools in the effective manifest for a bear/session should be registered into the model runtime.

## Managed MCP passthrough flow

For a managed MCP connected app:

1. Den stores connector configuration, secret references, toolset/read-only settings, and bear attachment.
2. Den materializes the MCP server attachment into Letta/Letta Code runtime config.
3. The runtime discovers the provider's raw MCP tools.
4. Den records the discovered tool list and adds those tools to the bear's effective capability manifest with provenance metadata.
5. The bear sees and calls raw MCP tool names.
6. Runtime/Letta/MCP executes the tool.
7. Den surfaces coarse configuration, discovered tools, drift, and audit/diagnostics where available.

## Den-brokered execution flow

For an exceptional Den-brokered tool:

1. Den computes effective capabilities for the current bear, user, channel, and session.
2. Den sends allowed Den-brokered tools to Codepool in `bear_channel` context.
3. Codepool registers those tools with the Letta Code SDK as external tools.
4. The bear calls a model-safe runtime tool name.
5. Codepool calls Den with trusted context and tool arguments.
6. Den authorizes the call.
7. Den resolves credentials/backend.
8. Den performs the action or calls the backend provider/MCP server.
9. Den records audit/metrics.
10. Den returns a normalized result to Codepool.
11. Codepool returns the tool result to Letta Code.

The model must not supply security-sensitive identifiers such as arbitrary `bear_id` or `user_id` for scoped Den-brokered tools. Those come from trusted runtime context.

## GitHub long-term model

GitHub should be a connected app capability in Den, implemented first as managed MCP passthrough.

Den owns:

- GitHub connector configuration;
- credential secret references;
- selected toolsets or explicit tool allowlists;
- read-only/write mode where supported;
- per-bear attachment;
- runtime tool discovery records;
- drift diagnostics;
- coarse audit/visibility.

The bear may see raw GitHub MCP tool names such as:

- `get_me`
- `get_teams`
- `search_users`
- `pull_request_read`
- `issue_read`
- `search_repositories`

BEARS-owned GitHub tools such as `github_get_pull_request` should be introduced only when raw MCP passthrough is insufficient for policy, audit, stability, or usability.

## Letta Code native tools

Letta Code native tools are not Den-brokered by default.

Examples:

- shell/file/edit tools;
- planning tools;
- subagent tools;
- skill invocation;
- native MemFS tools.

Den should still manage policy and observability around them:

1. Capture the runtime tool list from Letta Code session init.
2. Compare observed tools to a checked-in baseline for the pinned Letta Code version.
3. Surface unknown/missing tools in diagnostics.
4. Add per-bear policy for allowed/disallowed Letta Code tools where supported by the SDK.
5. Prefer runtime permission gating if full model-visible tool removal is unavailable.

This prevents upstream Letta Code tool changes from surprising BEARS operators.

## Imported skill compatibility report

Each imported external skill should produce a compatibility report.

Report categories:

- required shell commands;
- required CLIs;
- referenced model tool names;
- referenced MCP tools;
- expected files/paths;
- network domains;
- secrets/credentials;
- write/destructive actions;
- BEARS mappings;
- unresolved requirements;
- recommended approval policy.

Status values:

- `ready`;
- `partially_available`;
- `needs_setup`;
- `needs_adaptation`;
- `unsupported`;
- `disabled_by_policy`.

## Conflicts with existing plans

### Runtime naming in older plans

Older planning docs previously used dotted examples. Runtime tool names are now documented as model-safe snake case. If new dotted examples appear, treat them as documentation bugs and update them.

### Den-specific tool implementation still needs code migration

The Den-specific tool plan now uses product-facing model-safe names such as `get_current_bear` and `list_bear_members`. The current implementation may still need code migration away from older names. Den remains the provider/execution target in metadata.

### Existing plans mention Den-managed MCP servers

This plan keeps Den-managed MCP and clarifies its default mode:

- MCP is usually a managed passthrough connected app backend.
- Den owns catalog, attachment, configuration, visibility, and coarse policy.
- Raw MCP tools may be exposed to ordinary bears when the connector is enabled for that bear.
- Den-brokered wrappers are added only when there is a specific product or policy need.

### Skills after tools vs skills as workflow layer

The existing capability plan says skills come after tools. This still holds for implementation sequencing. We need working Den-brokered tools before first-class bear skills can depend on them.

## Roadmap

### Phase 1 — Rename and stabilize BEARS-native introspection tools

- Rename Den tools to model-safe product names.
- Stop advertising dotted or `den_` names.
- Keep temporary aliases only for migration.
- Maintain Den provider/execution metadata.

Initial names:

- `get_current_bear`
- `get_current_user`
- `list_bear_members`
- `list_bear_capabilities`
- `get_channel_context`
- `get_current_policy`

### Phase 2 — Capability manifest v1

- Define checked-in descriptor schema.
- Hardcode/seed BEARS-native descriptors.
- Include Den-brokered external tool descriptors.
- Pass only effective tools to Codepool.

### Phase 3 — Letta Code tool observability and policy

- Capture observed Letta Code tools at session init.
- Expose observed tools via Codepool/Den diagnostics.
- Add baseline manifest for pinned Letta Code version.
- Add per-bear allow/deny policy where SDK supports it.

### Phase 4 — GitHub connected app via managed MCP passthrough

- Define GitHub connector catalog entry.
- Configure toolsets, explicit tool allowlists, and read-only mode where supported.
- Attach GitHub MCP to a bear through Den-managed configuration.
- Record discovered raw MCP tools in the capability manifest.
- Surface drift and diagnostics.
- Add Den-brokered GitHub wrappers later only for specific policy/product needs.

### Phase 5 — Bear skill manifest

- Define bear skill metadata.
- Allow bear skills to reference only manifest tools.
- Compute skill availability from capability requirements.
- Materialize enabled skills for Letta Code.

### Phase 6 — Generic skill support

- Define abstract requirement vocabulary.
- Bind generic requirements to BEARS capabilities.
- Support host-specific binding notes without making Den names mandatory.

### Phase 7 — Imported skill pipeline

- Parse external skills.
- Produce compatibility reports.
- Map tool/CLI/MCP references to BEARS capabilities.
- Generate or attach BEARS bindings.
- Require operator approval before enabling.

## Open questions

These need product/architecture decisions before implementation hardens:

1. Should BEARS store generic abstract capability identifiers separately from runtime tool names?
2. Should imported skills be rewritten, wrapped with a binding file, or both?
3. Which GitHub MCP configuration should ship first: default toolsets, explicit tool allowlist, or read-only-only?
4. How strict should imported skill sandboxing be by default?
5. Which Letta Code native tools should be enabled by default for non-coding bears?
