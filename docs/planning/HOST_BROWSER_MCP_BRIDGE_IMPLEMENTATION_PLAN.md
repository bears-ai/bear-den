# Host Browser MCP Bridge Implementation Plan

**Status:** Draft  
**Date:** 2026-05-18  
**Related ADR:** `docs/architecture/adr/acp-host-browser-mcp-bridge.md`

## Goal

Add a browser-only host MCP bridge mode to the single installable `bears-acp-adapter` binary, then teach container-running ACP adapters to connect to that bridge as a constrained browser MCP source.

The result should support this deployment shape:

```/dev/null/host-browser-bridge-goal.txt#L1-4
Host install:
  bears-acp-adapter                  # normal ACP adapter mode
  bears-acp-adapter browser-bridge   # host browser MCP bridge mode

Container ACP session:
  container bears-acp-adapter -> http://host.docker.internal:9277/mcp -> host browser bridge -> host Chrome
```

The bridge must not expose host filesystem, host shell/process execution, host git operations, or arbitrary host MCP servers.

## Non-goals

- Do not expose the host adapter’s full ACP server to container adapters.
- Do not provide generic host filesystem/process tools through the bridge.
- Do not require launch-on-login.
- Do not require Chrome to be installed in every dev container.
- Do not replace Zed-forwarded `mcpServers`; continue to prefer client-forwarded MCP tools when they are available and discovered.

## Current state

Already implemented or partially implemented:

- Adapter parses Zed-forwarded ACP `mcpServers`.
- Adapter supports stdio MCP discovery/calls through `rmcp`.
- Adapter logs sanitized MCP server summaries, discovery results, and dynamic tool names.
- Adapter rewrites Docker `exec -it`/`-ti`/`-t`/`--tty` flags for stdio MCP because MCP JSON-RPC must not run through a TTY.
- Den appends dynamic MCP descriptors from `client_context.mcp.client_tools`.
- Built-in Chrome tools are treated as fallbacks and suppressed when client MCP tools are discovered.

Known issue motivating this plan:

- In dev-container sessions, Zed can forward `chrome-devtools-mcp` as a remote stdio server, but the MCP server runs inside the container and cannot find host Chrome.

## Architecture summary

Introduce a browser-only MCP bridge mode in `bears-acp-adapter`:

```/dev/null/host-browser-bridge-mode.txt#L1
bears-acp-adapter browser-bridge --listen 127.0.0.1:9277
```

The bridge exposes an MCP server over Streamable HTTP at `/mcp` and requires bearer authentication. It provides only browser tools.

The normal ACP adapter mode gains optional configuration:

```/dev/null/container-env.txt#L1-2
BEARS_HOST_BROWSER_MCP_URL=http://host.docker.internal:9277/mcp
BEARS_HOST_BROWSER_MCP_TOKEN=<token>
```

When configured, the adapter registers the host browser MCP bridge as an additional MCP source. Tool preference order:

1. Zed/client-forwarded MCP tools from ACP `mcpServers`.
2. Host browser MCP bridge tools.
3. BEARS built-in local Chrome/CDP fallback tools.
4. No browser tools.

## Phase 1: Refactor MCP registry for multiple sources

### Tasks

- Generalize `tools/bears-acp-adapter/src/tools/mcp.rs` from “ACP session-provided MCP servers only” to “MCP sources”.
- Keep Zed-forwarded `mcpServers` as source kind `client_forwarded`.
- Add source kind `host_browser_bridge`.
- Add source metadata into descriptor `x_bears`, for example:

```/dev/null/mcp-source-descriptor.json#L1-9
{
  "x_bears": {
    "source": "host_browser_bridge",
    "server": "host-browser",
    "original_tool": "take_snapshot",
    "transport": "streamable_http",
    "trust_boundary": "host_browser_only"
  }
}
```

- Ensure tool names remain provider-safe, e.g.:

