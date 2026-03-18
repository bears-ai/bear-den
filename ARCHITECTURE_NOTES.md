# Architecture Notes

## BEARS Stack Architecture

The BEARS Stack uses a layered architecture with specialized services deployed independently in Coolify for maximum flexibility and scalability.

### Deployment Model

**Platform**: Coolify (self-hosted PaaS)
**Orchestration**: Individual service deployments (not docker-compose)
**Networking**: Coolify internal Docker networking
**Storage**: Named volumes + shared volumes + Coolify-managed PostgreSQL

### Target architecture (Cabinet + Letta)

**Cabinet** (backed by **Outline**) is the **shared knowledgebase**: documents that **humans edit in Outline** and **agents access via BEARS Core**. It **does not replace** Letta’s native memory (blocks, conversations, built-in memory tools).

**The Git Sync + Qdrant + standalone knowledgebase service stack is obviated** by Cabinet for that shared-knowledge role. See [PLAN.md](PLAN.md).

```
┌─────────────────────────────────────────────────────────┐
│  OpenWebUI (:3000)          Outline (Cabinet UI)         │
└────────┬────────────────────────────┬──────────────────┘
         │                            │
         ▼                            │ docs / search
┌────────────────┐             ┌──────┴──────┐
│  BEARS Core    │──Cabinet───▶│  Outline    │
│  (proxy, auth) │   tools     │  (storage)  │
└────────┬───────┘             └─────────────┘
         │
         ▼
┌────────────────┐     ┌─────────────┐
│     Letta        │────▶│  LiteLLM    │
│  native memory   │     │   :4000     │
└────────────────┘     └─────────────┘
```

### Legacy architecture (Git + Qdrant knowledgebase)

Optional during migration. **Superseded by Cabinet** for human+agent shared knowledge.

```
OpenWebUI → Letta → Knowledgebase (:8080) ← Git Sync + bears-memory
                         ↓
                    Qdrant + Redis + PostgreSQL
```

### Service Responsibilities

#### Cabinet (Outline) — target shared knowledgebase

- **Purpose**: Long-lived knowledge **humans and agents** share; agents call Cabinet through **BEARS Core** (see [PLAN.md](PLAN.md)).
- **Does not replace**: Letta per-agent memory blocks and conversation state.
- **Obviates**: The separate Git+Qdrant “knowledgebase service” for that use case.

#### Git Sync (legacy — memory synchronization)

- **Purpose** (legacy only): Bidirectional Git synchronization for the old Markdown+Qdrant knowledgebase
- **Image**: Custom (Alpine + git + inotifywait)
- **Key Features**:
  - Clones content repository on startup
  - Watches for file changes using inotify
  - Commits and pushes immediately on changes
  - Pulls from origin every 5 minutes (configurable)
  - Uses rebase strategy for conflict resolution
  - Shares volume with the memory service

#### OpenWebUI (Primary Chat UI)

- **Purpose**: Modern, feature-rich chat interface for interacting with Letta agents
- **Image**: `ghcr.io/open-webui/open-webui:main` (or latest)
- **Ports**:
  - 3000 (Web UI)
- **Key Features**:
  - Multi-user authentication
  - Conversation management and search
  - File uploads and code execution
  - Function/tool calling support
  - Integration with Letta agents via open-webui-tools functions
  - Modern responsive UI
  - Extensible via functions, tools, and filters

#### Letta (Agent Framework)

- **Purpose**: Agent orchestration, tool execution, and workflow management
- **Image**: `letta/letta:latest`
- **Ports**:
  - 8283 (API Server - internal)
- **Key Features**:
  - Agent creation and management
  - Tool execution framework
  - Conversation management
  - **Native agent memory** (blocks, conversations); optional **Cabinet tools** via BEARS for shared Outline knowledge. Legacy: external knowledgebase API (Git+Qdrant)
  - Model routing via LiteLLM
  - Admin API for external UI integration

#### Knowledgebase / Memory Service (legacy)

- **Purpose** (legacy): Git-versioned Markdown + vector search API—**obviated by Cabinet (Outline)** for shared agent+human knowledge
- **Port**: 8080 (example)
- **When to deploy**: Only if still migrating or maintaining the old stack

#### Qdrant (Vector Database) — legacy KB

- **Purpose** (legacy): Vectors for the old knowledgebase service
- **Image**: `qdrant/qdrant:latest`
- **Port**: 6333
- **Key Features**:
  - Vector embeddings for semantic search
  - Fast similarity search
  - Used by the **legacy** memory/knowledgebase service if deployed
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

