# `bear_channel` Phase 7+ Plans

For the canonical role model and current role names, see [bear roles](../architecture/bear-roles.md).
This document captures planned work after the initial `bear_channel` migration. These items are intentionally **not implemented** in the phase that introduced `bear_channel` for Den web chat.

See also: [`../architecture/BEAR_CHANNEL_AND_ACP.md`](../architecture/BEAR_CHANNEL_AND_ACP.md).

## Current status

This document remains active for broader `bear_channel` follow-up work, but its ACP sections are now historical context rather than the canonical `pair` runtime plan.

ACP is in place and effectively finished as a product/runtime path, with ongoing refinement captured elsewhere. The correct ACP architecture for `pair` is the direct Den ⇄ adapter ⇄ Letta conversation API route described in [`ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md`](ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md), with follow-up hardening in [`ACP_ADAPTER_IMPROVEMENT_PLAN.md`](ACP_ADAPTER_IMPROVEMENT_PLAN.md).

What remains relevant here:

- `bear_channel` follow-up for Den web chat and other Codepool-backed runtime surfaces;
- capability context and server-tool management over `bear_channel` for those surfaces;
- richer runtime events and UI surfacing on the `bear_channel` path;
- legacy endpoint deprecation and compatibility planning for Codepool-backed clients.

What is no longer canonical here:

- ACP basic chat via `bear_channel` as the target architecture for `pair`;
- ACP client-tool relay via Codepool as the next implementation slice.

### Phase 7 entry checklist

Before deep implementation:

1. Get `./scripts/smoke.sh` green, or document a known local networking limitation and verify service health another way.
2. Keep unrelated documentation and editor-state changes out of the Phase 7 implementation branch.
3. Confirm ACP transport and schemas against the clients we want to support first.
4. Add tests around Den -> Codepool `bear_channel` request construction and browser SSE compatibility before adding ACP adapters.

### Recommended vertical slices

1. **Tool descriptor and capability registry**
   - Define Den-owned descriptors for all BEARS capabilities.
   - Include execution location: `server`, `client`, `remote_mcp`, or `subagent`.
   - Keep BEARS server tools such as Cabinet and memory behind Den policy; do not expose them as ACP client tools by default.
   - Pass only allowed client tool descriptors into `bear_channel.capabilities.client_tools`.

2. **ACP session bindings — implemented baseline**
   - Durable ACP session behavior is captured in the [ACP Session Bindings ADR](../architecture/adr/acp-session-bindings.md).
   - ACP-created sessions route later prompts through Den's stored `resolved_conversation_id` when available.
   - Den exposes ACP Code-token session/conversation list and history endpoints for adapters.
   - The adapter supports `session/list`, `session/resume`, `session/load`, `session/cancel`, and `session/close`.

3. **Server tool management over `bear_channel`**
   - Add Den-provided runtime capability context for Codepool.
   - Enable Cabinet/memory tools as Den-controlled server capabilities.
   - Route server tool execution through Den APIs so authorization, audit, and policy remain centralized.

4. **Richer runtime events and UI surfacing**
   - Surface server tool, client tool, subagent, memory, and artifact events to clients that advertise support.
   - Keep the Den web chat readable with an activity strip or collapsible timeline.

### ACP adapter distribution path — backlog

`bears-acp-adapter` should be distributed as a standalone CLI, not a full desktop app. Packaging/distribution remains relevant even though ACP itself is already in place; this is packaging/onboarding follow-up, not a blocker for the direct ACP runtime.

Backlog notes:

- Align GitHub release artifacts with npm installer expectations (`.tar.gz` / `.zip` vs raw binaries).
- Add or remove platform targets so workflow, npm platform map, and Homebrew formula match.
- Fill Homebrew `sha256` values from release output.
- Decide when macOS Developer ID signing and notarization are required for non-developer users.

1. **Source/dev install first**
   - Keep adapter source under `tools/bears-acp-adapter/`.
   - Developers build locally with Cargo and configure Zed as a custom ACP agent.
   - Token is provided via environment variable (`DEN_TOKEN`) or `--token-env`.

2. **Prebuilt CLI releases**
   - Publish GitHub release artifacts for at least:
     - `aarch64-apple-darwin` (macOS Apple Silicon)
     - `x86_64-unknown-linux-gnu` or `x86_64-unknown-linux-musl`
     - `x86_64-pc-windows-msvc.exe`
   - Add Linux ARM64 when demand appears.

