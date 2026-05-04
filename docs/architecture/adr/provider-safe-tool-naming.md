# Provider-Safe Tool Naming — Architecture Decision Record

## Status: Accepted

## Date: 2026-05-04

---

## Context

BEARS exposes tools through several boundaries:

- Letta/OpenAI-compatible model providers;
- Den policy and audit code;
- ACP adapter/client methods;
- MCP tools;
- server-side Den/Cabinet/Memory tools;
- future browser, terminal, Git, and artifact tools.

These boundaries do not share one naming grammar. In particular, OpenAI-compatible tool/function names reject dots and slashes and require names matching:

```text
^[a-zA-Z0-9_-]+$
```

A real ACP failure occurred when Den sent a Letta client tool named `fs.read_text_file`. Letta forwarded it to an OpenAI-compatible endpoint, which rejected it because `.` is not allowed in `tools[0].name`.

At the same time, BEARS still needs scoped, diagnostic-friendly tool identities. A provider-safe name such as `fs_read_text_file` is acceptable for model invocation, but it does not fully encode scope such as:

- ACP/local workspace vs server-side filesystem;
- Den/Cabinet/Memory ownership;
- execution target;
- user permission risk;
- audit policy.

Therefore tool naming must distinguish provider-facing identifiers from canonical BEARS identities and adapter/client methods.

---

## Decision

BEARS will use **separate tool names for separate boundaries**.

Every tool descriptor that may cross model/provider, Den policy, adapter, or client boundaries must define at least:

| Field | Example | Purpose |
| --- | --- | --- |
| `provider_name` | `fs_read_text_file` | Name sent to Letta/OpenAI-compatible providers as the callable tool/function name. |
| `canonical_name` | `acp.fs.read_text_file` | Stable BEARS semantic identity used in policy, audit, logs, metrics, and internal routing. |
| `execution_target` | `acp_client` | Where the tool actually runs. |
| `adapter_method` / `client_method` | `fs/read_text_file` | Protocol/client method invoked by the local adapter or downstream runtime. |
| `title` | `Read file` | Human-facing display label. |
| `risk` | `read_only` | Policy/audit risk class. |

### `provider_name`

`provider_name` is the only name sent as a model/provider callable tool name.

Rules:

```text
Required pattern: ^[a-zA-Z0-9_-]+$
Preferred style: lower_snake_case
Forbidden: dots, slashes, whitespace, URI-like names
```

Examples:

```text
fs_read_text_file
fs_write_text_file
fs_list_directory
fs_search_files
terminal_run_command
browser_capture_screenshot
git_status
mcp_call_tool
cabinet_search
memory_update
```

`provider_name` is a compact machine-safe symbol. It is not required to encode every scope/provenance detail.

### `canonical_name`

`canonical_name` is the stable BEARS semantic identity.

Rules:

```text
Preferred style: scoped.dot_name
Must include ownership/domain scope
Never sent to OpenAI-compatible providers as a callable tool name
```

Examples:

```text
acp.fs.read_text_file
acp.fs.write_text_file
acp.terminal.run_command
acp.browser.capture_screenshot
acp.mcp.call_tool
den.cabinet.search
den.memory.update
bear.skills.attach
```

The canonical name preserves scope even when the provider-facing name cannot.

### `execution_target`

`execution_target` makes provenance explicit and machine-readable. Common values:

```text
acp_client
adapter_local
den_server
letta_server
mcp_local
mcp_remote
codepool
browser_local
```

For example, an ACP local file reader should be described as:

```json
{
  "provider_name": "fs_read_text_file",
  "canonical_name": "acp.fs.read_text_file",
  "execution_target": "acp_client"
}
```

Even though the model sees `fs_read_text_file`, Den and operators can see that the tool is an ACP local workspace capability.

### Adapter/client method names

Adapter and client method names follow their protocol grammar and must not be reused as provider names unless they also satisfy provider-name constraints.

Examples:

```text
fs/read_text_file          # ACP client method
fs/write_text_file         # ACP client method
terminal/create            # ACP client method
bears/read_text_file       # BEARS adapter-private method / fallback
```

These names may contain `/` because they are JSON-RPC method names, not OpenAI-compatible tool names.

