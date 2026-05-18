# ADR: Host Browser MCP Bridge for ACP Adapter

**Status:** Accepted  
**Date:** 2026-05-18  
**Deciders:** Hans

## Context

BEARS supports ACP as the pair-programming channel into Zed and other ACP clients. The `bears-acp-adapter` can run in more than one place:

- On the **developer host**, where browser applications such as Chrome are installed and where non-container ACP clients can connect.
- Inside a **dev container**, where workspace filesystem and command execution should remain container-scoped.

Browser automation sits awkwardly across this boundary. Chrome is usually installed on the developer host, while a dev-container ACP adapter may not be able to find or launch Chrome. Zed can forward MCP context servers to external ACP agents via `mcpServers`, but in dev-container projects this can result in remote command wrapping such as `docker exec ...`, and the MCP server then runs inside the container. When Chrome is absent from the container, browser MCP discovery or calls fail.

We observed this concretely with `chrome-devtools-mcp`:

- Zed initially sent `mcpServers: []` until a custom context server with `remote: true` was configured.
- After forwarding worked, Zed wrapped the server as `docker exec ... -it ... npx -y chrome-devtools-mcp@latest`; the adapter had to sanitize the TTY flag because stdio MCP cannot run through a TTY.
- Once launched, the MCP server reported that it could not find Chrome at `/opt/google/chrome/chrome`, proving it was running inside the dev container rather than on the host where Chrome exists.

A tempting answer is to expose the host-installed ACP adapter to the container adapter as a full ACP peer. That is too broad. The container adapter must not be able to instruct the host adapter to read host files, write host files, run host shell commands, access host credentials, or use arbitrary host-local MCP servers. The host boundary should be intentionally narrow.

At the same time, we want installation and operation to stay simple. We want a **single host-installed binary** that can serve as:

1. the normal BEARS ACP adapter for host-side ACP clients, and
2. a host-local browser bridge for container-side adapters.

## Decision

The host-installed `bears-acp-adapter` binary will grow a **browser-only MCP bridge mode**. The same binary can be invoked either as the normal ACP adapter or as a constrained host-local MCP server/bridge exposing only approved browser tools.

Conceptually:

```/dev/null/acp-host-browser-mcp-bridge.txt#L1-5
Host clients:
  Zed / ACP client -> bears-acp-adapter --acp -> Den

Dev-container browser fallback:
  container bears-acp-adapter -> host.docker.internal:<port>/mcp -> bears-acp-adapter browser-bridge -> host Chrome
```

The container adapter will treat the host bridge as an MCP server source, not as a full ACP server. The bridge transport should be MCP Streamable HTTP where practical, with bearer-token authentication. The bridge exposes browser tools only.

## Scope of the bridge

The host bridge may expose tools equivalent to the current BEARS browser tools:

- open/navigate a page
- capture accessibility/page snapshot
- read console messages
- read network request metadata
- capture screenshot

Potentially risky browser capabilities, especially arbitrary JavaScript evaluation, must be separately considered and should not be enabled by default.

The host bridge must not expose:

- host filesystem read/write/list/edit/delete tools
- host process or terminal execution
- host git tools
- arbitrary host MCP registry access
- arbitrary ACP sessions/prompts
- generic “call any host MCP tool” functionality

The bridge is browser-only by design.

## Single binary model

The host-installed binary remains `bears-acp-adapter`. It should support multiple modes, for example:

```/dev/null/bears-acp-adapter-modes.txt#L1-6
# Existing ACP adapter mode, selected by default or explicit flag.
bears-acp-adapter --client zed ...

# Host-local browser MCP bridge mode.
bears-acp-adapter browser-bridge --listen 127.0.0.1:9277
```

This keeps host installation simple while making runtime intent explicit. The browser bridge mode should not initialize Den ACP session handling or expose ACP JSON-RPC methods. It should start only the browser MCP server surface.

## Transport and authentication

The preferred bridge transport is **MCP Streamable HTTP**:

```/dev/null/host-browser-mcp-url.txt#L1
http://host.docker.internal:9277/mcp
```

The bridge should require bearer authentication:

```/dev/null/browser-bridge-auth.txt#L1
Authorization: Bearer <token>
```

Configuration examples:

```/dev/null/host-bridge-env.txt#L1-2
BEARS_BROWSER_BRIDGE_TOKEN=<random-token>
bears-acp-adapter browser-bridge --listen 127.0.0.1:9277
```

```/dev/null/container-adapter-env.txt#L1-2
BEARS_HOST_BROWSER_MCP_URL=http://host.docker.internal:9277/mcp
BEARS_HOST_BROWSER_MCP_TOKEN=<same-token>
```

Binding to `127.0.0.1` is preferred. Docker Desktop host reachability should be tested; if `host.docker.internal` cannot reach a loopback-bound service, operators may bind to a Docker-facing interface only when token auth is enabled and the exposure is understood. Binding broadly to `0.0.0.0` should not be the default.

