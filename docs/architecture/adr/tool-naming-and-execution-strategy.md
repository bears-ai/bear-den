# Tool Naming and Execution Strategy — Architecture Decision Record

## Status: Accepted

## Date: 2026-05-08

---

## Context

BEARS exposes tools through multiple execution environments:

- Den server tools;
- ACP adapter-local tools;
- Chrome/DevTools tools;
- filesystem, Git, and process tools;
- future service, browser interaction, MCP, and external API tools.

The existing provider-safe naming ADR established that model/provider names must be safe for OpenAI-compatible providers and distinct from canonical BEARS identities. Since then, the tool surface has expanded enough that BEARS also needs a system-wide strategy for:

- model-facing provider names;
- canonical internal names;
- execution location;
- aliases and backward compatibility;
- collision handling;
- user-facing permission and approval behavior.

A concrete example is `web_fetch`. Public web fetch should be a Den-side capability because Den owns bear-level reference-source policy, URL/host approvals, fetch audit, and future caching. The adapter may still support local network fetch in the future, but that would be a distinct capability.

---

## Decision

BEARS will distinguish four tool naming layers:

| Layer | Purpose | Example |
| --- | --- | --- |
| Provider name | Model-facing callable name | `web_fetch` |
| Canonical name | Stable internal identity | `den.web.fetch` |
| Execution method | Where/how it runs | `den/web_fetch`, `bears/read_text_file` |
| Client/UI label | Human display | `Fetch URL` |

The model should see one concise provider name per capability, regardless of execution location. Implementation ownership such as `den_` or `adapter_` should not appear in provider names unless it is semantically necessary to avoid ambiguity.

---

## Descriptor ownership rule

Tool names and aliases must be descriptor-owned. Do not scatter provider aliases, legacy aliases, permission classes, UI labels, and execution methods across unrelated match statements.

Each tool descriptor should own:

- canonical name;
- current provider name;
- accepted provider aliases;
- execution class/method;
- adapter/client methods and aliases where applicable;
- permission class;
- display label/kind/risk;
- input schema/policy.

Routing, prompt descriptor generation, supported-tool diagnostics, UI labels, and approval policy should consult descriptors or descriptor resolvers. Future renames should normally be descriptor edits plus docs/tests, not broad string hunts.

## Provider names

Provider names are short, model-friendly, provider-safe identifiers.

Rules:

```text
Required pattern: ^[a-zA-Z0-9_-]+$
Preferred style: lower_snake_case
Forbidden: dots, slashes, whitespace, URI-like names
```

Examples:

```text
fs_read_text_file
fs_list_directory
fs_search_files
git_status
git_diff
git_log
git_show
git_add
process_run
web_fetch
chrome_open
service_restart
```

Provider names should not encode implementation ownership by default. For example, use `web_fetch`, not `den_web_fetch`, when the model just needs to fetch public reference material.

---

## Canonical names

Canonical names are stable, scoped BEARS identities used for routing, policy, audit, logs, and metrics.

Rules:

```text
Preferred style: scoped.dot_name
Must include ownership/domain scope
Never sent to OpenAI-compatible providers as callable tool names
```

Examples:

```text
acp.fs.read_text_file
acp.git.status
acp.process.run
acp.chrome.open
den.web.fetch
den.web.search
den.memory.write_entry
den.session.info
den.service.restart
```

---

## Execution classes

Every tool descriptor should include an execution class.

Recommended descriptor fields:

```json
{
  "provider_name": "web_fetch",
  "canonical_name": "den.web.fetch",
  "provider_aliases": ["den_web_fetch"],
  "execution": "den",
  "execution_method": "den/web_fetch",
  "kind": "fetch",
  "risk": "network_access",
  "permission_class": "network"
}
```

Execution classes:

| Execution | Meaning | Examples |
| --- | --- | --- |
| `adapter` | Local ACP adapter executes the tool | `fs_read_text_file`, `git_status`, `process_run`, `chrome_open` |
| `den` | Den executes the tool | `web_fetch`, `web_search`, `memory_read`, `session_info` |
| `external` | Future external service/runtime executes the tool | hosted search, MCP gateway, etc. |

Protocol rule:

> Den decides execution location. The adapter only executes tools explicitly marked `execution = "adapter"` and advertised by the adapter as direct local tools. Den-executed tools never generate ACP local `tool_request` events.

---

## Canonical provider names by domain

### Filesystem

Provider names:

```text
fs_read_text_file
fs_list_directory
fs_find_paths
fs_search_files
fs_stat
fs_edit_file
fs_create_text_file
fs_create_directory
fs_move_path
fs_copy_path
fs_apply_patch
fs_delete_path
```

Canonical namespace:

```text
acp.fs.*
```

Execution:

```text
adapter
```

### Git

Provider names:

