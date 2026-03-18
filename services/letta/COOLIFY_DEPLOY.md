# Letta - Coolify Deployment Guide

## Overview

Letta is the BEARS agent runtime: **native memory** (blocks, conversations, tools) and models via **LiteLLM**. Shared knowledge is **Cabinet** on **Outline**, exposed to agents through **Den** ([PLAN.md](../../PLAN.md)). Cabinet does **not** replace Letta’s per-agent memory.

## Prerequisites

- Coolify
- **LiteLLM** deployed and reachable
- **Den + Outline** when using Cabinet tools ([PLAN.md](../../PLAN.md))

## Deployment Steps

### 1. Deploy in Coolify

1. **Add New Resource** → **Docker Image**

2. **Basic Configuration**:
   - **Service Name**: `bears-letta`
   - **Image**: `letta/letta:latest`
   - **Deployment Type**: Public Docker Image

3. **Port Configuration**:
    - **Internal Port**: `8283` (API Server)
    - **External Access**: Internal only (accessed by LibreChat)

4. **Environment Variables**:

  ```bash
  # LiteLLM
  LLM_API_URL=http://bears-litellm:4000/v1

  # LiteLLM Master Key (optional)
  # If LiteLLM is configured to require a master key, set `LITELLM_MASTER_KEY`
  # in Letta's environment to the same value used by the LiteLLM service so
  # Letta can authenticate when calling the LiteLLM API. If you prefer to run
  # LiteLLM without authentication (development/testing only), leave this
  # variable unset and remove/comment `master_key` in the LiteLLM config.
  # Example: LITELLM_MASTER_KEY=sk-litellm-<hex>

   # Model Configuration
   MODEL_NAME=gpt-4

   # Letta Server Configuration
   LETTA_SERVER_PORT=8283
   LETTA_SERVER_PASS=<generate-secure-password>

   # OpenAI API Key (for embeddings)
   OPENAI_API_KEY=<your-openai-api-key>

   # Optional: Advanced Configuration
   # LETTA_SERVER_HOST=0.0.0.0
   # LOG_LEVEL=INFO
   ```

5. **Persistent Storage**:

   Create a volume for Letta configuration:

   - **Volume Name**: `bears-letta-data`
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
curl http://bears-letta:8283/v1/health
```

Access the Web UI:

```
http://<your-coolify-domain>:8283
# or
http://localhost:8283 (if exposed)
```

Test API:

```bash
curl http://bears-letta:8283/v1/agents
```

## Configuration Reference

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LLM_API_URL` | ✅ Yes | - | LiteLLM URL (`http://bears-litellm:4000/v1`) |
| `MODEL_NAME` | ✅ Yes | `gpt-4` | Default model for agents |
| `LETTA_SERVER_PORT` | No | `8283` | Web UI and API port |
| `LETTA_SERVER_PASS` | ✅ Yes | - | Admin password for Letta |
| `OPENAI_API_KEY` | ✅ Yes | - | For embeddings (can use LiteLLM instead) |
| `LETTA_SERVER_HOST` | No | `0.0.0.0` | Bind address |
| `LOG_LEVEL` | No | `INFO` | Logging verbosity |
| `LITELLM_MASTER_KEY` | Optional | - | Master key for LiteLLM API authentication. If omitted, LiteLLM will accept unauthenticated requests (only recommended for local/dev). |

### Service Dependencies

```
Letta → LiteLLM → providers
Cabinet tools (when Den + Outline are deployed) → Den → Outline
```

## Using Letta

### Primary UI

Many deployments use **OpenWebUI** or **LibreChat**; configure whichever you run. Agent **memory** stays in Letta; **shared knowledge** is **Cabinet (Outline)** in the target architecture.

### Admin Web UI (Optional)

For advanced agent management and development, access the Letta Web UI at `http://bears-letta:8283` (internal only):

1. **Login** with `LETTA_SERVER_PASS`
2. **Create an agent**:
    - Choose model (gpt-4, claude-3-5-sonnet, etc.)
   - Configure memory (Letta blocks; add Cabinet tools when Den is live)
    - Add tools/functions
3. **Chat with agent** in the UI
4. **View memory** - Letta blocks + conversation history; shared docs in Outline (Cabinet) when deployed

### API Access

```bash
# List agents
curl http://bears-letta:8283/v1/agents \
  -H "Authorization: Bearer $LETTA_SERVER_PASS"

# Create agent
curl -X POST http://bears-letta:8283/v1/agents \
  -H "Authorization: Bearer $LETTA_SERVER_PASS" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-assistant",
    "model": "gpt-4"
  }'

# Send message
curl -X POST http://bears-letta:8283/v1/agents/{agent_id}/messages \
  -H "Authorization: Bearer $LETTA_SERVER_PASS" \
  -H "Content-Type: application/json" \
  -d '{
    "message": "Hello, how are you?",
    "role": "user"
  }'
```

## Memory and knowledge