```/dev/null/mcp-tool-names.txt#L1-2
mcp__chrome_devtools_custom__take_snapshot
mcp__host_browser__take_snapshot
```

### Acceptance criteria

- Existing stdio MCP behavior remains unchanged.
- Logs identify source kind for each discovered MCP server and tool.
- Tool-call logs include a sanitized/truncated MCP result content preview when tools return errors, so host-vs-container runtime failures are visible without relying on model summaries.
- Dynamic descriptors include source metadata.

## Phase 2: Add MCP Streamable HTTP client support

### Tasks

- Add `rmcp` features required for Streamable HTTP client transport.
- Implement HTTP MCP server config support in `tools/mcp.rs`.
- Support auth headers for configured host bridge:

```/dev/null/mcp-http-auth.txt#L1
Authorization: Bearer <token>
```

- Extend current transport rejection logic:
  - Zed-forwarded HTTP MCP servers may be supported once this phase lands.
  - SSE remains unsupported unless deliberately added.
  - ACP transport remains future work per ACP MCP-over-ACP RFD.

### Acceptance criteria

- Adapter can connect to an MCP Streamable HTTP server and run `tools/list`.
- Adapter can call discovered HTTP MCP tools and return tool results to Den.
- HTTP errors and auth failures produce clear adapter logs.
- Existing stdio MCP tests/checks still pass.

## Phase 3: Add host browser bridge mode to the single binary

### Tasks

- Add command dispatch in `tools/bears-acp-adapter/src/main.rs` or equivalent:

```/dev/null/browser-bridge-cli.txt#L1-4
bears-acp-adapter browser-bridge \
  --bind 127.0.0.1:3766 \
  --path /mcp \
  --token "$BEARS_HOST_BROWSER_MCP_TOKEN"
```

- In `browser-bridge` mode:
  - Do not initialize Den ACP runtime.
  - Do not advertise or serve ACP JSON-RPC.
  - Start only an MCP server exposing browser tools.

- Decide implementation approach:
  1. reuse/extract BEARS CDP code from `tools/bears-acp-adapter/src/tools/chrome.rs`, or
  2. wrap `chrome-devtools-mcp`, or
  3. implement a thin MCP server over a smaller CDP helper.

Recommendation for first implementation: reuse/extract the existing BEARS Chrome CDP code so the bridge does not depend on `npx`, package install behavior, or stdout/stderr quirks.

### Acceptance criteria

- Host can run `bears-acp-adapter browser-bridge --listen ...` manually.
- `/health` or equivalent diagnostic endpoint confirms readiness, if exposed outside MCP.
- MCP `tools/list` returns only browser tools.
- Non-browser tools are not present.
- Bridge starts without contacting Den.

### Implementation notes (current)

Implemented in `tools/bears-acp-adapter`:

- `bears-acp-adapter browser-bridge --bind 127.0.0.1:3766 --path /mcp --token <token>`
- Environment fallbacks:
  - `BEARS_HOST_BROWSER_MCP_BIND`
  - `BEARS_HOST_BROWSER_MCP_PATH`
  - `BEARS_HOST_BROWSER_MCP_TOKEN`
  - `BEARS_HOST_BROWSER_MCP_ALLOWED_ORIGINS`
- Readiness endpoint:
  - `GET /health`
- MCP endpoint:
  - `POST /mcp` and related Streamable HTTP traffic on the configured path
- Auth:
  - `Authorization: Bearer <token>` required for MCP endpoint

Currently exposed bridge tools are browser-only wrappers over the adapter’s existing Chrome/CDP implementation:

- `browser_open`
- `browser_snapshot`
- `browser_console_messages`
- `browser_network_requests`
- `browser_screenshot`

Notable implementation details:

- the implementation uses `--bind` rather than the earlier draft’s `--listen`
- host Chrome discovery can use either an existing CDP endpoint or a local executable
- explicit executable overrides are now supported via:
  - `BEARS_CHROME_EXECUTABLE`
  - `BEARS_BROWSER_EXECUTABLE`

