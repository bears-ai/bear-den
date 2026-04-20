# Letta - Coolify Deployment Guide

**Stack order:** Deploy **after Bifrost** and **Garage**; before **Den** — see [DEPLOYMENT.md](../../docs/deployment/DEPLOYMENT.md). Details below.

## Overview

Letta is the BEARS **bear runtime**: each **bear** is a **Letta agent** (conversation loop, tools, native **memory** blocks). Models go through **Bifrost** (`LLM_API_URL`). Shared knowledge is **Cabinet** on **Outline**, exposed to **bears** through **Den** ([PLAN.md](../../docs/planning/PLAN.md)). Cabinet does **not** replace Letta’s per‑**bear** memory.

**Terminology:** In BEARS docs, **bear** = one assistant backed by a Letta agent. **Den** provisions bears (Letta API), **users↔bears** membership (many‑to‑many), and surfaces bears in the **Den chat UI** and **Letta Code** harness—see [PLAN.md](../../docs/planning/PLAN.md). The Letta HTTP API still uses paths like `/v1/agents`; that **agent** id is the runtime id for a bear.

## Prerequisites

- Coolify
- **Bifrost** deployed and reachable ([`../bifrost/COOLIFY_DEPLOY.md`](../bifrost/COOLIFY_DEPLOY.md))
- **Den + Outline** when using Cabinet tools ([PLAN.md](../../docs/planning/PLAN.md))

## Deployment Steps

### 1. Deploy in Coolify

1. **Add New Resource** → **Docker Image**

2. **Basic Configuration**:
   - **Service Name**: `bear-letta`
   - **Image**: `letta/letta:latest`
   - **Deployment Type**: Public Docker Image

3. **Port Configuration**:
    - **Internal Port**: `8283` (API Server)
    - **External Access**: Internal only (accessed by **Den**, operators, and other stack services)

4. **Environment Variables**:

  ```bash
  # Bifrost (OpenAI-compatible gateway)
  LLM_API_URL=http://bear-bifrost:8080/v1

  # The stock BEARS Bifrost config is file-based GitOps without a client gateway key.
  # Keep OPENAI_API_KEY set for embeddings and any Letta features that call OpenAI directly.

   # Model Configuration
   MODEL_NAME=gpt-4

   # Letta Server Configuration
   LETTA_SERVER_PORT=8283
   LETTA_SERVER_PASS=<generate-secure-password>
   LETTA_PG_URI=postgresql://<user>:<pass>@<host>:5432/<db>

   # OpenAI API Key (for embeddings)
   OPENAI_API_KEY=<your-openai-api-key>

   # Optional: Advanced Configuration
   # LETTA_SERVER_HOST=0.0.0.0
   # LOG_LEVEL=INFO
   ```

5. **Persistent Storage**:

   Create a volume for Letta configuration:

   - **Volume Name**: `bear-letta-data`
   - **Mount Path**: `/root/.letta`

6. **Health Check**:

   ```bash
   Command: curl -f http://localhost:8283/v1/health || exit 1
   Interval: 30s
   Timeout: 10s
   Retries: 3
   Start Period: 40s
   ```

7. **Restart Policy**: `unless-stopped`

8. **Deploy** the service

### 2. Verify Deployment

Check health:

```bash
curl http://bear-letta:8283/v1/health
```

Access the Web UI:

```
http://<your-coolify-domain>:8283
# or
http://localhost:8283 (if exposed)
```

Test API:

```bash
curl http://bear-letta:8283/v1/agents
```

## Configuration Reference

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LLM_API_URL` | ✅ Yes | - | Bifrost URL (`http://bear-bifrost:8080/v1`) |
| `MODEL_NAME` | ✅ Yes | `gpt-4` | Default model for new Letta agents (**bears**) |
| `LETTA_SERVER_PORT` | No | `8283` | Web UI and API port |
| `LETTA_SERVER_PASS` | ✅ Yes | - | Admin password for Letta |
| `LETTA_PG_URI` | Recommended | - | External Postgres URI — use **`postgresql://`** (not `postgres://`). The short form can break Alembic with `NoSuchModuleError: ... postgres.pg8000`. |
| `OPENAI_API_KEY` | ✅ Yes | - | For embeddings and direct OpenAI calls; chat completions use `LLM_API_URL` |
| `LETTA_SERVER_HOST` | No | `0.0.0.0` | Bind address |
| `LOG_LEVEL` | No | `INFO` | Logging verbosity |

