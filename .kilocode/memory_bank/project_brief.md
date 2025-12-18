# Project Brief: BEARS Stack

**B**ears **E**volving **A**gentic **R**easoning **S**ystem

## Project Overview

BEARS Stack is a long-running, modular assistant system that supports persistent identity, memory, and agency across multiple users and foundational models. It behaves like a member of the household—growing in awareness and usefulness over time—while remaining portable, transparent, and fully under user control.

The system is designed to be model-agnostic, self-hosted, and human-inspectable, with all memory stored in readable formats (Markdown, Git-versioned) and deployed via Docker containers managed by Coolify.

## Goals and Objectives

1. **Persistent Identity & Memory**: Create an assistant that maintains continuity across sessions, models, and time
2. **Multi-User Support**: Enable multiple household members (e.g., you and Shannon) to interact with personalized and shared memory contexts
3. **Model Agnosticism**: Decouple behavior from any single LLM provider using LiteLLM as a routing layer
4. **Transparency & Control**: Keep all memory human-readable, editable, and version-controlled
5. **Agentic Capabilities**: Enable autonomous task execution, planning, and tool use via Letta framework
6. **Scalable Memory**: Support both curated knowledge and large-scale data retrieval (RAG, MCPs)

## Scope

### Included
- Multi-user identity and memory management (personal + shared contexts)
- Model-agnostic LLM routing via LiteLLM
- Markdown-based memory system with Git versioning
- Knowledgebase integration for long-term memory (memories, history, projects)
- Letta agent framework for autonomous workflows
- Docker-based deployment via Coolify
- Project-scoped memory and context management
- Basic RAG capabilities for semantic retrieval
- Modular Content Providers (MCPs) for external data sources

### Excluded (Future Considerations)
- Advanced personalization (LoRA, DPO, self-distillation)
- Full web UI for memory browsing/editing
- Real-time multi-agent collaboration
- Mobile applications

## Target Audience

**Primary Users**: Household members (you and Shannon)
- Technical users comfortable with Git, Docker, and Markdown
- Seeking a personal AI assistant that grows with them over time
- Value privacy, transparency, and control over their data

**Secondary Audience**: Technical enthusiasts exploring agentic AI architectures

## Key Requirements

### Functional Requirements
1. Support multiple user identities with separate personal and shared memory spaces
2. Route requests to any LLM backend (OpenAI, Claude, Mistral, LM Studio, etc.)
3. Maintain human-readable memory in Markdown format
4. Version all memory changes via Git
5. Enable project-based memory scoping for focused workflows
   - Projects can be focused (e.g., "Move to PMTiles") or open-ended
   - Each project maintains its own scoped memory and context
   - Projects enable long-term agent continuity and user control over history
6. Support agent autonomy with tool execution and planning
   - Agents can execute tools or plans
   - Agents can loop autonomously or wait for input
   - Agents operate within the memory and scope of a specific user + project
7. Provide semantic search over large text corpora (emails, chats, bookmarks)

### Non-Functional Requirements
1. **Privacy**: All data self-hosted, no external dependencies for core functionality
2. **Transparency**: Memory must be inspectable and editable by humans
3. **Portability**: System can be moved between hosts without data loss
4. **Reliability**: Persistent storage via named Docker volumes
5. **Separation of Concerns**: Clear boundaries between agent short-term and long-term memory

## Technical Stack

### Core Infrastructure
- **Deployment**: Coolify (Docker Compose orchestration)
- **Containerization**: Docker with named volumes for persistence
- **Version Control**: Git for memory and configuration tracking

### AI/ML Components
- **Model Gateway**: LiteLLM (multi-provider routing)
- **Agent Framework**: Letta (autonomous workflows, tool execution)
- **Memory System**: Onyx (structured memory management)
- **Vector Store**: Qdrant, Weaviate, or Pinecone (semantic retrieval)

### Memory Architecture

The system implements a **hybrid memory model** combining multiple memory types:

- **Basic Memory (Symbolic)**: Markdown files with optional YAML frontmatter
  - Structured facts, plans, beliefs
  - Human-readable and editable
  - Version-controlled via Git
