# Den-specific bear tools implementation plan

This plan defines the first implementation slice for **Den-hosted bear tools**: tools whose value comes from Den's control-plane state, identity, membership, policy, and capability registry.

It follows:

- [Den architecture: Den meta tools](../architecture/DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools)
- [Bear memory tool boundary ADR](../architecture/adr/bear-memory-tool-boundary.md)
- [Bear capability management plan](BEAR_CAPABILITY_MANAGEMENT_PLAN.md)

## Boundary

Do **not** wrap Letta Code native MemFS tools by default. Letta Code remains the fast path for per-bear memory edits such as `memory` and `memory_apply_patch`.

Den tools are appropriate when Den adds one or more of:

- trusted Den identity and current-user context;
- users ↔ bears membership and roles;
- product policy and redaction;
- Den capability descriptors and effective capability sets;
- shared Cabinet knowledge access;
- governed workflows, audit, and approval.

## Initial read-only tool set

Implement these first:

| Tool | Priority | Purpose |
|---|---:|---|
| `den.bear.get_self` | P0 | Return the Den view of the current bear. |
| `den.user.get_current` | P0 | Return the Den-authenticated current user for this interaction. |
| `den.bear.list_members` | P0 | List users who have access to the current bear, redacted by policy. |
| `den.capabilities.list_self` | P0 | List Den-managed capabilities available to this bear/session. |
| `den.channel.get_context` | P1 | Return current channel/session/client context. |
| `den.policy.get_self` | P1 | Explain relevant policy for the current user + bear + channel. |

Defer:

| Candidate | Reason |
|---|---|
| `den.bear.list_related` | Cross-bear relationship discovery can leak sensitive membership/topology information. Revisit after policy and audit mature. |
| Arbitrary `den.user.get(id)` | Easy to leak profile information. Prefer current-user and current-bear-member queries first. |
| Mutating membership/profile/policy tools | Keep the first slice read-only. |
| Memory edit wrappers | Native Letta Code MemFS tools remain the default. |

## Naming

Use dotted Den namespace names:

- `den.bear.get_self`
- `den.user.get_current`
- `den.bear.list_members`
- `den.capabilities.list_self`
- `den.channel.get_context`
- `den.policy.get_self`

Avoid taking arbitrary `bear_id` or `user_id` parameters for these first tools. Tool execution must be bound to trusted invocation context provided by Den/Codepool, not model-supplied identifiers.

## Trusted invocation context

Every Den tool invocation is bound to context supplied by the runtime, not by the model:

- `bear_id`
- `bear_slug`
- `letta_agent_id`
- `user_id`
- `username`
- `membership_role`
- `conversation_id`
- `session_id`
- `request_id`
- `channel.family`
- `channel.client`
- `channel.protocol`

Tool inputs should be narrow. For example, `den.bear.list_members` may accept display options, but not an arbitrary bear identifier.

## Transport design

Use a generic Den tool invocation API rather than one endpoint per tool.

### Den internal endpoint

Add an internal endpoint to Den:

- `POST /internal/den-tools/invoke`

Request shape:

```json
{
  "tool_name": "den.bear.get_self",
  "arguments": {},
  "context": {
    "bear_id": "...",
    "bear_slug": "bruno",
    "letta_agent_id": "agent-...",
    "user_id": 123,
    "username": "alice",
    "membership_role": "admin",
    "conversation_id": "default",
    "session_id": "web:...",
    "request_id": "...",
    "channel": {
      "family": "browser_chat",
      "client": "den_web",
      "protocol": "den_chat"
    }
  }
}
```

Response shape:

```json
{
  "ok": true,
  "tool_name": "den.bear.get_self",
  "result": {}
}
```

Errors should return non-2xx HTTP status with:

```json
{
  "ok": false,
  "error": {
    "code": "forbidden",
    "message": "user is not a member of this bear"
  }
}
```

### Codepool execution

Codepool receives Den-provided `capabilities.server_tools` in `bear_channel`, registers those as Letta Code SDK external tools, and invokes Den's internal endpoint when the model calls one.

