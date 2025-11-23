# Git Sync - Coolify Deployment Guide

## Overview

The Git Sync service automatically synchronizes your BEARS memory content (memories, history, projects) with a GitHub repository. It watches for file changes and commits/pushes immediately, while pulling from origin every 5 minutes to sync changes from other sources.

## Prerequisites

- Coolify instance running
- GitHub repository for content (see `content-template/` for a template)
- GitHub Personal Access Token (PAT) with `Contents: Read and Write` permissions
- This service must be deployed **before** Onyx API Server

## Deployment Steps

### 1. Create GitHub Content Repository

Use the `content-template/` from this repository:

```bash
# Option A: Create new repo from template
cd content-template
git init
git add .
git commit -m "Initial commit"
git remote add origin https://github.com/YourUsername/bears-content.git
git push -u origin main

# Option B: Fork the template if published
# Just fork the template repository on GitHub
```

### 2. Create GitHub Personal Access Token

1. Go to GitHub → Settings → Developer Settings → Personal Access Tokens → Fine-grained tokens
2. Click "Generate new token"
3. Configure:
   - **Token name**: `BEARS Git Sync`
   - **Expiration**: 90 days (or longer)
   - **Repository access**: Only select your content repository
   - **Permissions**:
     - `Contents`: **Read and write**