3. **Optional npm wrapper**
   - Provide an npm package that downloads/runs the platform binary, matching the market pattern used by some ACP adapters.
   - This enables `npx`-style usage without requiring users to build from source.

4. **ACP Registry submission**
   - Once authentication, install metadata, and basic chat are stable, submit a registry entry so ACP clients can discover/install BEARS automatically.

5. **Editor extension/onboarding later**
   - Optional Zed/other editor extension can manage install, token setup, and adapter updates.
   - This should remain a packaging/onboarding layer over the same CLI adapter.

## 1. BEARS skills and Cabinet over `bear_channel`

Goal: BEARS catalog skills and Cabinet tools are active for Codepool-backed channels that use a bear, especially Den web chat and any future non-ACP Codepool-backed clients.

### Scope

- Define a central Den capability registry.
- Represent Cabinet tools as BEARS capabilities, not client-specific MCP-only tools.
- Load bear-assigned skills from the BEARS catalog into `bear_channel` runtime context.
- Resolve skill capability requirements against available server and client tools.

### Initial server tools

- `cabinet_search`
- `cabinet_read`
- `search_memory`
- `remember_memory`

### Design tasks

1. Define skill catalog schema: id, description, instructions, required capabilities, optional capabilities, memory/reflection policy.
2. Define tool descriptors with execution location: `server`, `client`, `remote_mcp`, `subagent`.
3. Add Den endpoint or bundle for Codepool to fetch trusted bear runtime capability context.
4. Decide whether Codepool executes Den tools directly through Den APIs or whether Den remains in the tool execution path.
5. Add audit records for Cabinet and memory tool usage.

## 2. Reflection and sub-agent event model

Goal: coding and product sessions can spawn reflection sub-agents that learn from the session and update relevant memory or Cabinet entries under policy.

### Scope

- Event stream includes sub-agent lifecycle events.
- Session transcript/tool-result excerpts are available for reflection.
- Reflection runs server-side and does not require local client tools in the first version.
- Memory writes are explicit/audited.

### Event types

- `subagent_started`
- `subagent_finished`
- `memory_update_proposed`
- `memory_update_recorded`
- `memory_update_rejected`

### Design tasks

1. Define reflection policy per bear and per skill.
2. Define transcript/event-log retention for reflection.
3. Decide when reflection is triggered: after significant turns, explicit request, skill completion, or session end.
4. Add a background job model for reflection sub-agents.
5. Add memory-write approval policy: automatic, proposal, explicit-only, or disabled.

## 3. Legacy endpoint deprecation

Goal: move Den and first-party clients to `bear_channel` while keeping compatibility for OpenWebUI and other generic clients.

### Current legacy endpoints

- Codepool `POST /v1/conversations/:conversationId/messages`
- Codepool `POST /v1/chat/completions`

### Plan

1. Keep endpoints active while Den web chat uses `bear_channel`.
2. Add metrics comparing legacy and `bear_channel` usage.
3. Document `bear_channel` as the canonical Den -> Codepool path.
4. Retain OpenAI-compatible endpoint as an edge compatibility adapter.
5. Deprecate only the Den-specific legacy conversation endpoint after all first-party traffic has migrated.

### Removal criteria

- Den web chat uses `bear_channel` in production for at least one release cycle.
- No first-party coding client path still depends on ACP-over-`bear_channel`.
- No first-party service depends on `/v1/conversations/:conversationId/messages`.
- Runbook exists for third-party compatibility users.

## 4. Rich web UI event surfacing

Goal: Den web chat surfaces BEARS runtime activity without breaking normal chat readability.

### Candidate UI events

- skill selected
- Cabinet search started/finished
- memory search/write
- sub-agent started/finished
- artifact created
- reflection completed

### Design tasks

1. Decide which events appear inline, in a collapsible activity panel, or only in debug mode.
2. Extend Deep Chat rendering or surrounding Den UI to show activity events.
3. Preserve accessible status text for screen readers.
4. Add event filtering by channel/client capability.
5. Add design fixtures under `/design/chat` and keep real chat layout synchronized.

### Initial recommendation

Start with a small activity strip or collapsible timeline above the composer. Keep normal assistant/user transcript uncluttered.