## Phase 4: Bridge security controls

### Tasks

- Require bearer token unless explicitly started with an unsafe development flag.
- Redact sensitive network data where feasible:
  - `authorization`
  - `cookie`
  - `set-cookie`
  - `x-api-key`
  - other obvious token-like headers
- Validate `chrome_open` URLs:
  - allow `http` and `https`
  - reject or require future explicit opt-in for `file`, `chrome`, `devtools`, `javascript`, and `data`
- Decide policy for JavaScript evaluation:
  - default: do not expose `evaluate_script` through host bridge
  - future: expose only with separate tool class and approval
- Ensure all bridge logs avoid secrets.

### Acceptance criteria

- Unauthenticated MCP requests are rejected.
- Token is never printed in logs.
- Tool list contains no host filesystem/process/git tools.
- Browser network output redacts sensitive headers.

### Implementation notes (current)

Implemented hardening so far:

- `browser_open` now validates URL schemes and only allows `http` and `https`.
- Browser network event output redacts common sensitive headers recursively, including:
  - `authorization`
  - `cookie`
  - `set-cookie`
  - `x-api-key`
  - `proxy-authorization`

Still pending for fuller Phase 4 completion:

- explicit policy for JavaScript evaluation exposure (currently not exposed by bridge tool list)
- any additional redaction classes beyond the common header set above
- optional integration tests that exercise the HTTP bridge endpoints directly

## Phase 5: Container adapter host bridge registration

### Tasks

- Read bridge config:

```/dev/null/container-bridge-env.txt#L1-3
BEARS_HOST_BROWSER_MCP_URL
BEARS_HOST_BROWSER_MCP_TOKEN
BEARS_HOST_BROWSER_MCP_SERVER_NAME=host-browser # optional
```

- If configured, add the host browser bridge to the MCP registry after Zed-forwarded MCP servers.
- Health/discovery behavior:
  - If bridge is unreachable, log a warning and continue without browser bridge tools.
  - Do not fail `session/new` just because the optional host bridge is down.
- Tool preference:
  - If Zed-forwarded MCP browser/client tools are discovered, suppress built-in `chrome_*` fallback tools.
  - If no client MCP tools but host bridge tools are discovered, advertise bridge browser tools.
  - If neither, fall back to built-in local Chrome/CDP only if available.

### Acceptance criteria

- Container adapter logs bridge discovery separately from Zed-forwarded MCP discovery.
- `/capabilities` or `/runtime` shows active browser tool source.
- If bridge is down, ACP sessions still start.

## Phase 6: Den descriptor and policy refinement

### Tasks

- Add descriptor guidance that distinguishes:
  - Zed/client-forwarded MCP tools
  - BEARS host browser bridge tools
  - BEARS built-in Chrome fallback tools
- Consider policy classification for host bridge tools:
  - read-only-ish browser inspection for snapshot/console/network/screenshot
  - browser mutation/navigation for open/navigate
  - separate class if JavaScript evaluation is ever added
- Improve Den logs around final `client_tools` payload size and source counts.

### Acceptance criteria

- Letta sees clear browser tool descriptors when host bridge tools are active.
- Tool descriptions indicate the tools operate on the host browser, not the container filesystem.
- Den logs show dynamic MCP tool counts by source.

## Phase 7: Slash-command diagnostics

### Tasks

Update adapter slash commands:

- `/doctor`
- `/capabilities`
- `/runtime`

Show:

- ACP `mcpServers` count and summaries from Zed.
- Client-forwarded MCP discovery status.
- Host browser bridge configured/unconfigured.
- Host bridge health/discovery status.
- Active browser source:
  - `client_forwarded_mcp`
  - `host_browser_bridge`
  - `local_chrome_fallback`
  - `none`
- Reasons browser tools are unavailable.

### Acceptance criteria