The SDK supports external tools through `CreateSessionOptions.tools`; Codepool should use that mechanism rather than implementing ad hoc local tools inside Letta.

## Capability descriptors

Start with a hardcoded built-in descriptor list. A DB-backed registry can come later.

Descriptor fields needed for the first slice:

- `name`
- `description`
- `kind`
- `provider`
- `execution_target`
- `scope`
- `availability`
- `permissions`
- `approval_policy`
- `input_schema`

Initial descriptors:

| Name | Kind | Provider | Execution target | Scope | Approval |
|---|---|---|---|---|---|
| `den.bear.get_self` | `server_tool` | `den` | `den` | `bear` | `never` |
| `den.user.get_current` | `server_tool` | `den` | `den` | `session` | `never` |
| `den.bear.list_members` | `server_tool` | `den` | `den` | `bear` | `never` |
| `den.capabilities.list_self` | `server_tool` | `den` | `den` | `session` | `never` |
| `den.channel.get_context` | `server_tool` | `den` | `den` | `session` | `never` |
| `den.policy.get_self` | `server_tool` | `den` | `den` | `session` | `never` |

## Authorization and redaction

Initial policy:

| Tool | Member | Bear admin | Site/operator admin |
|---|---:|---:|---:|
| `den.bear.get_self` | allow | allow | allow when scoped to an accessible/current bear |
| `den.user.get_current` | allow | allow | allow |
| `den.bear.list_members` | allow redacted | allow roles | allow roles when scoped |
| `den.capabilities.list_self` | allow | allow | allow |
| `den.channel.get_context` | allow | allow | allow |
| `den.policy.get_self` | allow | allow | allow |

Redaction defaults:

- Never expose `passhash` or raw auth internals.
- Do not expose member emails in the first slice.
- Prefer `user_id`, `username`, `display_name`, and `role` only.
- Site/operator admin flags should appear only if necessary for the tool's purpose.
- External channel IDs should be omitted unless explicitly required and policy-reviewed.

## Implementation slices

### Slice 1 — `den.bear.get_self` vertical slice

Goal: prove the full Den → Codepool → Den tool path with one safe read-only tool.

Deliverables:

1. Den descriptor for `den.bear.get_self`.
2. Den internal invocation endpoint and dispatcher.
3. Membership authorization based on trusted `bear_id` and `user_id`.
4. Codepool registration of the Den external tool for the Letta Code SDK session.
5. Codepool invocation of `POST /internal/den-tools/invoke`.
6. Structured result returned to the bear.
7. Logging/audit-style structured event for invocation.
8. Test or smoke route proving non-members are denied.

### Slice 2 — `den.user.get_current`

Add current-user lookup from Den's users table, with redacted output.

### Slice 3 — `den.bear.list_members`

Use Den membership state. Apply role-aware redaction. Verify normal member vs bear admin behavior.

### Slice 4 — `den.capabilities.list_self`

Return the hardcoded effective Den server tools for the current context.

### Slice 5 — channel and policy tools

Add:

- `den.channel.get_context`
- `den.policy.get_self`

## Testing plan

### Den tests

- Member can invoke `den.bear.get_self`.
- Non-member cannot invoke `den.bear.get_self`.
- Current user output is redacted.
- Member list does not expose emails.
- Unknown tool returns a structured error.
- Spoofed or mismatched context is rejected.

### Codepool tests

- Only Den tools declared in `capabilities.server_tools` are registered.
- Unknown Den tools are not registered.
- Den invocation failures return tool errors to Letta Code.
- Trusted context is forwarded to Den.

### Smoke test

- Seed user + bear + membership.
- Send a prompt through the runtime path that causes `den.bear.get_self` to be called.
- Verify the result is generated through Den authorization.

## Non-goals for the first implementation

- Cabinet tools.
- Memory edit wrappers.
- Mutating membership tools.
- Arbitrary user lookup.
- Cross-bear discovery.
- Full capability management UI.
- DB-backed capability registry.
- Full audit UI.
