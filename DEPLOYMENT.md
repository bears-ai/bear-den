# BEARS Stack Deployment Checklist

## Pre-Deployment Checklist

### ✅ Configuration Files
- [x] `.env.example` created with all required variables
- [x] `docker-compose.yaml` configured with correct ports
- [x] `litellm-config.yaml` configured for model routing
- [x] Onyx configured for Git-versioned memory management
- [x] Port conflicts resolved (Letta API: 3000, Letta ADE: 8283, Onyx: 8080, Qdrant: 6333, LiteLLM: 4000)
- [x] Health checks added to all services
- [x] Restart policies configured
- [x] Volume mounts configured for memory directories

### ✅ Directory Structure
- [x] `memories/` directory created with README
- [x] `memories/personal/` subdirectory created
- [x] `memories/shared/` subdirectory created
- [x] `history/` directory created with README
- [x] `projects/` directory created with README

### 📋 Before First Deployment

- [ ] Copy `.env.example` to `.env`
- [ ] Add your `OPENAI_API_KEY` to `.env`
- [ ] Add your `ANTHROPIC_API_KEY` to `.env`
- [ ] Generate secure random string for `LETTA_API_KEY`
- [ ] Generate secure random string for `LITELLM_MASTER_KEY`
- [ ] Generate secure random string for `POSTGRES_PASSWORD`
- [ ] Review and customize `litellm-config.yaml` if needed
- [ ] Ensure Docker and Docker Compose are installed
- [ ] Ensure at least 4GB RAM is available

### 🔧 ARM64 Deployment Notes

**Good news**: This stack works on ARM64 (Apple Silicon, AWS Graviton, etc.)!

- **LiteLLM**: Native ARM64 support ✅
- **Letta**: Uses x86_64 emulation via `platform: linux/amd64` in docker-compose.yaml
- **Other services**: All support ARM64 natively (PostgreSQL, Qdrant, Onyx)

**Performance Note**: Letta runs under emulation on ARM64, which adds ~10-20% overhead. This is acceptable for most use cases. If you need maximum performance, consider using an x86_64 server, but it's not required.

## Deployment Steps

### 1. Environment Setup

```bash
# Copy environment template
cp .env.example .env

# Edit with your API keys
nano .env  # or use your preferred editor

# Generate secure keys (example using openssl)
openssl rand -hex 32  # Use for LETTA_API_KEY
openssl rand -hex 32  # Use for LITELLM_MASTER_KEY
openssl rand -hex 32  # Use for POSTGRES_PASSWORD
```

### 2. Start Services

```bash
# Pull latest images
docker-compose pull

# Start all services in detached mode
docker-compose up -d

# Watch logs during startup
docker-compose logs -f
```

### 3. Verify Deployment

```bash
# Check service status
docker-compose ps

# All services should show (healthy) status after ~1 minute

# Test individual endpoints
curl http://localhost:3000/health  # Letta API
curl http://localhost:8080/health  # Onyx API
curl http://localhost:6333/health  # Qdrant
curl http://localhost:4000/health  # LiteLLM

# Access Letta Web UI in browser
open http://localhost:8283  # macOS
# or visit http://localhost:8283 in your browser
```

### 4. Initial Configuration

```bash
# Initialize Git for memory tracking (if not already done)
git init
git add memories/ history/ projects/
git commit -m "Initialize memory directories"

# Create your first user memory file
mkdir -p memories/personal/$(whoami)
echo "# My Preferences" > memories/personal/$(whoami)/preferences.md
git add memories/
git commit -m "Add initial user preferences"
```

## Post-Deployment Verification

### Service Health Checks

| Service | Port | Health Endpoint | Expected Response |
|---------|------|-----------------|-------------------|
| Letta API | 3000 | `/health` | 200 OK |
| Letta ADE (Web UI) | 8283 | N/A | Web interface |
| Onyx API | 8080 | `/health` | 200 OK |
| Qdrant | 6333 | `/health` | 200 OK |
| LiteLLM | 4000 | `/health` | 200 OK |

### Volume Verification

```bash
# Verify named volumes exist
docker volume ls | grep bears-stack

# Expected volumes:
# - bears-stack_qdrant_data
# - bears-stack_letta_data
# - bears-stack_onyx_db_data
```

### Memory Directory Verification

```bash
# Verify directory structure
ls -la memories/
ls -la history/
ls -la projects/

# Verify Onyx can access memory directories
docker-compose exec onyx-api-server ls -la /app/memories
docker-compose exec onyx-api-server ls -la /app/history
docker-compose exec onyx-api-server ls -la /app/projects
```

## Troubleshooting

### Services Not Starting

1. **Check logs**
   ```bash
   docker-compose logs <service-name>
   ```

2. **Common issues**
   - Missing environment variables → Check `.env` file
   - Port conflicts → Check `lsof -i :<port>`
   - Insufficient memory → Check `docker stats`
   - Image pull failures → Check internet connection

### Port Conflicts

If you see "port already in use" errors:

```bash
# Find what's using the port
lsof -i :8283  # Letta ADE (Web UI)
lsof -i :3000  # Letta API
lsof -i :8080  # Onyx API
lsof -i :6333  # Qdrant
lsof -i :4000  # LiteLLM

# Option 1: Stop the conflicting service
# Option 2: Change port in docker-compose.yaml
```

### Health Checks Failing

```bash
# Check if service is actually running
docker-compose ps

# Check service logs
docker-compose logs <service-name>

# Restart specific service
docker-compose restart <service-name>

# Full restart
docker-compose down
docker-compose up -d
```

### Memory Not Persisting

