# Architecture notes

Single-page view of the BEARS stack on Coolify. **Roadmap and contracts:** [PLAN.md](PLAN.md). **Den + multi-user web:** [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).

## Target architecture

**Cabinet** (**Outline**) is the shared knowledgebase: humans edit in Outline; agents access it through **Den** (Cabinet API, policy). **Letta** keeps **native memory** (blocks, conversations)—Cabinet does not replace that.

```
Open WebUI         Outline (human editing)
    │                      ▲
    ▼                      │
   Den ──Cabinet API───────┘
    │
    ▼
  Letta ──► LiteLLM ──► providers
```

**Until Den is deployed:** Open WebUI talks to Letta directly. Add Den + Outline per [PLAN.md](PLAN.md).

## Components

| Component | Role |
|-----------|------|
| **Den** | Auth, routing to Letta, Cabinet API; **LiteLLM** only for observability (Letta → LiteLLM direct) |
| **Letta** | Agents, tools, memory blocks, conversations |
| **LiteLLM** | Unified model API |
| **Open WebUI** | Primary web chat (optional: LibreChat) |
| **Outline** | Cabinet storage and UI |

## Letta

- Image: `letta/letta:latest`, port `8283`  
- Models via `LLM_API_URL` → LiteLLM  
- Shared knowledge: Cabinet tools (via Den) when deployed  

## Data flow

**Web (target with Den):** User → Open WebUI → Den → Letta → LiteLLM → providers.

**Web (today, no Den):** User → Open WebUI → Letta → LiteLLM → providers.

**LettaBot (Slack/WhatsApp):** **v1** is LettaBot → Letta direct. Optional later: LettaBot → Den → Letta ([PLAN.md](PLAN.md)).

**Cabinet:** Agent tool calls → Den → Outline.

## Ports (internal)

| Service | Port |
|---------|------|
| Open WebUI | 3000 |
| Letta | 8283 |
| LiteLLM | 4000 |

Expose only what users need (e.g. Open WebUI via Coolify proxy).

## Multi-user

[DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) — Den (Axum), self-hosted Letta; **v1** web via Den, LettaBot direct ([PLAN.md](PLAN.md)).

## Next steps

Den + Outline deployment, observability dashboards—see [PLAN.md](PLAN.md).
