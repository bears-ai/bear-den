# 🧠 Memory Architecture Brief for Agentic Assistant

## Overview

This memory system supports a modular, inspectable agent that can reason over:

- Curated user knowledge (editable by humans)
- Massive personal corpora (emails, chats, bookmarks, etc.)
- Structured preferences and profiles
- Semantic and episodic recall across time

The architecture separates **curated long-term memory**, **RAG-based semantic memory**, and **modular content providers (MCPs)** for high-volume sources.

---

## 1. Memory System Components

### A. Basic Memory (Curated Markdown)

- **Format:** Markdown with optional YAML frontmatter
- **Purpose:** Human-editable long-term facts, preferences, relationships, procedural knowledge
- **Location:** Git-tracked local or remote repo
- **Example:**

\`\`\`markdown
## Personal Preferences
- Name: Shannon
- Preferred editor: Obsidian

## Life Events
- 2025-08: Moved to Amsterdam
\`\`\`

---

### B. Semantic Memory (Vector Store via RAG)

- **Tool:** Qdrant, Weaviate, or Pinecone
- **Purpose:** Semantic retrieval over large text corpora
- **Indexed Sources:**
  - Chunked and embedded versions of:
    - Emails
    - Chat logs
    - Bookmarked pages (full text)
    - Journals, transcripts, etc.
- **Use:** Retrieved during tasks to expand context and inform reasoning

---

### C. Slot Memory (Key-Value Store)

- **Format:** KV store or typed document store
- **Purpose:** Fast access to structured metadata
- **Examples:**
  - \`user.name → "Shannon"\`
  - \`user.city_future → "Amsterdam"\`
  - \`agent.current_goal → "Summarize past 3 months of travel"\`

---

### D. Episodic Memory

- **Format:** Structured JSON or Markdown logs
- **Purpose:** Logs of past events, thoughts, and interactions
- **Use Cases:** Timeline reconstruction, debugging, long-term learning

---

## 2. Large-Scale Data Integration via MCPs

### Modular Content Providers (MCPs)

Each large data source is treated as a separate queryable service.

| Source    | Format        | Example Queries                             |
|-----------|---------------|---------------------------------------------|
| Emails    | Maildir / API | \`email.search("from:shannon travel")\`       |
| Bookmarks | JSON / DB     | \`bookmarks.tagged("design")\`                |
| Chat Logs | JSONL         | \`chats.with("mom", before="2012")\`          |
| Calendar  | ICS / API     | \`calendar.events_between("2024-01", "2024-03")\` |

These sources are optionally indexed into the vector store, and can also be queried directly via agents.

---

## 3. Interaction Flow

### Inference-Time Read

\`\`\`text
User Query
   ↓
Assistant Core
   ├─ Basic Memory → Prompt-injected facts
   ├─ Vector DB → Semantically relevant passages
   └─ Slot Memory → Structured values
\`\`\`

### Post-Interaction Write

\`\`\`text
Assistant Output
   ├─ Summarize & embed → Vector Store
   ├─ Update slots → Key-Value Store
   └─ Suggest Markdown edits → Basic Memory (optional user approval)
\`\`\`

---

## 4. Sync and Distillation Patterns

- Summarization agents promote key content into Basic Memory (Markdown)
- Human edits to memory trigger re-embedding
- Conflicts between sources flagged for review
- Metadata tags (e.g. \`#travel\`, \`#health\`) support thematic indexing

---

## 5. Implementation Suggestions

- Use Docker Compose to deploy:
  - Vector DB (Qdrant / Weaviate)
  - Onyx or equivalent file-based Basic Memory
  - LiteLLM as model gateway
  - MCP microservices with REST or GRPC APIs
- Shared Git repo for configuration and memory files
- Long-term storage via named Docker volumes
- Git integration for tracking memory edits

---

## 6. Optional Enhancements

- Agent-based summarizers for MCP data (e.g. "Summarize 2019 emails")
- Memory promotion UI for human approval
- Web interface to browse and edit Markdown memory
- Time-aware vector embeddings with temporal decay