```text
git_status
git_diff
git_log
git_show
git_add
git_restore
git_commit
git_stash
```

Canonical namespace:

```text
acp.git.*
```

Execution:

```text
adapter
```

### Process

Provider name:

```text
process_run
```

Canonical name:

```text
acp.process.run
```

Execution:

```text
adapter
```

### Web

Provider names:

```text
web_fetch
web_search
```

Canonical names:

```text
den.web.fetch
den.web.search
```

Execution:

```text
den
```

If local user-network fetching is needed later, it must be a separate provider name:

```text
local_web_fetch
```

Canonical name:

```text
acp.web.local_fetch
```

Execution:

```text
adapter
```

### Chrome / CDP

Provider names:

```text
chrome_open
chrome_snapshot
chrome_console_messages
chrome_network_requests
chrome_screenshot
```

Canonical namespace:

```text
acp.chrome.*
```

Execution:

```text
adapter
```

Chrome tools assume a Chrome-family DevTools Protocol endpoint configured by `BEARS_CHROME_CDP_URL` or `BEARS_BROWSER_CDP_URL`.

### Memory and situation tools

Advertised provider names should eventually drop implementation prefixes:

```text
session_info
memory_write_entry
memory_status
memory_browse
memory_read
memory_search
```

Canonical names remain Den-owned:

```text
den.session.info
den.memory.write_entry
den.memory.status
den.memory.browse
den.memory.read
den.memory.search
```

Existing `den_*` provider names should remain accepted as aliases during migration.

---

## Alias policy

BEARS may accept provider aliases for backward compatibility, but should advertise only the canonical provider name.

Example:

```json
{
  "provider_name": "web_fetch",
  "canonical_name": "den.web.fetch",
  "provider_aliases": ["den_web_fetch"],
  "execution": "den"
}
```

Rules:

1. Provider aliases are accepted during mapping.
2. Only the preferred provider name is advertised to new model turns.
3. Aliases must map to exactly one canonical name.
4. Alias use should be logged for later deprecation analysis.

---

## Collision policy

Provider names must be unique in a single model tool roster.

If two tools would share a provider name, Den must not silently choose one. It must either:

1. choose a deterministic scoped provider name, such as `local_web_fetch`; or
2. reject descriptor construction with a diagnostic naming the colliding canonical names.

Default preference:

- use concise names when no collision exists;
- add explicit semantic scope only when needed.

Examples:

| Provider name | Meaning |
| --- | --- |
| `web_fetch` | Den-side public web fetch |
| `local_web_fetch` | Optional future adapter-side local/user-network fetch |

---

## Den-side `web_fetch`

`web_fetch` is Den-executed.

Den owns:

- bear-level reference-source policy;
- URL/host approvals;
- fetch audit/logging;
- future caching;
- result settlement to Letta/model.

Adapter does not execute canonical `web_fetch`.

### Approval scopes

Den-side `web_fetch` approvals support:

- exact normalized URL;
- normalized host, including explicit non-default port.

Approval options for unapproved sources should include:

```text
allow_once
allow_url
allow_host
reject_once
```

No global approval is included initially.

### ACP adapter role

For ACP sessions, the adapter may bridge Den permission UI, but it does not execute the fetch.

Flow:

```text
Model calls web_fetch
Den checks approval policy
If approved: Den fetches
If not approved: Den emits permission_request
Adapter asks ACP client for permission
Adapter returns decision to Den
Den fetches or denies
Den settles tool result with original tool_call_id
```

Den-executed `web_fetch` must not produce an ACP local `tool_request` event.

---

## Adapter direct capabilities

Adapter direct tool advertisement must include only tools it can execute locally.

Therefore:

- advertise `fs_*`, `git_*`, `process_run`, `terminal_run_command`, `chrome_*`;
- do not advertise canonical `web_fetch`;
- if local fetch is added later, advertise `local_web_fetch` instead.

Den must treat adapter direct capabilities as the source of truth for adapter-executed tools. If an adapter does not advertise a local tool, Den must not expose that local tool to the model for that turn.

---

## Tool addition and adapter compatibility policy

Adding a tool must not automatically require users to update their ACP adapter. Compatibility depends on the execution class.

### Den-executed tools

Adding a Den-executed tool is normally backward-compatible with existing adapters.

Examples:

```text
web_fetch
web_search
session_info
memory_read
memory_search
update_plan
```

Rules:

1. Den executes the tool entirely server-side.
2. The adapter does not need implementation code for the tool.
3. Den may advertise the new tool to the model immediately, subject to role/policy.
4. Do not bump the Den ↔ adapter contract just because a Den-executed tool was added.
5. Do not introduce new required adapter-facing SSE event shapes as part of “just adding a Den tool.” If a new event shape is required, that is a protocol change, not merely a new tool.
6. Tool-specific validation failures should settle the tool result with `status="error"`; they should not bubble out as generic stream-processing failures.

