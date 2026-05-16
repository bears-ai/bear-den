# Pair Letta Message Boundary Refactor Plan

## Status

Phase 1 implementation started.

## Problem

The `pair` role's first live channel is ACP, but the issue is not ACP-specific. Den currently assembles turn-local runtime/tool/workflow guidance into the text sent to Letta as a `role=user` message for ACP pair turns. That causes Den-generated instruction text to become part of Letta conversation history as if it were human-authored content.

This has produced several downstream problems:

- User-visible history needs sanitizers to hide `<system-reminder>` and related scaffolding.
- Conversation titles and summaries need defensive stripping.
- Compaction may preserve or summarize runtime scaffolding as conversation content.
- Old workflow/tool guidance can be replayed as stale user-authored text in later turns.
- Stale approval/tool-call recovery becomes harder to reason about because conversation history contains runtime-control text.

The Letta team confirmed the safe stateful-agent pattern:

1. Stable behavior/instructions belong in agent persona/system/memory.
2. Each turn should send only the new user input to the Letta messages endpoint.
3. Tool/runtime surface should use structured fields such as `client_tools` / `client_skills` and tool returns.
4. `override_system` exists for per-request non-persisted instructions, but it is system replacement semantics, not additive developer/runtime context.
5. Runtime context should not be placed in `messages[].content` unless we accept it becoming message history.

## Guiding invariant

For every `pair` channel, not just ACP:

> The persisted Letta `role=user` message must contain only human-authored user input. Den-generated runtime, workflow, tool, memory, or environment instructions must never be appended to `messages[].content`.

ACP may construct channel context, but this invariant belongs in the shared `pair`/Letta send path.

## Non-goals

- Do not introduce Codepool into the ACP `pair` pipeline as part of this refactor.
- Do not make ACP adapter behavior responsible for systemic prompt hygiene.
- Do not add broad workplace-agent persona to compensate for removed per-turn prose. Workplace agents should remain minimally personified and capability/policy driven.
- Do not remove existing history sanitizers immediately; they are still needed for legacy polluted conversations.
- Do not use `override_system` casually as an additive context mechanism.

## Design principles

1. **Clean user content first**
   - Fix the source of polluted history before optimizing model guidance.

2. **Structured context over prose injection**
   - Prefer `client_tools`, tool descriptors, tool returns, Den-enforced policy, and queryable tools over appended reminders.

3. **Stable instructions stay stable**
   - Put durable role behavior into agent config/persona/system prompt or memory, with minimal workplace-agent persona.

4. **Turn-local state must not masquerade as user text**
   - Plan state, permission mode, workspace roots, activity state, and memory-write policy are Den runtime facts.
   - They should be represented structurally or via explicit tools, not appended to a user message.

5. **`override_system` requires complete replacement semantics**
   - If used, Den must construct the complete desired system text for the request.
   - Do not pass only the turn-local addendum as `override_system`.

6. **Legacy cleanup remains defense-in-depth**
   - `sanitize_visible_transcript_text` remains for old histories and unexpected upstream leakage.

## Current problematic flow

ACP currently builds:

```text
prompt_with_tool_context = user_prompt + plan_mode_context + activity_context + tool_prompt_context
```

Then `LettaClient::post_conversation_messages_streaming` sends:

```json
{
  "messages": [
    {
      "role": "user",
      "content": "<user prompt plus Den-generated runtime context>"
    }
  ],
  "streaming": true,
  "stream_tokens": true,
  "agent_id": "agent-...",
  "client_tools": []
}
```

The `messages[0].content` field is persisted by Letta as a user message.

## Target flow

Shared pair send path should send:

```json
{
  "messages": [
    {
      "role": "user",
      "content": "<human-authored user prompt only>"
    }
  ],
  "streaming": true,
  "stream_tokens": true,
  "agent_id": "agent-...",
  "client_tools": []
}
```

Optional future use of `override_system` must follow replacement semantics:

```json
{
  "messages": [
    {
      "role": "user",
      "content": "<human-authored user prompt only>"
    }
  ],
  "override_system": "<complete replacement system prompt, not only turn-local context>"
}
```

## Phase 1: Stop polluting user messages

### 1.1 Extend Letta client request shape