- **Purpose**: Cache for **legacy** knowledgebase (if deployed); other uses per service
- **Image**: `redis:7-alpine`
- **Port**: 6379
- **Key Features**:
  - Fast temporary storage
  - Session data caching
  - Query result caching

#### PostgreSQL (Database)

- **Purpose**: Backend for **legacy** memory service, Outline, Letta, or other apps as deployed
- **Deployment**: Coolify-managed service
- **Port**: 5432 (internal only)
- **Key Features**:
  - Stores memory-service metadata
  - Automatic backups via Coolify
  - Managed updates and maintenance

### Knowledge and memory (terminology)

| Component | Role |
|-----------|------|
| **Letta memory** | Per-agent blocks, conversations, built-in memory tools—not replaced by Cabinet. |
| **Cabinet (Outline)** | Shared knowledgebase for humans + agents (via BEARS). |
| **Legacy content repo** | `memories/`, `history/`, `projects/` + Git Sync—**legacy**; migrate to Outline decks/collections when adopting Cabinet. |

See [PLAN.md](PLAN.md) and [content-template/README.md](content-template/README.md).

#### Legacy: two-repository Git-backed KB

If you still run Git Sync + knowledgebase: configuration in this repo, Markdown files in a **content repository**, shared `bears-memory` volume, Qdrant indexing. **Obviated by Cabinet** for new designs.

### Data flow (target)

1. User → OpenWebUI (or LettaBot via BEARS Core) → Letta  
2. Letta uses **native memory** + model calls via LiteLLM  
3. When agents use **Cabinet tools**, BEARS Core proxies to **Outline**; humans edit the same docs in Outline’s UI  

### Data flow (legacy KB)

1. User → OpenWebUI → Letta → legacy knowledgebase API → Qdrant + Git-backed Markdown → Git Sync → GitHub  
2. Prefer migrating this flow to Cabinet per PLAN.md  

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

**Shared Volume** (legacy multi-service):
- `bears-memory` → Git Sync + legacy knowledgebase service only

**Coolify-Managed**:
- PostgreSQL data → Handled by Coolify with automatic backups

#### Service Discovery

Services communicate using Coolify's internal DNS:

```bash
# Format: <service-name>:<port>
redis://bears-redis:6379
http://bears-qdrant:6333
http://bears-knowledgebase:8080  # legacy KB only
http://bears-litellm:4000
http://bears-letta:8283
postgresql://<postgres-host>:5432/<memory-db>
```

**Note**: Service names must match exactly what's configured in Coolify.

### Port Mapping

| Service | Internal Port | External Access | Purpose |
|---------|--------------|-----------------|---------|
| OpenWebUI | 3000 | Via Coolify proxy | Primary chat UI |
| Letta | 8283 | Internal only | Agent orchestration API |
| Knowledgebase (legacy) | 8080 | Optional | Legacy memory API |
| Qdrant | 6333 | Internal only | Legacy vector DB |
| LiteLLM | 4000 | Internal only | Model gateway |
| Redis | 6379 | Internal only | Cache |
| PostgreSQL | 5432 | Internal only | Database |
| Git Sync | N/A | No exposed ports | Sync service |

**Security**: Only OpenWebUI should be exposed externally. All other services are internal-only.

### Security Notes

- Authentication disabled on the memory service for local deployment (if applicable)
- All services on private Docker network
- Only specified ports exposed to host
- Sensitive data in `.env` file (not committed to Git)

### Current Integration: OpenWebUI + Letta

**Current Setup**: Letta agents are connected to OpenWebUI as "models" using functions from [open-webui-tools](https://github.com/Haervwe/open-webui-tools). This allows users to select and interact with Letta agents directly from the OpenWebUI interface. This is suitable for single-organization or development deployments where all users share access to the same Letta instance.

**Integration Method**:
- Functions from open-webui-tools repository are installed in OpenWebUI
- Letta agents appear as selectable models in OpenWebUI's model list
- Direct API communication between OpenWebUI and Letta service

**For multi-user production** with per-user agents, user identity mapping, and access control, the canonical approach is the **Authentication Proxy** architecture. See **[MULTIUSER_PROXY_ARCHITECTURE.md](MULTIUSER_PROXY_ARCHITECTURE.md)** for the full design. In that model, OpenWebUI and LettaBot talk to the proxy (not directly to Letta); the proxy handles auth, user→agent routing, and enforcement. Session and mapping strategies for the current direct-integration setup are documented in `services/letta/OPENWEBUI_SESSIONS.md`.

### Future enhancements

- BEARS Core + Cabinet (Outline) per [PLAN.md](PLAN.md)
- Deprecate legacy Git+Qdrant knowledgebase after migration
- MCP, multi-agent collaboration, observability
