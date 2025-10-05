# 🐻 BEARS Stack

**B**ears **E**volving **A**gentic **R**easoning **S**ystem

Configuration for our agentic assistants platform, using:

- [Letta](https://github.com/letta-ai/letta) – Agent server and orchestration layer
- [Onyx](https://github.com/onyx-ai/onyx) – Long-term vector memory
- [Qdrant](https://github.com/qdrant/qdrant) – Vector DB backend for Onyx
- [LiteLLM](https://github.com/BerriAI/litellm) – Unified LLM gateway
- [Coolify](https://coolify.io) (optional) – PaaS for deployment and management

## Quick Start

### Prerequisites

- Docker and Docker Compose installed
- API keys for OpenAI and/or Anthropic
- At least 4GB RAM available for containers

### Initial Setup

1. **Clone the repository**
   ```bash
   git clone <repository-url>
   cd bears-stack
   ```

2. **Configure environment variables**
   ```bash
   cp .env.example .env
   # Edit .env and add your API keys
   ```

3. **Start the services**
   ```bash
   docker-compose up -d
   ```

4. **Verify services are running**
   ```bash
   docker-compose ps
   ```

### Service Endpoints

Once deployed, the following services will be available:

- **Letta ADE (Web UI)**: http://localhost:8283
- **Letta API Server**: http://localhost:8080
- **Onyx Memory System**: http://localhost:9090
- **Qdrant Vector DB**: http://localhost:6333
- **LiteLLM Gateway**: http://localhost:4000

## Environment Variables

Required environment variables (see [`.env.example`](.env.example)):

- `OPENAI_API_KEY` - OpenAI API key for GPT models and embeddings
- `ANTHROPIC_API_KEY` - Anthropic API key for Claude models
- `LETTA_API_KEY` - Authentication key for Letta API (generate a secure random string)
- `LITELLM_MASTER_KEY` - Master key for LiteLLM gateway (generate a secure random string)

## Architecture

### Memory System

The BEARS Stack implements a hybrid memory architecture:

- **Basic Memory** (`memories/`) - Human-editable Markdown files with Git versioning
  - `personal/` - User-specific private memory
  - `shared/` - Household shared memory
- **Episodic Memory** (`history/`) - Timestamped interaction logs
- **Project Memory** (`projects/`) - Project-scoped context and notes
- **Semantic Memory** - Vector embeddings in Qdrant for RAG

See [`memories/README.md`](memories/README.md) for details on the memory system.

### Service Architecture

```
┌─────────────┐
│   Letta     │ ← Agent orchestration & tool execution
│ ADE: :8283  │ ← Web UI for agent management
│ API: :8080  │
└──────┬──────┘
       │
       ├──────→ ┌─────────────┐
       │        │   Onyx      │ ← Memory management
       │        │   :9090     │
       │        └──────┬──────┘
       │               │
       │               ↓
       │        ┌─────────────┐
       │        │   Qdrant    │ ← Vector storage
       │        │   :6333     │
       │        └─────────────┘
       │
       └──────→ ┌─────────────┐
                │  LiteLLM    │ ← Model gateway
                │   :4000     │
                └─────────────┘
```

## Configuration

### LiteLLM Configuration

Edit [`litellm-config.yaml`](litellm-config.yaml) to configure model routing:

```yaml
general_settings:
  default_model: openai/gpt-4
  telemetry: true

model_routing:
  openai/gpt-4:
    model_name: gpt-4
    provider: openai
```

### Onyx Configuration

Edit [`onyx_config/onyx.yaml`](onyx_config/onyx.yaml) to configure memory collections and embeddings.

## Deployment

### Local Development

```bash
docker-compose up
```

### Production Deployment with Coolify

1. Import this repository into Coolify
2. Set environment variables in Coolify dashboard
3. Deploy using the provided `docker-compose.yaml`

### Health Checks

All services include health checks. Monitor with:

```bash
docker-compose ps
```

Healthy services will show `(healthy)` status.

## Maintenance

### Viewing Logs

```bash
# All services
docker-compose logs -f

# Specific service
docker-compose logs -f letta
```

### Backing Up Data

```bash
# Backup vector database
docker-compose exec qdrant tar czf /tmp/qdrant-backup.tar.gz /qdrant/storage
docker cp $(docker-compose ps -q qdrant):/tmp/qdrant-backup.tar.gz ./backups/

# Backup memory files (already in Git)
git add memories/ history/ projects/
git commit -m "Backup memory state"
```

### Updating Services

```bash
docker-compose pull
docker-compose up -d
```

## Troubleshooting

### Port Conflicts

If you see port binding errors, check for conflicts:

```bash
# Check what's using the ports
lsof -i :3000  # Letta
lsof -i :8080  # Onyx
lsof -i :6333  # Qdrant
lsof -i :4000  # LiteLLM
```

### Service Won't Start

Check logs for the specific service:

```bash
docker-compose logs <service-name>
```

### Memory Not Persisting

Ensure named volumes are properly configured:

```bash
docker volume ls | grep bears-stack
```

## Development

### Project Structure

```
bears-stack/
├── docker-compose.yaml      # Service orchestration
├── litellm-config.yaml      # LLM gateway configuration
├── onyx_config/
│   └── onyx.yaml           # Memory system configuration
├── memories/               # Long-term memory (Git-tracked)
│   ├── personal/          # User-specific memory
│   └── shared/            # Household shared memory
├── history/               # Episodic memory logs
├── projects/              # Project-scoped context
└── .kilocode/
    └── memory_bank/       # Project documentation
```

## Documentation

- [Project Brief](.kilocode/memory_bank/project_brief.md) - Overall goals and architecture
- [Memory Architecture](.kilocode/memory_bank/memory_architecture_brief.md) - Detailed memory system design
- [Memories README](memories/README.md) - Memory file format and usage
- [History README](history/README.md) - Episodic memory structure
- [Projects README](projects/README.md) - Project-scoped memory

## License

[Add your license here]

## Contributing

[Add contribution guidelines here]