Add optional `override_system` support to `LettaClient::post_conversation_messages_streaming`, but do not use it by default.

Purpose:

- Documents the supported Letta mechanism.
- Keeps Rust client ready for controlled experiments.
- Does not yet risk replacing the agent's compiled system prompt.

### 1.2 Send clean user prompt from ACP pair

In ACP `prompt_inner`:

- Keep building/logging `plan_mode_context`, `activity_context`, and `tool_prompt_context` for now.
- Stop concatenating them into the user message.
- Send only `prompt` to Letta as the user content.
- Continue sending `client_tools` structurally.
- Log context lengths and `runtime_context_sent_as_user_content=false`.

### 1.3 Add tests for the invariant

Add tests around the ACP/Letta request body to assert:

- `messages[0].content` equals the human prompt exactly.
- `messages[0].content` does not contain `<system-reminder>`.
- `client_tools` still appear when expected.
- `override_system` is absent by default.

## Phase 2: Move required guidance to safe homes

Audit each currently appended context block.

### 2.1 `tool_prompt_context`

Likely destinations:

- `client_tools` descriptors and metadata.
- Den policy enforcement.
- Tool errors/returns that explain unavailable actions.
- A queryable `session_info` / runtime-state tool if the model needs current state.

Avoid large prose guidance in the user message.

### 2.2 `plan_mode_context`

Likely destinations:

- Minimal stable pair system instruction that the agent should respect workflow tools/state.
- Workplan/activity tools and descriptors.
- Tool availability and denial messages.
- Optional future complete `override_system` only after testing replacement semantics.

### 2.3 `activity_context`

Likely destinations:

- Queryable activity/workplan tools.
- Initial UI/session events.
- Tool descriptors/returns.

Do not inject as user text.

## Phase 3: Optional `override_system` experiment

Only after Phase 1 proves clean histories and identifies model regressions:

1. Find or construct the full base pair system prompt.
2. Build a complete replacement system string with a bounded turn-local addendum.
3. Use `override_system` behind an env flag or role-runtime feature flag.
4. Test that the persisted user message remains clean.
5. Test that model behavior improves without workplace-agent over-personification.

Caution:

- Workplace agents should remain purpose/capability/policy oriented.
- Avoid rich persona prose unless it is explicitly required.
- Prefer concise operating contracts over personality.

## Phase 4: Remove duplicated display cleanup only when safe

After enough clean-history confidence:

- Keep Rust sanitizers for legacy conversations.
- Reduce browser-side duplicate stripping only if server history is guaranteed clean for new conversations.
- Add diagnostics to identify conversations polluted before the refactor.

## Success criteria

- New ACP pair turns persist only human text as `role=user`.
- No new `<system-reminder>` blocks appear in Letta user-message history.
- ACP pair tool use still works through `client_tools` and Den tool-turn handling.
- Existing polluted conversations remain display-safe via legacy sanitizers.
- Stale approval recovery complexity decreases over time because prompt scaffolding no longer compounds conversation state.

## Open questions

1. Does Letta expose `client_skills` on this endpoint, and should Den use it for pair runtime affordances?
2. Can Den retrieve the compiled/persisted pair system prompt safely if we later test `override_system` replacement?
3. Which currently appended instructions are truly required for model behavior versus only Den/tool enforcement?
4. Should there be a shared `PairTurnRequest` abstraction before or after the initial ACP cleanup?

## Immediate implementation checklist

- [x] Add `override_system` field support to Rust `LettaClient::post_conversation_messages_streaming`.
- [x] Change ACP pair prompt send to use clean `prompt` as Letta user input.
- [x] Preserve `client_tools` in the request.
- [x] Add request-body assertions for clean user message invariant in ACP gateway tests.
- [ ] Run ACP stream tests and Den check.
  - Den check passes.
  - Targeted ACP gateway test currently blocked by local Postgres pool timeout in the test harness.
- [x] Keep history sanitizers unchanged.

## Notes from Letta team

- Letta agents are stateful.
- Do not use them like stateless ChatCompletions by replaying or flattening transcript/system content into per-turn payloads.
- `override_system` is documented as non-persisted but replacement-oriented.
- No documented additive `developer_context` / `runtime_context` field was identified on the public HTTP API.
