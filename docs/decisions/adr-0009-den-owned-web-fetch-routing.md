# Den-Owned Web Fetch Routing and Approval Strategy — Architecture Decision Record

## Status: Accepted

## Date: 2026-05-08

---

## Context

`web_fetch` appears simple at the model interface: fetch a URL and return bounded text. In BEARS, however, the execution location and approval policy have architectural consequences.

BEARS has multiple agent roles and runtimes:

- `pair` agents run through ACP and may have a local adapter attached to the user's editor/workspace.
- `work` agents run through Letta Code/Codepool without a user's ACP adapter.
- `talk`, `curate`, and future roles may need public reference material but should not implicitly access user-local services.

Public reference fetches are naturally bear-level capabilities. Den can own source policy, approvals, audit, and future caching. Local development URLs such as `localhost` are different: they are only meaningful from a specific user machine and require an ACP/local adapter.

The goal is to keep the model-facing tool surface simple while preserving correct execution and safety boundaries.

---

## Decision

BEARS will expose a single model-facing provider tool:

```text
web_fetch
```

Den owns routing, policy, approvals, and audit for this tool.

The model does not choose between Den-side and adapter-side fetch variants.

---

## Execution routing

Den classifies the URL before execution.

```text
web_fetch(public URL) -> Den executes
web_fetch(configured local URL in ACP pair session) -> Den may delegate to adapter
web_fetch(local URL without eligible adapter) -> Den denies or returns runtime error
```

The adapter-local path is an implementation detail. It must not be advertised as a separate model-facing tool in normal rosters.

If an adapter-local implementation exists, it should be invoked only by Den using an internal/private adapter method, for example:

```text
bears/local_web_fetch
```

The model-facing provider name remains:

```text
web_fetch
```

---

## Public URL behavior

Public HTTP(S) URLs are fetched by Den.

Den is responsible for:

- URL validation;
- SSRF guards;
- bear-level source policy;
- user approvals for unknown sources;
- audit/logging;
- result settlement back to the model with the original `tool_call_id`.

Public `web_fetch` must not generate an ACP local `tool_request` event.

---

## Local URL behavior

Some `pair` workflows need to fetch user-local development URLs such as local docs or dev servers.

Local URL fetch is allowed only when all are true:

1. The current runtime is an ACP `pair` session.
2. An adapter is attached and advertises the private local fetch capability.
3. The URL host is in the configured local-host allowlist.
4. Den policy and user approval permit the fetch.
5. The adapter revalidates the URL host before fetching.

Default local host policy should be conservative. Suggested defaults:

```text
localhost
127.0.0.1
::1
```

A deployment may configure local hosts with an environment variable such as:

```text
BEARS_LOCAL_WEB_HOSTS=localhost,127.0.0.1,::1
```

Den and the adapter should both validate local host policy as defense in depth.

---

## Role behavior

All roles see the same model-facing tool name where web fetch is available:

```text
web_fetch
```

Execution differs by runtime and policy.

| Role/runtime | Public approved host | Public unknown host | Local host |
| --- | --- | --- | --- |
| `pair` ACP | Fetch in Den | Request approval or deny by policy | Delegate to adapter if configured and approved |
| `work` Letta Code | Fetch in Den | Deny or require task-level approval | Deny |
| `talk` web/Slack | Fetch in Den | Surface approval through that channel or deny | Deny |
| `curate` | Restricted by policy | Usually deny | Deny |

For `work` agents, default policy should prefer approved/preferred sources only. Background agents should not gain broad egress implicitly.

---

## Approval model

Den owns `web_fetch` approvals.

Approval scopes:

```text
url
host
```

### URL scope

Approves one normalized URL.

Useful for one-off references.

### Host scope

Approves a normalized host, including explicit non-default port when present.

Useful for documentation domains.

Examples:

```text
docs.rs
example.com:8443
```

Approval options for an unapproved URL should be:

