# Projects Directory

This directory contains project-scoped memory and context files.

## Structure

Each project gets its own subdirectory:

```
projects/
├── bears-stack/
│   ├── context.md
│   ├── goals.md
│   └── notes.md
├── move-to-pmtiles/
│   ├── context.md
│   └── progress.md
```

## Purpose

Projects enable:
- **Focused workflows** - Scoped memory for specific tasks
- **Long-term continuity** - Maintain context across sessions
- **User control** - Clear history and progress tracking

## Format

Project files are Markdown with optional YAML frontmatter:

```markdown
---
project: bears-stack
status: active
started: 2025-10-05
---

## Project Context
Setting up the BEARS Stack infrastructure...

## Current Goals
- [ ] Deploy Docker containers
- [ ] Configure memory system
- [ ] Test multi-user support
```

## Usage

When working on a specific project, the assistant loads the project's context to maintain continuity and focus.