- **Semantic Memory**: Vector embeddings for RAG
  - Dense vector retrieval via optional vector DB
  - Enables semantic search over large corpora
- **Slot Memory**: Key-value store for structured metadata
  - Fast access to frequently used values
- **Episodic Memory**: JSON/Markdown logs of interactions
  - Session transcripts and agent logs with timestamps
  - Enables timeline reconstruction and debugging
- **MCPs**: Modular Content Providers for external data (emails, bookmarks, etc.)

### Storage Structure
```
memories/
  personal/          # User-specific private memory (factual beliefs, skills, preferences)
  shared/            # Household shared memory (common experiences, household knowledge)
history/             # Session transcripts and agent logs (versioned with timestamps)
projects/            # Project-scoped memory files (per-project context)
```

**Identity as First-Class Concept**: The assistant knows *who* it's interacting with and *in what context*, enabling personalized responses while maintaining shared household knowledge.

## Timeline and Milestones

This is an exploratory, iterative project with flexible timelines. Development will proceed in capability-driven phases:

- **Phase 1**: Core infrastructure (Docker, Coolify, LiteLLM)
- **Phase 2**: Basic memory system (Markdown, Git, Onyx)
- **Phase 3**: Multi-user support and memory scoping
- **Phase 4**: Agent framework integration (Letta)
- **Phase 5**: Advanced features (RAG, MCPs, semantic search)

Specific dates and deadlines will be determined as the project evolves.

## Success Criteria

1. **Continuity**: Assistant maintains identity and context across sessions and model switches
2. **Usability**: Both users can interact naturally with personalized responses
3. **Transparency**: All memory is readable and editable by humans
4. **Reliability**: System runs continuously with minimal maintenance
5. **Extensibility**: New capabilities (tools, MCPs, models) can be added modularly
6. **Privacy**: All data remains under user control on self-hosted infrastructure

## Constraints and Assumptions

### Technical Constraints
- **Self-Hosted Only**: All services run on user-controlled infrastructure via Coolify
- **Docker-Based**: All components deployed as containers with persistent volumes
- **Git-Versioned**: Memory and configuration tracked in Git repositories
- **Model Agnostic**: No dependency on any single LLM provider
- **Human-Readable Memory**: All memory must be stored in formats humans can read/edit

### Design Assumptions
- Users are comfortable with technical tools (Git, Docker, Markdown)
- Household use case (2-4 users maximum)
- Memory separation between users is important but not cryptographically enforced
- Agent memory (short-term and long-term) requires clear separation of concerns
- System will evolve iteratively based on usage patterns

### Privacy Considerations
- Multi-user privacy is a concern but not critical (household trust model)
- Personal vs. shared memory boundaries enforced at application level
- No external API calls for core functionality (except chosen LLM providers)

## Stakeholders

- **Primary Users**: You and Shannon (household members)
- **System Administrator**: You (deployment, maintenance, configuration)
- **Contributors**: Open to future community contributions if open-sourced

## Additional Notes

### Experimental Goals
- Explore identity continuity and assistant "consciousness" across sessions and models
- Investigate lightweight personalization techniques (LoRA, DPO, self-distillation) to grow capabilities
- Enable episodic, ambient, or interactive use without starting from scratch each time
- Test hybrid memory models (symbolic + semantic + episodic)
- Investigate scalable augmentation via retrieval-augmented generation (RAG) over large data corpora

### Key Design Principles
1. **Behavior Reusability**: Agent behavior, memory, and tools are reusable across models
2. **User Editability**: Both users can manually revise any part of the memory via Markdown or Git
3. **Unified Deployment**: All services deployed via a unified Docker Compose file
4. **Named Volumes**: Persistent storage for Onyx, vector stores (Qdrant/Pinecone/Weaviate)

### Related Documentation
- [`agentic-assistant-architecture.md`](agentic-assistant-architecture.md) - Detailed architectural design
- [`memory_architecture_brief.md`](memory_architecture_brief.md) - Memory system specifications

### Current Status
Project is in initial setup phase. Core infrastructure (Docker Compose, LiteLLM config) has been started.