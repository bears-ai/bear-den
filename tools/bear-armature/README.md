# bear-armature

`bear-armature` is the local stdio edge for Agent Client Protocol clients such as Zed.

It speaks ACP JSON-RPC over stdin/stdout and talks to Den over BearWire v1. BearWire is required; the legacy Den `/acp/**` HTTP fallback is retired.

The session lifecycle, transport, tool surface, and headless startup described below were checked against the local branch on **2026-10-06**, not a shipped build or live provider. Start at the maintained [BearWire and ACP topic](../../docs/topics/bearwire-acp.md) for final test evidence, the successful separate Den production/musl validation-image build, current boundaries, and remaining work. That image was not deployed; no service restart or real-model/provider smoke occurred. No migrations or dependencies were added, but the shared typed protocol requires compatible Den/armature upgrades.

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
- `session/set_mode` and `session/set_config_option` for mode/model selection, subject to Den session access and policy
- BearWire -> ACP `session/update` projection for assistant text/thought chunks, tool cards/status, and structured session state
- Local workspace tools for reading, listing, finding, searching, and stat'ing paths; exact-text edits (`fs_edit_file`), text-file and directory creation, move/rename, copy, simple unified patches, and deletion
- Local git inspection and mutation tools, bounded captured process execution (`process_run`), and ACP terminal execution (`terminal_run_command`)
- ACP filesystem requests (`fs/read_text_file`, `fs/write_text_file`) for client-surface operations; disk-backed reads normally execute locally
- Discovery and invocation of tools from ACP-forwarded **stdio** MCP servers, plus a separately configured host-browser bridge over streamable HTTP

`fs/write_text_file` remains a whole-file text create/replace request, not a granular edit API. The broader filesystem operations above are separate armature-local tools. Workspace-root, sensitive-path, mode, and permission checks still apply; an advertised tool is not a grant to execute it.

ACP terminals require the client to advertise `terminal: true`. `run_command` prefers the client terminal when available and otherwise uses local process execution; `process_run` explicitly requests bounded captured output.

Session setup requires an absolute local `cwd`. The adapter prefers explicit `params.cwd`, then known client workspace URI/folder fallbacks if they normalize to an absolute local path. Relative or missing `cwd` values are rejected with a JSON-RPC validation error so Den only persists resumable sessions with a truthful filesystem context.

ACP `mcpServers` must be an array of stdio server definitions; the armature starts local subprocesses for discovery/calls. ACP-supplied HTTP and SSE definitions are rejected, so `mcpCapabilities.http = false` and `mcpCapabilities.sse = false` remain truthful. The separately configured host-browser bridge uses `BEARS_HOST_BROWSER_MCP_URL` and `BEARS_HOST_BROWSER_MCP_TOKEN`; it is not general ACP HTTP MCP support.

Live BearWire projection keeps execution ownership explicit: Den-hosted tool events are rendered as display-only tool cards and must not produce `client.tool.result` responses from the armature, while armature-local tool calls are executed through the trusted local boundary. Tool-card states are monotonic for the live surface; once a card reaches `completed` or `failed`, later stale `pending`/`in_progress` updates for the same tool call are suppressed.

## Session access and history

Den returns a typed `session.access` projection with `state` (`awaiting_hat`, `executable`, or `read_only`) and `may_select_hat`. ACP lifecycle results expose it in `_meta.bears.access`; `/status` and `/conversation` also explain the state. It is advisory: Den rechecks canonical authority at every effect.

- `session/new` persists an owner-bound client session. A valid configured IDE default can admit its durable conversation immediately. Without one, it stays `awaiting_hat`, with no resolved conversation or history binding, until a valid explicit hat selection. Ordinary prompts cannot execute while awaiting a hat.
- `hats.list`, session/model inspection, and reconnect do not materialize a pending conversation. Adding an IDE default later does not silently admit that pending session on reconnect.
- `session/load` replays only Den's explicit `history_conversation_id` as ACP `session/update` notifications. Replay may include user/assistant text, thought chunks, and completed tool records to the extent persisted. Pending sessions have no transcript to replay. `session/resume` restores without replaying history.
- Authorized history inspection, including Bear-admin inspection of another owner's source, remains `read_only` when it lacks execution authority. It does not rebind ownership or the source, permit productive prompts, `/hat` selection, `/focus`, compaction, or mode/model mutation. Read-only lifecycle results omit modes and offer no config options.
- `session/list` maps Den's actual `client_session_id` to ACP `sessionId`, not the database row ID. A failed load/resume returns an error; it never fabricates a replacement pending session or overwrites a known local binding with one. Unknown explicitly requested history is rejected, not treated as a request for a fresh source.
- Neither reconnect nor direct `run.start` can substitute an owned or fresh transcript for a read-only session binding. Den checks the stored and resolved sources independently before execution.

Session and canonical conversation identifiers are **opaque**. The armature validates and forwards Den's canonical projection; it does not infer access from prefixes such as `conv-` or `new-`. Missing, malformed, or inconsistent access projections fail closed, including against older Den versions. Token preflight alone does not prove lifecycle compatibility.

Den atomically publishes a pending session's canonical conversation/owner/hat binding under the same session-row transaction/lock. A direct start with no session row uses a separate short publication lock. Competing starts or hat selection adopt the winning source or fail, and metadata writes preserve the latest resolved source rather than overwriting it from a stale pending alias. This is source-publication atomicity, **not** an inference lease or stronger Work-revocation guarantee.