- A user can diagnose “why don’t I see browser tools?” without reading raw ACP logs.
- Diagnostics do not expose tokens, env values, or sensitive headers.

## Phase 8: Tests

### Unit tests

- Docker TTY rewrite:
  - `-it` -> `-i`
  - `-ti` -> `-i`
  - `-t` removed
  - `--tty` removed
  - non-Docker commands unchanged
- MCP server summary redaction:
  - env/header names only
  - URL query/fragment/credentials removed
- Descriptor source metadata.
- Host bridge tool allowlist.

### Integration tests

- Mock stdio MCP server discovery and tool call.
- Mock Streamable HTTP MCP server discovery and tool call.
- Host bridge unreachable does not fail session creation.
- Host bridge discovered tools are advertised when no client MCP tools exist.
- Client-forwarded MCP tools suppress fallback browser tools.
- No host filesystem/process tools appear in bridge tool list.

### Manual smoke tests

1. Run host bridge manually.
2. Start dev-container BEARS ACP thread.
3. Confirm adapter logs:

```/dev/null/expected-logs.txt#L1-4
acp_mcp_configure source=client_forwarded ...
host_browser_mcp_configure ...
host_browser_mcp_discovery_ok ...
session/prompt ... mcp_tool_count=...
```

4. Ask the agent to use a discovered host browser tool by exact name.
5. Confirm `acp_mcp_call_start` and `acp_mcp_call_ok`.

## Operational notes

### Host startup

No launch-on-login is required. Operators start the bridge manually or via a task:

```/dev/null/start-bridge.txt#L1-4
BEARS_HOST_BROWSER_MCP_TOKEN=<token> \
BEARS_CHROME_EXECUTABLE="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
bears-acp-adapter browser-bridge --bind 127.0.0.1:3766 --path /mcp
```

### Container configuration

```/dev/null/container-config.txt#L1-2
BEARS_HOST_BROWSER_MCP_URL=http://host.docker.internal:3766/mcp
BEARS_HOST_BROWSER_MCP_TOKEN=<same-token>
```

### Zed task example

A Zed task can start the host bridge without making it a login service. Exact host-path handling depends on local installation.

## Rollout sequence

1. Land documentation and current diagnostics.
2. Add tests for existing MCP stdio registry, Docker TTY rewrite, and MCP error-result content previews.
3. Add Streamable HTTP MCP client support.
4. Add `browser-bridge` mode with minimal browser MCP tools.
5. Add container adapter host bridge registration.
6. Add slash-command diagnostics.
7. Run manual smoke tests from a dev-container Zed project.
8. Decide whether to retire or keep the older built-in local Chrome fallback path.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Host bridge accidentally exposes broad host tools | Browser-only MCP server mode; allowlist tools; tests assert no filesystem/process/git tools |
| Token leaks in logs | Never log auth headers or env values; redaction tests |
| Browser tools expose sensitive page data | Redact network headers; require normal ACP approval flow; document risk |
| Docker cannot reach loopback-bound host bridge | Test `host.docker.internal`; document bind-address fallback with token requirement |
| Multiple browser tool sources confuse model | Descriptor guidance and active-source diagnostics |
| HTTP MCP support takes longer than expected | Keep BEARS-only browser HTTP API as fallback option, but do not prefer it architecturally |

## Open implementation questions

1. Which `rmcp` version/features should be used for Streamable HTTP client support while staying compatible with the existing dependency tree?
2. Should the bridge be implemented directly with `rmcp` server traits or by adapting existing Chrome handlers through a thin MCP layer?
3. Should browser bridge tools be advertised as dynamic `mcp__host_browser__...` tools only, or also mapped to the existing `chrome_*` provider names for continuity?
4. How should image results be represented through Letta and ACP when the bridge returns screenshots?
5. Should the bridge manage a dedicated Chrome profile lazily, or only connect to an already-running Chrome/CDP endpoint?
