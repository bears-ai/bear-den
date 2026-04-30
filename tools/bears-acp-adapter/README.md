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

Use any Den API origin reachable from the process running the adapter. For Zed on macOS, this normally means a host-reachable HTTPS URL, a separate API hostname, or a published API port on the web host. `BEARS_DEN_API_URL` must be the API origin only, not the full `/acp/bears/.../prompt` endpoint.

You can validate configuration without starting ACP stdio:

```bash
bears-acp-adapter --check-config
```

You can also validate which Den server build the adapter reaches, without speaking ACP to the editor:

```bash
bears-acp-adapter --check-server
```

This fetches `GET /version` from `BEARS_DEN_API_URL` and prints Den's service name, package version, git SHA, and build timestamp when available.

If the adapter is started by an ACP client with missing or invalid configuration, it stays running and returns a JSON-RPC error on `session/prompt` with specific setup instructions. This avoids opaque client-side errors such as “server shut down unexpectedly” when, for example, `BEARS_DEN_API_URL` was never set.

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

## macOS downloaded binary warning

GitHub release/artifact downloads are unsigned today. macOS may quarantine the downloaded adapter and show an error such as “Apple cannot check it for malicious software” or “developer cannot be verified”.

For local testing, remove the quarantine flag and ensure the file is executable:

```bash
chmod +x /path/to/bears-acp-adapter-aarch64-apple-darwin
xattr -d com.apple.quarantine /path/to/bears-acp-adapter-aarch64-apple-darwin
```

Use the Intel filename if you downloaded the x86_64 build. You can verify the binary after clearing quarantine with:

```bash
/path/to/bears-acp-adapter-aarch64-apple-darwin --help
```

Building locally with Cargo also avoids the browser download quarantine path:

```bash
cargo build --release --manifest-path tools/bears-acp-adapter/Cargo.toml
```

Production distribution should add Developer ID signing and Apple notarization before we ask non-developer users to install the adapter.

## Debugging

- Run `bears-acp-adapter --check-config` from the same shell or wrapper environment used by your editor.
- Run `bears-acp-adapter --check-server` to print the Den `/version` response reached by `BEARS_DEN_API_URL`.
- Open Zed command palette: `dev: open acp logs`.
- The adapter writes logs only to stderr.
- Stdout is reserved for JSON-RPC protocol messages.
- HTTP failures include targeted hints for common cases: bad token (`401`), missing scope or membership (`403`), wrong API URL or disabled ACP gateway (`404`), wrong web/API origin (`405`), rate limits (`429`), and Den server errors (`5xx`).
- ACP `sessionId` values identify the client-side ACP session. The adapter sends `conversation_id: "default"` so Den/Codepool starts the Letta Code runtime from the bear's configured `letta_agent_id`, not from an `acp-...` session id.