- **Letta:** blocks, conversations, built-in memory tools.  
- **Cabinet:** shared docs on **Outline**, via **Den** tools—see [PLAN.md](../../PLAN.md).

## Monitoring

### Health Check

```bash
curl http://bears-letta:8283/v1/health
```

### Agent Status

```bash
# List all agents
curl http://bears-letta:8283/v1/agents

# Get agent details
curl http://bears-letta:8283/v1/agents/{agent_id}
```

### Logs

View in Coolify dashboard:

```bash
# Look for:
# - "Letta server started"
# - "Connected to LLM provider"
# - Agent creation/interaction logs
```

## Troubleshooting

### Service Won't Start

**Solutions**:
- Check LiteLLM healthy
- Verify environment variables are correct
- Ensure port 8283 is not already in use
- Review logs for specific errors

### Can't Connect to LiteLLM

**Problem**: Model requests failing

**Solutions**:
- Verify `LLM_API_URL=http://bears-litellm:4000/v1`
- Check LiteLLM service is healthy
- Test: `curl http://bears-litellm:4000/health/liveliness`
- Verify LiteLLM has valid API keys
 - If you require authentication: ensure Letta is configured to present the LiteLLM master key by setting `LITELLM_MASTER_KEY` in Letta's environment to the same value used by the LiteLLM service. See `services/letta/.env.example` and `services/litellm/.env.example` for examples.
 - If you prefer unauthenticated LiteLLM: remove or comment out `master_key` in `services/litellm/litellm-config.yaml` (the file shipped in this repo has the `master_key` line commented out). Running without auth is convenient for local/dev but is insecure for production — use internal networks or a proxy if you choose this mode.

### LiteLLM Authentication (Letta)

Letta calls LiteLLM at `LLM_API_URL`. To authenticate, LiteLLM expects requests to include an `Authorization: Bearer <master-key>` header. Provide the same master key in the Letta environment so Letta can forward it when making model requests.

Quick test (replace with your key):

```bash
# From a machine/container that can reach LiteLLM:
curl -i -H "Authorization: Bearer sk-litellm-..." http://bears-litellm:4000/v1/models

# From inside Letta (after setting env var), check Letta can reach LiteLLM via an agent or by inspecting logs for successful 200 responses when Letta makes model calls.
```

If your Letta build exposes a different configuration name for forwarding LLM credentials, consult the Letta documentation. See `services/letta/.env.example` for the variable name used in this repository.

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

### Agent Response Time

Factors: model choice, context size, tool latency (e.g. Cabinet). Use streaming where supported.

## Open WebUI Integration

Letta can be integrated with Open WebUI to provide a modern chat interface with session management. This allows you to use Open WebUI's UI while leveraging Letta's agent capabilities.

### Overview

When integrating Open WebUI with Letta, you need to:
1. Map Open WebUI chat sessions to Letta agents
2. Route messages from Open WebUI to the appropriate Letta agent
3. Manage session persistence and context

### Session Management Strategies

**One Agent Per User** (Recommended for personalization):
- All chats from a user share the same Letta agent
- Agent learns user preferences across all conversations
- Better long-term memory and personalization

**One Agent Per Chat** (Recommended for isolation):
- Each Open WebUI chat gets its own Letta agent
- Complete isolation between conversations
- Better for project-specific or topic-specific chats

### Implementation

See [`OPENWEBUI_SESSIONS.md`](OPENWEBUI_SESSIONS.md) for a complete guide on:
- Session mapping strategies
- Pipe function implementation
- Code examples
- Integration with Letta + optional Den/Cabinet

### Quick Start

1. **Deploy the pipe function** using the example in [`openwebui_pipe_example.py`](openwebui_pipe_example.py)
2. **Configure environment variables** (see [`openwebui_integration.env.example`](openwebui_integration.env.example))
3. **Register the pipe function** in Open WebUI as a custom model
4. **Test the integration** by creating a chat in Open WebUI

### Configuration

Add these environment variables to your Open WebUI service (or pipe function service):

```bash
LETTA_API_URL=http://bears-letta:8283/v1
LETTA_SERVER_PASS=<your-letta-password>
SESSION_STRATEGY=user  # or "chat"
```

See [`openwebui_integration.env.example`](openwebui_integration.env.example).

## Advanced Configuration

### Custom Models

Configure different models per agent:

```bash
# In agent creation
{
  "model": "anthropic/claude-3-5-sonnet",
  "model_config": {
    "temperature": 0.7,
    "max_tokens": 4096
  }
}
```

### Agent Tools

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

### Multi-Agent Collaboration

Shared team context: **Cabinet (Outline)** or Letta shared blocks.

## Deployment completion

- [ ] LiteLLM healthy; Letta reaches `LLM_API_URL`
- [ ] OpenWebUI/LibreChat can chat with an agent
- [ ] Den + Outline + Cabinet tools when rolled out ([PLAN.md](../../PLAN.md))

**Services:** `bears-litellm`, `bears-letta`, UI; later **Outline + Den**.
