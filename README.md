# 🐻 Basic Environment for Agents Runtime Server (BEARS)

Each assistant in the product is a **bear** (one **Letta** agent). **BEARS** names the **stack**; **users↔bears** is many‑to‑many, with **Den** provisioning bears and clients—see [PLAN.md](PLAN.md).

**Configuration repository** for deploying that stack on **Coolify**:

- **[Letta](https://github.com/letta-ai/letta)** — **Bear** runtime, native memory (blocks, conversations, tools)  
- **[Open WebUI](https://github.com/open-webui/open-webui)** (open-webui) — Chat UI  
- **[Outline](https://www.getoutline.com/)** + **Den** (Rust/Axum) — Control plane + **Cabinet**; see [PLAN.md](PLAN.md). **Self-hosted Letta only** (no Letta Cloud).  
- **[LiteLLM](https://github.com/BerriAI/litellm)** — Model gateway  
- **[Coolify](https://coolify.io)** — Deployment  

## Documentation map

| I want to… | Read |
|------------|------|
| **Deploy on Coolify** | [DEPLOYMENT.md](DEPLOYMENT.md), then `services/*/COOLIFY_DEPLOY.md` |
| **Understand the stack** | This file + [ARCHITECTURE_NOTES.md](ARCHITECTURE_NOTES.md) |
| **Roadmap & phases** (Den, Cabinet, Outline) | [PLAN.md](PLAN.md) |
| **Multi-user web** (Den + Letta) | [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) |
| **Phase 1 build (Den; Twain = M0 bootstrap only)** | [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) |
| **Open WebUI ↔ Letta** (direct) | [services/letta/OPENWEBUI_INTEGRATION.md](services/letta/OPENWEBUI_INTEGRATION.md), [OPENWEBUI_SESSIONS.md](services/letta/OPENWEBUI_SESSIONS.md) |

*Tooling-oriented notes under `.kilocode/memory_bank/` use the same **bear** vocabulary; they are for assistants, not end-user docs.*

## Architecture

| Piece | Role |
|-------|------|
| **Letta memory** | Per‑**bear** context (blocks, conversations)—not replaced by Cabinet |
| **Cabinet (Outline)** | Long-lived docs; people edit in Outline, **bears** use tools via **Den** |
| **Den** | Control plane: **bear** lifecycle (Letta + Open WebUI + LettaBot), **users↔bears** membership, identity, routing, policy, Cabinet API; **LiteLLM only for observability** ([PLAN.md](PLAN.md)) |

## Quick start (Coolify)

1. Deploy **LiteLLM** → **Letta** (`LLM_API_URL`) → **Open WebUI**  
2. Install [open-webui-tools](https://github.com/Haervwe/open-webui-tools) in Open WebUI for **bears** (Letta agents)  
3. Roll out **Outline** and **Den** per [PLAN.md](PLAN.md) for Cabinet and channels  

**Guide:** [DEPLOYMENT.md](DEPLOYMENT.md)

### Internal endpoints (typical)

- Open WebUI (`bears-openwebui`): `http://bears-openwebui:3000`  
- Letta: `http://bears-letta:8283`  
- LiteLLM: `http://bears-litellm:4000`  

## Repository layout

```
services/
├── den/             # Phase 1 control plane (Axum); see PHASE1_BOOTSTRAP.md
├── litellm/
├── letta/
├── librechat/       # optional UI
└── openwebui/
PHASE1_BOOTSTRAP.md
PLAN.md
DEPLOYMENT.md
ARCHITECTURE_NOTES.md
DEN_ARCHITECTURE.md
```

## Environment variables

Per-service `.env.example` files. Common: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `LETTA_SERVER_PASS`, `LLM_API_URL`, `LITELLM_MASTER_KEY`.

## Open WebUI + Letta

Direct integration via [open-webui-tools](https://github.com/Haervwe/open-webui-tools). For production multi-user identity and **Den**, see [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md).

## License

Add a `LICENSE` file to the repository root when you publish or distribute this configuration.
