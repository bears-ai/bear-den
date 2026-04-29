# Architecture notes

Single-page view of the BEARS stack on Coolify. **Roadmap and contracts:** [PLAN.md](../planning/PLAN.md). **Den + multi-user web:** [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).

## Target architecture

**Cabinet** (**Outline**) is the shared knowledgebase: humans edit in Outline; **bears** (Letta agents) access it through **Den** (Cabinet API, policy). **Letta** keeps **native memory** (blocks, conversations) per bear—Cabinet does not replace that. **Bear** = one agent; **BEARS** = the stack. See [PLAN.md](../planning/PLAN.md) terminology.

Three layers (see [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md)): **persistence (Letta)** → **harness (Letta Code)** → **control plane (Den)** for operations; web and Slack sit on the harness.

```
Den (chat UI) ───────────────┐     Outline (human editing)
                             ├──► Den ──Cabinet API───────┘
                             │      │──────────────► Garage (S3)
                             │      └──► Letta Code ──► Letta ──► Bifrost ──► providers
```
(Den serves the first-party browser chat UI (Deep Chat); **Letta Code** is the mandatory harness for agent chat — [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).)

**Until Den is deployed:** you can exercise Letta + Bifrost directly; add Den + Outline per [PLAN.md](../planning/PLAN.md).

## Components

| Component | Role |
|-----------|------|
| **Den** | **Operator console** (browser: users, bears, Letta provision, **skills and MCP servers per bear**, harness deploy config); **bear** provisioning on Letta + **Letta Code** config + **skill and MCP materialization**; **local MCP catalog** and per-bear attachments (Phase 1); **users↔bears** membership; auth; **web** routing **Den → Letta Code**; first-party chat UI; **[Den meta tools](DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools)** (**control-plane tool definitions and policy in Den**; **Letta Code** brokers execution; **no** ad hoc tool code in Letta for these; MCP remains for optional third-party servers); Cabinet API; **Bifrost** for **observability on the bear model path** (Letta → Bifrost direct for chat); future Den-side LLM usage may differ ([PLAN.md](../planning/PLAN.md) §2.5) |
| **Letta Code** | **[Harness](https://docs.letta.com/letta-code)** for web (via Den) and **Slack** ([Channels](https://docs.letta.com/letta-code/channels/), beta); uses **Letta** for persistence; loads [skills](https://docs.letta.com/letta-code/skills/) from paths Den manages; **brokers** [Den meta tools](DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools) (Den APIs) to agents. **WhatsApp** is desired but not in Letta Code Channels yet. |
| **Letta** | **Persistence** for the harness: tools, memory blocks, conversations per Letta agent (**bear**) |
| **Bifrost** | Unified OpenAI-compatible model gateway (`/v1`) — see `services/bifrost/` |
| **Den chat UI** | **Only** first-party web chat — Deep Chat web component served by Den; reference client for Den streaming APIs |
| **Garage** | S3-compatible object storage — **artifacts** bucket (agent outputs, uploads, routines; **not** in Letta) + **separate** Cabinet bucket (Outline); GC on artifacts — [artifacts-garage.md](adr/artifacts-garage.md), [`services/garage/`](../../services/garage/) |
| **Outline** | Cabinet storage and UI |

## Letta

- Image: `letta/letta:latest`, port `8283`  
- Models via `LLM_API_URL` → Bifrost  
- Shared knowledge: Cabinet tools (via Den) when deployed  
- **Shared memory blocks:** multiple concurrent writers can race on the same block; compiled context can be stale across conversations until recompile—see [PLAN.md § Shared memory blocks and concurrency](../planning/PLAN.md#shared-memory-blocks-and-concurrency-letta).

## Data flow

**Web (target with Den):** User → Den chat page → **Den → Letta Code → Letta** → Bifrost → providers.

**Web (no Den yet):** exercise Letta + Bifrost directly (e.g. Letta UI on `:8283`); first-party multi-user web chat lands with Den.

**Slack + harness:** **Slack** → **Letta Code** ([Channels](https://docs.letta.com/letta-code/channels/)) → **Letta**; **web** → **Den → Letta Code → Letta**. Den drives **which bears**, **which skills**, **which MCP servers**, and harness config. **WhatsApp:** not in Letta Code Channels yet—roadmap. Optional later: channel-only Den proxy for audit ([PLAN.md](../planning/PLAN.md)).

**Cabinet:** Bear tool calls → **Letta Code** → **Den** → Outline. **Architecture:** agent-facing Cabinet operations use the **[Den meta tools](DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools)** pattern (Den APIs + Letta Code broker), not a separate MCP layer by default.

## Ports (internal)

| Service | Port |
|---------|------|
| Den (HTTP) | app-defined (e.g. 3000 behind Coolify) |
| Garage S3 API | 3900 |
| Garage Admin | 3903 |
| Letta | 8283 |
| Bifrost | 8080 |

Expose only what users need (e.g. Den behind Coolify proxy).

## Object storage (Garage)

[Garage](https://garagehq.deuxfleurs.fr/) is the S3-compatible store for **files that must not live in Letta** (or large blobs in Postgres). Den issues presigned S3 URLs — browsers upload/download directly to Garage; Den never proxies the bytes.

- **`bears-artifacts`** — Ephemeral **artifacts**: agent/tool/skill outputs, **human uploads**, routine file outputs; **metadata** (e.g. `conversation_id`, provenance); **garbage collection** by Den policy. Not Cabinet.
- **`bears-cabinet`** — **Cabinet / Outline** attachments only (Phase 2+); **no** artifact GC rules from Den; optional **promote artifact → Cabinet** UX later.
- **Auth:** scoped service keys (Den for artifacts; Outline/Cabinet wiring separately).
- **Backup:** Garage volumes remain part of the three-input contract (repo + DB backups + object storage).

Architecture: [artifacts-garage.md](adr/artifacts-garage.md). Deploy: [`services/garage/COOLIFY_DEPLOY.md`](../../services/garage/COOLIFY_DEPLOY.md).

## Multi-user

[DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) — Den (Axum), self-hosted Letta; **v1** web via Den, **Letta Code** harness ([PLAN.md](../planning/PLAN.md)).

## Next steps

Den + Outline deployment, observability dashboards—see [PLAN.md](../planning/PLAN.md). **Phase 1** includes the **MCP local catalog** (Coolify for server processes, Den for catalog and bear attachments)—[DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) (Den-managed MCP servers). **Phase 2** adds Cabinet tools—[PLAN.md §3 Phase 2](../planning/PLAN.md#phase-2--introduce-cabinet-as-an-abstract-service-outline-still-in-background).
