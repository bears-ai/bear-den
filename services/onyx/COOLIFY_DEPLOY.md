# Onyx API Server - Coolify Deployment Guide

## Overview

Onyx is the memory management service that handles Git-versioned Markdown files, metadata storage in PostgreSQL, and vector indexing in Qdrant. It's the heart of the BEARS memory system.

## Prerequisites

- Coolify instance running
- **Git Sync service** deployed and healthy (provides memory files)
- **Redis service** deployed and healthy
- **Qdrant service** deployed and healthy
- **Coolify-managed PostgreSQL database** created
- Deploy **before** Letta

## Deployment Steps

### 1. Create PostgreSQL Database in Coolify

1. Go to **Databases** → **Add Database** → **PostgreSQL**
2. Configure:
   - **Database Name**: `bears-onyx-db`
   - **PostgreSQL Version**: `15` or higher
   - **Username**: `postgres` (default) or custom
   - **Password**: Generate secure password
3. **Deploy** and wait for healthy status
4. **Note the connection details** - you'll need them for Onyx

### 2. Deploy Onyx API Server

1. **Add New Resource** → **Docker Image**

2. **Basic Configuration**:
   - **Service Name**: `bears-onyx`
   - **Image**: `onyxdotapp/onyx-backend:latest`
   - **Deployment Type**: Public Docker Image

3. **Port Configuration**:
   - **Internal Port**: `8080`
   - **External Port**: `8080` (optional, for API access)

4. **Environment Variables**:

   ```bash
   # PostgreSQL Configuration (from Coolify-managed database)
   POSTGRES_HOST=<coolify-postgres-host>
   POSTGRES_PORT=5432
   POSTGRES_USER=postgres
   POSTGRES_PASSWORD=<your-postgres-password>
   POSTGRES_DB=onyx

   # Redis Configuration (from bears-redis service)
   REDIS_HOST=bears-redis
   REDIS_PORT=6379

   # Qdrant Configuration (from bears-qdrant service)
   QDRANT_HOST=bears-qdrant
   QDRANT_PORT=6333

   # OpenAI API (for embeddings)
   OPENAI_API_KEY=<your-openai-api-key>

   # Authentication (disabled for single-user deployment)
   AUTH_TYPE=disabled

   # Optional: Advanced Configuration
   # LOG_LEVEL=info
   # WEB_DOMAIN=yourdomain.com
   ```

5. **Persistent Storage**:

   Mount the **same volume** as Git Sync:

   - **Volume Name**: `bears-memory` (shared with git-sync)
   - **Mount Path**: `/app/memory`

   **Critical**: This must be the same volume that Git Sync uses!

6. **Health Check**:

   Configure in Coolify:
   ```bash
   Command: wget --no-verbose --tries=1 --spider http://localhost:8080/health || exit 1
   Interval: 30s
   Timeout: 10s
   Retries: 3
   Start Period: 60s
   ```

7. **Custom Start Command**:

   Override the default command to specify paths:
   ```bash
   /bin/sh -c "uvicorn onyx.main:app --host 0.0.0.0 --port 8080"
   ```

8. **Restart Policy**: `unless-stopped`

9. **Deploy** the service

### 3. Run Database Migrations

**Important**: Onyx requires database migrations before it can start properly.

#### Option A: Manual Migration (Recommended for First Deployment)

1. SSH into the Onyx container in Coolify terminal
2. Run migrations:
   ```bash
   alembic upgrade head
   ```
3. Exit and verify Onyx starts successfully

#### Option B: Automatic Migration on Startup

Add to the start command:
```bash
/bin/sh -c "alembic upgrade head && uvicorn onyx.main:app --host 0.0.0.0 --port 8080"
```

**Note**: First deployment doesn't need migrations if database is empty.

### 4. Verify Deployment

Check health in Coolify:

```bash
curl http://bears-onyx:8080/health
# Should return: {"status": "ok"} or similar
```

Test API:

```bash
curl http://bears-onyx:8080/api/manage/admin/connector/
# Should return empty list initially
```

Check memory files are accessible:

```bash
# In Coolify terminal for onyx service
ls -la /app/memory/
# Should show: memories/, history/, projects/
```

## Configuration Reference

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `POSTGRES_HOST` | ✅ Yes | - | PostgreSQL host (Coolify-managed DB) |
| `POSTGRES_PORT` | No | `5432` | PostgreSQL port |
| `POSTGRES_USER` | ✅ Yes | `postgres` | PostgreSQL username |
| `POSTGRES_PASSWORD` | ✅ Yes | - | PostgreSQL password |
| `POSTGRES_DB` | ✅ Yes | `onyx` | Database name |
| `REDIS_HOST` | ✅ Yes | - | Redis service name (`bears-redis`) |
| `REDIS_PORT` | No | `6379` | Redis port |
| `QDRANT_HOST` | ✅ Yes | - | Qdrant service name (`bears-qdrant`) |
| `QDRANT_PORT` | No | `6333` | Qdrant port |
| `OPENAI_API_KEY` | ✅ Yes | - | OpenAI API key for embeddings |
| `AUTH_TYPE` | No | `disabled` | Authentication mode |
| `LOG_LEVEL` | No | `info` | Logging level (debug/info/warning/error) |
| `WEB_DOMAIN` | No | - | Domain for web UI (if used) |

