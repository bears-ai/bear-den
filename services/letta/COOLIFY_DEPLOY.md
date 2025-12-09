# Letta - Coolify Deployment Guide

## Overview

Letta is the agent orchestration framework that ties everything together. It provides the web UI, API server, and agent runtime that uses Onyx for memory and LiteLLM for model access.

## Prerequisites

- Coolify instance running
- **All other services deployed and healthy**:
  - ✅ Git Sync
  - ✅ Redis
  - ✅ Qdrant
  - ✅ PostgreSQL (Coolify-managed)
  - ✅ Onyx API Server
  - ✅ LiteLLM

## Deployment Steps

### 1. Deploy in Coolify

1. **Add New Resource** → **Docker Image**

2. **Basic Configuration**:
   - **Service Name**: `bears-letta`
   - **Image**: `letta/letta:latest`
   - **Deployment Type**: Public Docker Image

3. **Port Configuration**:
   - **Internal Port**: `8283` (Web UI + API)
   - **External Port**: `8283` or use Coolify proxy with custom domain

4. **Environment Variables**:

   ```bash
   # Onyx Integration
   ONYX_URL=http://bears-onyx:8080

  # LiteLLM Integration
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
| `ONYX_URL` | ✅ Yes | - | Onyx API URL (`http://bears-onyx:8080`) |
| `LLM_API_URL` | ✅ Yes | - | LiteLLM URL (`http://bears-litellm:4000/v1`) |
| `MODEL_NAME` | ✅ Yes | `gpt-4` | Default model for agents |
| `LETTA_SERVER_PORT` | No | `8283` | Web UI and API port |
| `LETTA_SERVER_PASS` | ✅ Yes | - | Admin password for Letta |
| `OPENAI_API_KEY` | ✅ Yes | - | For embeddings (can use LiteLLM instead) |
| `LETTA_SERVER_HOST` | No | `0.0.0.0` | Bind address |
| `LOG_LEVEL` | No | `INFO` | Logging verbosity |
| `LITELLM_MASTER_KEY` | Optional | - | Master key for LiteLLM API authentication. If omitted, LiteLLM will accept unauthenticated requests (only recommended for local/dev). |

### Service Dependencies

Letta requires these services to be healthy:

```
Letta
  ├── Onyx API Server
  │   ├── PostgreSQL (Coolify-managed)
  │   ├── Redis
  │   ├── Qdrant
  │   └── Git Sync
  └── LiteLLM
      ├── OpenAI API
      └── Anthropic API
```

## Using Letta

### Web UI (Admin Development Environment)

Access at `http://your-domain:8283`:

1. **Login** with `LETTA_SERVER_PASS`
2. **Create an agent**:
   - Choose model (gpt-4, claude-3-5-sonnet, etc.)
   - Configure memory (automatically uses Onyx)
   - Add tools/functions
3. **Chat with agent** in the UI
4. **View memory** - agent memories stored in Onyx → Git

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
    "model": "gpt-4",
    "memory": {"type": "onyx"}
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

## Integration with Onyx

### How It Works

1. **Agent creates memory** → Letta calls Onyx API
2. **Onyx writes Markdown file** → `/app/memory/memories/...`
3. **Git Sync detects change** → Commits and pushes to GitHub
4. **Memory is searchable** → Qdrant vector index
5. **Memory persists** → Git version control

### Memory Flow

```
User chats → Letta agent → Onyx API → Markdown file → Git Sync → GitHub
                                    ↓
                                 Qdrant (vectors)
                                    ↓
                              PostgreSQL (metadata)
```

### Viewing Agent Memories

Check the content repository on GitHub:

```
memories/
└── personal/
    └── agent-my-assistant/
        ├── conversation-context.md
        ├── user-preferences.md
        └── learned-facts.md
```

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
# - "Connected to Onyx"
# - "Connected to LLM provider"
# - Agent creation/interaction logs
```

## Troubleshooting

### Service Won't Start

**Solutions**:
- Check all dependencies are healthy (Onyx, LiteLLM)
- Verify environment variables are correct
- Ensure port 8283 is not already in use
- Review logs for specific errors

### Can't Connect to Onyx

**Problem**: "Connection refused" to Onyx

**Solutions**:
- Verify `ONYX_URL=http://bears-onyx:8080`
- Check Onyx service is healthy
- Test: `curl http://bears-onyx:8080/health`
- Ensure both services in same Coolify network

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

### Agents Not Creating Memories

**Problem**: No memory files created

**Solutions**:
- Check Onyx connection
- Verify Onyx can write to `/app/memory/`
- Check Git Sync is running
- Review Letta logs for Onyx API errors
- Test Onyx manually: Create a memory via Onyx API

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

### Agent Isolation

Each agent can have isolated memory:

```
memories/
├── personal/
│   ├── agent-alice/
│   ├── agent-bob/
│   └── agent-shared/
```

## Performance Tuning

### Resource Limits

```bash
Memory: 1-2 GB
CPU: 1-2 cores
```

### Agent Response Time

Factors affecting speed:
- LiteLLM model choice (GPT-4 slower than GPT-3.5)
- Onyx query complexity
- Memory size (larger memories = slower search)

### Optimization Tips

1. Use faster models for less critical tasks
2. Limit memory context size
3. Cache frequent queries in Redis
4. Use streaming responses for better UX

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

Deploy multiple agents that share memory:

```
memories/shared/team-context.md
```

## Deployment Completion

### Verification Checklist

- [ ] All services healthy in Coolify
- [ ] Git Sync syncing to GitHub
- [ ] Redis responding to ping
- [ ] Qdrant has collections
- [ ] PostgreSQL accepting connections
- [ ] Onyx API returning health OK
- [ ] LiteLLM proxying to LLM providers
- [ ] Letta Web UI accessible
- [ ] Test agent created successfully
- [ ] Agent memories appearing in GitHub

### Full Stack Test

1. **Create test agent** in Letta Web UI
2. **Chat with agent** - ask it to remember something
3. **Check GitHub** - verify memory file created
4. **Check Qdrant** - verify vectors indexed
5. **Chat again** - verify agent recalls previous context

## Next Steps

Your BEARS Stack is now fully deployed! 🎉

### Getting Started

1. **Create your first agent** in the Web UI
2. **Customize memory structure** in the content repo
3. **Add more models** to LiteLLM config
4. **Set up monitoring** (optional)
5. **Configure backups** (PostgreSQL, Qdrant)

### Further Reading

- Letta documentation: https://docs.letta.ai
- Onyx documentation: https://docs.onyx.app
- LiteLLM documentation: https://docs.litellm.ai

## Coolify Service Summary

Your complete BEARS Stack:

```bash
# Infrastructure
bears-redis          (Cache)
bears-qdrant         (Vector DB)
bears-postgres       (Database - Coolify-managed)

# Core Services
bears-git-sync       (Memory sync)
bears-onyx           (Memory management)
bears-litellm        (Model gateway)

# Agent Framework
bears-letta          (Orchestration + Web UI)
```

All connected through Coolify's internal Docker network! 🐻