---

## Scope preservation rule

Provider-safe names may be short, but scope must be preserved elsewhere.

For model-facing descriptors, descriptions must identify scope and execution target when ambiguity is possible. For ACP local tools, descriptions should say they are local/client/workspace tools.

Example Letta client tool descriptor:

```json
{
  "name": "fs_read_text_file",
  "description": "ACP local workspace tool. Reads a UTF-8 text file from the user's editor workspace through the local adapter. Use only for user workspace files, not server files.",
  "parameters": {
    "type": "object",
    "properties": {
      "path": { "type": "string" }
    },
    "required": ["path"]
  }
}
```

The model-facing name is `fs_read_text_file`; the scope is carried by description and Den's internal descriptor:

```json
{
  "provider_name": "fs_read_text_file",
  "canonical_name": "acp.fs.read_text_file",
  "execution_target": "acp_client",
  "adapter_method": "fs/read_text_file",
  "risk": "read_only"
}
```

---

## Collision policy

`provider_name` must be unique within the tool list sent to a model provider for a single request.

If two tools would share a provider name, Den must not silently choose one. It must either:

1. choose a deterministic provider-safe alias with explicit scope, such as `acp_fs_read_text_file` vs `server_fs_read_text_file`; or
2. reject tool descriptor construction with a diagnostic error naming the colliding canonical names.

The default preference is:

- use concise domain names when no collision exists, e.g. `fs_read_text_file`;
- add scope prefix only when needed for uniqueness or safety, e.g. `acp_fs_read_text_file`.

---

## Validation requirements

Tool descriptor construction must validate:

1. `provider_name` matches `^[a-zA-Z0-9_-]+$`.
2. `provider_name` is unique in the provider request.
3. `canonical_name` is present and scoped.
4. `execution_target` is present.
5. provider → canonical mapping is explicit.
6. adapter/client method is present for client-executed tools.
7. risk/permission metadata is present for tools with external effects.

Tests must include a regression that `fs.read_text_file` never appears in Letta/OpenAI `client_tools[].name` or `tools[].name`.

---

## Logging and metrics rules

Logs and metrics should prefer `canonical_name`, and include `provider_name` where useful for model-provider debugging.

Recommended log fields:

```text
tool.provider_name
tool.canonical_name
tool.execution_target
tool.risk
tool_call_id
request_id
acp_session_id
```

Metrics should use bounded-cardinality labels. Prefer canonical names from a registry, not raw client/MCP-provided arbitrary names.

---

## Security and UX consequences

### Positive

- Prevents provider-side 400s from invalid tool names.
- Preserves scoped BEARS identities for policy and audit.
- Avoids leaking transport implementation details into every model-facing tool name.
- Allows concise, model-friendly provider names.
- Makes future ACP/MCP/server/browser tool collisions diagnosable.

### Trade-offs

- Every tool needs a small descriptor mapping instead of one string.
- Debugging requires looking at both provider and canonical names.
- Registry/tests must enforce consistency to avoid drift.

---

## Application to current ACP file tool

Current direct ACP read-file tool should be represented as:

```json
{
  "provider_name": "fs_read_text_file",
  "canonical_name": "acp.fs.read_text_file",
  "execution_target": "acp_client",
  "adapter_method": "bears/read_text_file",
  "client_method": "fs/read_text_file",
  "title": "Read file",
  "kind": "read",
  "risk": "read_only"
}
```

Only `provider_name` is sent to Letta as `client_tools[].name`.

Den maps Letta `tool_call_message.tool_call.name == "fs_read_text_file"` back to `canonical_name == "acp.fs.read_text_file"` before policy, transport, and audit handling.

---

## Follow-up work

1. Add a central Den tool descriptor registry.
2. Add provider-name validation tests for all Letta/OpenAI-facing tools.
3. Update ACP direct local tool runtime code to use descriptor fields instead of scattered string literals.
4. Update adapter mapping to use canonical/adapter method fields.
5. Add collision handling and tests.
6. Apply the same naming rules to Den server-side tools, Cabinet tools, Memory tools, MCP tool exposure, browser tools, terminal tools, and future artifact tools.
