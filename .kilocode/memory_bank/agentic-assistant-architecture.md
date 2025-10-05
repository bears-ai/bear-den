# 🧠 Project Summary: Personalized, Agentic Assistant Framework

## 🎯 Objective
Create a long-running, modular **assistant system** that supports persistent identity, memory, and agency across multiple users and foundational models. It should behave like a member of the household—growing in awareness and usefulness over time—while remaining portable, transparent, and fully under your control.

---

## 🧩 Key Architectural Elements

### 1. Model-Agnostic, Modular Design
- Behavior is decoupled from any one LLM provider.
- Uses **LiteLLM** to route requests to any supported backend (OpenAI, Claude, Mistral, LM Studio, etc.).
- Agent behavior, memory, and tools are reusable across models.

### 2. Multi-User Support
- Supports multiple humans (e.g. *you* and *Shannon*), each with:
  - **Personal memory** (private thoughts, preferences, goals).
  - **Shared memory** (common experiences, household knowledge).
- Identity is first-class: the assistant knows *who* it's interacting with, and *in what context*.

### 3. Memory System
- **Basic Memory** (Markdown): Human-readable/editable; version-controlled via Git.
- **Onyx**: Powers long-term memory; structured into:
  - `memories/`: Factual beliefs, skills, preferences (user- or system-authored).
  - `history/`: Session transcripts and agent logs (versioned with timestamps).
  - `projects/`: Scoped memory files per project (see below).
- **Memory Scoping**:
  - `personal/` and `shared/` subfolders distinguish memory accessibility.
  - Enables separate recall for individual vs household knowledge.

### 4. Projects
- Each assistant interaction or workflow belongs to a **project**:
  - Can be focused (e.g. “Move to PMTiles” or “Agents server deployment”) or open-ended.
  - Projects maintain their own scoped memory and context.
  - Projects enable long-term agent continuity and user control over history.

### 5. Agent Framework
- Uses **Letta** for agent workflows.
- Agents can:
  - Execute tools or plans,
  - Loop autonomously or wait for input,
  - Operate within the memory and scope of a specific user + project.

### 6. Hosting & Deployment
- Uses **Coolify** to manage deployments.
- All services (Letta, Onyx, LiteLLM, etc.) run as **Docker containers**, deployed via a unified **Compose file**.
- Named Docker volumes persist memory and state for:
  - Onyx
  - Vector stores (Qdrant/Pinecone/Weaviate if used)

---

## 🧠 Advanced Memory & Retrieval

### Hybrid Memory Model:
- **Symbolic memory** (structured facts, plans, beliefs).
- **Semantic memory** (dense vector retrieval via optional vector DB).
- **User-editable**: Both you and Shannon can manually revise any part of the memory via Markdown or Git.

### Scalable Augmentation:
- Assistant can pull in large data corpora (emails, chat logs, notes) via **retrieval-augmented generation (RAG)**.
- Long-term goal: add modular **MCPs** (Modular Context Providers) to index and search these external data archives.

---

## 🧪 Experimental Goals
- Explore identity continuity and assistant “consciousness” across sessions and models.
- Use lightweight personalization (LoRA, DPO, self-distillation) to grow capabilities.
- Enable episodic, ambient, or interactive use without starting from scratch each time.
