# BEARS Stack - Coolify Deployment Guide

Complete guide for deploying the BEARS Stack on Coolify with separate service deployments.

## Table of Contents

1. [Overview](#overview)
2. [Prerequisites](#prerequisites)
3. [Architecture](#architecture)
4. [Deployment Order](#deployment-order)
5. [Step-by-Step Deployment](#step-by-step-deployment)
6. [Post-Deployment](#post-deployment)
7. [Verification](#verification)
8. [Troubleshooting](#troubleshooting)

## Overview

The BEARS Stack uses a **two-repository architecture**:

1. **This repository** (`bears-deploy`) - Configuration and deployment guides
2. **Content repository** - Your memory files (created from `content-template/`)

Services are deployed individually in Coolify, leveraging:
- Coolify-managed PostgreSQL with automatic backups
- Internal Docker networking for service communication
- Git-based memory synchronization
- Persistent volumes for data

## Prerequisites

### Infrastructure

- ✅ Coolify instance running (v4.0+)
- ✅ At least 6 GB RAM available
- ✅ 20 GB disk space for data and images
- ✅ Domain name (optional, for HTTPS access)

### Accounts and Keys

- ✅ GitHub account
- ✅ OpenAI API key (for GPT models and embeddings)
- ✅ Anthropic API key (for Claude models)
- ✅ GitHub Personal Access Token (PAT) with Contents: Read/Write permissions

### Local Tools

- ✅ Git client
- ✅ `openssl` for generating secure keys
- ✅ Web browser for Coolify and Letta UI

## Architecture

### Service Dependencies

```
Layer 4: Application
├── OpenWebUI (Primary chat UI + Multi-user support)
└── Letta (Agent orchestration API + tooling)

Layer 3: APIs
├── Knowledgebase / Memory Service API (Memory management)
└── LiteLLM (Model gateway)

Layer 2: Memory
└── Git Sync (GitHub synchronization)

Layer 1: Infrastructure
├── PostgreSQL (Database - Coolify-managed)
├── Redis (Cache)
├── Qdrant (Vector database)
├── MongoDB (User data - Coolify-managed or external)
└── MeiliSearch (Search functionality - optional)
```

### Data Flow

```
User → OpenWebUI → Letta → LiteLLM → OpenAI/Anthropic APIs
        ↓          ↓
        ↓     Knowledgebase / Memory Service ← PostgreSQL (metadata)
        ↓          ↓
        ↓          Qdrant (vectors)
        ↓          Redis (cache)
     Markdown files
        ↓
    Git Sync → GitHub (backup)

OpenWebUI handles UI, authentication, and conversation management while delegating agent execution to Letta via open-webui-tools functions.
```

## Deployment Order

Services **must** be deployed in this order:

1. **PostgreSQL** (Coolify-managed database)
2. **Redis** (Cache layer)
3. **Qdrant** (Vector database)
4. **Git Sync** (Memory synchronization)
5. **Knowledgebase / Memory Service** (Memory management)
6. **LiteLLM** (Model gateway)
7. **Letta** (Agent orchestration)
8. **OpenWebUI** (Primary chat UI)

## Step-by-Step Deployment

### Step 0: Prepare Content Repository

Before deploying any services, create your content repository.

#### 0.1. Fork Content Template

```bash
# Clone this repository locally
git clone https://github.com/TheArtificial/bears-deploy.git
cd bears-deploy

# Copy content template
cp -r content-template ../bears-content
cd ../bears-content

# Initialize as new repository
rm -rf .git
git init
git add .
git commit -m "Initial commit from BEARS content template"

# Create repository on GitHub and push
# (Create "bears-content" repository on GitHub first)
git remote add origin https://github.com/YourUsername/bears-content.git
git branch -M main
git push -u origin main
```

#### 0.2. Create GitHub Personal Access Token

1. Go to GitHub → Settings → Developer Settings → Personal Access Tokens → Fine-grained tokens
2. Click "Generate new token"
3. Configure:
   - **Token name**: `BEARS Git Sync`
   - **Expiration**: 90 days
   - **Repository access**: Only `bears-content`
   - **Permissions**: Contents - **Read and write**
4. Generate and **save the token** (you won't see it again!)

### Step 1: Deploy PostgreSQL

#### 1.1. Create Database in Coolify

1. Go to Coolify → **Databases** → **Add Database**
2. Select **PostgreSQL**
3. Configure:
   - **Name**: `bears-postgres`
   - **Version**: `17`
   - **Username**: `postgres` (default)
   - **Password**: Click "Generate" or use: `openssl rand -base64 32`
   - **Database Name**: `<memory-db>`
4. Click **Deploy**
5. Wait for status: **Healthy** ✅

#### 1.2. Save Connection Details

```bash
# Note these for the knowledgebase/memory service deployment:
POSTGRES_HOST=<coolify-generated-host-name>
POSTGRES_PORT=5432
POSTGRES_USER=postgres
POSTGRES_PASSWORD=<your-generated-password>
POSTGRES_DB=<memory-db>
```

**Verify**: Check database is accessible in Coolify dashboard.

---

### Step 2: Deploy Redis

See [`services/redis/COOLIFY_DEPLOY.md`](services/redis/COOLIFY_DEPLOY.md) for detailed instructions.

#### 2.1. Create Service

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
   - **Service Name**: `bears-redis`
   - **Image**: `redis:7-alpine`
   - **Port**: 6379 (internal only)

#### 2.2. Add Persistent Storage

- **Volume Name**: `bears-redis-data`
- **Mount Path**: `/data`

#### 2.3. Configure Health Check

```bash
Command: redis-cli ping | grep PONG
Interval: 10s
Timeout: 5s
Retries: 5
```

#### 2.4. Deploy

Click **Deploy** and wait for status: **Healthy** ✅

**Verify**: Test in Coolify terminal: `redis-cli ping` → `PONG`

---

### Step 3: Deploy Qdrant

See [`services/qdrant/COOLIFY_DEPLOY.md`](services/qdrant/COOLIFY_DEPLOY.md) for detailed instructions.

#### 3.1. Create Service

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
   - **Service Name**: `bears-qdrant`
   - **Image**: `qdrant/qdrant:latest`
   - **Port**: 6333 (internal)

#### 3.2. Add Persistent Storage

- **Volume Name**: `bears-qdrant-data`
- **Mount Path**: `/qdrant/storage`

#### 3.3. Configure Health Check

```bash
Command: wget --no-verbose --tries=1 --spider http://localhost:6333/readyz || exit 1
Interval: 30s
Timeout: 10s
Start Period: 60s
```

#### 3.4. Set Resource Limits

- **Memory**: 2 GB
- **CPU**: 2 cores

#### 3.5. Deploy

Click **Deploy** and wait for status: **Healthy** ✅

**Verify**: Test in Coolify terminal: `curl http://localhost:6333/` → Returns Qdrant version info

---

### Step 4: Deploy Git Sync

See [`services/git-sync/COOLIFY_DEPLOY.md`](services/git-sync/COOLIFY_DEPLOY.md) for detailed instructions.

#### 4.1. Create Service

1. Coolify → **Add Resource** → **Docker Image**
2. Choose **Build from Git Repository**
3. Configure:
   - **Service Name**: `bears-git-sync`
   - **Git Repository**: `https://github.com/TheArtificial/bears-deploy`
   - **Branch**: `main`
   - **Dockerfile**: `services/git-sync/Dockerfile`
   - **Build Context**: `services/git-sync`

#### 4.2. Configure Environment Variables

```bash
# Content Repository
GIT_SYNC_REPO=https://github.com/YourUsername/bears-content.git
GIT_SYNC_BRANCH=main

# GitHub Authentication
GIT_USERNAME=your-github-username
GIT_PASSWORD=ghp_your_personal_access_token

# Git Identity
GIT_AUTHOR_NAME=BEARS Git Sync
GIT_AUTHOR_EMAIL=git-sync@yourdomain.com

# Optional: Sync interval (default: 300s / 5 min)
GIT_SYNC_INTERVAL=300
```

#### 4.3. Create Shared Volume

**Critical**: This volume will be shared with the knowledgebase/memory service!

- **Volume Name**: `bears-memory`
- **Mount Path**: `/data`

#### 4.4. Deploy

Click **Deploy** and watch logs for:

```
🐻 BEARS Git Sync starting...
📦 Cloning repository for the first time...
✅ Repository cloned successfully
✅ Git sync is running!
```

**Verify**:
- Check logs show successful clone
- Test in terminal: `ls -la /data/` → Should show `memories/`, `history/`, `projects/`, `.git/`
- Check GitHub repository for auto-commit test

---

### Step 5: Deploy Knowledgebase / Memory Service API

See the knowledgebase/memory service deployment guide for detailed instructions.

#### 5.1. Create Service

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
   - **Service Name**: `bears-knowledgebase` (or another name you choose)
   - **Image**: `<your-knowledgebase-image>` (choose the RMCP+Qdrant image or other implementation)
   - **Port**: 8080

#### 5.2. Configure Environment Variables

```bash
# PostgreSQL (from Step 1)
POSTGRES_HOST=<your-coolify-postgres-host>
POSTGRES_PORT=5432
POSTGRES_USER=postgres
POSTGRES_PASSWORD=<from-step-1>
POSTGRES_DB=<memory-db>

# Redis
REDIS_HOST=<bears-redis-ip-addr>
REDIS_PORT=6379

# Qdrant
QDRANT_HOST=bears-qdrant
QDRANT_PORT=6333

# OpenAI API
OPENAI_API_KEY=sk-your-openai-api-key

# Authentication
AUTH_TYPE=disabled
```

#### 5.3. Mount Shared Volume

**Critical**: Use the SAME volume as Git Sync!

- **Volume Name**: `bears-memory` (same as git-sync)
- **Mount Path**: `/app/memory`

#### 5.4. Configure Health Check

```bash
Command: wget --no-verbose --tries=1 --spider http://localhost:8080/health || exit 1
Interval: 30s
Timeout: 10s
Start Period: 60s
```

#### 5.5. Deploy

Click **Deploy** and watch logs for startup/connectivity messages (varies by implementation):

```
Knowledgebase backend started
Connected to PostgreSQL
Connected to Qdrant
```

**Note**: First deployment doesn't need migrations (empty database).

**Verify**:
- Test: `curl http://bears-knowledgebase:8080/health` → expected health response
- Check terminal: `ls -la /app/memory/` → Should show `memories/`, `history/`, `projects/`

---

### Step 6: Deploy LiteLLM

See [`services/litellm/COOLIFY_DEPLOY.md`](services/litellm/COOLIFY_DEPLOY.md) for detailed instructions.

#### 6.1. Create Service

1. Coolify → **Add Resource** → **Docker Image**
2. Choose **Build from Git Repository**
3. Configure:
   - **Service Name**: `bears-litellm`
   - **Git Repository**: `https://github.com/TheArtificial/bears-deploy`
   - **Branch**: `main`
   - **Dockerfile**: `services/litellm/docker/litellm/Dockerfile`
   - **Build Context**: `services/litellm/docker/litellm`

#### 6.2. Configure Environment Variables

```bash
# LLM Provider API Keys
OPENAI_API_KEY=sk-your-openai-key
ANTHROPIC_API_KEY=sk-ant-your-anthropic-key

# LiteLLM Configuration
LITELLM_MASTER_KEY=<generate: openssl rand -hex 32>
PORT=4000
```

#### 6.3. Mount Configuration File

Mount `services/litellm/litellm-config.yaml` from repository:

- **Source**: From repository at `services/litellm/litellm-config.yaml`
- **Target**: `/app/config.yaml`
- **Read Only**: Yes

**Or** create custom config volume.

<!-- #### 6.4. Set Command Override

```bash
--config /app/config.yaml --port 4000
``` -->

#### 6.5. Configure Health Check

```bash
Command: wget --no-verbose --tries=1 --spider http://localhost:4000/health/liveliness || exit 1
Interval: 30s
Timeout: 10s
Start Period: 40s
```

#### 6.6. Deploy

Click **Deploy** and wait for status: **Healthy** ✅

**Verify**:
- Test: `curl http://bears-litellm:4000/health/liveliness`
- List models: `curl http://bears-litellm:4000/v1/models`

---

### Step 7: Deploy Letta

See [`services/letta/COOLIFY_DEPLOY.md`](services/letta/COOLIFY_DEPLOY.md) for detailed instructions.

#### 7.1. Create Service

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
   - **Service Name**: `bears-letta`
   - **Image**: `letta/letta:latest`
   - **Port**: 8283 (expose externally or use Coolify proxy)

#### 7.2. Configure Environment Variables

```bash
# Service Integration
KNOWLEDGEBASE_URL=http://bears-knowledgebase:8080
LLM_API_URL=http://bears-litellm:4000/v1

# Model Configuration
MODEL_NAME=gpt-5

# Letta Server
LETTA_SERVER_PORT=8283
LETTA_SERVER_PASS=<generate: openssl rand -base64 32>

# OpenAI (for embeddings)
OPENAI_API_KEY=sk-your-openai-key

# LiteLLM Master Key (optional)
# If LiteLLM requires a master key, set this to match the `LITELLM_MASTER_KEY` used by the `bears-litellm` service.
# For local/dev you may leave this unset to allow unauthenticated LiteLLM (not recommended for production).
# Example: LITELLM_MASTER_KEY=sk-litellm-<hex>
```

#### 7.3. Add Persistent Storage

- **Volume Name**: `bears-letta-data`
- **Mount Path**: `/root/.letta`

#### 7.4. Configure Health Check

```bash
Command: curl -f http://localhost:8283/v1/health || exit 1
Interval: 30s
Timeout: 10s
Start Period: 40s
```

#### 7.5. Deploy

Click **Deploy** and wait for status: **Healthy** ✅

**Verify**:
- Test: `curl http://bears-letta:8283/v1/health`
- Access Web UI at configured domain or `http://<server-ip>:8283`

---

### Step 8: Deploy OpenWebUI (Primary Chat UI)

OpenWebUI provides a modern, extensible chat interface for interacting with Letta agents.

#### 8.1. Create OpenWebUI Service

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
    - **Service Name**: `bears-openwebui`
    - **Image**: `ghcr.io/open-webui/open-webui:main` (or `latest`)
    - **Port**: 3000 (expose externally via Coolify proxy)

#### 8.2. Environment Variables

```bash
# Core Configuration
WEBUI_SECRET_KEY=<generate: openssl rand -base64 32>
WEBUI_JWT_SECRET_KEY=<generate: openssl rand -base64 32>
WEBUI_JWT_ACCESS_TOKEN_EXPIRES_IN=86400
WEBUI_JWT_REFRESH_TOKEN_EXPIRES_IN=604800

# Database (OpenWebUI uses SQLite by default, or PostgreSQL)
# For PostgreSQL (recommended for production):
DATABASE_URL=postgresql://user:password@bears-postgres:5432/openwebui

# Letta Integration
LETTA_API_URL=http://bears-letta:8283/v1
LETTA_SERVER_PASS=<your-letta-password>

# Optional: Knowledgebase integration
KNOWLEDGEBASE_URL=http://bears-knowledgebase:8080
```

**Important**: Generate secure secrets:
```bash
WEBUI_SECRET_KEY=$(openssl rand -base64 32)
WEBUI_JWT_SECRET_KEY=$(openssl rand -base64 32)
```

#### 8.3. Add Persistent Storage

- **Volume Name**: `bears-openwebui-data`
- **Mount Path**: `/app/backend/data`

#### 8.4. Configure Health Check

```bash
Command: curl -f http://localhost:3000/api/health || exit 1
Interval: 30s
Timeout: 10s
Start Period: 60s
```

#### 8.5. Deploy

Click **Deploy** and wait for **Healthy** status.

#### 8.6. Configure Domain and SSL

1. In Coolify, configure custom domain for OpenWebUI service
2. Enable SSL/TLS certificate
3. Access OpenWebUI at `https://openwebui.yourdomain.com`

---

### Step 9: Install OpenWebUI Tools Integration

To connect Letta agents as "models" in OpenWebUI, install functions from [open-webui-tools](https://github.com/Haervwe/open-webui-tools).

#### 9.1. Access OpenWebUI

1. Navigate to your OpenWebUI instance
2. Log in or create an admin account
3. Go to **Settings** → **Workspace** → **Functions**

#### 9.2. Install Letta Integration Function

1. Visit the [open-webui-tools repository](https://github.com/Haervwe/open-webui-tools)
2. Find the function that connects to Letta agents (or use the pipe function from `services/letta/openwebui_pipe_example.py`)
3. Copy the function code into OpenWebUI's Functions section
4. Configure the function with your Letta API URL and credentials:
   - `LETTA_API_URL=http://bears-letta:8283/v1`
   - `LETTA_SERVER_PASS=<your-letta-password>`

#### 9.3. Register Letta Agents as Models

1. In OpenWebUI, go to **Settings** → **Models**
2. Add a custom model/provider that uses your Letta integration function
3. Letta agents will appear as selectable models in the chat interface

**Note**: See `services/letta/OPENWEBUI_SESSIONS.md` for detailed session management strategies and `services/letta/openwebui_pipe_example.py` for a complete pipe function implementation.

---

## Post-Deployment

### Access the Web UI

#### OpenWebUI (Primary Chat UI)
1. Navigate to your configured OpenWebUI domain or `http://<server-ip>:3000`
2. Create an admin account or register as a new user
3. Configure Letta agents as models (via open-webui-tools functions)
4. Start chatting with Letta agents!

#### Letta (Agent Management API)
1. Access internally via `http://bears-letta:8283` or VPN
2. Login with `LETTA_SERVER_PASS`
3. Create/maintain agents, tools, and memory integrations
4. Use API for automation or advanced workflows

### Verify End-to-End Functionality

1. **Create a test agent** in Letta Web UI
2. **Chat with the agent** - ask it to remember something
3. **Check GitHub** - verify memory file was created and auto-committed
4. **Check Qdrant** - `curl http://bears-qdrant:6333/collections`
5. **Chat again** - verify agent recalls the previous context

### Configure Domain (Optional)

1. In Coolify, add custom domain for Letta service
2. Configure SSL/TLS certificate
3. Access via `https://your-domain.com`

## Verification

### Service Health Checklist

Check all services are healthy in Coolify dashboard:

- [ ] PostgreSQL - **Healthy** ✅
- [ ] Redis - **Healthy** ✅
- [ ] Qdrant - **Healthy** ✅
- [ ] Git Sync - **Healthy** ✅
- [ ] Knowledgebase API - **Healthy** ✅
- [ ] LiteLLM - **Healthy** ✅
- [ ] Letta - **Healthy** ✅
- [ ] OpenWebUI - **Healthy** ✅

### Connectivity Tests

```bash
# From any service terminal in Coolify:

# Test Redis
redis-cli -h bears-redis ping

# Test Qdrant
curl http://bears-qdrant:6333/

# Test Knowledgebase / Memory Service
curl http://bears-knowledgebase:8080/health

# Test LiteLLM
curl http://bears-litellm:4000/health/liveliness

# Test Letta
curl http://bears-letta:8283/v1/health

# Test OpenWebUI
curl http://bears-openwebui:3000/api/health
```

### Memory Sync Verification

1. Check Git Sync logs for successful syncs
2. Visit GitHub repository - should have recent auto-commits
3. Create a test file locally and push - should sync within 5 minutes
4. Create an agent memory in Letta - should appear in GitHub

## Troubleshooting

### Service Won't Start

1. Check logs in Coolify dashboard
2. Verify environment variables are set correctly
3. Ensure dependencies are healthy (check service order)
4. Review service-specific troubleshooting in `COOLIFY_DEPLOY.md` files

### Connectivity Issues

**Problem**: Service A can't connect to Service B

**Solutions**:
- Verify both services in same Coolify project
- Check service names match environment variables
- Test connectivity from Coolify terminal
- Ensure target service is healthy

### Git Sync Not Pushing

**Problem**: No commits appearing on GitHub

**Solutions**:
- Verify `GIT_PASSWORD` (PAT) is valid and has write permissions
- Check `GIT_SYNC_REPO` URL is correct
- Review Git Sync logs for authentication errors
- Test PAT: `curl -H "Authorization: token $GIT_PASSWORD" https://api.github.com/user`

### Memory Files Not Found

**Problem**: Knowledgebase / memory service can't read memory files

**Solutions**:
- Verify `bears-memory` volume is shared between Git Sync and the memory service
- Check Git Sync cloned successfully: `ls /data/` in git-sync terminal
- Check the memory service can see files: `ls /app/memory/` in the knowledgebase container
- Review mount paths in both services

### Agents Not Creating Memories

**Problem**: Letta agents don't persist memories

**Solutions**:
- Verify Letta → knowledgebase connection: `curl $KNOWLEDGEBASE_URL/health` from Letta terminal
- Check memory service logs for errors
- Test memory service write permissions: Check `/app/memory/` is writable
- Review Git Sync logs for commit errors

### Resource Exhaustion

**Problem**: Services OOMKilled or slow performance

**Solutions**:
- Check resource usage in Coolify
- Increase memory limits (especially Qdrant, memory service)
- Scale vertically or horizontally
- Monitor disk space

### OpenWebUI Connection Issues

**Problem**: OpenWebUI can't connect to Letta agents

**Solutions**:
- Verify Letta integration function is properly installed in OpenWebUI
- Check `LETTA_API_URL` and `LETTA_SERVER_PASS` are correct in function configuration
- Ensure Letta service is healthy: `curl http://bears-letta:8283/v1/health`
- Review OpenWebUI logs for connection errors
- Verify Letta agents are properly registered as models in OpenWebUI

**Problem**: Letta agents not appearing in model list

**Solutions**:
- Verify open-webui-tools function is installed and enabled
- Check function configuration matches your Letta setup
- Review OpenWebUI function logs for errors
- Ensure Letta API is accessible from OpenWebUI container

**Problem**: Database connection issues (if using PostgreSQL)

**Solutions**:
- Verify PostgreSQL service is healthy
- Check `DATABASE_URL` format: `postgresql://user:password@bears-postgres:5432/openwebui`
- Ensure network connectivity between services
- Review OpenWebUI logs for database errors

## Next Steps

### Production Hardening

- [ ] Enable authentication on the knowledgebase/memory service (`AUTH_TYPE=basic`)
- [ ] Set up HTTPS for external access
- [ ] Configure Coolify backups
- [ ] Set up monitoring/alerting
- [ ] Document recovery procedures
- [ ] Test backup/restore process

### Customization

- [ ] Add more models to LiteLLM config
- [ ] Customize memory structure in content repository
- [ ] Create project-specific memory directories
- [ ] Set up multiple agents for different purposes
- [ ] Configure agent tools/functions

### Ongoing Maintenance

- [ ] Monitor service health daily
- [ ] Review Git commits weekly
- [ ] Update Docker images monthly
- [ ] Rotate API keys quarterly
- [ ] Review and optimize memory structure

## Support

For detailed troubleshooting:

- **Service-specific issues**: See `services/{service}/COOLIFY_DEPLOY.md`
- **Architecture questions**: Review `ARCHITECTURE_NOTES.md`
- **Memory system**: See `content-template/README.md`

---

**Deployment Complete!** 🎉

Your BEARS Stack is now fully operational with:
- ✅ Git-versioned memory management
- ✅ Automatic GitHub synchronization
- ✅ Semantic search via Qdrant
- ✅ Multi-model support via LiteLLM
- ✅ Coolify-managed infrastructure
- ✅ Modern chat UI with OpenWebUI
- ✅ Multi-user authentication and conversation management
- ✅ Letta agent integration via open-webui-tools

Start building your agentic assistants with Letta (agent management) and OpenWebUI (modern chat interface)! 🐻

**Future Enhancement**: A middleware layer is planned to provide user-identity mapping, agent access control, and user-aware memory context. See `ARCHITECTURE_NOTES.md` for details.
