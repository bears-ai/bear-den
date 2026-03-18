# 🐻 Basic Environment for Agents Runtime Server (BEARS)

**Configuration repository** for deploying an agentic assistants platform with Coolify, using:

- [Letta](https://github.com/letta-ai/letta) – Agent runtime (conversation loop, **native memory** blocks, tools)
- [Open WebUI](https://github.com/open-webui/open-webui) – Modern chat UI with multi-user support
- **Cabinet** (via [Outline](https://www.getoutline.com/)) – Shared **knowledgebase** humans and agents can read and edit together (see [PLAN.md](PLAN.md); does **not** replace Letta memory)
- [LiteLLM](https://github.com/BerriAI/litellm) – Unified LLM gateway
- [Coolify](https://coolify.io) – Self-hosted PaaS for deployment and management

**Legacy (optional):** Git-synced Markdown + Qdrant knowledgebase service—**obviated** by Cabinet once you adopt Outline; kept in-repo only for existing deployments or migration. See [Legacy Git/Qdrant knowledgebase](#legacy-gitqdrant-knowledgebase) below.

## Architecture overview

### Target stack (Cabinet + Letta)

1. **This repository** (`bears-deploy`) – Configuration and deployment guides  
2. **Letta** – Agents, tools, and **Letta’s own memory** (blocks, conversations, etc.)  
3. **Cabinet (Outline)** – Long-lived docs and knowledge **shared by humans and agents** (agents reach it via BEARS Core tools per [PLAN.md](PLAN.md))  
4. **BEARS Core** (planned) – Auth-aware proxy, identity, policy, Cabinet API for agents  

Roadmap and phases: **[PLAN.md](PLAN.md)**.

### Knowledge vs memory (terminology)

| Layer | Role |
|-------|------|
| **Letta memory** | Per-agent context: blocks, conversation state, built-in memory tools. **Unchanged** by Cabinet. |
| **Cabinet (Outline)** | Shared knowledgebase: reference material, history, project notes—**editable in Outline by people** and **by agents through BEARS**. |
| **Legacy KB** (Git + Qdrant service) | **Superseded** by Cabinet for this purpose; no longer the recommended path. |

## Quick Start with Coolify

### Prerequisites

- Coolify instance running
- API keys for OpenAI and/or Anthropic
- For **target** stack: LiteLLM + Letta + OpenWebUI; **Outline** when deploying Cabinet (see PLAN.md Phase 3)
- Approximately **4 GB RAM** for a minimal stack (Letta + LiteLLM + OpenWebUI); more if you add Outline and BEARS Core

### Deployment order (target)

1. **LiteLLM** (model gateway)  
2. **Letta** (agent API; configure `LLM_API_URL`; Cabinet tools when BEARS is available)  
3. **OpenWebUI** (or LibreChat—see `services/librechat/`)  
4. **Outline + BEARS Core + Cabinet wiring** – per [PLAN.md](PLAN.md) when you enable shared knowledgebase  

### Legacy deployment order (Git/Qdrant knowledgebase only)

Only if you still run the old memory service:

1. PostgreSQL, Redis, Qdrant  
2. Git Sync + content repository  
3. Knowledgebase / memory service API  
4. LiteLLM → Letta → OpenWebUI  

See [DEPLOYMENT.md](DEPLOYMENT.md) and service guides; those steps are labeled **legacy** there.

### Deployment steps (summary)

1. Follow **[PLAN.md](PLAN.md)** for BEARS Core and Cabinet rollout.  
2. Service guides: [`services/`](services/) – [LiteLLM](services/litellm/COOLIFY_DEPLOY.md), [Letta](services/letta/COOLIFY_DEPLOY.md), etc.  
3. Access OpenWebUI at your Coolify domain (e.g. port 3000).

### Service endpoints (internal, typical)

- **OpenWebUI**: `http://bears-openwebui:3000`
- **Letta API**: `http://bears-letta:8283`
- **LiteLLM**: `http://bears-litellm:4000`
- **Outline** (when deployed): per your Coolify service name
- **Legacy knowledgebase** (if retained): `http://bears-knowledgebase:8080`

## Repository structure

```
bears-deploy/
├── services/
│   ├── git-sync/          # Legacy: Git memory sync
│   ├── redis/             # Legacy: cache for old KB
│   ├── qdrant/            # Legacy: vectors for old KB
│   ├── knowledgebase/     # Legacy: Git+Qdrant KB service
│   ├── litellm/
│   ├── letta/
│   ├── openwebui/
│   └── librechat/
├── content-template/      # Legacy: template for Git-backed content repo
├── PLAN.md                # Roadmap: BEARS Core, Cabinet, Outline
├── DEPLOYMENT.md
├── ARCHITECTURE_NOTES.md
├── MULTIUSER_PROXY_ARCHITECTURE.md
└── README.md
```

## Environment variables

See each service’s `.env.example`. Common keys: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `LETTA_SERVER_PASS`, `LITELLM_MASTER_KEY`, `LLM_API_URL`.

**Legacy Git Sync** (only if using old KB): `GIT_SYNC_REPO`, `GIT_USERNAME`, `GIT_PASSWORD`.

## Cabinet and Letta (recommended model)

- **Cabinet** (Outline-backed, exposed through **BEARS Core**) holds durable knowledge both people and agents use.  
- **Letta** keeps **its own** memory model for the agent loop; Cabinet complements it—it does **not** replace memory blocks or conversation memory.  
- The **Git + Qdrant standalone knowledgebase** is **not** needed once Cabinet is in use for agent-visible archival knowledge.

Details: [PLAN.md](PLAN.md), [ARCHITECTURE_NOTES.md](ARCHITECTURE_NOTES.md).

## Legacy Git/Qdrant knowledgebase

The **content-template** layout (`memories/`, `history/`, `projects/`) and the **Git Sync + Qdrant + knowledgebase API** stack were the previous way to give agents searchable Markdown archives. That path is **obviated by Cabinet (Outline)** for shared human+agent knowledge.

- **Still on the legacy stack?** You can run it until you migrate content into Outline collections (“decks”) and point agents at Cabinet tools.  
- **New deployments** should plan for **Cabinet + Outline** per PLAN.md rather than standing up the full Git/Qdrant KB.

See [content-template/README.md](content-template/README.md) for the legacy template and migration notes.

## OpenWebUI + Letta integration

Letta agents can be connected to OpenWebUI as “models” via [open-webui-tools](https://github.com/Haervwe/open-webui-tools). For production multi-user identity and routing, see **[MULTIUSER_PROXY_ARCHITECTURE.md](MULTIUSER_PROXY_ARCHITECTURE.md)** (auth proxy; aligns with **BEARS Core** in PLAN.md). Session notes for direct OpenWebUI→Letta: `services/letta/OPENWEBUI_SESSIONS.md`.

## Service architecture (target)

```
┌─────────────┐     ┌─────────────┐
│  OpenWebUI  │     │   Outline   │  ← humans edit Cabinet docs
└──────┬──────┘     └──────▲──────┘
       │                   │
       ▼                   │ (docs / search)
┌─────────────┐     ┌──────┴──────┐
│ BEARS Core  │────▶│  Cabinet    │  ← agent tools (search/read/write)
│  (planned)  │     │  (Outline)  │
└──────┬──────┘     └─────────────┘
       │
       ▼
┌─────────────┐     ┌─────────────┐
│   Letta     │────▶│  LiteLLM    │
│  (memory +  │     │   :4000     │
│   agents)   │     └─────────────┘
└─────────────┘
```

Letta’s **internal memory** is not shown separately above; it remains inside Letta.

## Configuration

- LiteLLM: [`services/litellm/litellm-config.yaml`](services/litellm/litellm-config.yaml)  
- Letta: [`services/letta/.env.example`](services/letta/.env.example)  
- Legacy: Git Sync, Redis, Qdrant, knowledgebase service envs under `services/*`

## Deployment

See **[DEPLOYMENT.md](DEPLOYMENT.md)**. Target vs legacy steps are called out there.

### Quick checklist (target-oriented)

1. Deploy LiteLLM  
2. Deploy Letta (`LLM_API_URL` → LiteLLM)  
3. Deploy OpenWebUI + open-webui-tools  
4. Roll out Outline + BEARS Core + Cabinet per **PLAN.md**

### Quick checklist (legacy KB)

1. PostgreSQL, Redis, Qdrant  
2. Content repo from `content-template/`  
3. Git Sync + knowledgebase service  
4. LiteLLM → Letta (`KNOWLEDGEBASE_URL` if still using legacy KB) → OpenWebUI  

## Backup and recovery

- **Cabinet / Outline:** Back up Outline’s database and exports per Outline ops guides.  
- **Letta:** Agent state in Letta volumes + Letta/Cloud backup practices.  
- **Legacy:** Git content repo + Postgres/Qdrant as in older docs.

## Monitoring

Coolify health checks and logs per service. LiteLLM for model usage; BEARS Core (when deployed) for per-user/agent observability per PLAN.md.

## Troubleshooting

- **Letta ↔ LiteLLM:** Check `LLM_API_URL` and any `LITELLM_MASTER_KEY`.  
- **Legacy KB:** See DEPLOYMENT.md troubleshooting for Git Sync, Qdrant, knowledgebase connectivity.  
- Service-specific: `services/*/COOLIFY_DEPLOY.md`.

## Security

Strong secrets, HTTPS via Coolify, private repos for any legacy content, no keys in Git.

## Documentation

- **[PLAN.md](PLAN.md)** – BEARS Core, Cabinet, Outline, phases  
- **[DEPLOYMENT.md](DEPLOYMENT.md)** – Coolify deployment (target + legacy)  
- **[ARCHITECTURE_NOTES.md](ARCHITECTURE_NOTES.md)** – Stack architecture  
- **[MULTIUSER_PROXY_ARCHITECTURE.md](MULTIUSER_PROXY_ARCHITECTURE.md)** – Multi-user proxy + Cabinet note  
- **[content-template/README.md](content-template/README.md)** – Legacy content template  

## License

[Add your license here]

## Acknowledgments

Letta, Outline, LiteLLM, Open WebUI, Coolify.