```bash
# Check volume mounts
docker-compose exec onyx-api-server df -h

# Verify volumes exist
docker volume inspect bears-stack_qdrant_data
docker volume inspect bears-stack_letta_data
docker volume inspect bears-stack_onyx_db_data

# If volumes are missing, recreate them
docker-compose down -v
docker-compose up -d
```

## Coolify Deployment

### Prerequisites

- Coolify instance running
- Git repository accessible to Coolify
- Environment variables configured in Coolify

### Steps

1. **Add Repository to Coolify**
   - Go to Coolify dashboard
   - Add new resource → Docker Compose
   - Connect to your Git repository

2. **Configure Environment Variables**
   - In Coolify, go to Environment Variables
   - Add all variables from `.env.example`
   - Save configuration

3. **Deploy**
   - Click "Deploy" in Coolify
   - Monitor deployment logs
   - Verify all services are healthy

4. **Configure Domain (Optional)**
   - Add custom domain in Coolify
   - Configure SSL/TLS certificates
   - Update service URLs if needed

## Backup Strategy

### What Needs Backing Up?

**Critical Data (Git-versioned):**
- `memories/` - All semantic memory in Markdown files
- `history/` - Conversation transcripts and logs
- `projects/` - Project context and notes

These are your **source of truth** and are already backed up via Git commits.

**Ephemeral Data (Can be rebuilt):**
- PostgreSQL (`onyx_db_data`) - Metadata only, can be regenerated from Git files
- Qdrant (`qdrant_data`) - Vector embeddings, can be re-indexed from memory files
- Letta (`letta_data`) - Configuration, can be recreated

**Backup Priority:**
1. **Essential**: Git repository (memories, history, projects) - This is your only irreplaceable data
2. **Optional**: Qdrant vectors - Saves re-indexing time but can be rebuilt
3. **Skip**: PostgreSQL - Just metadata that Onyx regenerates from files

### Automated Backups

```bash
# Create backup script
cat > backup.sh << 'EOF'
#!/bin/bash
DATE=$(date +%Y%m%d_%H%M%S)
BACKUP_DIR="./backups/$DATE"
mkdir -p "$BACKUP_DIR"

# Backup Qdrant data (optional - saves re-indexing time)
docker-compose exec -T qdrant tar czf - /qdrant/storage > "$BACKUP_DIR/qdrant.tar.gz"

# Backup memory files (Git) - THIS IS THE CRITICAL BACKUP
git add memories/ history/ projects/
git commit -m "Backup: $DATE" || true
git push  # Push to remote for off-site backup

echo "Backup completed: $BACKUP_DIR"
echo "Memory files committed to Git (source of truth)"
EOF

chmod +x backup.sh
```

### Restore from Backup

```bash
# Restore memory files (Git) - PRIMARY RESTORE METHOD
git checkout <commit-hash>
# or
git pull  # If restoring from remote

# Restart services - Onyx will regenerate PostgreSQL metadata from files
docker-compose down
docker-compose up -d

# Optional: Restore Qdrant data (if you backed it up)
# This saves re-indexing time but is not required
docker-compose down
docker volume rm bears-stack_qdrant_data
docker volume create bears-stack_qdrant_data
docker run --rm -v bears-stack_qdrant_data:/qdrant/storage -v $(pwd)/backups/YYYYMMDD_HHMMSS:/backup alpine tar xzf /backup/qdrant.tar.gz -C /
docker-compose up -d

# If you didn't backup Qdrant, Onyx will re-index from memory files automatically
```

### Disaster Recovery

If you lose everything except your Git repository:

```bash
# Clone your repository
git clone <your-repo-url> bears-stack
cd bears-stack

# Set up environment
cp .env.example .env
# Edit .env with your API keys

# Start services - Onyx will rebuild everything from memory files
docker-compose up -d

# Onyx automatically:
# - Recreates PostgreSQL metadata from Markdown files
# - Re-indexes all content into Qdrant vectors
# - Restores full system state from Git history
```

## Monitoring

### Log Monitoring

```bash
# Real-time logs for all services
docker-compose logs -f

# Logs for specific service
docker-compose logs -f letta

# Last 100 lines
docker-compose logs --tail=100
```

### Resource Monitoring

```bash
# Container resource usage
docker stats

# Disk usage
docker system df

# Volume usage
docker volume ls
```

## Maintenance

### Regular Tasks

- **Daily**: Check service health and logs
- **Weekly**: Review and commit memory changes
- **Monthly**: Update Docker images, backup data
- **Quarterly**: Review and optimize memory structure

### Updating Services

```bash
# Pull latest images
docker-compose pull

# Restart with new images
docker-compose up -d

# Clean up old images
docker image prune -a
```

## Security Considerations

- [ ] Use strong, unique values for `LETTA_API_KEY` and `LITELLM_MASTER_KEY`
- [ ] Never commit `.env` file to Git (already in `.gitignore`)
- [ ] Restrict network access to services if deployed publicly
- [ ] Regularly update Docker images for security patches
- [ ] Review memory files for sensitive information before sharing
- [ ] Use HTTPS/TLS if exposing services publicly
- [ ] Consider implementing rate limiting for API endpoints

## Next Steps

After successful deployment:

1. Create user-specific memory files in `memories/personal/`
2. Set up your first project in `projects/`
3. Test agent interactions through Letta API
4. Configure additional LLM models in `litellm-config.yaml`
5. Set up automated backups
6. Configure monitoring and alerting (optional)

## Support

For issues or questions:
- Check service logs: `docker-compose logs <service>`
- Review documentation in `.kilocode/memory_bank/`
- Check project issues on GitHub