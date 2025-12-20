# Architecture Notes

## BEARS Stack Architecture

The BEARS Stack uses a layered architecture with specialized services deployed independently in Coolify for maximum flexibility and scalability.

### Deployment Model

**Platform**: Coolify (self-hosted PaaS)
**Orchestration**: Individual service deployments (not docker-compose)
**Networking**: Coolify internal Docker networking
**Storage**: Named volumes + shared volumes + Coolify-managed PostgreSQL

### Current Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Application Layer                    │
│                                                          │
│  ┌─────────────┐                                        │
│  │ LibreChat  │ ← Primary UI for agent interactions    │
│  │   :3080     │   (Modern chat interface)              │
│  └──────┬──────┘                                        │
└─────────┼───────────────────────────────────────────────┘
           │
┌─────────┼───────────────────────────────────────────────┐
│         │              Agent Layer                       │
│         │                                                │
│         └──────→ ┌─────────────┐                        │
│                  │   Letta     │ ← Agent orchestration   │
│                  │   :8283     │   (Tools, memory, state) │
│                  └──────┬──────┘                        │
└─────────┼───────────────────────────────────────────────┘
           │
┌─────────┼───────────────────────────────────────────────┐
│         │              API Layer                        │
│         │                                                │
│         ├──────→ ┌─────────────┐                        │
│         │        │ Knowledgebase│ ← Memory management   │
│         │        │   :8080     │   (Git + Markdown)     │
│         │        └──────┬──────┘                        │
│         │               │                                │
│         └──────→ ┌─────────────┐                        │
│                  │  LiteLLM    │ ← Model gateway        │
│                  │   :4000     │   (OpenAI, Claude)     │
│                  └─────────────┘                        │
└────────────────────────────────────────────────────────┘
                         │
┌───────────────────────┼─────────────────────────────────┐
│                       │        Memory Layer              │
│                       │                                  │
│                       └──────→ ┌─────────────┐          │
│                                │  Git Sync   │          │
│                                │             │          │
│                                └──────┬──────┘          │
│                                       │                  │
│                                       ↓                  │
│                              Shared Volume: bears-memory │
│                              (memories/, history/,       │
│                               projects/, .git/)          │
└──────────────────────────────────────────────────────────┘
                                        │
┌──────────────────────────────────────┼──────────────────┐
│                                      │  Infrastructure  │
│                                      │                  │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐    │
│  │ PostgreSQL  │  │    Redis    │  │   Qdrant    │    │
│  │  (Coolify-  │  │   :6379     │  │   :6333     │    │
│  │   managed)  │  │             │  │             │    │
│  └─────────────┘  └─────────────┘  └─────────────┘    │
│   (metadata)        (cache)          (vectors)         │
└──────────────────────────────────────────────────────────┘
           │                  │                  │
           └──────────────────┴──────────────────┘
                              │
                     Coolify Internal Network
```

### Service Responsibilities

#### Git Sync (Memory Synchronization)

- **Purpose**: Bidirectional Git synchronization between Coolify and GitHub
- **Image**: Custom (Alpine + git + inotifywait)
- **Key Features**:
  - Clones content repository on startup
  - Watches for file changes using inotify
  - Commits and pushes immediately on changes
  - Pulls from origin every 5 minutes (configurable)
  - Uses rebase strategy for conflict resolution
  - Shares volume with the memory service

#### LibreChat (Primary Chat UI)

- **Purpose**: Modern, feature-rich chat interface for interacting with Letta agents
- **Image**: `ghcr.io/cpfiffer/letta-libre:latest`
- **Ports**:
  - 3080 (Web UI)
- **Key Features**:
  - Multi-user authentication with MongoDB
  - Conversation management and search
  - File uploads and code execution
  - Integration with Letta agent APIs
  - Modern responsive UI

#### Letta (Agent Framework)

- **Purpose**: Agent orchestration, tool execution, and workflow management
- **Image**: `letta/letta:latest`
- **Ports**:
  - 8283 (API Server - internal)
- **Key Features**:
  - Agent creation and management
  - Tool execution framework
  - Conversation management
  - Integration with an external knowledgebase (memory service) for memory
  - Model routing via LiteLLM
  - Admin API for external UI integration

#### Knowledgebase / Memory Service

- **Purpose**: Git-versioned memory system with Markdown files and a vector search API
- **Image**: (varies — this repository uses a separate knowledgebase adapter)
- **Port**: 8080 (example)
- **Key Features**:
  - Manages `memories/`, `history/`, and `projects/` directories
  - Reads/writes Markdown files with YAML frontmatter
  - PostgreSQL backend for metadata (optional)
  - Integration with Qdrant (or other vector DB) for semantic search
  - Shares volume with Git Sync

#### Qdrant (Vector Database)

-- **Purpose**: Semantic memory and vector storage
-- **Image**: `qdrant/qdrant:latest`
-- **Port**: 6333
-- **Key Features**:
  - Vector embeddings for semantic search
  - Fast similarity search
  - Used by the memory service / knowledgebase for RAG capabilities
  - Collections auto-created by the memory service or adapter

#### LiteLLM (Model Gateway)

- **Purpose**: Unified interface to multiple LLM providers
- **Image**: Custom (Python + litellm[proxy])
- **Port**: 4000
- **Key Features**:
  - Model-agnostic routing
  - Support for OpenAI, Anthropic, and other providers
  - Consistent API interface
  - Load balancing and fallbacks

#### Redis (Cache)

-- **Purpose**: Cache layer for the memory service
- **Image**: `redis:7-alpine`
- **Port**: 6379
- **Key Features**:
  - Fast temporary storage
  - Session data caching
  - Query result caching

#### PostgreSQL (Database)

-- **Purpose**: Backend database for the memory service
- **Deployment**: Coolify-managed service
- **Port**: 5432 (internal only)
- **Key Features**:
  - Stores memory-service metadata
  - Automatic backups via Coolify
  - Managed updates and maintenance### Memory System

The memory system uses a **two-repository architecture**:

#### Configuration Repository (This Repo)

**Purpose**: Deployment configuration, service definitions, documentation

**Contents**:
- Service configurations (`services/`)
- Deployment guides (`COOLIFY_DEPLOY.md` files)
- Environment variable templates (`.env.example` files)
- Architecture documentation

#### Content Repository

**Purpose**: Actual memory files, version-controlled and backed up

**Structure**:
1. **`memories/`** - Long-term semantic memory
   - Markdown files with YAML frontmatter
   - Git-versioned for full history
   - Human-readable and editable
   - Separated into `personal/` and `shared/` contexts

2. **`history/`** - Episodic memory
   - Session transcripts and logs
   - Timestamped interactions
   - JSON or Markdown format

3. **`projects/`** - Project memory
   - Project-scoped context
   - Goals, notes, and progress tracking
   - Enables long-term continuity

4. **`.git/`** - Git repository metadata
   - Managed by Git Sync service
   - Provides version control and audit trail

#### Memory File Format

Markdown with YAML frontmatter:

```markdown
---
title: "Example Memory"
tags: ["preference", "personal"]
created: "2025-11-23T10:30:00Z"
updated: "2025-11-23T15:45:00Z"
---

