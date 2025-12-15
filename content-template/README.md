# BEARS Content Repository

This repository contains the memory, history, and project data for your BEARS Stack deployment. It is automatically synchronized with your running BEARS services via the Git Sync container.

## Repository Structure

```
.
├── memories/          # Long-term semantic memory
│   ├── personal/     # User-specific private memories
│   └── shared/       # Household/team shared memories
├── history/          # Episodic memory (conversation logs)
└── projects/         # Project-scoped context and notes
```

## How It Works

### Automatic Git Synchronization

The `git-sync` service in your BEARS Stack deployment:

1. **Clones this repository** on first startup
2. **Watches for file changes** using inotify
3. **Commits and pushes immediately** when the memory service modifies memory files
4. **Pulls from origin every 5 minutes** to sync changes from other sources
5. **Uses rebase strategy** to handle conflicts cleanly

### Memory File Format

Memory files are stored as **Markdown with YAML frontmatter**:

```markdown
---
title: "Example Memory"
tags: ["preference", "user"]
created: 2025-11-23T10:30:00Z
updated: 2025-11-23T10:30:00Z
---

# Example Memory

This is the content of the memory in Markdown format.

## Benefits

- Human-readable and editable
- Git-versionable with full history
- Searchable with semantic embeddings via Qdrant
```

## Getting Started

### Fork This Repository

1. Fork or clone this template repository
2. Customize the structure if needed
3. Add your initial memory files
4. Configure Git Sync in Coolify with this repository URL

### Initial Setup

```bash
# Clone your forked content repository
git clone https://github.com/YourUsername/bears-content.git
cd bears-content

# Create your first personal memory
mkdir -p memories/personal/yourname
cat > memories/personal/yourname/preferences.md << 'EOF'
---
title: "My Preferences"
tags: ["preferences", "personal"]
created: $(date -u +"%Y-%m-%dT%H:%M:%SZ")
---

# My Preferences

## Communication Style
- Concise and direct
- Technical depth when needed

## Work Schedule
- Available 9am-5pm EST
EOF

# Commit and push
git add .
git commit -m "Add initial preferences"
git push
```

### Manual Edits

You can manually edit memory files in this repository:

1. Clone the repository locally
2. Edit Markdown files as needed
3. Commit and push changes
4. Git Sync will pull changes within 5 minutes
5. The memory service will re-index updated content automatically

## Directory Details

### `memories/`

**Purpose**: Long-term semantic memory that persists across conversations

**Structure**:
- `personal/` - Private memories for individual users
- `shared/` - Shared memories accessible to all users/agents

**File Naming**: Use descriptive names like `coding-preferences.md`, `project-context.md`

**Example**:
```
memories/
├── personal/
│   └── alice/
│       ├── preferences.md
│       ├── work-schedule.md
│       └── favorite-tools.md
└── shared/
    ├── team-guidelines.md
    ├── coding-standards.md
    └── common-workflows.md
```

### `history/`

**Purpose**: Episodic memory - timestamped conversation logs and interaction history

**Structure**: Organized by date and session

**File Format**: JSON or Markdown with timestamps

**Example**:
```
history/
├── 2025-11/
│   ├── 2025-11-23-morning-session.json
│   ├── 2025-11-23-afternoon-session.json
│   └── 2025-11-24-standup.md
└── 2025-12/
    └── ...
```

### `projects/`

**Purpose**: Project-scoped context, goals, and progress tracking

**Structure**: One directory per project

**Example**:
```
projects/
├── website-redesign/
│   ├── README.md
│   ├── goals.md
│   ├── decisions.md
│   └── progress.md
└── api-migration/
    ├── README.md
    └── architecture.md
```

## Git Sync Configuration

### Required Environment Variables

When deploying the Git Sync service in Coolify, configure these variables:

```bash
# Repository Configuration
GIT_SYNC_REPO=https://github.com/YourUsername/bears-content.git
GIT_SYNC_BRANCH=main

# Authentication (use GitHub Personal Access Token)
GIT_USERNAME=your-github-username
GIT_PASSWORD=ghp_YourPersonalAccessToken

# Git Identity
GIT_AUTHOR_NAME=BEARS Git Sync
GIT_AUTHOR_EMAIL=bears-sync@yourdomain.com
```

### GitHub Personal Access Token

Create a fine-grained personal access token with:
- **Repository access**: Only this repository
- **Permissions**: `Contents` (Read and Write)

## Backup and Recovery

### Your Data is Safe

This repository **IS** your backup. All critical data lives here in Git, version-controlled and recoverable.

**What's backed up automatically**:
- ✅ All memory files (this Git repository)
- ✅ Full edit history (Git commits)
- ✅ Timestamps and metadata (Git commits)

**What's ephemeral** (can be rebuilt from this repo):
- PostgreSQL metadata (memory service regenerates from files)
- Qdrant vectors (memory service re-indexes from files)
- Letta configuration (recreate manually)

### Disaster Recovery

If your entire BEARS Stack is lost:

1. **Clone this repository** on a new server
2. **Deploy BEARS services** in Coolify
3. **Point Git Sync** to this repository
4. **The memory service can automatically rebuild**:
    - PostgreSQL metadata from Markdown files
    - Qdrant vector embeddings from content
    - Full system state from Git history

Your memory is fully restored! 🎉

## Best Practices

### Commit Messages

Git Sync auto-generates commit messages like:
```
Auto-sync: 3 files changed (2025-11-23 14:32:15)
```

For manual edits, use descriptive messages:
```
Add meeting notes from Q4 planning
Update coding preferences with new formatter settings
Archive old project: website-v1
```

### File Organization

- **Keep files small**: Split large memories into focused topics
- **Use tags**: Leverage YAML frontmatter for categorization
- **Archive old data**: Move inactive projects to `archive/` subdirectories
- **Review regularly**: Git history shows what's being accessed

### Security

- **Never commit secrets**: API keys, passwords, etc.
- **Review before sharing**: If you plan to share this repo, audit for sensitive data
- **Use private repositories**: Keep your memories private by default
- **Rotate access tokens**: Periodically refresh GitHub PAT

## Troubleshooting

### Git Sync Not Pushing Changes

1. Check Git Sync logs in Coolify
2. Verify GitHub credentials are correct
3. Ensure PAT has write permissions
4. Check for Git conflicts (sync will log errors)

### Changes Not Appearing in the memory service

1. Wait 5 minutes for automatic pull
2. Check the memory service logs for indexing errors
3. Verify file format (valid YAML frontmatter)
4. Restart the memory service if needed

### Merge Conflicts

Git Sync uses rebase strategy to minimize conflicts:
- Local changes are rebased onto remote changes
- If rebase fails, sync logs the error
- Fix conflicts manually and force push if needed

## Advanced Usage

### Multiple Deployment Instances

You can have multiple BEARS deployments sync to the same content repo:
- Development instance: `git-sync` branch = `dev`
- Production instance: `git-sync` branch = `main`

### Custom Sync Frequency

Edit the Git Sync container's cron schedule to change pull frequency (default: 5 minutes).

### Manual Sync Trigger

Restart the Git Sync service to force an immediate pull from origin.

## Support

For issues with:
- **Git Sync service**: Check BEARS deployment documentation
- **Memory file format**: See the memory service documentation
- **Git operations**: Standard Git troubleshooting applies

## License

This is your personal data repository. Choose an appropriate license or keep it private.
