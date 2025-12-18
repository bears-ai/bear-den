# 🐻 Basic Environment for Agent Runtimes Stack (BEARS)

**Configuration repository** for deploying an agentic assistants platform with Coolify, using:

- [Letta](https://github.com/letta-ai/letta) – Agent server and orchestration layer
- [LibreChat](https://github.com/danny-avila/LibreChat) – Modern chat UI with multi-user support
- Knowledgebase / Memory service (e.g. RMCP + Qdrant) – Memory management with Git-versioned Markdown
- [Qdrant](https://github.com/qdrant/qdrant) – Vector database for semantic memory
- [LiteLLM](https://github.com/BerriAI/litellm) – Unified LLM gateway
- [PostgreSQL](https://www.postgresql.org/) – Database backend for memory services
- [Redis](https://redis.io/) – Cache layer for memory services
- [MongoDB](https://www.mongodb.com/) – Database for LibreChat user management
- [Coolify](https://coolify.io) – Self-hosted PaaS for deployment and management

## Architecture Overview

This is a **two-repository architecture**:

1. **This repository** (`bears-deploy`) - Configuration and deployment guides
2. **Content repository** - Your memory files (memories/, history/, projects/)

Memory content is automatically synced to/from GitHub via the Git Sync service, providing version control, backup, and portability.

## Quick Start with Coolify

### Prerequisites

- Coolify instance running
- GitHub account for content repository
- API keys for OpenAI and/or Anthropic
- Approximately 4-6 GB RAM available across services

### Deployment Order

Services must be deployed in this specific order due to dependencies:

1. **Infrastructure Layer**
   - Redis (cache)
   - Qdrant (vector database)
   - PostgreSQL (Coolify-managed database)

2. **Memory Layer**
   - Git Sync (memory synchronization)

3. **API Layer**
   - Knowledgebase / Memory Service (memory management)
   - LiteLLM (model gateway)

4. **Application Layer**
   - Letta (agent orchestration + Web UI)

### Deployment Steps

1. **Fork the content template**

   See [`content-template/`](content-template/) for a ready-to-use template repository.

2. **Deploy each service in Coolify**

   Follow the detailed deployment guides in [`services/`](services/):

   - [`services/redis/COOLIFY_DEPLOY.md`](services/redis/COOLIFY_DEPLOY.md)
   - [`services/qdrant/COOLIFY_DEPLOY.md`](services/qdrant/COOLIFY_DEPLOY.md)
   - [`services/git-sync/COOLIFY_DEPLOY.md`](services/git-sync/COOLIFY_DEPLOY.md)
   - Knowledgebase/Memory Service deployment guide (see service directory)
   - [`services/litellm/COOLIFY_DEPLOY.md`](services/litellm/COOLIFY_DEPLOY.md)
   - [`services/letta/COOLIFY_DEPLOY.md`](services/letta/COOLIFY_DEPLOY.md)
   - [`services/librechat/COOLIFY_DEPLOY.md`](services/librechat/COOLIFY_DEPLOY.md)

3. **Access the Web UI**

   Once deployed, access Letta at your configured Coolify domain or:
   ```
   http://your-coolify-domain:8283
   ```

### Service Endpoints (Internal)

Services communicate via Coolify's internal Docker networking:

- **Letta Web UI**: `http://bears-letta:8283`
- Knowledgebase API: `http://bears-knowledgebase:8080`
- **Qdrant**: `http://bears-qdrant:6333`
- **LiteLLM**: `http://bears-litellm:4000`
- **Redis**: `redis://bears-redis:6379`

## Repository Structure

```
bears-deploy/                    # This repository
├── services/                   # Service-specific configurations
│   ├── git-sync/              # Memory synchronization service
│   │   ├── Dockerfile
│   │   ├── sync.sh
│   │   ├── .env.example
│   │   └── COOLIFY_DEPLOY.md
│   ├── redis/                 # Cache layer
│   ├── qdrant/                # Vector database
│   ├── knowledgebase/         # Memory management service
│   ├── litellm/               # Model gateway
│   └── letta/                 # Agent orchestration
├── content-template/          # Template for content repository
│   ├── memories/              # Memory files structure
│   ├── history/               # Conversation logs
│   ├── projects/              # Project context
│   └── README.md
├── archive/                   # Archived docker-compose setup
│   └── docker-compose.yaml
├── DEPLOYMENT.md              # Coolify deployment guide
├── ARCHITECTURE_NOTES.md      # Architecture documentation
└── README.md                  # This file
```

## Environment Variables

Each service has its own `.env.example` file in [`services/{service}/`](services/). Key variables:

**Required API Keys:**
- `OPENAI_API_KEY` - OpenAI API key for GPT models and embeddings
- `ANTHROPIC_API_KEY` - Anthropic API key for Claude models

- **Service Keys (generate secure random strings):**
- `LETTA_SERVER_PASS` - Admin password for Letta (use: `openssl rand -base64 32`)
- `LITELLM_MASTER_KEY` - Master key for LiteLLM (use: `openssl rand -hex 32`) — optional; LiteLLM may be run without authentication for local/dev but this is insecure for production.
- `POSTGRES_PASSWORD` - Password for PostgreSQL database

**Required Git Sync:**
- `GIT_SYNC_REPO` - Your content repository URL
- `GIT_USERNAME` - GitHub username
- `GIT_PASSWORD` - GitHub Personal Access Token (with Contents: Read/Write)

## Memory System

The memory system uses a **two-repository architecture**:

### Configuration Repository (This Repo)

Contains deployment configuration, service definitions, and documentation.

### Content Repository

Contains your actual memory files, managed automatically by Git Sync:

- **`memories/`** - Long-term semantic memory
  - `personal/` - User-specific private memory
  - `shared/` - Household/team shared memory
- **`history/`** - Episodic memory (timestamped interaction logs)
- **`projects/`** - Project-scoped context and notes

See [`content-template/README.md`](content-template/README.md) for details on the memory system.

### How It Works

1. **Git Sync** clones your content repository to a shared volume
2. The knowledgebase / memory service reads/writes Markdown files from the shared volume
3. **Git Sync** detects file changes and commits/pushes to GitHub
4. **Qdrant** indexes memory content for semantic search
5. **PostgreSQL** stores metadata
6. **Letta** agents use memories via the knowledgebase API

### Memory File Format

Markdown files with YAML frontmatter:

```markdown
---
title: "Example Memory"
tags: ["preference", "personal"]
created: "2025-11-23T10:30:00Z"
---

# Memory Content

Human-readable Markdown content.
```

## Service Architecture

```
┌─────────────┐
│   Letta     │ ← Agent orchestration + Web UI
│   :8283     │
└──────┬──────┘
       │
      ├──────→ ┌─────────────┐
      │        │   Memory Service      │ ← Memory management (Git + Markdown)
      │        │   :8080     │
       │        └──────┬──────┘
       │               │
      │               ├──────→ ┌─────────────┐
      │               │        │  PostgreSQL │ ← memory metadata DB (Coolify-managed)
       │               │        └─────────────┘
       │               │
       │               ├──────→ ┌─────────────┐
       │               │        │   Qdrant    │ ← Vector storage
       │               │        │   :6333     │
       │               │        └─────────────┘
       │               │
       │               ├──────→ ┌─────────────┐
       │               │        │    Redis    │ ← Cache layer
       │               │        │   :6379     │
       │               │        └─────────────┘
       │               │
       │               └──────→ ┌─────────────┐
       │                        │  Git Sync   │ ← Memory sync to GitHub
       │                        └─────────────┘
       │
       └──────→ ┌─────────────┐
                │  LiteLLM    │ ← Model gateway (OpenAI, Claude, etc.)
                │   :4000     │
                └─────────────┘
```

All services communicate via Coolify's internal Docker networking.

## Configuration

### LiteLLM Configuration

Edit [`services/litellm/litellm-config.yaml`](services/litellm/litellm-config.yaml) to configure model routing:

```yaml
general_settings:
  default_model: openai/gpt-4
  telemetry: true

model_routing:
  openai/gpt-4:
    model_name: gpt-4
    provider: openai

  anthropic/claude-3-5-sonnet:
    model_name: claude-3-5-sonnet-20241022
    provider: anthropic
```

### Service Configuration

Each service has its own configuration:

- **Git Sync**: [`services/git-sync/.env.example`](services/git-sync/.env.example)
- **Redis**: Uses defaults, see [`services/redis/`](services/redis/)
- **Qdrant**: Uses defaults, see [`services/qdrant/`](services/qdrant/)
- **Knowledgebase**: Service-specific environment configuration
- **LiteLLM**: [`services/litellm/.env.example`](services/litellm/.env.example)
- **Letta**: [`services/letta/.env.example`](services/letta/.env.example)

## Deployment

See [`DEPLOYMENT.md`](DEPLOYMENT.md) for complete deployment instructions.

### Quick Deployment Checklist

1. ✅ Create PostgreSQL database in Coolify
2. ✅ Deploy Redis
3. ✅ Deploy Qdrant
4. ✅ Create and push content repository (fork `content-template/`)
5. ✅ Deploy Git Sync (with content repo credentials)
6. ✅ Deploy Knowledgebase/Memory Service API
7. ✅ Deploy LiteLLM
8. ✅ Deploy Letta
9. ✅ Access Web UI and create first agent

### Health Checks

All services include health checks visible in Coolify dashboard. Services should show **healthy** status within 1-2 minutes of deployment.

## Backup and Recovery

### What's Backed Up Automatically

**Critical Data (Git-versioned in content repository):**
- ✅ All memory files (`memories/`, `history/`, `projects/`)
- ✅ Full edit history (Git commits)
- ✅ Timestamps and metadata

**Managed by Coolify:**
- ✅ PostgreSQL database (automatic backups)
- ✅ Service configurations

**Can be Rebuilt:**
- Qdrant vectors (re-index from memory files)
- Redis cache (ephemeral data)

### Backup Strategy

**Essential**: Git repository (memories, history, projects)
- Automatically synced to GitHub by Git Sync service
- This is your **only irreplaceable data**

**Optional**: Qdrant snapshots
- Saves re-indexing time but can be rebuilt

**Automatic**: PostgreSQL backups via Coolify

### Disaster Recovery

If you lose everything except your GitHub content repository:

1. Redeploy all services in Coolify
2. Git Sync clones content from GitHub
3. Knowledgebase automatically:
   - Regenerates PostgreSQL metadata from Markdown files
   - Re-indexes all content into Qdrant vectors
   - Restores full system state

Your memory is fully restored! 🎉

## Monitoring

### Service Health

View in Coolify dashboard:
- All services should show **healthy** status
- Check logs for any errors or warnings

### Memory Sync Status

Check your content repository on GitHub:
- Recent commits should show auto-sync messages
- Verify memory files are being updated

### Resource Usage

Monitor in Coolify:
- Memory usage per service
- CPU utilization
- Disk space (especially Qdrant and PostgreSQL)

## Troubleshooting

### Check Service Logs

In Coolify, view logs for any service that's unhealthy.

### Common Issues

**Git Sync not pushing**:
- Verify GitHub PAT is valid and has write permissions
- Check `GIT_SYNC_REPO` URL is correct
- Review Git Sync logs for errors

**Knowledgebase can't connect to services**:
- Ensure all services are in same Coolify project
- Verify service names match environment variables
- Check PostgreSQL, Redis, Qdrant are healthy

**Letta can't reach Knowledgebase or LiteLLM**:
- Verify `KNOWLEDGEBASE_URL` and `LLM_API_URL` are correct
- Check both services are healthy
- Test connectivity from Coolify terminal

**Memory files not found**:
- Verify Git Sync cloned content successfully
- Check `bears-memory` volume is shared between Git Sync and Knowledgebase
- Review Git Sync logs for clone errors

For detailed troubleshooting, see service-specific deployment guides in [`services/`](services/).

## Security Considerations

- ✅ Use strong passwords for all services
- ✅ Rotate API keys and tokens regularly
- ✅ Keep content repository private on GitHub
- ✅ Use Coolify's built-in authentication for external access
- ✅ Enable HTTPS via Coolify proxy for public endpoints
- ❌ Never commit API keys or passwords to Git
- ❌ Don't expose internal services publicly without authentication

## Development

### Repository Structure

This is a **configuration repository** - it contains deployment configs and documentation, not application code.

**For local development**, see the archived `docker-compose.yaml` in [`archive/`](archive/).

**For Coolify deployment**, follow service-specific guides in [`services/`](services/).

### Contributing

When making changes:

1. Test deployment in a Coolify development environment
2. Update relevant `COOLIFY_DEPLOY.md` files
3. Update this README and `DEPLOYMENT.md`
4. Document any new environment variables in `.env.example` files

## Documentation

- **[DEPLOYMENT.md](DEPLOYMENT.md)** - Complete Coolify deployment guide
- **[ARCHITECTURE_NOTES.md](ARCHITECTURE_NOTES.md)** - Detailed architecture documentation
- **[content-template/README.md](content-template/README.md)** - Memory system guide
- **Service-specific guides** - See [`services/{service}/COOLIFY_DEPLOY.md`](services/)

## Support

For issues or questions:

- **Deployment issues**: Check service-specific `COOLIFY_DEPLOY.md` guides
- **Memory system**: See `content-template/README.md`
- **Architecture questions**: Review `ARCHITECTURE_NOTES.md`
- **Service logs**: View in Coolify dashboard

## License

[Add your license here]

## Acknowledgments

Built with:
- [Letta](https://github.com/letta-ai/letta) - Agent framework
- Knowledgebase service - Memory management
- [Qdrant](https://github.com/qdrant/qdrant) - Vector database
- [LiteLLM](https://github.com/BerriAI/litellm) - LLM gateway
- [Coolify](https://coolify.io) - Deployment platform

