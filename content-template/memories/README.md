# Memories Directory

This directory contains the long-term memory system for the BEARS Stack.

## Structure

- `personal/` - User-specific private memory (factual beliefs, skills, preferences)
- `shared/` - Household shared memory (common experiences, household knowledge)

## Format

Memory files are written in Markdown with optional YAML frontmatter:

```markdown
---
user: shannon
category: preferences
updated: 2025-10-05
---

## Personal Preferences
- Name: Shannon
- Preferred editor: Obsidian
- Communication style: Direct, technical
```

## Version Control

All memory files are tracked in Git. Each change should be committed with a descriptive message explaining what was learned or updated.

## Usage

The assistant will read from these files during conversations to maintain context and continuity. Users can manually edit these files to correct or update information.