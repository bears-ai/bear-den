# BEARS ACP adapter

`bears-acp-adapter` is a local stdio edge adapter for Agent Client Protocol clients such as Zed.

It speaks ACP JSON-RPC over stdin/stdout and calls the remote Den API ACP gateway over HTTPS/SSE.

## Current scope

Implemented first slice:

- `initialize`
- `session/new`
- `session/prompt`
- Den SSE -> ACP `session/update` text/thought chunks

Not implemented yet:

- client tools
- cancellation forwarding
- session resume/load/list
- MCP relay
- terminal/file-system tool execution

## Build

From the repository root:

```bash
cargo build --manifest-path tools/bears-acp-adapter/Cargo.toml
```

The binary will be at:

```bash
tools/bears-acp-adapter/target/debug/bears-acp-adapter
```

## Required environment

The adapter needs a Den API URL, bear slug, and bearer token with `acp:chat` scope.

```bash
export BEARS_DEN_API_URL="https://api.bears.[domain]" # or another public API origin, e.g. https://bears.[domain]:3001
export BEARS_BEAR_SLUG="test-bear"
export BEARS_DEN_TOKEN="..."
```

Use any Den API origin reachable from the process running the adapter. For Zed on macOS, this normally means a host-reachable HTTPS URL, a separate API hostname, or a published API port on the web host.

## Zed custom agent config

In Zed settings, add a custom agent server. Adjust the command path and environment values:

```json
{
  "agent_servers": {
    "BEARS": {
      "type": "custom",
      "command": "/absolute/path/to/bears-acp-adapter",
      "args": ["--client", "zed"],
      "env": {
        "BEARS_DEN_API_URL": "https://api.bears.[domain]",
        "BEARS_BEAR_SLUG": "test-bear",
        "BEARS_DEN_TOKEN": "..."
      }
    }
  }
}
```

For local development, prefer `--token-env` so the token is not written into Zed settings:

```json
{
  "agent_servers": {
    "BEARS": {
      "type": "custom",
      "command": "/absolute/path/to/bears-acp-adapter",
      "args": ["--client", "zed", "--token-env", "BEARS_DEN_TOKEN"],
      "env": {
        "BEARS_DEN_API_URL": "https://api.bears.[domain]",
        "BEARS_BEAR_SLUG": "test-bear"
      }
    }
  }
}
```

Then use Zed's agent panel to start a new custom external-agent thread for `BEARS`.

## Debugging

- Open Zed command palette: `dev: open acp logs`.
- The adapter writes logs only to stderr.
- Stdout is reserved for JSON-RPC protocol messages.
