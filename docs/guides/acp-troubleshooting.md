# ACP Troubleshooting Runbook

This runbook covers the current BEARS ACP direct path:

```text
Zed/OpenCode ⇄ bears-acp-adapter ⇄ Den ACP gateway ⇄ Letta conversation API
```

ACP `pair` role traffic should not route through Codepool.

---

## 1. Verify deployed versions

Check Den:

```bash
curl -s "$DEN_API_URL/version"
```

Check adapter startup in the editor logs:

```text
bears-acp-adapter: starting version=... build_git_sha=... local_head_sha=...
```

If Den and adapter are not both current, fix that first. Many ACP failures are version skew.

## 1a. Inspect bear environment and status

The ACP adapter now exposes a single read-only diagnostic tool, `bear_environment`, plus `/status` as a compact human rendering of the same underlying environment snapshot.

Use these when you need to distinguish between:

- adapter runtime problems
- Den reachability problems
- session/MCP registration problems
- host browser bridge configuration problems
- local Chrome fallback problems

Expected behavior:

- `bear_environment` returns structured environment state for the current bear/session/runtime.
- `/status` renders a compact summary from the same shared snapshot.
- If Den cannot be reached, `/status` should still show meaningful degraded status rather than failing silently.

For host browser bridge debugging, the most relevant fields are:

- `browser.active_source`
- `services.den`
- `environment_variants.acp_adapter.host_browser_bridge_env`
- `environment_variants.acp_adapter.session_mcp`
- `diagnostics.status`
- `diagnostics.warnings`
- `diagnostics.errors`

---

## 2. Basic chat diagnostic

Prompt:

```text
Reply with exactly: hello from bear
```

Expected adapter log:

```text
bears-acp-adapter: Den stream summary ... event_types={"assistant_text_delta": ..., "turn_complete": 1} ... saw_assistant_output=true
```

Expected Den log:

```text
ACP Letta stream summary ... mapped_events>0 ... adapter_event_types={"assistant_text_delta": ...}
```

If basic chat fails, do not debug file tools yet.

---

## 3. File read diagnostic

Prompt with an absolute path under the current workspace:

```text
Read /absolute/path/to/small-file.txt and summarize it.
```

Expected flow:

1. Letta emits `approval_request_message` or `tool_call_message`.
2. Den maps it to `tool_request`.
3. Adapter logs `requesting permission` if approval is required.
4. Adapter calls ACP client `fs/read_text_file` if the client advertises it.
5. Adapter logs fallback only if client does not advertise `fs.readTextFile`.
6. Adapter posts result to Den.
7. Den posts Letta approval/tool return to the same `conv-*`.
8. Letta emits assistant text.

Useful adapter log snippets:

```text
bears-acp-adapter: requesting permission session_id=... tool_call_id=... tool_name=... path=...
bears-acp-adapter: client fs/read_text_file path=... bytes=... duration_ms=...
bears-acp-adapter: posted tool result session_id=... tool_call_id=... response=...
bears-acp-adapter: Den stream summary ...
```

Useful Den log snippets:

```text
ACP tool request registered ... tool_call_id=... tool_name=...
ACP tool result received ... body_tool_call_id=... body_approval_request_id=...
ACP Letta stream summary ... native_message_types=... adapter_event_types=...
```

Expected user-visible tool UX:

- The ACP client should show a human-readable tool card, such as `Reading /absolute/path/to/small-file.txt`, not a generic `tool_call` title.
- Permission prompts should include the concrete target and risk, such as the path, URL host, command/cwd, memory scope, or plan id.
- Raw `args` may be attached as diagnostic/raw input, but visible content should prefer Den `display.title`, `display.subtitle`, `display.approval_summary`, and bounded summaries.
- If a new tool renders generically, verify that its Den/ACP descriptor includes display metadata and that the adapter is consuming `event.display`.

---

## 4. Common failures

### Invalid provider tool name

Symptom:

```text
Invalid 'tools[0].name': string does not match pattern
```

Cause: Den sent a provider tool name with `.`, `/`, or whitespace.

Expected provider name:

```text
fs_read_text_file
```

Not:

```text
fs.read_text_file
fs/read_text_file
```

See `docs/architecture/adr/provider-safe-tool-naming.md`.

### Empty turn with approval requests

Symptom:

```text
Letta completed the turn without producing displayable ACP output
message_types={"approval_request_message": ...}
mapped_events=0
```

Cause: Den did not parse Letta's tool-call stream shape.

Actions:

1. Set Den env var:

```bash
ACP_DEBUG_EVENT_SAMPLE_CHARS=8000
```

2. Restart Den.
3. Reproduce once.
4. Copy one full `approval_request_message` sample, keeping `tool_call` / `tool_calls` / `arguments` / `input` intact.

### Tool return 409 conflict

Symptom:

```text
Cannot send a new message: Another request is currently being processed
```

Cause: Den sent a Letta tool return before draining the original Letta stream.

Expected behavior: Den stores the tool result and posts the Letta continuation only after original stream EOF.

### Invalid tool call IDs

Symptom:

```text
Invalid tool call IDs. Expected '[call_...]', but received '[fs_read_text_file]'
```

Cause: Den sent the provider tool name instead of Letta's `tool_call_id` in the tool return.

Expected: `tool_return.tool_call_id` is the original `call_...` id.

### Letta approval shape 422

Symptom:

```text
Unable to extract tag using discriminator 'type'
```

Cause: Den sent an approval return without inner `type: "tool"`.

Expected:

```json
{
  "type": "approval",
  "approval_request_id": "message-...",
  "approve": true,
  "approvals": [
    {
      "type": "tool",
      "status": "success",
      "tool_call_id": "call_...",
      "tool_return": "..."
    }
  ]
}
```

### Missing file path

Symptom:

```text
Letta requested fs_read_text_file without a path argument
```

Cause: Letta emitted a complete non-empty argument object without a string `path`, or Den parsed the wrong field.

If the raw sample shows argument fragments, Den should accumulate until valid JSON appears.

---

## 5. Safe raw sample collection

Set:

```bash
ACP_DEBUG_EVENT_SAMPLE_CHARS=8000
```

Then find in Den logs:

```text
ACP Letta stream summary
unmapped_event_samples=[...]
```

Redact secrets and local usernames if desired, but preserve:

- `message_type`
- `id`
- `run_id`
- `step_id`
- `tool_call`
- `tool_calls`
- `name`
- `tool_call_id`
- `arguments`
- `input`
- `args`

---

## 6. Protocol boundaries

Do not confuse these layers:

```text
Editor ⇄ adapter: ACP JSON-RPC over stdio
Adapter ⇄ Den: BEARS-private HTTPS/SSE transport
Den ⇄ Letta: Letta REST/SSE
```

Den ⇄ adapter event names include:

```text
assistant_text_delta
status_text
tool_request
conversation_resolved
turn_complete
error
```

These are not raw ACP messages; the adapter translates them into ACP `session/update` and client requests.
