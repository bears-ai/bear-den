# 🐻 Basic Environment for Agents Runtime Server (BEARS)

**Configuration repository** for deploying an agent platform on **Coolify**:

- **[Letta](https://github.com/letta-ai/letta)** — Agent runtime, native memory (blocks, conversations, tools)  
- **[Open WebUI](https://github.com/open-webui/open-webui)** — Chat UI  
- **[Outline](https://www.getoutline.com/)** + **Den** (Rust/Axum) — Control plane + **Cabinet**; see [PLAN.md](PLAN.md). **Self-hosted Letta only** (no Letta Cloud).  
- **[LiteLLM](https://github.com/BerriAI/litellm)** — Model gateway  
- **[Coolify](https://coolify.io)** — Deployment  

## Architecture

| Piece | Role |
|-------|------|
| **Letta memory** | Per-agent context (blocks, conversations)—not replaced by Cabinet |
| **Cabinet (Outline)** | Long-lived docs; people edit in Outline, agents use tools via **Den** |
| **Den** | Control plane: identity, routing, policy, Cabinet API; **LiteLLM only for observability** ([PLAN.md](PLAN.md)) |

## Quick start (Coolify)

1. Deploy **LiteLLM** → **Letta** (`LLM_API_URL`) → **OpenWebUI**  
2. Install [open-webui-tools](https://github.com/Haervwe/open-webui-tools) in OpenWebUI for Letta agents  
3. Roll out **Outline** and **Den** per [PLAN.md](PLAN.md) for Cabinet and channels  

**Guide:** [DEPLOYMENT.md](DEPLOYMENT.md)

### Internal endpoints (typical)

- OpenWebUI: `http://bears-openwebui:3000`  
- Letta: `http://bears-letta:8283`  
- LiteLLM: `http://bears-litellm:4000`  

## Repository layout

```
services/
├── litellm/
├── letta/
├── librechat/       # optional UI
└── openwebui/
PLAN.md
DEPLOYMENT.md
ARCHITECTURE_NOTES.md
MULTIUSER_PROXY_ARCHITECTURE.md
```

## Environment variables

Per-service `.env.example` files. Common: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `LETTA_SERVER_PASS`, `LLM_API_URL`, `LITELLM_MASTER_KEY`.

## OpenWebUI + Letta

Direct integration via open-webui-tools. For production multi-user identity and **Den**, see [MULTIUSER_PROXY_ARCHITECTURE.md](MULTIUSER_PROXY_ARCHITECTURE.md).

## Docs

- [DEPLOYMENT.md](DEPLOYMENT.md)  
- [PLAN.md](PLAN.md) — Den, Cabinet, phases  
- [ARCHITECTURE_NOTES.md](ARCHITECTURE_NOTES.md)  
- [MULTIUSER_PROXY_ARCHITECTURE.md](MULTIUSER_PROXY_ARCHITECTURE.md)  
- `services/*/COOLIFY_DEPLOY.md`  

## License

[Add your license here]