### Adapter-executed tools

Adding an adapter-executed tool must be capability-gated so old adapters continue to work without seeing the new tool.

Examples:

```text
fs_*
git_*
process_run
terminal_run_command
chrome_*
```

Rules:

1. The adapter must advertise support in `client_context.adapter.direct_tools.<provider_name>.supported = true`.
2. For legacy compatibility, the adapter may also advertise `client_context.direct_tools.<provider_name> = true`.
3. Den must advertise an adapter-executed tool only when the current adapter context declares support for that provider name.
4. If the adapter does not advertise the tool, Den must omit it from model-facing `client_tools` for that turn.
5. Do not bump the Den ↔ adapter contract for an additive local tool when the existing `tool_request`, permission, and tool-result shapes are sufficient.
6. Use per-tool capability versions for behavior changes to a local tool; reserve the global adapter contract for incompatible transport/schema changes.

### Den ↔ adapter contract versioning

The adapter contract version describes the private Den ↔ adapter transport/schema contract. It is not a tool catalog version.

Do not bump the adapter contract for:

- adding a Den-executed tool;
- adding an adapter-executed tool that is capability-gated;
- adding optional fields;
- adding diagnostics/log fields;
- changing tool descriptions, labels, or prompt guidance;
- accepting a new provider alias while keeping old aliases accepted.

Bump the adapter contract only for incompatible changes, such as:

- changing required fields in Den ↔ adapter requests or responses;
- changing existing SSE event shapes in a way old adapters cannot parse;
- changing tool-result settlement semantics incompatibly;
- removing accepted fields or aliases that old adapters rely on;
- requiring previously optional compatibility metadata.

Missing `adapter_contract` metadata should remain accepted unless Den intentionally ships a breaking adapter protocol change. This prevents one-sided Den-only deployments from breaking already-running compatible adapter processes.

### New tool checklist

When adding a Den-executed tool:

1. Add a canonical `den.*` name and provider-safe alias in Den descriptors.
2. Add accepted legacy aliases only at routing boundaries if needed.
3. Add an invocation handler in Den.
4. Add prompt guidance only if the model needs usage hints.
5. Ensure the tool does not require adapter direct support.
6. Do not bump the adapter contract.

When adding an adapter-executed tool:

1. Add an ACP descriptor and policy in Den.
2. Add adapter implementation.
3. Add adapter direct capability advertisement.
4. Ensure Den filters the descriptor through `adapter_supports_tool` / direct-tool capability checks.
5. Add approval metadata and result UX.
6. Do not bump the adapter contract unless the Den ↔ adapter message shape changed incompatibly.

---

## Permission class naming

Permission classes describe risk domains, not provider implementations.

Recommended classes:

```text
read_files
edit_files
delete_files
git_read
git_write
command_run
network
browser
service_control
```

Examples:

| Provider | Permission class |
| --- | --- |
| `fs_read_text_file` | `read_files` |
| `fs_edit_file` | `edit_files` |
| `fs_delete_path` | `delete_files` |
| `git_status` | `git_read` |
| `git_commit` | `git_write` |
| `process_run` | `command_run` |
| `web_fetch` | `network` |
| `chrome_open` | `browser` |

---

## Consequences

### Positive

- Cleaner model-facing tool names.
- Den can choose execution location centrally.
- `web_fetch` can support bear-level source policy and approvals.
- Adapter no longer needs to coordinate public reference fetch policy.
- Backward compatibility is preserved with aliases.
- Future local/network variants have clear names.

### Trade-offs

- Tool descriptors need explicit execution metadata.
- Den must maintain provider alias mapping.
- Some existing prompts and tests need migration from `den_*` names.
- Den-side approval flow for server tools requires separate permission-request plumbing.

---

## Migration plan

1. Add explicit descriptor fields:
   - `provider_name`
   - `canonical_name`
   - `provider_aliases`
   - `execution`
   - `execution_method`
   - `permission_class`
2. Advertise `web_fetch` for Den-side fetch.
3. Keep accepting `den_web_fetch` as alias.
4. Stop adapter advertising canonical `web_fetch`.
5. Add Den-side URL/host approval flow for unapproved fetches.
6. Update prompt guidance to prefer `web_fetch`, not `den_web_fetch`.
7. Migrate memory/situation provider names away from `den_*` prefixes: `session_info`, `memory_write_entry`, `memory_status`, `memory_browse`, `memory_read`, and `memory_search`.

---

## Relationship to previous ADR

This ADR extends `provider-safe-tool-naming.md`.

That ADR established safe provider syntax and separated provider names from canonical names. This ADR adds a system-wide naming and execution-location strategy, including aliasing, collision rules, and the decision that `web_fetch` is Den-executed while local workspace tools are adapter-executed.
