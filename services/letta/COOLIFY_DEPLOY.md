# Letta - Coolify Deployment Guide

## Overview

Letta is the agent orchestration framework that provides the API server and agent runtime for the BEARS Stack. It integrates with external memory services via a knowledgebase tools API (embed/upsert/search/get) and with LiteLLM for model access. LibreChat serves as the primary user interface, connecting to Letta's agent APIs. Prefer a knowledgebase layer so archival memory remains controlled and read-only by default.

## Prerequisites

- Coolify instance running
- **All other services deployed and healthy**:
  - ✅ Git Sync
  - ✅ Redis
  - ✅ Qdrant
  - ✅ PostgreSQL (Coolify-managed)
  - ✅ LiteLLM
  - ⚪ Knowledgebase / Memory Service (optional — prefer a dedicated knowledgebase service)

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
  # Knowledgebase / Memory service
  # Letta should call a knowledgebase tools API rather than depending
  # directly on an internal memory implementation. Set this to the
  # Knowledgebase service URL that provides embed/upsert/search/get
  # endpoints. Example: `http://bears-knowledgebase:8080`.
  KNOWLEDGEBASE_URL=http://bears-knowledgebase:8080

  # If you have a legacy adapter, configure it in the knowledgebase layer

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
| `KNOWLEDGEBASE_URL` | ✅ Yes | - | URL of the knowledgebase tools API (embed/upsert/search/get). Example: `http://bears-knowledgebase:8080` |
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
  ├── Knowledgebase / Memory Service
  │   ├── PostgreSQL (Coolify-managed) [optional]
  │   ├── Redis
  │   ├── Qdrant
  │   └── Git Sync
  └── LiteLLM
      ├── OpenAI API
      └── Anthropic API
```

## Using Letta

### Primary Usage via LibreChat

Letta agents are primarily accessed through the LibreChat UI, which provides a modern chat interface for interacting with agents. LibreChat handles user authentication, conversation management, and connects to Letta's agent APIs.

### Admin Web UI (Optional)

For advanced agent management and development, access the Letta Web UI at `http://bears-letta:8283` (internal only):

1. **Login** with `LETTA_SERVER_PASS`
2. **Create an agent**:
    - Choose model (gpt-4, claude-3-5-sonnet, etc.)
   - Configure memory (point agents to the configured knowledgebase)
    - Add tools/functions
3. **Chat with agent** in the UI
4. **View memory** - agent memories stored via the knowledgebase → Git

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
    "memory": {"type": "knowledgebase"}
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

## Integration with Memory Backends

Letta should interact with a knowledgebase tools API that exposes a small, stable contract (embed/upsert/search/get). The knowledgebase service can use Qdrant for vectors and Git Sync for archival markdown; Letta should call the knowledgebase API rather than directly mutating archival data.

### How It Works (recommended)

1. **Agent creates memory** → Letta calls the configured `KNOWLEDGEBASE_URL` API
2. **Knowledgebase writes or indexes** → Markdown files for archival + vectors in Qdrant (or other vector store)
3. **Git Sync detects change** → Commits and pushes archival files to GitHub
4. **Memory is searchable** → Qdrant vector index (queried via the Knowledgebase API)
5. **Memory persists** → Git version control (archival records should be treated as read-only by default)

### Memory Flow

```
User chats → Letta agent → Knowledgebase API → (Qdrant vectors + Markdown archive) → Git Sync → GitHub
                  ↓
                 Qdrant (vectors)
                  ↓
                PostgreSQL (metadata)
```

### Viewing Agent Memories

Check the content repository on GitHub (archival markdown files):

```
memories/
└── personal/
  └── agent-my-assistant/
    ├── conversation-context.md
    ├── user-preferences.md
    └── learned-facts.md
```

### Recommendations

- Treat archival markdown as immutable by default; do not let agents overwrite/archive records without a controlled process.
- Use a knowledgebase service (we plan to provide an RMCP + Qdrant implementation) to centralize embedding and search logic.
  - Keep legacy adapters optional; prefer the knowledgebase API as the single integration contract.

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
# - "Connected to memory service"
# - "Connected to LLM provider"
# - Agent creation/interaction logs
```

## Troubleshooting

### Service Won't Start

**Solutions**:
- Check all dependencies are healthy (knowledgebase/memory service, LiteLLM)
- Verify environment variables are correct
- Ensure port 8283 is not already in use
- Review logs for specific errors

### Can't Connect to the memory service

**Problem**: "Connection refused" to the configured knowledgebase/memory service

**Solutions**:
- Verify `KNOWLEDGEBASE_URL` is set and correct in Letta's environment
- Check the memory service is healthy
- Test: `curl <KNOWLEDGEBASE_URL>/health`
- Ensure both services are in the same Coolify network

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
- Check the memory service connection (KNOWLEDGEBASE_URL)
- Verify the memory service can write to its configured archive path (e.g. `/app/memory/`)
- Check Git Sync is running
- Review Letta logs for memory service API errors
- Test the memory service manually: Create a memory via its API

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
  - Memory service query complexity
- Memory size (larger memories = slower search)

### Optimization Tips

1. Use faster models for less critical tasks
2. Limit memory context size
3. Cache frequent queries in Redis
4. Use streaming responses for better UX

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
- Integration with BEARS memory system

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
SESSION_STORAGE=redis
REDIS_URL=redis://bears-redis:6379
```

For detailed configuration, see [`openwebui_integration.env.example`](openwebui_integration.env.example).

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
[ ] Knowledgebase / memory service returning health OK
- [ ] LiteLLM proxying to LLM providers
- [ ] Letta API accessible (internal)
- [ ] LibreChat deployed and accessible
- [ ] Test agent created via API
- [ ] Agent memories appearing in GitHub

### Full Stack Test

1. **Create test agent** via Letta API or admin Web UI
2. **Chat with agent** through LibreChat - ask it to remember something
3. **Check GitHub** - verify memory file created
4. **Check Qdrant** - verify vectors indexed
5. **Chat again** - verify agent recalls previous context

## Next Steps

Your BEARS Stack is now fully deployed! 🎉

### Getting Started

1. **Deploy LibreChat** using the cpfiffer/letta-libre fork
2. **Create your first agent** via Letta API or admin Web UI
3. **Access agents** through LibreChat's modern interface
4. **Customize memory structure** in the content repo
5. **Add more models** to LiteLLM config
6. **Set up monitoring** (optional)
7. **Configure backups** (PostgreSQL, Qdrant)

### Further Reading

- Letta documentation: https://docs.letta.ai
Memory service documentation: (provider-specific or internal)
LiteLLM documentation: https://docs.litellm.ai

## Coolify Service Summary

Your complete BEARS Stack:

```bash
# Infrastructure
bears-redis          (Cache)
bears-qdrant         (Vector DB)
bears-postgres       (Database - Coolify-managed)

# Core Services
bears-git-sync       (Memory sync)
bears-knowledgebase  (Memory management)
bears-litellm        (Model gateway)

# Agent Framework
bears-letta          (Agent orchestration API)
bears-librechat      (Primary chat UI)
```

All connected through Coolify's internal Docker network! 🐻