4. Click "Generate token" and **copy the token** (you won't see it again!)

### 3. Deploy in Coolify

1. **Add New Resource** → **Docker Image**

2. **Basic Configuration**:
   - **Service Name**: `bears-git-sync`
   - **Image**: Build from Dockerfile (see step 4)
   - **Deployment Type**: Build from Git Repository

3. **Build Configuration**:
   - **Git Repository**: `https://github.com/TheArtificial/bears-depoy` (this config repo)
   - **Branch**: `main`
   - **Dockerfile Location**: `services/git-sync/Dockerfile`
   - **Build Context**: `services/git-sync`

4. **Environment Variables**:

   ```bash
   # Required: Content Repository Configuration
   GIT_SYNC_REPO=https://github.com/YourUsername/bears-content.git
   GIT_SYNC_BRANCH=main

   # Required: GitHub Authentication
   GIT_USERNAME=your-github-username
   GIT_PASSWORD=ghp_YourPersonalAccessTokenHere

   # Required: Git Author Identity
   GIT_AUTHOR_NAME=BEARS Git Sync
   GIT_AUTHOR_EMAIL=git-sync@yourdomain.com

   # Optional: Sync Interval (default: 300 seconds / 5 minutes)
   GIT_SYNC_INTERVAL=300
   ```

5. **Persistent Storage**:

   Create a **named volume** called `bears-memory`:

   - **Volume Name**: `bears-memory`
   - **Mount Path**: `/data`
   - **Description**: Shared volume for memory content (used by git-sync and onyx)

6. **Health Check** (automatically configured in Dockerfile):
   - Checks if `.git` directory exists in `/data`
   - Interval: 30s
   - Timeout: 10s
   - Start period: 60s

7. **Restart Policy**: `unless-stopped`

8. **Deploy** the service

### 4. Verify Deployment

Check the logs in Coolify:

```
🐻 BEARS Git Sync starting...
Repository: https://github.com/YourUsername/bears-content.git
Branch: main
Sync interval: 300s
📦 Cloning repository for the first time...
✅ Repository cloned successfully
🔄 Initial sync with remote...
✅ Pulled successfully
👀 Watching for file changes in /data...
✅ Git sync is running!
```

Verify the volume has content:

```bash
# In Coolify terminal for git-sync service
ls -la /data
# Should show: memories/, history/, projects/, .git/
```

## Configuration Reference

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `GIT_SYNC_REPO` | ✅ Yes | - | HTTPS URL of your content repository |
| `GIT_SYNC_BRANCH` | No | `main` | Branch to sync |
| `GIT_USERNAME` | ✅ Yes | - | GitHub username |
| `GIT_PASSWORD` | ✅ Yes | - | GitHub Personal Access Token |
| `GIT_AUTHOR_NAME` | No | `BEARS Git Sync` | Name for auto-commits |
| `GIT_AUTHOR_EMAIL` | No | `git-sync@bears.local` | Email for auto-commits |
| `GIT_SYNC_INTERVAL` | No | `300` | Seconds between pulls from origin |

### Volume Configuration

**Important**: The `bears-memory` volume must be shared with Onyx API Server!

```
Volume Name: bears-memory
Mount Path: /data
```

This volume will contain:
- `memories/` - Long-term semantic memory
- `history/` - Conversation logs
- `projects/` - Project-scoped context
- `.git/` - Git repository metadata

## Service Behavior

### On File Changes

When Onyx modifies files in `/data`:

1. `inotifywait` detects the change immediately
2. Git Sync waits 2 seconds (debounce for multiple rapid changes)
3. Stages all changes: `git add -A`
4. Commits with auto-generated message
5. Pushes to `origin/$BRANCH` immediately

**Example commit message**:
```
Auto-sync: 3 file(s) changed (2025-11-23 14:32:15 UTC)

Changed files:
M       memories/shared/team-guidelines.md
A       history/2025-11/2025-11-23-session.json
M       projects/website/progress.md
```

### Periodic Pulls

Every 5 minutes (default):

1. Pulls from `origin/$BRANCH` with rebase strategy
2. If local uncommitted changes exist, stashes them first
3. Pulls and rebases
4. Pops stash back if there were local changes
5. Logs success or conflict errors

### Conflict Resolution

Uses `git pull --rebase` strategy:

- **No conflicts**: Changes merge cleanly
- **Conflicts**: Aborts rebase, logs error, keeps local changes
- **Manual fix needed**: SSH into container and resolve manually

## Troubleshooting

### Logs Show "Repository clone failed"

**Problem**: Can't clone content repository

**Solutions**:
- Verify `GIT_SYNC_REPO` URL is correct
- Check `GIT_USERNAME` matches your GitHub username
- Verify `GIT_PASSWORD` is a valid GitHub PAT
- Ensure PAT has `Contents: Read and write` permission
- Check repository exists and is accessible

### Logs Show "Push failed"

**Problem**: Can't push commits to GitHub

**Solutions**:
- Verify PAT hasn't expired
- Check PAT has write permissions
- Ensure no branch protection rules blocking pushes
- Verify network connectivity from Coolify

### Logs Show "Pull failed! Check for conflicts"

**Problem**: Rebase conflicts during pull

**Solutions**:
1. SSH into container: `docker exec -it <container-id> sh`
2. Check status: `cd /data && git status`
3. Resolve conflicts manually
4. Complete rebase: `git rebase --continue`
5. Or abort: `git rebase --abort`

### Changes Not Syncing

**Problem**: Files modified but not committed/pushed

**Solutions**:
- Check logs for inotifywait errors
- Verify `/data` is mounted correctly
- Restart service to force sync
- Check file permissions

### Service Health Check Failing

**Problem**: Container shows unhealthy

**Solutions**:
- Check if `.git` directory exists in `/data`
- Review logs for clone errors
- Verify volume is mounted at `/data`
- Restart service after fixing issues

## Integration with Onyx

The Onyx API Server must mount the same `bears-memory` volume:

**Git Sync**:
```
Volume: bears-memory → /data
```

**Onyx API**:
```
Volume: bears-memory → /app/memory
```

When configured this way:
- Onyx writes to `/app/memory/memories/`, `/app/memory/history/`, `/app/memory/projects/`
- Git Sync sees changes in `/data/memories/`, `/data/history/`, `/data/projects/`
- Changes are automatically committed and pushed

## Manual Sync Trigger

To force an immediate sync:

```bash
# Restart the service in Coolify
# This will:
# 1. Pull latest changes from origin
# 2. Resume watching for file changes
```

## Monitoring

### Check Sync Status

View recent commits:

```bash
# In Coolify terminal
cd /data
git log --oneline -10
```

### Check What's Pending

```bash
# In Coolify terminal
cd /data
git status
```

### Watch Live Logs

In Coolify, view live logs to see:
- File change detections
- Commit/push operations
- Periodic pull results
- Any errors or conflicts

## Security Best Practices

1. **Use Fine-Grained PAT**: Limit access to only the content repository
2. **Set Expiration**: Use 90-day expiration and rotate regularly
3. **Private Repository**: Keep content repository private
4. **Rotate Tokens**: Update PAT before expiration
5. **Audit Commits**: Review auto-commits for unexpected changes
6. **Monitor Logs**: Watch for failed pushes or suspicious activity

## Advanced Configuration

### Custom Sync Interval

Sync every 1 minute instead of 5:

```bash
GIT_SYNC_INTERVAL=60
```

### Multiple Branches

Deploy separate instances for different environments:

**Development**:
```bash
GIT_SYNC_BRANCH=dev
```

**Production**:
```bash
GIT_SYNC_BRANCH=main
```

### Custom Commit Author

```bash
GIT_AUTHOR_NAME=Alice's BEARS Assistant
GIT_AUTHOR_EMAIL=alice+bears@example.com
```

## Next Steps

After Git Sync is running:

1. ✅ Verify logs show successful clone and sync
2. ✅ Check volume contains `memories/`, `history/`, `projects/`
3. ➡️ Deploy **Onyx API Server** (with same `bears-memory` volume)
4. ✅ Test: Modify a file in content repo, watch it sync to Coolify
5. ✅ Test: Have Onyx create a memory, watch it commit/push to GitHub
