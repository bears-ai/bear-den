# 🐻 Basic Environment for Agents Runtime Server (BEARS)

Each assistant in the product is a **bear** (one **Letta** agent). **BEARS** names the **stack**; **users↔bears** is many‑to‑many, with **Den** provisioning bears and clients—see [docs/planning/PLAN.md](docs/planning/PLAN.md).

**Light monorepo:** this repository holds **documentation** under [`docs/`](docs/README.md), **Coolify-oriented configs** under `services/`, and the **Den** control plane as a **Rust** codebase at repo root in **`den/`** (Phase 1 bootstrap: [docs/planning/PHASE1_BOOTSTRAP.md](docs/planning/PHASE1_BOOTSTRAP.md)).

**Configuration repository** for deploying that stack on **Coolify**:

- **[Letta](https://github.com/letta-ai/letta)** — **Bear** runtime, native memory (blocks, conversations, tools)
- **[Open WebUI](https://github.com/open-webui/open-webui)** (open-webui) — Chat UI
- **Den chat UI** — First-party browser chat (Deep Chat web component, same Den APIs as Open WebUI); see [docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md)
- **[Outline](https://www.getoutline.com/)** + **Den** (Rust/Axum) — Control plane + **Cabinet**; see [docs/planning/PLAN.md](docs/planning/PLAN.md). **Self-hosted Letta only** (no Letta Cloud).
- **[Bifrost](https://github.com/maximhq/bifrost)** — Model gateway (OpenAI-compatible `/v1`; file-based config in `services/bifrost/`)
- **[Coolify](https://coolify.io)** — Deployment

**Repository layout, cloning, and sparse checkout:** [docs/README.md](docs/README.md). **Notes for coding agents:** [AGENTS.md](AGENTS.md).

## Documentation map

| I want to… | Read |
|------------|------|
| **Deploy on Coolify** | [docs/deployment/DEPLOYMENT.md](docs/deployment/DEPLOYMENT.md), then `services/*/COOLIFY_DEPLOY.md` |
| **Understand the stack** | This file + [docs/architecture/ARCHITECTURE_NOTES.md](docs/architecture/ARCHITECTURE_NOTES.md) |
| **Roadmap & phases** (Den, Cabinet, Outline) | [docs/planning/PLAN.md](docs/planning/PLAN.md) |
| **Multi-user web** (Den + Letta; Den chat UI or Open WebUI) | [docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md) |
| **Phase 1 build** (Den; operator console first; Trestle = M0 bootstrap only) | [docs/planning/PHASE1_BOOTSTRAP.md](docs/planning/PHASE1_BOOTSTRAP.md) |
| **Open WebUI ↔ Letta** (direct) | [services/letta/OPENWEBUI_INTEGRATION.md](services/letta/OPENWEBUI_INTEGRATION.md), [OPENWEBUI_SESSIONS.md](services/letta/OPENWEBUI_SESSIONS.md) |
| **Full doc index** | [docs/README.md](docs/README.md) |

*Tooling-oriented notes under `.kilocode/memory_bank/` use the same **bear** vocabulary; they are for assistants, not end-user docs.*

## Architecture

| Piece | Role |
|-------|------|
| **Letta memory** | Per‑**bear** context (blocks, conversations)—not replaced by Cabinet |
| **Cabinet (Outline)** | Long-lived docs; people edit in Outline, **bears** use tools via **Den** |
| **Den** | Control plane: **bear** lifecycle (Letta + Open WebUI + **Letta Code** harness), **users↔bears** membership, identity, routing, policy, Cabinet API; first-party chat UI **served from Den** ([docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md)); **Bifrost only for observability** ([docs/planning/PLAN.md](docs/planning/PLAN.md)) |

## Quick start (Coolify)

1. Deploy **Bifrost** → **Letta** (`LLM_API_URL`) → **Open WebUI**
2. Install [open-webui-tools](https://github.com/Haervwe/open-webui-tools) in Open WebUI for **bears** (Letta agents)
3. Roll out **Outline** and **Den** per [docs/planning/PLAN.md](docs/planning/PLAN.md) for Cabinet and channels

**Guide:** [docs/deployment/DEPLOYMENT.md](docs/deployment/DEPLOYMENT.md)

### Internal endpoints (typical)

- Open WebUI (`bears-openwebui`): `http://bears-openwebui:3000`
- Letta: `http://bears-letta:8283`
- Bifrost: `http://bears-bifrost:8080`

## Repository layout

```
den/                 # Den control plane (Rust / Axum); see docs/planning/PHASE1_BOOTSTRAP.md
docs/
├── README.md        # Doc index; shallow clone & sparse-checkout hints
├── planning/        # PLAN.md, PHASE1_BOOTSTRAP.md
├── deployment/      # DEPLOYMENT.md
└── architecture/    # ARCHITECTURE_NOTES.md, DEN_ARCHITECTURE.md
services/
├── bifrost/
├── letta/
└── openwebui/       # (when present)
README.md
AGENTS.md
```

## Environment variables

Per-service `.env.example` files. Common: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `LETTA_SERVER_PASS`, `LLM_API_URL` (Letta → Bifrost).

## Open WebUI + Letta

Direct integration via [open-webui-tools](https://github.com/Haervwe/open-webui-tools). For production multi-user identity and **Den**, see [docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md).

## License

Add a `LICENSE` file to the repository root when you publish or distribute this configuration.