## Choosing a hat in the IDE

The connection and its code token remain **Bear-scoped**: they authenticate the human and armature, not a hat or tool grant. A Bear admin can mark one hat as this Bear's **IDE default** on the Hats page. New sessions can use that valid default; existing sessions and history are not retroactively admitted or rebound.

Enter `/hat` to list hats without consuming selection eligibility. Enter `/hat <hat name or UUID>` while Den projects `may_select_hat: true` and before an admitted productive interaction. Names are matched case-insensitively. Den admits a pending source or verifies the initial selection against an existing live owned source; the armature updates its canonical binding only after validating Den's response.

The armature reserves the initial prompt/selection while it is in flight, rejecting a competing selection until it finishes. Listing, diagnostics, invalid hats, and failed admission do not themselves consume eligibility; failures release the reservation so selection can be retried if Den still permits it. A successful explicit selection or admitted productive interaction closes the armature's initial selection opportunity, even if later turn delivery fails. Successful restore refreshes eligibility from Den rather than guessing from process-local history. Load/resume rejects while an initial prompt/selection reservation is held, preserves that reservation, and fences new productive/configuration/selection requests while restoring. Generation checks reject delayed stale history/state before projection or cache/eligibility replacement. To change hats after that boundary, start a new conversation.

Wearing a hat is not a blanket filesystem, command, credential, network, or Work grant. Supported persistent hat grants are narrowly scoped; the broader grant resolver is still incomplete. Work uses the assigned Job's hat, not the IDE default.

## Chrome DevTools tools

The adapter can expose Chrome/Chromium/Edge browser tools when browser automation is actually
available. External browser MCP tools suppress the built-in Chrome fallback; use `/status` to inspect the active browser source.

Availability is detected in this order:

1. explicit CDP endpoint via `BEARS_CHROME_CDP_URL`
2. explicit CDP endpoint via `BEARS_BROWSER_CDP_URL`
3. managed local browser launch, if a supported Chrome/Chromium/Edge executable can be found

If neither an explicit CDP endpoint nor a launchable local browser is available, the adapter does
not advertise the Chrome tools.

When managed local browser launch is used, the adapter starts a local headless browser with a
temporary profile and a localhost-only remote-debugging port on first use.

## Headless Work startup

`bear-armature headless` executes one checked-out Work run without an ACP editor. It uses the standard connection environment plus `DEN_WORK_ORDER_ID` (an exact non-nil Work-run UUID), optional `DEN_WORKSPACE` (default `/workspace`), and `DEN_HEADLESS_DEADLINE_SECS` (default 840).

Before checkout, it requires Den's BearWire `initialize` response to advertise `capabilities.expected_work_source: true`. It validates `work.checkout`'s success, allowed Work-run gate, exact requested run ID in both the response and gate binding, non-nil execution-attempt UUID, positive fence epoch, and non-empty prompt. Only then does it send the typed `expected_work_source` (`work_run_id`, `execution_attempt_id`, `fence_epoch`) on both `session.open` and `run.start`.

That expectation is an identity check, not a grant. Den rechecks the live run/attempt/fence and canonical authorization; a mismatch detected at a recheck cannot fall back to an ordinary IDE conversation. Work authority does not override transcript ownership, active status, or archive markers. Exact-source requests fail closed rather than reusing an already-active turn without proven original Work source/attempt/fence.

**Exact Work is startup preflight with rechecks, not an atomic inference lease.** It does not hold authority across inference or guarantee instantaneous attempt/hat revocation after a successful check. **Upgrade Den and armature together** for this boundary; do not bypass a capability or fence failure. See the [compatibility guide](../../services/den/docs/guides/bearwire-compatibility.md).

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

The adapter needs a Den API URL, bear slug, and bearer token with `armature:chat` scope. BearWire is the required Den ↔ armature transport.

```bash
export DEN_API_URL="https://api.bears.[domain]" # or another public API origin, e.g. https://bears.[domain]:3001
export BEAR_SLUG="test-bear"
export DEN_TOKEN="..."
```

Normally leave `BEARS_BEARWIRE` unset (or set it to `auto`/`true`). `auto` enables BearWire; it no longer means fallback negotiation. `BEARS_BEARWIRE=off` disables the required transport and causes Den operations to fail, not switch to `/acp/**`. `BEARS_BEARWIRE_REQUIRED` and `BEARS_LEGACY_ACP_HTTP` are not supported fallback controls.

Use any Den API origin reachable from the process running the adapter. For Zed on macOS, this normally means a host-reachable HTTPS URL, a separate API hostname, or a published API port on the web host. `DEN_API_URL` must be the API origin only, not the full `/bearwire/v1/rpc` endpoint.

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
- Check token scope (`armature:chat`), Bear membership, and that the API origin exposes `/bearwire/v1/rpc`; there is no legacy ACP gateway fallback.
- Prompt failures that successfully reached Den include Den `/version` metadata in the JSON-RPC error data when it can be fetched, which helps confirm the deployed server build while debugging.
- ACP `sessionId` identifies the client session, not the database session row or canonical conversation. Use `/conversation` and `/status` for Den's opaque binding and access state.
- If access projection decoding or headless `expected_work_source` negotiation fails, check Den/armature compatibility rather than enabling retired transport flags.
- See [ACP troubleshooting](../../docs/guides/acp-troubleshooting.md) for hat admission, read-only replay, failed resume, and tool-boundary diagnostics.
