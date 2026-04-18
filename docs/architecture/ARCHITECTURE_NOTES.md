# Architecture notes

Single-page view of the BEARS stack on Coolify. **Roadmap and contracts:** [PLAN.md](../planning/PLAN.md). **Den + multi-user web:** [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).

## Target architecture

**Cabinet** (**Outline**) is the shared knowledgebase: humans edit in Outline; **bears** (Letta agents) access it through **Den** (Cabinet API, policy). **Letta** keeps **native memory** (blocks, conversations) per bear—Cabinet does not replace that. **Bear** = one agent; **BEARS** = the stack. See [PLAN.md](../planning/PLAN.md) terminology.

Three layers (see [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md)): **persistence (Letta)** → **harness (Letta Code)** → **control plane (Den)** for operations; web and Slack sit on the harness.

```
Den (chat UI) ────┐     Outline (human editing)
Open WebUI (opt.) ┤              ▲
                  ├──► Den ──Cabinet API───────┘
                  │      │──────────────► Garage (S3)
                  │      └──► Letta Code ──► Letta ──► Bifrost ──► providers
```
(Den serves the first-party browser chat UI; **Open WebUI** is optional; **Letta Code** is the mandatory harness for agent chat — [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).)

**Until Den is deployed:** Open WebUI may talk to Letta directly. Add Den + Outline per [PLAN.md](../planning/PLAN.md).

## Components

| Component | Role |
|-----------|------|
| **Den** | **Operator console** (browser: users, bears, Letta provision, **skills and MCP servers per bear**, harness deploy config); **bear** provisioning on Letta + **Letta Code** config + **skill and MCP materialization**; **local MCP catalog** and per-bear attachments (Phase 1); **users↔bears** membership; auth; **web** routing **Den → Letta Code**; first-party chat UI; **[Den meta tools](DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools)** (**control-plane tool definitions and policy in Den**; **Letta Code** brokers execution; **no** ad hoc tool code in Letta for these; MCP remains for optional third-party servers); Cabinet API; **Bifrost** for **observability on the bear model path** (Letta → Bifrost direct for chat); future Den-side LLM usage may differ ([PLAN.md](../planning/PLAN.md) §2.5) |
| **Letta Code** | **[Harness](https://docs.letta.com/letta-code)** for web (via Den) and **Slack** ([Channels](https://docs.letta.com/letta-code/channels/), beta); uses **Letta** for persistence; loads [skills](https://docs.letta.com/letta-code/skills/) from paths Den manages; **brokers** [Den meta tools](DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools) (Den APIs) to agents. **WhatsApp** is desired but not in Letta Code Channels yet. |
| **Letta** | **Persistence** for the harness: tools, memory blocks, conversations per Letta agent (**bear**) |
| **Bifrost** | Unified OpenAI-compatible model gateway (`/v1`) — see `services/bifrost/` |
| **Den chat UI** | **Primary** first-party chat UI — Deep Chat web component served by Den; reference client for Den streaming APIs |
| **Open WebUI** | Full-featured web chat **optional** when deployed (e.g. LibreChat) |
| **Garage** | S3-compatible object storage — chat media uploads, generated images, binary artifacts ([`services/garage/`](../../services/garage/)) |
| **Outline** | Cabinet storage and UI |

## Letta

- Image: `letta/letta:latest`, port `8283`  
- Models via `LLM_API_URL` → Bifrost  
- Shared knowledge: Cabinet tools (via Den) when deployed  

## Data flow

**Web (target with Den):** User → Den chat page **(default)** **or** optional **Open WebUI** → **Den → Letta Code → Letta** → Bifrost → providers.

**Web (today, no Den):** User → Open WebUI → Letta → Bifrost → providers.

**Slack + harness:** **Slack** → **Letta Code** ([Channels](https://docs.letta.com/letta-code/channels/)) → **Letta**; **web** → **Den → Letta Code → Letta**. Den drives **which bears**, **which skills**, **which MCP servers**, and harness config. **WhatsApp:** not in Letta Code Channels yet—roadmap. Optional later: channel-only Den proxy for audit ([PLAN.md](../planning/PLAN.md)).

**Cabinet:** Bear tool calls → **Letta Code** → **Den** → Outline. **Architecture:** agent-facing Cabinet operations use the **[Den meta tools](DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools)** pattern (Den APIs + Letta Code broker), not a separate MCP layer by default.

## Ports (internal)

| Service | Port |
|---------|------|
| Open WebUI | 3000 |
| Garage S3 API | 3900 |
| Garage Admin | 3903 |
| Letta | 8283 |
| Bifrost | 8080 |

Expose only what users need (e.g. Open WebUI via Coolify proxy).

## Object storage (Garage)

[Garage](https://garagehq.deuxfleurs.fr/) is the S3-compatible store for binary media that should not live in PostgreSQL or Letta's context window. Den issues presigned S3 URLs — browsers upload/download directly to Garage; Den never proxies the bytes.

- **Bucket:** `bears-media` (chat attachments, generated images)
- **Auth:** scoped service key per Den instance (not root credentials)
- **Backup:** Garage data volumes are part of the three-input contract (repo + DB backups + object storage)

Deploy guide: [`services/garage/COOLIFY_DEPLOY.md`](../../services/garage/COOLIFY_DEPLOY.md).

## Multi-user

[DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) — Den (Axum), self-hosted Letta; **v1** web via Den, **Letta Code** harness ([PLAN.md](../planning/PLAN.md)).

## Next steps

Den + Outline deployment, observability dashboards—see [PLAN.md](../planning/PLAN.md). **Phase 1** includes the **MCP local catalog** (Coolify for server processes, Den for catalog and bear attachments)—[DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) (Den-managed MCP servers). **Phase 2** adds Cabinet tools—[PLAN.md §3 Phase 2](../planning/PLAN.md#phase-2--introduce-cabinet-as-an-abstract-service-outline-still-in-background).
