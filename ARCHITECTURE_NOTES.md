# Architecture Notes

## BEARS stack

Coolify-hosted services on internal Docker networking.

### Target architecture

**Cabinet** ( **Outline** ) is the shared knowledgebase: humans edit in Outline; agents access it through **Den** (Cabinet API, policy). **Letta** keeps **native memory** (blocks, conversations)—Cabinet does not replace that.

```
OpenWebUI          Outline (human editing)
    │                      ▲
    ▼                      │
   Den ──Cabinet API───────┘
    │
    ▼
  Letta ──► LiteLLM ──► providers
```

Until Den is deployed, OpenWebUI talks to Letta directly; add Den + Outline per [PLAN.md](PLAN.md).

### Components

| Component | Role |
|-----------|------|
| **Den** | Auth, routing to Letta, Cabinet API; **LiteLLM** only for observability (Letta → LiteLLM direct) |
| **Letta** | Agents, tools, memory blocks, conversations |
| **LiteLLM** | Unified model API |
| **OpenWebUI** | Primary web chat (optional: LibreChat) |
| **Outline** | Cabinet storage and UI |

### Letta

- Image: `letta/letta:latest`, port `8283`  
- Models via `LLM_API_URL` → LiteLLM  
- Shared knowledge: Cabinet tools (via Den) when deployed  

### Data flow

1. User → OpenWebUI (or LettaBot → Den) → Letta  
2. Letta → LiteLLM → providers  
3. Cabinet tool calls → Den → Outline  

### Ports (internal)

| Service | Port |
|---------|------|
| OpenWebUI | 3000 |
| Letta | 8283 |
| LiteLLM | 4000 |

Expose only what users need (e.g. OpenWebUI via Coolify proxy).

### Multi-user

[DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) — Den (Axum), self-hosted Letta; **v1** web via Den, LettaBot direct ([PLAN.md](PLAN.md)).

### Future

Den + Outline deployment, observability dashboards—see [PLAN.md](PLAN.md).