```text
allow_once
allow_url
allow_host
reject_once
```

No global approval is included initially.

---

## Approval transport for ACP sessions

For ACP `pair` sessions, Den may use the adapter as a permission UI bridge.

This is a permission-only event, not a local tool request.

Example Den event:

```json
{
  "type": "permission_request",
  "permission_id": "perm_...",
  "tool_call_id": "call_...",
  "tool_name": "web_fetch",
  "title": "Fetch URL",
  "reason": "BEARS wants to fetch a public reference URL.",
  "target": {
    "kind": "url",
    "url": "https://docs.rs/tokio/latest/tokio/",
    "host": "docs.rs"
  },
  "options": ["allow_once", "allow_url", "allow_host", "reject_once"]
}
```

Adapter response:

```json
{
  "permission_id": "perm_...",
  "decision": "allow_host",
  "scope_kind": "host",
  "scope_value": "docs.rs"
}
```

After the decision, Den executes or denies the fetch and settles the original model tool call.

The adapter must not post a `web_fetch` tool result unless Den delegated an internal local fetch method.

---

## URL normalization

Den normalizes before approval and audit.

Recommended URL normalization:

- lower-case scheme;
- lower-case host;
- remove fragment;
- preserve path and query;
- normalize default ports:
  - `https://example.com:443/a` -> `https://example.com/a`
  - `http://example.com:80/a` -> `http://example.com/a`

Recommended host normalization:

- lower-case host;
- include explicit non-default port;
- omit default port.

---

## Audit/logging

Den should log each attempted fetch, regardless of approval outcome.

No caching is implemented in this decision.

Suggested audit fields:

```text
bear_id
session_id
tool_call_id
url
final_url
host
execution_location    // den | adapter_local
approval_kind         // preapproved | user_url | user_host | allow_once | denied
http_status
content_type
bytes
fetched_at
```

Caching may be added later, but is explicitly out of scope for this ADR.

---

## Provider naming

This decision follows `tool-naming-and-execution-strategy.md`.

Provider name:

```text
web_fetch
```

Canonical name:

```text
den.web.fetch
```

Backward-compatible aliases may be accepted:

```text
den_web_fetch
```

Only `web_fetch` should be advertised to new model turns after migration.

---

## Adapter direct capabilities

The ACP adapter must advertise only tools it can execute as model-facing local tools.

Canonical `web_fetch` should not be advertised as an adapter direct tool.

If local fetch remains implemented in the adapter, it should be private/internal or, if ever exposed, use a distinct provider name such as:

```text
local_web_fetch
```

However, normal agent usage should continue to use only:

```text
web_fetch
```

---

## Consequences

### Positive

- Agents learn one simple web fetch tool.
- `pair` and `work` agents share the same model-facing experience.
- Den owns bear-level source policy and approvals.
- Local development fetch remains possible for ACP pair sessions.
- Public fetch behavior works across non-ACP roles.
- Future caching can be added centrally in Den.

### Trade-offs

- Den must route by URL locality.
- Den must maintain a permission flow for server-side tools.
- Adapter must support permission-only events separate from local tool requests.
- Local fetch requires coordination between Den and adapter, but remains hidden from the model.

---

## Implementation plan

1. Advertise `web_fetch` as a Den-executed server tool.
2. Accept `den_web_fetch` as a provider alias.
3. Stop advertising adapter direct `web_fetch`.
4. Add Den URL/host approval storage.
5. Add Den `permission_request` event for unapproved fetches.
6. Add adapter handling for permission-only events.
7. Add Den-side public fetch execution with SSRF guards.
8. Add adapter-private local fetch execution for configured local hosts.
9. Add audit logging for fetch attempts and results.
10. Defer caching.

---

## Non-goals

- No web fetch caching in the initial implementation.
- No broad local network access.
- No model-facing `den_web_fetch` or `local_web_fetch` variants in normal operation.
- No global web-fetch approval option initially.