### Service Dependencies

```
Letta → Bifrost → providers
Cabinet tools (when Den + Outline are deployed) → Den → Outline
```

## Using Letta

### Primary UI

End-user chat in BEARS is **Den embedded Deep Chat** → Den → Letta Code → Letta. Per‑**bear** **memory** stays in Letta; **shared knowledge** is **Cabinet (Outline)** in the target architecture.

### Admin Web UI (Optional)

For advanced **bear** (Letta agent) management and development, access the Letta Web UI at `http://bear-letta:8283` (internal only):

1. **Login** with `LETTA_SERVER_PASS`
2. **Create a bear** (Letta: “create agent”):
    - Choose model (gpt-4, claude-3-5-sonnet, etc.)
   - Configure memory (Letta blocks; add Cabinet tools when Den is live)
    - Add tools/functions
3. **Chat with the bear** in the UI
4. **View memory** - Letta blocks + conversation history; shared docs in Outline (Cabinet) when deployed  

In production, prefer **Den** as the system of record for which bears exist and who may use them ([PLAN.md](../../docs/planning/PLAN.md)); the Letta UI remains useful for ops and debugging.

### API Access

```bash
# List bears (Letta API: GET /v1/agents)
curl http://bear-letta:8283/v1/agents \
  -H "Authorization: Bearer $LETTA_SERVER_PASS"

# Create a bear (Letta API: POST /v1/agents)
curl -X POST http://bear-letta:8283/v1/agents \
  -H "Authorization: Bearer $LETTA_SERVER_PASS" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-assistant",
    "model": "gpt-4"
  }'

# Send message ({agent_id} = Letta’s id for that bear)
curl -X POST http://bear-letta:8283/v1/agents/{agent_id}/messages \
  -H "Authorization: Bearer $LETTA_SERVER_PASS" \
  -H "Content-Type: application/json" \
  -d '{
    "message": "Hello, how are you?",
    "role": "user"
  }'
```

## Memory and knowledge

- **Letta:** blocks, conversations, built-in memory tools.  
- **Cabinet:** shared docs on **Outline**, via **Den** tools—see [PLAN.md](../../docs/planning/PLAN.md).

## Monitoring

### Health Check

```bash
curl http://bear-letta:8283/v1/health
```

### Bear / agent status (Letta API)

```bash
# List all bears (same as Letta agents)
curl http://bear-letta:8283/v1/agents

# Get one bear’s details
curl http://bear-letta:8283/v1/agents/{agent_id}
```

### Logs

View in Coolify dashboard:

```bash
# Look for:
# - "Letta server started"
# - "Connected to LLM provider"
# - Bear/agent creation and chat logs
```

## Troubleshooting

### `NoSuchModuleError: Can't load plugin: sqlalchemy.dialects:postgres.pg8000`

**Cause:** `LETTA_PG_URI` used the **`postgres://`** scheme. Letta’s migration path rewrites it to `postgres+pg8000://…`, but SQLAlchemy registers the driver as **`postgresql.pg8000`**, not `postgres.pg8000`.

**Fix:** Change the URI to use the canonical scheme:

```text
postgresql://USER:PASSWORD@HOST:5432/DATABASE
```

Coolify’s database UI sometimes copies `postgres://`; replace it with `postgresql://`.

### `LETTA_PG_URI` looks different in logs than in Coolify

**Expected.** Letta parses your URI and may log SQLAlchemy dialect URLs with an explicit driver (e.g. `postgresql+pg8000://…` for Alembic). **As long as the scheme starts with `postgresql://`**, migrations should load the correct plugin.

### Service Won't Start

**Solutions**:
- Verify environment variables are correct (`LETTA_SERVER_PASS`, `LLM_API_URL`, …)
- Ensure port 8283 is not already in use
- From the Letta container network, confirm Bifrost is reachable (see below)
- Review logs for specific errors