# Memory Content

Human-readable Markdown content that describes
the memory, preference, or context.
```

#### Shared Volume Architecture

**Critical Design**: Git Sync and the memory service share the same volume!

```
Volume: bears-memory (shared)
├── Git Sync mounts at: /data
└── Memory service mounts at: /app/memory

Data flow:
1. Git Sync clones repo → /data
2. Memory service reads/writes → /app/memory (same volume)
3. Git Sync detects changes → commits + pushes
4. Memory service indexes → Qdrant (vectors) + PostgreSQL (metadata)
```

### Data Flow

1. **User Interaction** → LibreChat receives user input via modern chat UI
2. **Agent Request** → LibreChat sends request to Letta agent API
3. **Memory Retrieval** → Letta queries the knowledgebase (memory service) for relevant context
4. **Semantic Search** → The memory service uses Qdrant for vector similarity
5. **LLM Inference** → Letta routes to appropriate model via LiteLLM
6. **Agent Response** → Letta processes response and sends back to LibreChat
7. **Memory Update** → Letta/memory service writes Markdown file to shared volume
8. **Git Synchronization** → Git Sync detects change, commits, and pushes to GitHub
9. **Vector Indexing** → Memory service updates Qdrant with new embeddings
10. **Metadata Storage** → Memory service updates PostgreSQL with file metadata

### Deployment Considerations

#### Coolify-Specific Architecture

- All services deployed as **individual resources** in Coolify
- **Internal networking**: Services communicate via Coolify's Docker network using service names
- **Volumes**: Named volumes for persistence + shared volume for memory files
- **PostgreSQL**: Coolify-managed database service with automatic backups
- **No docker-compose**: Services are independent for maximum flexibility

#### Volume Strategy

**Named Volumes** (service-specific):
- `bears-redis-data` → Redis persistence
- `bears-qdrant-data` → Vector database storage
- `bears-letta-data` → Letta configuration

**Shared Volume** (multi-service):
- `bears-memory` → Shared between Git Sync and the memory service for memory files

**Coolify-Managed**:
- PostgreSQL data → Handled by Coolify with automatic backups

#### Service Discovery

Services communicate using Coolify's internal DNS:

```bash
# Format: <service-name>:<port>
redis://bears-redis:6379
http://bears-qdrant:6333
http://bears-knowledgebase:8080
http://bears-litellm:4000
http://bears-letta:8283
postgresql://<postgres-host>:5432/<memory-db>
```

**Note**: Service names must match exactly what's configured in Coolify.

### Port Mapping

| Service | Internal Port | External Access | Purpose |
|---------|--------------|-----------------|---------|
| LibreChat | 3080 | Via Coolify proxy | Primary chat UI |
| Letta | 8283 | Internal only | Agent orchestration API |
| Knowledgebase / Memory Service | 8080 | Optional | Memory API |
| Qdrant | 6333 | Internal only | Vector DB |
| LiteLLM | 4000 | Internal only | Model gateway |
| Redis | 6379 | Internal only | Cache |
| PostgreSQL | 5432 | Internal only | Database |
| Git Sync | N/A | No exposed ports | Sync service |

**Security**: Only LibreChat should be exposed externally. All other services are internal-only.

### Security Notes

-- Authentication disabled on the memory service for local deployment (if applicable)
- All services on private Docker network
- Only specified ports exposed to host
- Sensitive data in `.env` file (not committed to Git)

### Future Enhancements

-- Add authentication to the memory service API
- Implement MCP (Modular Content Providers) for external data
- Add web UI for memory browsing/editing
- Implement multi-agent collaboration
- Add monitoring and observability
