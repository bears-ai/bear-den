# Project Brief: BEARS Stack

**B**ears **E**volving **A**gentic **R**easoning **S**ystem

## Project Overview

BEARS Stack is a long-running, modular assistant system that supports persistent identity, memory, and agency across multiple users and foundational models. It behaves like a member of the household—growing in awareness and usefulness over time—while remaining portable, transparent, and fully under user control.

The system is model-agnostic and self-hosted. **Shared knowledge** is **Cabinet** on **Outline** (human- and **bear**-editable via tools); **Letta** holds per‑**bear** memory (each **bear** is a Letta agent). **Den** provisions bears, **users↔bears** membership (many‑to‑many), and surfaces bears in Open WebUI / **Letta Code**. See [`docs/planning/PLAN.md`](../../docs/planning/PLAN.md) and Den / multi-user web in [`docs/architecture/DEN_ARCHITECTURE.md`](../../docs/architecture/DEN_ARCHITECTURE.md).

## Goals and Objectives

1. **Persistent Identity & Memory**: Create an assistant that maintains continuity across sessions, models, and time
2. **Multi-User Support**: Enable multiple household members (e.g., you and Shannon) to interact with personalized and shared memory contexts
3. **Model Agnosticism**: Decouple behavior from any single LLM provider using **Bifrost** as the model gateway
4. **Transparency & Control**: Shared knowledge human-readable and editable (Outline); Letta memory inspectable in Letta
5. **Agentic capabilities**: Enable autonomous task execution, planning, and tool use per **bear** via Letta
6. **Scalable Memory**: Support both curated knowledge and large-scale data retrieval (RAG, MCPs)

## Scope

### Included
- Multi-user identity and memory management (personal + shared contexts)
- Model-agnostic LLM routing via Bifrost
- **Cabinet (Outline)** for shared long-term knowledge (humans + **bears**); Letta native memory per **bear**  
- Letta as **bear** runtime for autonomous workflows (tools, planning)
- Docker-based deployment via Coolify
- Project-scoped memory and context management
- Basic RAG capabilities for semantic retrieval
- Modular Content Providers (MCPs) for external data sources

### Excluded (Future Considerations)
- Advanced personalization (LoRA, DPO, self-distillation)
- Full web UI for memory browsing/editing
- Real-time multi-bear collaboration (beyond shared bears + Cabinet)
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
3. Shared knowledge human-readable and editable in **Outline (Cabinet)**
4. Letta per‑**bear** memory and conversation state
5. Enable project-based memory scoping for focused workflows
   - Projects can be focused (e.g., "Move to PMTiles") or open-ended
   - Each project maintains its own scoped memory and context
   - Projects enable long-term **bear** continuity and user control over history
6. Support **bear** autonomy with tool execution and planning
   - Bears can execute tools or plans
   - Bears can loop autonomously or wait for input
   - Each bear operates within Letta memory and Den policy (user + **bear** + project scope)
7. Provide semantic search over large text corpora (emails, chats, bookmarks)

### Non-Functional Requirements
1. **Privacy**: All data self-hosted, no external dependencies for core functionality
2. **Transparency**: Memory must be inspectable and editable by humans
3. **Portability**: System can be moved between hosts without data loss
4. **Reliability**: Persistent storage via named Docker volumes
5. **Separation of Concerns**: Clear boundaries between **bear** (Letta) context and shared **Cabinet** knowledge

## Technical Stack

### Core Infrastructure
- **Deployment**: Coolify (Docker Compose orchestration)
- **Containerization**: Docker with named volumes for persistence
- **Config**: Git for this deploy repo; knowledge in Outline

### AI/ML Components
- **Model Gateway**: Bifrost (multi-provider routing)
- **Bear runtime**: Letta (autonomous workflows, tool execution; one Letta agent per bear)
- **Memory**: Letta native + **Cabinet (Outline)** for shared knowledge
- **Multi-user auth & bear registry**: **Den** (Axum) in front of **self-hosted Letta**; many **bears** per user and shared bears. See [`docs/architecture/DEN_ARCHITECTURE.md`](../../docs/architecture/DEN_ARCHITECTURE.md) / [`docs/planning/PLAN.md`](../../docs/planning/PLAN.md).

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
  - Session transcripts and **bear** logs with timestamps
  - Enables timeline reconstruction and debugging
- **MCPs**: Modular Content Providers for external data (emails, bookmarks, etc.)

### Storage Structure
```
memories/
  personal/          # User-specific private memory (factual beliefs, skills, preferences)
  shared/            # Household shared memory (common experiences, household knowledge)
history/             # Session transcripts and **bear** logs (versioned with timestamps)
projects/            # Project-scoped memory files (per-project context)
```

**Identity as First-Class Concept**: The assistant knows *who* it's interacting with and *in what context*, enabling personalized responses while maintaining shared household knowledge.

## Timeline and Milestones

This is an exploratory, iterative project with flexible timelines. Development will proceed in capability-driven phases:

- **Phase 1**: Core infrastructure (Docker, Coolify, Bifrost)
- **Phase 2**: Cabinet (Outline) + Letta memory
- **Phase 3**: Multi-user support and memory scoping
- **Phase 4**: **Bear** runtime hardening (Letta) + Den provisioning
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
- **Bear** memory (Letta) vs shared **Cabinet** requires clear separation of concerns
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
1. **Behavior Reusability**: **Bear** behavior, memory, and tools are reusable across models
2. **User Editability**: Both users can manually revise any part of the memory via Markdown or Git
3. **Unified Deployment**: All services deployed via a unified Docker Compose file
4. **Named Volumes**: Letta, Outline, Bifrost (optional bind for `config.json` only; no DB by default)

### Related Documentation
- [`agentic-assistant-architecture.md`](agentic-assistant-architecture.md) - Detailed architectural design
- [`memory_architecture_brief.md`](memory_architecture_brief.md) - Memory system specifications

### Current Status
Project is in initial setup phase. Core infrastructure (Docker Compose, Bifrost `config.json`) has been started.