### Volume Configuration

**Critical**: Must share volume with Git Sync service!

```
Volume Name: bears-memory (same as git-sync)
Mount Path: /app/memory
Contents: memories/, history/, projects/, .git/
```

**File Structure**:
```
/app/memory/
├── memories/
│   ├── personal/
│   └── shared/
├── history/
├── projects/
└── .git/            # Managed by git-sync
```

## Service Connectivity

### Coolify Internal URLs

Onyx connects to other services:

```bash
# PostgreSQL (Coolify-managed)
postgresql://<postgres-user>:<postgres-password>@<postgres-host>:5432/onyx

# Redis
redis://bears-redis:6379

# Qdrant
http://bears-qdrant:6333
```

### Onyx API Access

Other services (like Letta) connect to Onyx:

```bash
# Internal (from other Coolify services)
http://bears-onyx:8080

# External (if exposed)
https://your-domain.com  # via Coolify proxy
```

## Memory Management

### How Onyx Uses Memory Files

1. **Git Sync** clones content repo to `/data`
2. **Onyx** mounts same volume at `/app/memory`
3. **Onyx reads** Markdown files from `/app/memory/memories/`, `/app/memory/history/`, `/app/memory/projects/`
4. **Onyx writes** new/updated files to same locations
5. **Git Sync detects** changes and commits/pushes to GitHub

### Memory Directory Mapping

| Git Sync Path | Onyx Path | Purpose |
|---------------|-----------|---------|
| `/data/memories/` | `/app/memory/memories/` | Long-term semantic memory |
| `/data/history/` | `/app/memory/history/` | Episodic memory logs |
| `/data/projects/` | `/app/memory/projects/` | Project-scoped context |

### File Format

Onyx expects Markdown files with YAML frontmatter:

```markdown
---
title: "Example Memory"
tags: ["preference", "personal"]
created: "2025-11-23T10:30:00Z"
updated: "2025-11-23T15:45:00Z"
---

# Example Memory Content

This is the actual memory content in Markdown.
```

## Database Migrations

### When Migrations Are Needed

- ✅ **First deployment**: No migrations needed (empty database)
- ✅ **Onyx version upgrade**: Run `alembic upgrade head`
- ✅ **Schema changes**: Run `alembic upgrade head`

### Manual Migration Steps

```bash
# 1. SSH into Onyx container (via Coolify terminal)

# 2. Run migrations
alembic upgrade head

# 3. Verify
alembic current

# 4. If issues, check logs
alembic history
```

### Rollback (if needed)

```bash
# Downgrade one revision
alembic downgrade -1

# Downgrade to specific revision
alembic downgrade <revision-id>
```

## Indexing and Vector Search

### Initial Indexing

On first start, Onyx will:
1. Scan `/app/memory/memories/`, `/app/memory/history/`, `/app/memory/projects/`
2. Parse Markdown files
3. Generate embeddings (via OpenAI API)
4. Store vectors in Qdrant
5. Store metadata in PostgreSQL

This may take several minutes for large memory collections.

### Re-indexing

To force re-index:

```bash
# Via API (if authentication is disabled)
curl -X POST http://bears-onyx:8080/api/admin/reindex

# Or restart Onyx service in Coolify
```

### Monitoring Indexing

Check Qdrant collections:

```bash
curl http://bears-qdrant:6333/collections
```

Look for `onyx_chunks` and `onyx_search` collections.

## Monitoring

### Health Check

```bash
curl http://bears-onyx:8080/health
```

### API Endpoints

```bash
# List connectors
curl http://bears-onyx:8080/api/manage/admin/connector/

# Check user info (if auth enabled)
curl http://bears-onyx:8080/api/me

# Admin endpoints (if auth disabled)
curl http://bears-onyx:8080/api/admin/settings
```

### Logs

View in Coolify dashboard or terminal:

```bash
# Look for:
# - "Onyx backend started"
# - "Connected to PostgreSQL"
# - "Connected to Qdrant"
# - "Indexing completed"
```

## Troubleshooting

### Service Won't Start

**Problem**: Container exits or crashes on startup

**Solutions**:
- Check environment variables are correct
- Verify PostgreSQL is accessible
- Confirm Redis and Qdrant are healthy
- Check logs for specific error messages
- Ensure `/app/memory` volume is mounted