### Can't connect to Bifrost

**Problem**: Model requests failing

**Solutions**:
- Verify `LLM_API_URL=http://bear-bifrost:8080/v1` (include the `/v1` suffix).
- Check the Bifrost service: `curl http://bear-bifrost:8080/health` and `curl http://bear-bifrost:8080/v1/models`
- Confirm Bifrost has valid provider keys in its environment and `services/bifrost/config.json` mounted at `/app/data/config.json` ([`../bifrost/COOLIFY_DEPLOY.md`](../bifrost/COOLIFY_DEPLOY.md)).

### Gateway authentication (Letta)

With the **file-based GitOps** `config.json` shipped in this repo, Bifrost does **not** require a client `Authorization` header for `/v1` calls; provider keys live on the gateway. Letta still needs **`OPENAI_API_KEY`** for embeddings and any direct OpenAI usage.

If you enable **Bifrost governance / virtual keys**, align Letta’s outbound credential with Bifrost’s documentation (often the OpenAI-compatible `Authorization` header).

Quick test:

```bash
curl -sS http://bear-bifrost:8080/v1/models
```

If your Letta build exposes a different configuration name for forwarding LLM credentials, consult the Letta documentation. See `services/letta/.env.example` for the variables used in this repository.

### Web UI Not Loading

**Problem**: Blank page or 404

**Solutions**:
- Verify port 8283 is exposed
- Check Coolify proxy configuration
- Test direct access: `http://<server-ip>:8283`
- Review browser console for errors
- Clear browser cache

## Security

### Admin Password

Generate a secure password:

```bash
openssl rand -base64 32
# Use for LETTA_SERVER_PASS
```

### API Authentication

All API requests require the admin password:

```bash
Authorization: Bearer <LETTA_SERVER_PASS>
```

### Network Security

- ✅ Use Coolify proxy for HTTPS
- ✅ Restrict access with Coolify authentication
- ✅ Use strong admin password
- ❌ Don't expose publicly without authentication
- ❌ Don't commit passwords to Git

## Performance Tuning

### Resource Limits

```bash
Memory: 1-2 GB
CPU: 1-2 cores
```

### Bear response time

Factors: model choice, context size, tool latency (e.g. Cabinet). Use streaming where supported.

## Codepool harness (separate service)

First-party web chat is **Den embedded Deep Chat** → **Den** → **`codepool/`** (Letta Code SDK) → **Letta** — see [DEN_ARCHITECTURE.md](../../docs/architecture/DEN_ARCHITECTURE.md). This directory documents only the **Letta API server** (`letta/letta`). You **must** also deploy **Codepool** per [`../../codepool/COOLIFY_DEPLOY.md`](../../codepool/COOLIFY_DEPLOY.md); Den’s **`CODEPOOL_BASE_URL`** targets that HTTP service, not an alias of this Letta container.

## Advanced Configuration

### Custom Models

Configure different models per bear:

```bash
# In Letta agent (bear) creation
{
  "model": "anthropic/claude-3-5-sonnet",
  "model_config": {
    "temperature": 0.7,
    "max_tokens": 4096
  }
}
```

### Bear tools (Letta)

Add custom tools/functions:

```bash
# Via API or Web UI
{
  "tools": [
    {"name": "search_web", "function": "..."},
    {"name": "send_email", "function": "..."}
  ]
}
```

### Multi-user / shared bears

Shared team context: **Cabinet (Outline)**, Letta shared blocks, or a **shared bear** with many users (Den manages membership—[PLAN.md](../../docs/planning/PLAN.md)).

## Deployment completion

- [ ] Bifrost healthy; Letta reaches `LLM_API_URL`
- [ ] **Codepool** (`codepool/`, `bear-codepool`) is deployed and Den can reach it at **`CODEPOOL_BASE_URL`**; **Den** can reach Letta for provisioning via **`LETTA_BASE_URL`**; end users chat through Den’s web UI
- [ ] Den + Outline + Cabinet tools when rolled out ([PLAN.md](../../docs/planning/PLAN.md))

**Services:** `bear-bifrost`, `bear-letta`, UI; later **Outline + Den**.
