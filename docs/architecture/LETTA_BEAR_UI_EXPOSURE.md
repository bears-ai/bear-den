# Letta APIs: what the Den bear UI exposes vs not

Den’s **operator bear forms** and **JSON admin create-bear** API sync a subset of [Letta agent](https://docs.letta.com/api/resources/agents/) settings. This page lists **Letta functionality that is not yet exposed** through those surfaces so operators know when to use the Letta UI, API, or future Den work.

Confirm field names and enums against **your** self-hosted Letta image; see [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).

## Exposed via bears (Den registry → Letta)

| Area | Letta usage |
|------|----------------|
| Create agent | `POST /v1/agents` with `name`, `system`, `model`, optional `agent_type`, optional `tool_ids` |
| Update agent | `PATCH /v1/agents/{id}` with the same logical fields when a bear row is saved and an agent id exists |
| Model picker | `GET /v1/models/` (LLM handles) |
| Tool picker | `GET /v1/tools/` |
| Diagnostics (edit page) | `GET /v1/agents/{id}` for read-only block/tool summaries and raw JSON |

Operator edits are intended to **sync to Letta automatically**; failures are shown on the form when Letta rejects a `PATCH`.

The `bears.tools_enabled` JSON column remains for **legacy / admin API** use; new HTML flows store **`letta_tool_ids`** instead of the old Tools JSON textarea.

## Not exposed in the bear UI (use Letta or extend Den)

These come from the public [Agents API](https://docs.letta.com/api/resources/agents/) and related resources; they are **not** wired through Den’s bear create/edit forms today:

- **Template-based provisioning** — e.g. `POST /v1/templates/{template_version}/agents` with `memory_variables`, `tool_variables`, `identity_ids`, `initial_message_sequence`, etc.
- **Initial `memory_blocks` on create** and **`block_ids`** attachment UI — Den does not author Letta memory blocks from the bear form (only displays block labels on the edit page when Letta returns them).
- **`embedding`** model handle, **`embedding_chunk_size`** (if applicable to your version).
- **`compaction_settings`** and **`context_window_limit`**.
- **`model_settings`** / per-model generation knobs (temperature, max tokens, parallel tool calls, response format, etc.).
- **`enable_sleeptime`**, **`message_buffer_autoclear`**, **`hidden`**, **`folder_ids`**, **`identity_ids`**, **`metadata`**.
- **`include_base_tools`** and **`include_base_tool_rules`** toggles.
- **Per-block edits** — e.g. `PATCH /v1/agents/{agent_id}/core-memory/blocks/{block_label}` and other block subroutes.
- **Tool rules**, **secrets**, **tool execution environment** fields where Letta exposes them on agent state.
- **Agent listing / deletion / archive** beyond what Den needs for a single linked agent id.

For the authoritative request/response shapes, use the [Letta API reference](https://docs.letta.com/api-reference/agents/create) and [PATCH agent](https://docs.letta.com/api-reference/agents/modify) pages for your deployment version.
