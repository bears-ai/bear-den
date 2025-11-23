#!/bin/bash
set -e

# Configuration from environment variables
REPO_URL="${GIT_SYNC_REPO:?GIT_SYNC_REPO environment variable is required}"
BRANCH="${GIT_SYNC_BRANCH:-main}"
SYNC_INTERVAL="${GIT_SYNC_INTERVAL:-300}"  # Default: 5 minutes (300 seconds)

# Git credentials
GIT_USERNAME="${GIT_USERNAME:?GIT_USERNAME environment variable is required}"
GIT_PASSWORD="${GIT_PASSWORD:?GIT_PASSWORD environment variable is required}"

# Git author info
GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-BEARS Git Sync}"
GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-git-sync@bears.local}"

# Configure git
git config --global user.name "$GIT_AUTHOR_NAME"
git config --global user.email "$GIT_AUTHOR_EMAIL"
git config --global credential.helper store
git config --global pull.rebase true

# Setup authenticated repo URL
AUTH_REPO_URL=$(echo "$REPO_URL" | sed "s|https://|https://${GIT_USERNAME}:${GIT_PASSWORD}@|")

echo "🐻 BEARS Git Sync starting..."
echo "Repository: $REPO_URL"
echo "Branch: $BRANCH"
echo "Sync interval: ${SYNC_INTERVAL}s"

# Initial clone or use existing repo
if [ ! -d ".git" ]; then
    echo "📦 Cloning repository for the first time..."
    git clone --branch "$BRANCH" "$AUTH_REPO_URL" /tmp/repo

    # Move contents to /data
    shopt -s dotglob
    mv /tmp/repo/* /data/ 2>/dev/null || true
    rm -rf /tmp/repo

    echo "✅ Repository cloned successfully"
else
    echo "📂 Using existing repository"
    git remote set-url origin "$AUTH_REPO_URL"
fi

# Function to commit and push changes
commit_and_push() {
    local changes=$(git status --porcelain | wc -l)

    if [ "$changes" -gt 0 ]; then
        echo "📝 Detected $changes file change(s), committing..."

        git add -A

        # Get list of changed files for commit message
        local changed_files=$(git diff --cached --name-status | head -n 5)
        local timestamp=$(date -u +"%Y-%m-%d %H:%M:%S UTC")

        git commit -m "Auto-sync: $changes file(s) changed ($timestamp)

Changed files:
$changed_files" || {
            echo "⚠️  Commit failed (possibly nothing to commit)"
            return 0
        }

        echo "⬆️  Pushing changes to origin..."
        if git push origin "$BRANCH"; then
            echo "✅ Changes pushed successfully"
        else
            echo "❌ Push failed! Will retry on next sync."
            return 1
        fi
    fi
}

# Function to pull changes from remote
pull_changes() {
    echo "⬇️  Pulling changes from origin..."

    # Stash any uncommitted changes
    local had_changes=false
    if [ -n "$(git status --porcelain)" ]; then
        git stash push -m "Auto-stash before pull $(date -u +"%Y-%m-%d %H:%M:%S")"
        had_changes=true
    fi

    # Pull with rebase
    if git pull --rebase origin "$BRANCH"; then
        echo "✅ Pulled successfully"

        # Pop stash if we had changes
        if [ "$had_changes" = true ]; then
            if git stash pop; then
                echo "✅ Restored local changes"
            else
                echo "⚠️  Conflict while restoring local changes - manual intervention may be needed"
                git stash list
            fi
        fi
    else
        echo "❌ Pull failed! Check for conflicts."

        # Abort rebase if it failed
        git rebase --abort 2>/dev/null || true

        # Pop stash back if we had one
        if [ "$had_changes" = true ]; then
            git stash pop 2>/dev/null || echo "⚠️  Could not restore stashed changes"
        fi

        return 1
    fi
}

# Initial pull to sync with remote
echo "🔄 Initial sync with remote..."
pull_changes || echo "⚠️  Initial pull failed, continuing anyway..."

# Start periodic sync in background
(
    while true; do
        sleep "$SYNC_INTERVAL"
        echo ""
        echo "⏰ Periodic sync triggered (every ${SYNC_INTERVAL}s)..."
        pull_changes || echo "⚠️  Periodic pull failed"
    done
) &
PERIODIC_PID=$!

# Monitor file changes with inotifywait
echo ""
echo "👀 Watching for file changes in /data..."
echo "   Monitoring: memories/, history/, projects/"
echo ""

# Use inotifywait to watch for file changes
inotifywait -m -r \
    --exclude '\.git|\.git-sync-lock|\.swp|\.DS_Store' \
    -e modify,create,delete,move \
    /data | while read -r directory event filename; do

    # Debounce: wait a moment for multiple rapid changes
    sleep 2

    echo ""
    echo "🔔 Change detected: $event $directory$filename"

    # Commit and push immediately
    commit_and_push
done &
WATCH_PID=$!

echo "✅ Git sync is running!"
echo "   - Watching for file changes (immediate commit/push)"
echo "   - Periodic pull every ${SYNC_INTERVAL}s"
echo ""

# Wait for either process to exit
wait -n $PERIODIC_PID $WATCH_PID

# If we get here, something went wrong
echo "❌ Git sync process exited unexpectedly"
exit 1
