# bear-armature

`bear-armature` is the local stdio edge for Agent Client Protocol clients such as Zed.

It speaks ACP JSON-RPC over stdin/stdout and talks to Den over BearWire by default. The legacy Den `/acp/**` HTTP path remains available temporarily as an explicit fallback.

The legacy binary name `bears-acp-adapter` remains available as a symlink for existing editor configurations.

## Current scope

Implemented:

- `initialize`
- `authenticate`
- `session/new`
- `session/list`
- `session/load` / `session/resume`
- `session/prompt`
- `session/cancel`
- `session/close`
- BearWire -> ACP `session/update` projection for assistant text/thought chunks, tool cards/status, and structured session state
- ACP client-tool relay for editor file-system tools:
  - `fs/read_text_file`
  - `fs/write_text_file`

`fs/write_text_file` is a whole-file text create/replace operation. It is not a granular patch/edit API and does not cover directory creation, delete, move/rename, or copy operations.

Session setup requires an absolute local `cwd`. The adapter prefers explicit `params.cwd`, then known client workspace URI/folder fallbacks if they normalize to an absolute local path. Relative or missing `cwd` values are rejected with a JSON-RPC validation error so Den only persists resumable sessions with a truthful filesystem context.

ACP-provided `mcpServers` are intentionally rejected when non-empty. BEARS currently exposes Den tools plus ACP client filesystem bridges, and does not own stdio MCP subprocess lifecycle. The adapter also reports `mcpCapabilities.http = false` and `mcpCapabilities.sse = false` until real MCP support exists.

Live BearWire projection keeps execution ownership explicit: Den-hosted tool events are rendered as display-only tool cards and must not produce `client.tool.result` responses from the armature, while armature-local tool calls are executed through the trusted local boundary. Tool-card states are monotonic for the live surface; once a card reaches `completed` or `failed`, later stale `pending`/`in_progress` updates for the same tool call are suppressed.

`session/load` replays persisted conversation history and typed surface records as ACP `session/update` notifications where Den has enough structured history to reconstruct them. This can include user/assistant text, reasoning/thought chunks, and completed tool records; richer upstream runtime event history is still limited to what Den persisted as replayable surface state.

`session/list` lists persisted/resumable Den ACP sessions only. With BearWire configured, `session/new` opens a Den-owned session and durable conversation before the first prompt; no model turn is recorded until the first ordinary prompt.

Not implemented yet:

- MCP relay
- terminal tool execution
- broader file mutation tools beyond ACP's standard read/write text-file requests

## Choosing a hat in the IDE

The connection and its code token remain **Bear-scoped**: they authenticate the human and armature, not a hat or tool grant. A Bear admin can mark one hat as this Bear's **IDE default** on the Hats page. New ACP conversations use it when configured; existing conversations are unchanged. A Bear without a default starts new editor conversations unbound unless a hat is chosen before the first turn.

On a fresh IDE conversation, enter `/hat` to list the available hats without using the selection opportunity. Enter `/hat <hat name or UUID>` **before any other productive interaction** to wear a different hat (or choose one if no default exists). Den verifies and saves the choice against the durable conversation; the editor cannot change it after a turn, nor can reconnection choose a different hat. To change responsibilities later, start a new conversation. A hat does not grant filesystem tools, credentials, outbound access, or permission to run Work; Work uses the assigned Job's hat.

## Chrome DevTools tools

The adapter can expose Chrome/Chromium/Edge browser tools when browser automation is actually
available.

Availability is detected in this order:

1. explicit CDP endpoint via `BEARS_CHROME_CDP_URL`
2. explicit CDP endpoint via `BEARS_BROWSER_CDP_URL`
3. managed local browser launch, if a supported Chrome/Chromium/Edge executable can be found

If neither an explicit CDP endpoint nor a launchable local browser is available, the adapter does
not advertise the Chrome tools.

When managed local browser launch is used, the adapter starts a local headless browser with a
temporary profile and a localhost-only remote-debugging port on first use.

## Build

From the repository root:

```bash
cargo build --manifest-path tools/bear-armature/Cargo.toml
```

The binary will be at:

```bash
tools/bear-armature/target/debug/bear-armature
```

## Required environment

The adapter needs a Den API URL, bear slug, and bearer token with `acp:chat` scope. BearWire is the default Den ↔ armature transport.

```bash
export DEN_API_URL="https://api.bears.[domain]" # or another public API origin, e.g. https://bears.[domain]:3001
export BEAR_SLUG="test-bear"
export DEN_TOKEN="..."
```

Transport controls:

```bash
# Default: BearWire auto/probe mode. Usually leave unset.
# export BEARS_BEARWIRE=auto

# Require BearWire and fail instead of falling back if Den does not support it.
export BEARS_BEARWIRE_REQUIRED=1

# Temporary Phase-4 escape hatch: force legacy /acp HTTP.
export BEARS_LEGACY_ACP_HTTP=1

# Disable BearWire without forcing the legacy marker explicitly.
export BEARS_BEARWIRE=off
```

Prefer `BEARS_LEGACY_ACP_HTTP=1` over `BEARS_BEARWIRE=off` when deliberately testing or recovering the legacy path, because `/status` and `/doctor` report the legacy-forced mode clearly.

