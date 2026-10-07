# ACP Troubleshooting Runbook

This runbook covers the ACP armature path:

```text
Editor ⇄ ACP stdio ⇄ bear-armature ⇄ BearWire v1 ⇄ Den native agent loop ⇄ Bifrost
```

The armature uses BearWire, not the retired `/acp/**` HTTP gateway or an external harness. Lifecycle/startup guidance below was checked against the **local branch on 2026-10-06**, not a shipped/live build or provider exchange. Start with the maintained [BearWire and ACP topic](../topics/bearwire-acp.md).

---

## 1. Verify deployed versions

Check Den:

```bash
curl -s "$DEN_API_URL/version"
```

Check adapter startup in the editor logs:

```text
bear-armature: starting version=... build_git_sha=... local_head_sha=...
```

Build metadata identifies what you reached; it is not a compatibility gate. The [local production-image validation](../topics/bearwire-acp.md#den-production-image-validation-build-only) built the separate tag `bears-den-armature-validation:local` without restart/deployment or real-model/provider smoke; it does not identify the running service build. The change adds no migrations or dependencies, but the shared typed protocol requires compatible Den/armature upgrades. Check the [BearWire compatibility guide](../../services/den/docs/guides/bearwire-compatibility.md) for the semantic boundary:

- The token needs `armature:chat` scope and access to the configured Bear. `DEN_API_URL` must be the API origin exposing `/bearwire/v1/rpc`.
- Leave `BEARS_BEARWIRE` unset or set it to `auto`/`true`. `off` disables required transport, not a fallback. `BEARS_LEGACY_ACP_HTTP` and `BEARS_BEARWIRE_REQUIRED` are not recovery switches.
- Ordinary token preflight can succeed against an older BearWire v1 Den, but session creation/load/resume/prompt still requires the typed session access projection. A missing or invalid projection fails closed; upgrade Den rather than deriving access from a conversation ID.
- Headless startup additionally requires `initialize.capabilities.expected_work_source: true` before checkout. Upgrade Den and armature together; never bypass the capability or checkout gate/fence checks.

## 1a. Inspect bear environment and status

The ACP adapter exposes a single read-only diagnostic tool, `bear_environment`, plus `/status` as a compact human rendering of the same underlying environment snapshot.

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

### Check session admission before a model turn

Use `/status` or `/conversation` and inspect `_meta.bears.access` in ACP lifecycle replies:

| Access | Diagnosis and action |
|---|---|
| `awaiting_hat` | The client session exists but has no durable conversation/history yet. Run `/hat`, then `/hat <name or UUID>` if `may_select_hat` is true. A valid IDE default admits a fresh session; adding one later does not admit an existing pending session on reconnect. |
| `executable` | Den currently verifies a canonical source. Continue the chat diagnostic; individual effects still recheck authority. |
| `read_only` | This is authorized history inspection, not execution as the source's owner. Replay is expected, but productive prompts, `/hat` selection, `/focus`, compaction, and mode/model changes are blocked. Start a new owned conversation to work. |

`/hat` listing, diagnostics, an invalid hat, or rejected admission do not themselves consume selection eligibility. An initial prompt/selection in flight reserves the opportunity; wait for it to finish before retrying. Failure releases the reservation, but retry only if Den still permits selection. A successfully selected hat or admitted productive interaction closes the armature's initial selection opportunity even if later delivery fails. Successful restore refreshes it from Den; stale local state is not authority. Load/resume rejects while an initial prompt/selection reservation is held and does not clear it. Restore fences new productive/configuration/selection requests; a session-generation change rejects delayed stale state/history before projection or cache replacement. For a busy restore, wait for the reserved interaction to finish. For “Session changed while history was being restored”, retry against Den's current state rather than forcing the stale response.

A failed `session/load` or `session/resume` is an error, never a synthetic new session. Check the ACP `sessionId` from `session/list` (Den's actual `client_session_id`), Bear/token ownership, and connectivity. Do not substitute a database row ID or rewrite the conversation ID. The known local binding is not replaced by a fabricated pending one after failure. Unknown explicitly requested history also fails instead of becoming a fresh session. Direct `run.start`, just like reconnect, cannot swap an owned or fresh transcript into a read-only session; start a genuinely new session rather than editing its binding.

“Admitted canonical session conversation changed” indicates that a competing publication won or stale metadata no longer matches the latest canonical source. Den publishes the source atomically under a short publication transaction/lock; reload its canonical state rather than overwriting the binding or replaying a stale pending alias. This publication lock is not an inference lease.

For headless Work, collect the `work.checkout` gate, requested/returned Work-run IDs, execution-attempt ID, and fence epoch. Missing, mismatched, denied, or malformed checkout state must stop before `session.open`/`run.start`. An “expected checked-out Work source is unavailable or changed” error means Den could not verify that exact live source; inspect the canonical Work/attempt state, not the IDE default. Do not synthesize a new fence or retry as ordinary Pair work.

A valid Work checkout is not permission to use any transcript: startup independently requires an existing transcript to be actor-owned, active, and not archive-marked. “Cannot prove the active turn admitted this exact Work source and fence” means Den rejected unproven reuse of an already-active turn; a currently valid Work association does not establish that turn's original admission. Inspect the canonical turn/source before retrying.

Exact Work validation is **startup preflight with rechecks**, not an atomic inference lease. It does not guarantee instantaneous attempt/hat revocation or hold that authority throughout inference. Do not infer a continuous revocation guarantee from a successful checkout or `run.start`.

---

## 2. Basic chat diagnostic

Prompt:

```text
Reply with exactly: hello from bear
```

Enable `/debug verbose` for adapter diagnostics, then look for a matching accepted run and terminal event:

```text
bear-armature: BearWire run.start accepted session_id=... run_id=... after=...
bear-armature: BearWire run terminal event received session_id=... run_id=... diagnostics=...
```

Correlate Den logs using that session/run ID. A terminal event can describe failure or cancellation, not only success; check the event outcome and visible ACP response. Do not expect the retired adapter-SSE `assistant_text_delta`/`turn_complete` or `ACP Letta stream summary` log shape.

If basic chat fails, do not debug file tools yet.

---

## 3. File read diagnostic

Prompt with an absolute path under the current workspace:

```text
Read /absolute/path/to/small-file.txt and summarize it.
```

Expected file-read flow during a prompt turn:

1. Den emits a BearWire tool event/obligation with descriptor-owned execution target. Den-hosted tool cards remain display-only; they must not trigger armature execution or `client.tool.result`.
2. For a client-owned read, the armature checks the descriptor's target/permission policy and resolves the path through the typed workspace boundary.
3. It reads/searches/stat's disk-backed workspace paths locally by default. Sensitive or escaping paths are denied or filtered. A read-only tool name alone is not a grant: approval follows the current descriptor/session policy; an eligible Den-owned exact-root hat grant is rechecked before local execution.
4. If approval is required, the adapter logs `requesting permission`. A Den permission obligation is settled with `client.permission.result`, not a fabricated tool result.
5. The adapter delegates to ACP client `fs/read_text_file` only for explicit editor-buffer/client-surface semantics, then verifies the client response against local file metadata.
6. The armature claims the local tool obligation and posts `client.tool.result` against the exact run/tool call. It logs the response when verbose, or when Den reports a stalled/ignored continuation.
7. Den continues the same native turn and emits BearWire message/terminal events, projected as ACP updates.

Useful adapter log snippets:

```text
bear-armature: requesting permission session_id=... tool_call_id=... tool_name=... path=...
bear-armature: read_text_file session_id=... path=... line=... limit=... bytes=... returned_lines=... truncated=... duration_ms=...
bear-armature: BearWire tool result response debug class=continued session_id=... run_id=... tool_call_id=... response={"ok":true,"continuation":"started",...}
bear-armature: BearWire run terminal event received session_id=... run_id=... diagnostics=...
```

Correlate Den's run and obligation state by `session_id`, `run_id`, `tool_call_id`, and, for a permission wait, the permission/obligation IDs. Legacy `/acp/**` tool-return logs are not the current boundary.

Session lifecycle replay expectations:

- `session/load` replays only Den's explicit `history_conversation_id` before responding, including historical `user_message_chunk` and `agent_message_chunk` updates where persisted. A pending session has no history; authorized read-only inspection preserves the exact source, including when an admin inspects another owner's history.
- `session/resume` restores the session without replaying history, per ACP resume semantics.
- ACP replay is client-side rendering. Den/model context replay remains owned by canonical conversation storage and next-turn request construction.

Expected user-visible tool UX:

- The ACP client should show a human-readable tool card, such as `Reading /absolute/path/to/small-file.txt`, not a generic `tool_call` title.
- Permission prompts should include the concrete target and risk, such as the path, URL host, command/cwd, memory scope, or plan id.
- Raw `args` may be attached as diagnostic/raw input, but visible content should prefer Den `display.title`, `display.subtitle`, `display.approval_summary`, and bounded summaries.
- If a new tool renders generically, verify that its Den/ACP descriptor includes display metadata and that the adapter is consuming `event.display`.
- For file reads and searches, disk-backed workspace paths execute in `bear-armature` by default, subject to descriptor-owned `approval_policy`, `target_policy`, and `sensitive_path_policy` plus current session/hat checks. Do not diagnose a permission wait as a bug solely because the tool is read-only. ACP client read delegation is reserved for explicit editor-buffer/client-surface semantics.
- If an ACP client returns `{ "content": "" }` for a missing or non-empty file, treat it as a client bug; the adapter verifies delegated client responses and converts invalid success into a failed tool result so the model turn can continue with the error.
- Canonical source paths are listed in `docs/architecture/repository-shape.md`; use `tools/bear-armature/` for source references and reserve `bears-acp-adapter` for legacy binary/package compatibility.

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

### Empty turn with tool requests

Symptom:

```text
completed the turn without producing displayable ACP output
mapped_events=0
```

Check whether Den accepted `run.start`, whether BearWire emitted a required client obligation, and whether the armature claimed/settled it. Do not infer the failing layer from a legacy `mapped_events` counter.

Actions:

1. Enable `/debug verbose` and reproduce once in an executable session.
2. Collect the matching run ID, BearWire event type/sequence, execution target, obligation/tool-call IDs, and any RPC error.
3. Compare required obligations with the adapter's advertised surface. Unsupported obligations or missing terminal delivery should produce an explicit error, not a fabricated successful turn.
4. Use the safe collection guidance below before sharing logs.

### Tool return while turn still active

Symptom:

```text
Cannot send a new message: Another request is currently being processed
```

Cause: Adapter or Den posted a continuation before the active turn accepted it, or a duplicate prompt raced the in-flight turn.

Expected behavior: tool results settle against the registered `tool_call_id` for the active turn; new prompts should queue or reject per active-turn policy.

### Invalid tool call IDs

Symptom:

```text
Invalid tool call IDs. Expected '[call_...]', but received '[fs_read_text_file]'
```

Cause: Den or the adapter sent the provider tool name instead of the runtime `tool_call_id` in the tool return.

Expected: tool result payloads reference the original `tool_call_id` from the `tool_request` event.

### Approval JSON shape rejected

Symptom:

```text
Unable to extract tag using discriminator 'type'
```

Cause: Den sent an approval return without the expected structured approval payload.

Verify the BearWire boundary: permission obligations use typed `client.permission.result` decisions; client-tool obligations use `client.tool.result` with the original run/tool-call/obligation identity. See the tests in `services/den/crates/den-bearwire/src/methods/`, not the retired ACP gateway.

### Missing file path

Symptom:

```text
requested fs_read_text_file without a path argument
```

Cause: Model emitted a tool call without a string `path`, or Den parsed the wrong field.

If debug samples show argument fragments, Den should accumulate until valid JSON appears.

---

## 5. Safe raw sample collection

Enable `/debug verbose` for one reproduction (or set `BEAR_DEBUG=verbose` in the adapter environment before starting it), then return to `/debug off`. Collect matching adapter/Den logs and the ACP error data; `/debug` also exposes a focused-execution diagnostic bundle.

Redact tokens, credentials, private content, and local usernames before sharing. Preserve the identifiers needed to correlate state:

- BearWire event type and sequence
- client session ID and canonical conversation ID (opaque, not inferred from prefixes)
- run, obligation, permission, and tool-call IDs
- tool name and execution target
- Work-run/execution-attempt IDs and fence epoch for headless startup
- relevant argument shape and structured RPC error, with secret values removed

---

## 6. Protocol boundaries

Do not confuse these layers:

```text
Editor ⇄ adapter: ACP JSON-RPC over stdio
Adapter ⇄ Den: BearWire v1 JSON-RPC + ordered event pages
Den: in-process native agent loop + Bifrost /v1 streaming
```

Current BearWire event names include:

```text
message.delta
tool_call.requested
tool_call.completed
tool_call.failed
client.waiting
run.completed
run.failed
run.cancelled
run.interrupted
```

These are not raw ACP messages; the armature translates them into ACP `session/update` and client requests. Den session access is an explicit typed projection, and headless Work identity is an explicit `expected_work_source` field—not control text embedded in a transcript. The [JSON design specification](../architecture/bearwire-json-spec.md) remains a draft; check implemented shapes against the protocol crate and [compatibility guide](../../services/den/docs/guides/bearwire-compatibility.md).
