# BEARS content repository (legacy)

> **Status: legacy.** This template supported the **Git-synced Markdown + Qdrant knowledgebase** path. The **target** BEARS design uses **Cabinet** ([Outline](https://www.getoutline.com/)) as the shared knowledgebase humans and agents edit—see [PLAN.md](../PLAN.md) and [README.md](../README.md). That model **obviates** this repo layout for new deployments. Use this template only if you still run Git Sync + the old knowledgebase, or while **migrating** content into Outline collections (“decks”).

## Migration note (Git → Cabinet)

Conceptual mapping when moving to Outline:

| Legacy path | Cabinet / Outline |
|-------------|-------------------|
| `memories/shared/` | Knowledge deck / shared docs |
| `memories/personal/` | Per-user or restricted collections (per BEARS policy) |
| `history/` | History deck or dated docs |
| `projects/` | Projects deck |

Letta **memory blocks** and conversation state are unchanged; only **shared archival knowledge** moves from Git files to Outline.

---

## Original template documentation

This repository contains the memory, history, and project data for a **legacy** BEARS deployment. It is synchronized via the Git Sync container when that stack is in use.

### Repository structure

```
.
├── memories/          # Long-term semantic memory
│   ├── personal/     # User-specific private memories
│   └── shared/       # Household/team shared memories
├── history/          # Episodic memory (conversation logs)
└── projects/         # Project-scoped context and notes
```

### How it worked (legacy)

The `git-sync` service:

1. Cloned this repository on first startup  
2. Watched for file changes  
3. Committed and pushed when the knowledgebase service modified files  
4. Pulled from origin periodically  

### Memory file format

Markdown with YAML frontmatter (legacy):

```markdown
---
title: "Example Memory"
tags: ["preference", "user"]
created: 2025-11-23T10:30:00Z
---

# Example Memory
```

### Git Sync configuration (legacy)

When using Git Sync in Coolify:

```bash
GIT_SYNC_REPO=https://github.com/YourUsername/bears-content.git
GIT_SYNC_BRANCH=main
GIT_USERNAME=your-github-username
GIT_PASSWORD=ghp_YourPersonalAccessToken
GIT_AUTHOR_NAME=BEARS Git Sync
GIT_AUTHOR_EMAIL=bears-sync@yourdomain.com
```

### Backup (legacy)

With this stack, the Git repo was the source of truth for Markdown; Qdrant could be rebuilt from files.

---

For the current architecture and Cabinet rollout, see **[PLAN.md](../PLAN.md)** and **[README.md](../README.md)**.
