# Architecture notes

Single-page view of the BEARS stack on Coolify. **Roadmap and contracts:** [PLAN.md](../planning/PLAN.md). **Den + multi-user web:** [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).

## Target architecture

**Cabinet** (**Outline**) is the shared knowledgebase: humans edit in Outline; **bears** (Letta agents) access it through **Den** (Cabinet API, policy). **Letta** keeps **native memory** (blocks, conversations) per bear—Cabinet does not replace that. **Bear** = one agent; **BEARS** = the stack. See [PLAN.md](../planning/PLAN.md) terminology.

```
Den (chat UI) ────┐     Outline (human editing)
Open WebUI (opt.) ┤              ▲
                  ├──► Den ──Cabinet API───────┘
                  │      │──────────────► Garage (S3)
                  │      └──► LettaBot ──► Letta ──► Bifrost ──► providers
```
(Den serves the first-party browser chat UI; **Open WebUI** is optional; **LettaBot** is required for agent chat — [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).)

**Until Den is deployed:** Open WebUI may talk to Letta directly. Add Den + Outline per [PLAN.md](../planning/PLAN.md).

## Components

| Component | Role |
|-----------|------|
| **Den** | **Operator console** (browser: users, bears, Letta provision, **skills per bear**, LettaBot yaml); **bear** provisioning on Letta + **LettaBot** config + **skill materialization**; **users↔bears** membership; auth; **web** routing **Den → LettaBot**; first-party chat UI; Cabinet API; **Bifrost** only for observability (Letta → Bifrost direct) |
| **LettaBot** | **Agent runtime** for web (via Den) and channels; uses **Letta** for persistence; loads [skills](https://docs.letta.com/letta-code/skills/) from paths Den manages |
| **Letta** | **Persistence** for LettaBot: tools, memory blocks, conversations per Letta agent (**bear**) |
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

**Web (target with Den):** User → Den chat page **(default)** **or** optional **Open WebUI** → **Den → LettaBot → Letta** → Bifrost → providers.

**Web (today, no Den):** User → Open WebUI → Letta → Bifrost → providers.

**LettaBot (Slack/WhatsApp and agent runtime):** Channels → **LettaBot → Letta**; web → **Den → LettaBot → Letta**. Den drives **which bears**, **which skills**, and **LettaBot** config. Optional later: channel-only Den proxy for audit ([PLAN.md](../planning/PLAN.md)).

**Cabinet:** Bear tool calls → Den → Outline.

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

[DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) — Den (Axum), self-hosted Letta; **v1** web via Den, LettaBot direct ([PLAN.md](../planning/PLAN.md)).

## Next steps

Den + Outline deployment, observability dashboards—see [PLAN.md](../planning/PLAN.md).
