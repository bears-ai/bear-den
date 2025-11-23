# History Directory

This directory contains episodic memory - chronological logs of interactions, thoughts, and events.

## Structure

History files are organized by date:

```
history/
├── 2025/
│   ├── 10/
│   │   ├── 2025-10-05-session-001.md
│   │   └── 2025-10-05-session-002.json
│   └── 09/
└── index.json
```

## Format

Sessions can be stored as either Markdown or JSON:

### JSON Format
```json
{
  "session_id": "2025-10-05-session-001",
  "timestamp": "2025-10-05T16:51:00Z",
  "user": "shannon",
  "project": "bears-stack",
  "interactions": [
    {
      "role": "user",
      "content": "Initialize the project brief",
      "timestamp": "2025-10-05T16:41:00Z"
    }
  ],
  "summary": "Initialized BEARS Stack project brief",
  "tags": ["setup", "documentation"]
}
```

## Purpose

- Timeline reconstruction
- Debugging and analysis
- Memory promotion (extracting key facts to permanent memory)
- Session continuity across restarts