### Can't Connect to PostgreSQL

**Problem**: "Connection refused" or "Could not connect to server"

**Solutions**:
- Verify `POSTGRES_HOST` matches Coolify database name
- Check `POSTGRES_PASSWORD` is correct
- Ensure both services in same Coolify project
- Test connection: `psql -h $POSTGRES_HOST -U $POSTGRES_USER -d $POSTGRES_DB`

### Can't Connect to Redis

**Problem**: Redis connection errors in logs

**Solutions**:
- Verify `REDIS_HOST=bears-redis` (or your service name)
- Check Redis service is healthy
- Test: `redis-cli -h bears-redis ping`
- Ensure both services in same Coolify network

### Can't Connect to Qdrant

**Problem**: Qdrant connection errors

**Solutions**:
- Verify `QDRANT_HOST=bears-qdrant` (or your service name)
- Check Qdrant service is healthy
- Test: `curl http://bears-qdrant:6333/`
- Ensure both services in same Coolify network

### Memory Files Not Found

**Problem**: Onyx can't read memory files

**Solutions**:
- Verify `bears-memory` volume is mounted at `/app/memory`
- Check Git Sync has cloned content to volume
- Test: `ls -la /app/memory/` in Onyx container
- Ensure Git Sync service started before Onyx

### Embeddings Not Generated

**Problem**: No vectors in Qdrant

**Solutions**:
- Verify `OPENAI_API_KEY` is valid and has credits
- Check Onyx logs for embedding errors
- Ensure memory files have valid Markdown format
- Trigger re-index via API

### Migration Errors

**Problem**: "alembic.util.exc.CommandError" or migration failures

**Solutions**:
- Check PostgreSQL connection is working
- Verify database user has schema modification permissions
- Try manual migration: SSH into container, run `alembic upgrade head`
- Check migration history: `alembic history`
- If stuck, reset database (⚠️ destroys data): Drop DB, recreate, run migrations

## Security Considerations

### Authentication

For production, enable authentication:

```bash
# Change from disabled to basic or oauth
AUTH_TYPE=basic

# Add admin user credentials
AUTH_ADMIN_USERNAME=admin
AUTH_ADMIN_PASSWORD=secure-password-here
```

### API Access

- ✅ Use Coolify proxy for HTTPS
- ✅ Restrict external access if not needed
- ✅ Enable authentication for public deployments
- ❌ Don't expose port 8080 publicly without auth
- ❌ Don't commit API keys to Git

### Database Security

- ✅ Use strong PostgreSQL password
- ✅ Keep database internal (no public access)
- ✅ Enable backups in Coolify
- ✅ Rotate credentials periodically

## Performance Tuning

### Resource Limits

```bash
Memory: 1-2 GB (minimum 512 MB)
CPU: 1-2 cores
```

### Caching

Redis caches frequently accessed data:
- Query results
- Session data
- Temporary embeddings

Increase Redis memory if cache evictions are frequent.

### Database Connection Pool

Onyx uses SQLAlchemy connection pooling. For high load:

```bash
# Add to environment variables
DB_POOL_SIZE=20
DB_MAX_OVERFLOW=10
```

### Embedding Batch Size

For faster initial indexing:

```bash
EMBEDDING_BATCH_SIZE=100
```

## Backup and Recovery

### What to Backup

1. **PostgreSQL database** - Coolify handles this automatically
2. **Qdrant vectors** - Can be rebuilt from memory files
3. **Memory files** - Backed up by Git (in content repo)

### Recovery Process

If Onyx fails:

1. **Restore PostgreSQL** from Coolify backup
2. **Redeploy Onyx** with same configuration
3. **Memory files** automatically synced from Git
4. **Qdrant vectors** can be re-indexed if needed

## Advanced Configuration

### Custom Embedding Model

Use different OpenAI model:

```bash
OPENAI_EMBEDDING_MODEL=text-embedding-3-large
```

### Multiple Embedding Providers

Onyx supports multiple providers - see Onyx documentation for advanced config.

### Web UI (Optional)

Deploy Onyx web UI separately if needed (separate container).

## Next Steps

After Onyx is running:

1. ✅ Verify health check passes
2. ✅ Test API: `curl http://bears-onyx:8080/health`
3. ✅ Check memory files: `ls /app/memory/memories/`
4. ✅ Verify indexing: Check Qdrant collections
5. ➡️ Deploy **LiteLLM** (model gateway)
6. ➡️ Deploy **Letta** (agent orchestration)

## Coolify Service Name Reference

When deploying Letta, you'll need to reference Onyx:

```bash
# If you named the service "bears-onyx"
ONYX_URL=http://bears-onyx:8080

# If you named it something else
ONYX_URL=http://<your-service-name>:8080
```