## Tool preference order

The pair ACP tool surface should prefer client-local tools over BEARS fallbacks:

1. Zed/client-forwarded MCP tools discovered from `mcpServers`.
2. Configured BEARS host browser MCP bridge tools.
3. BEARS built-in local Chrome/CDP fallback tools.
4. No browser tools.

When client-forwarded MCP browser tools are discovered, built-in BEARS browser tools should be treated as fallbacks and not advertised to the model. When no client MCP browser tools are discovered, the host browser MCP bridge may provide the browser fallback.

## Container-to-host security boundary

The container adapter may connect to the host browser MCP bridge only as a constrained MCP client. It must not be given a generic host ACP connection or an unrestricted host MCP registry.

Defense in depth:

- The host bridge exposes only browser tools.
- The host bridge requires an authentication token.
- The container adapter should allowlist bridge tool names/categories before advertising them.
- The container adapter should continue to run filesystem/process/git tools in the container, not on the host.
- Network and console results should redact obvious sensitive headers/values where feasible.

## Relationship to ACP MCP forwarding and MCP-over-ACP

This decision complements Zed-forwarded `mcpServers`; it does not replace them. If Zed forwards a client MCP server and BEARS can discover tools from it, those tools remain the preferred client-local surface.

The ACP MCP-over-ACP RFD may eventually provide a cleaner way for clients or proxies to expose MCP servers over the existing ACP connection. Until that stabilizes, the host browser bridge uses ordinary MCP transport, preferably Streamable HTTP. The adapter should retain comments and structure that make a future `type: "acp"` transport source easy to add.

## Consequences

### Positive

- Host Chrome can be used from dev-container ACP sessions without installing Chrome in every container.
- Operators can start the bridge manually when needed; no launch-on-login requirement.
- Host installation remains a single binary.
- The host boundary is narrow and auditable.
- The bridge uses MCP’s existing discovery/call/schema model instead of a BEARS-only tool protocol.
- The design generalizes to other host-local browser clients without granting arbitrary host access.

### Negative / Tradeoffs

- We must implement MCP Streamable HTTP client support in the adapter if we choose HTTP for the bridge.
- We must implement or wrap a host browser MCP server mode.
- Token and bind-address handling add operational complexity.
- Browser automation can expose sensitive page contents, console logs, and network metadata even when filesystem/process tools are not exposed.
- There are now multiple possible browser surfaces, so diagnostics and descriptor guidance must clearly explain which surface is active.

## Alternatives considered

### Full host ACP adapter as bridge

The container adapter could connect to a host-installed full ACP adapter and request browser operations through ACP. Rejected because a full ACP connection is too broad: it risks exposing host filesystem, process execution, host credentials, host MCP servers, and arbitrary ACP prompt/session behavior to the container adapter.

### BEARS-only host browser HTTP API

A custom `/chrome/open`, `/chrome/snapshot`, etc. HTTP API would be simple and fast to implement. Rejected as the preferred long-term direction because MCP already provides tool discovery, schemas, calls, and errors. A custom API can remain a fallback if MCP HTTP support proves too costly.

### Install Chrome in every dev container

This keeps all tool execution inside the container. Rejected as the default because it makes every dev container heavier, still does not control the user’s host browser, and requires per-container Chrome configuration. It remains viable for deployments that intentionally want browser automation isolated inside the container.

### Launch host Chrome on login

A macOS `launchd` service could keep a debug Chrome profile available. Rejected for this use case because the operator does not want launch-on-login behavior. The bridge should be manually startable and may launch/manage Chrome lazily while it is running.

## Open questions

1. Should the host bridge wrap `chrome-devtools-mcp`, reuse BEARS’ existing CDP implementation, or implement a small MCP server directly?
2. Should arbitrary JavaScript evaluation be excluded, separately approved, or never exposed through the bridge?
3. What exact bind-address defaults work best across macOS Docker Desktop, Linux Docker, and remote SSH/container environments?
4. Should the token be supplied manually, generated into a file, or derived from existing BEARS/Den credentials?
5. How should screenshot/image content be represented through Den/Letta and ACP UI surfaces?
6. Should the host bridge be usable by non-container ACP adapters as a browser MCP source, or is it only for container fallback?
7. Should bridge health and active tool source appear in `/doctor`, `/capabilities`, and `/runtime` slash commands?

## References

- ACP Session Setup and `mcpServers`: <https://agentclientprotocol.com/protocol/session-setup>
- ACP MCP-over-ACP RFD: <https://agentclientprotocol.com/rfds/mcp-over-acp>
- Zed external agents configuration boundaries: <https://zed.dev/docs/ai/external-agents#configuration-boundaries>
- Zed MCP docs: <https://zed.dev/docs/ai/mcp>
- BEARS MCP services ADR: `docs/architecture/adr/mcp-services.md`
- Pair tool discovery and scope orientation ADR: `docs/architecture/adr/pair-tool-discovery-and-scope-orientation.md`