Use any Den API origin reachable from the process running the adapter. For Zed on macOS, this normally means a host-reachable HTTPS URL, a separate API hostname, or a published API port on the web host. `DEN_API_URL` must be the API origin only, not the full `/acp/bears/.../prompt` endpoint.

You can validate configuration without starting ACP stdio:

```bash
bear-armature acp --check-config
```

For a more user-friendly setup report, run:

```bash
bear-armature doctor
```

`doctor` prints the installed command path, version/build metadata, OS/architecture, required environment status, Den `/version` reachability when configuration is valid, and copy/paste-ready ACP client environment hints.

## Updates

The macOS `.pkg` install can update itself by downloading and verifying a newer signed/notarized package from the public update manifest. Check for updates with:

```bash
bear-armature update-check
```

Install an available update with the macOS Installer GUI:

```bash
bear-armature update
```

For terminal-driven installs, use:

```bash
bear-armature update --install --yes
```

Update options:

- `--channel <stable|beta>` selects the public update channel. The default is `BEAR_ARMATURE_UPDATE_CHANNEL`, `BEARS_ACP_UPDATE_CHANNEL`, or `stable`.
- `--manifest-url <url>` overrides the manifest URL. The default stable arm64 macOS manifest is `https://bears-ai.github.io/bear-den/bear-armature/stable/aarch64-apple-darwin.json` (with fallback to the legacy `bears-acp-adapter` path).
- `--open` downloads, verifies, and opens the `.pkg` in macOS Installer.
- `--install`/`--cli` downloads, verifies, and runs `sudo /usr/sbin/installer`.
- `--download-only` downloads and verifies the `.pkg` without installing.

Verification checks include the manifest SHA-256 digest, macOS package signature, optional expected Developer ID Installer identity/team ID, Gatekeeper install assessment, and stapled notarization ticket validation.

You can also validate which Den server build the adapter reaches, without speaking ACP to the editor:

```bash
bear-armature acp --check-server
```

This fetches `GET /version` from `DEN_API_URL` and prints Den's service name, package version, git SHA, and build timestamp when available.

If the adapter is started by an ACP client with missing or invalid configuration, it stays running and returns a JSON-RPC error on `session/prompt` with specific setup instructions. This avoids opaque client-side errors such as “server shut down unexpectedly” when, for example, `DEN_API_URL` was never set.

## Zed custom agent config

In Zed settings, add a custom agent server. Adjust the command path and environment values:

```json
{
  "agent_servers": {
    "BEARS": {
      "type": "custom",
      "command": "/absolute/path/to/bear-armature",
      "args": ["acp", "--client", "zed"],
      "env": {
        "DEN_API_URL": "https://api.bears.[domain]",
        "BEAR_SLUG": "test-bear",
        "DEN_TOKEN": "..."
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
      "command": "/absolute/path/to/bear-armature",
      "args": ["acp", "--client", "zed", "--token-env", "DEN_TOKEN"],
      "env": {
        "DEN_API_URL": "https://api.bears.[domain]",
        "BEAR_SLUG": "test-bear"
      }
    }
  }
}
```

Legacy editors that invoke the binary without the `acp` subcommand still work when they pass `--api-url`, `--bear`, and token flags directly:

```json
{
  "args": ["--client", "zed", "--token-env", "DEN_TOKEN"]
}
```

Then use Zed's agent panel to start a new custom external-agent thread for `BEARS`.

## macOS downloaded binary warning

GitHub release/artifact downloads are unsigned today. macOS may quarantine the downloaded adapter and show an error such as “Apple cannot check it for malicious software” or “developer cannot be verified”.

For local testing, remove the quarantine flag and ensure the file is executable:

```bash
chmod +x /path/to/bear-armature-aarch64-apple-darwin
xattr -d com.apple.quarantine /path/to/bear-armature-aarch64-apple-darwin
```

Use the Intel filename if you downloaded the x86_64 build. You can verify the binary after clearing quarantine with:

```bash
/path/to/bear-armature-aarch64-apple-darwin --help
```

Building locally with Cargo also avoids the browser download quarantine path:

```bash
cargo build --release --manifest-path tools/bear-armature/Cargo.toml
```

Production distribution should add Developer ID signing and Apple notarization before we ask non-developer users to install the adapter.

## Debugging

- Run `bear-armature doctor` for a user-friendly setup report.
- Run `bear-armature acp --check-config` from the same shell or wrapper environment used by your editor.
- Run `bear-armature acp --check-server` to print the Den `/version` response reached by `DEN_API_URL`.
- Open Zed command palette: `dev: open acp logs`.
- The adapter writes logs only to stderr.
- Stdout is reserved for JSON-RPC protocol messages.
- HTTP failures include targeted hints for common cases: bad token (`401`), missing scope or membership (`403`), wrong API URL or disabled ACP gateway (`404`), wrong web/API origin (`405`), rate limits (`429`), and Den server errors (`5xx`).
- Prompt failures that successfully reached Den include Den `/version` metadata in the JSON-RPC error data when it can be fetched, which helps confirm the deployed server build while debugging.
- ACP `sessionId` values identify the client-side ACP session. The adapter lets Den bind a new session to a BEARS conversation, stores Den `conversation_resolved` events, and sends the resolved `conv-...` id on future prompts when available.
- Local editor file-system tool relay through the old external harness was removed. Future ACP tool support should be implemented in a dedicated ACP runtime rather than this adapter path.
