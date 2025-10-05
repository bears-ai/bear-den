# Architecture Notes

## BEARS Stack Architecture

The BEARS Stack uses a layered architecture with specialized services for different aspects of the agentic assistant system.

### Current Architecture

```
┌─────────────┐
│   Letta     │ ← Agent orchestration & tool execution
│ ADE: :8283  │ ← Web UI for agent management
│ API: :3000  │
└──────┬──────┘
       │
       ├──────→ ┌─────────────┐
       │        │   Onyx      │ ← Memory management (Git + Markdown)
       │        │   :8080     │
       │        └──────┬──────┘
       │               │
       │               ├──────→ ┌─────────────┐
       │               │        │  PostgreSQL │ ← Onyx database
       │               │        └─────────────┘
       │               │
       │               └──────→ ┌─────────────┐
       │                        │   Qdrant    │ ← Vector storage
       │                        │   :6333     │
       │                        └─────────────┘
       │
       └──────→ ┌─────────────┐
                │  LiteLLM    │ ← Model gateway (OpenAI, Claude, etc.)
                │   :4000     │
                └─────────────┘
```

### Service Responsibilities

#### Letta (Agent Framework)
- **Purpose**: Agent orchestration, tool execution, and workflow management
- **Ports**: 
  - 8283 (Web UI - Admin Development Environment)
  - 3000 (API Server)
- **Key Features**:
  - Agent creation and management
  - Tool execution framework
  - Conversation management
  - Integration with Onyx for memory

#### Onyx (Memory Management)
- **Purpose**: Git-versioned memory system with Markdown files
- **Port**: 8080
- **Key Features**:
  - Manages `memories/`, `history/`, and `projects/` directories
  - Git version control for all memory changes
  - Human-readable Markdown format
  - PostgreSQL backend for metadata
  - Integration with Qdrant for semantic search

#### Qdrant (Vector Database)
- **Purpose**: Semantic memory and vector storage
- **Port**: 6333
- **Key Features**:
  - Vector embeddings for semantic search
  - Fast similarity search
  - Used by Onyx for RAG capabilities

#### LiteLLM (Model Gateway)
- **Purpose**: Unified interface to multiple LLM providers
- **Port**: 4000
- **Key Features**:
  - Model-agnostic routing
  - Support for OpenAI, Anthropic, and other providers
  - Consistent API interface

#### PostgreSQL (Database)
- **Purpose**: Backend database for Onyx
- **Port**: Internal only (not exposed)
- **Key Features**:
  - Stores Onyx metadata
  - Persistent storage via Docker volume

### Memory System

The memory system is managed by Onyx and consists of:

1. **Basic Memory** (`memories/`)
   - Markdown files with YAML frontmatter
   - Git-versioned for full history
   - Human-readable and editable
   - Separated into `personal/` and `shared/` contexts

2. **Episodic Memory** (`history/`)
   - Session transcripts and logs
   - Timestamped interactions
   - JSON or Markdown format

3. **Project Memory** (`projects/`)
   - Project-scoped context
   - Goals, notes, and progress tracking
   - Enables long-term continuity

4. **Semantic Memory** (Qdrant)
   - Vector embeddings of memory content
   - Enables semantic search and RAG
   - Automatically indexed by Onyx

### Data Flow

1. **User Interaction** → Letta receives request
2. **Memory Retrieval** → Letta queries Onyx for relevant context
3. **Semantic Search** → Onyx uses Qdrant for vector similarity
4. **LLM Inference** → Letta routes to appropriate model via LiteLLM
5. **Memory Update** → Onyx commits changes to Git and updates vectors

### Deployment Considerations

- All services run in Docker containers
- Named volumes for persistent data:
  - `qdrant_data` - Vector database storage
  - `letta_data` - Letta configuration and state
  - `onyx_db_data` - PostgreSQL database
- Memory directories mounted from host:
  - `./memories` → `/app/memories` (in Onyx)
  - `./history` → `/app/history` (in Onyx)
  - `./projects` → `/app/projects` (in Onyx)

### Port Mapping

| Service | Internal Port | External Port | Purpose |
|---------|--------------|---------------|---------|
| Letta API | 8080 | 3000 | API access |
| Letta ADE | 8283 | 8283 | Web UI |
| Onyx | 8080 | 8080 | Memory API |
| Qdrant | 6333 | 6333 | Vector DB |
| LiteLLM | 4000 | 4000 | Model gateway |
| PostgreSQL | 5432 | - | Internal only |

### Security Notes

- Authentication disabled on Onyx for local deployment
- All services on private Docker network
- Only specified ports exposed to host
- Sensitive data in `.env` file (not committed to Git)

### Future Enhancements

- Add authentication to Onyx API
- Implement MCP (Modular Content Providers) for external data
- Add web UI for memory browsing/editing
- Implement multi-agent collaboration
- Add monitoring and observability