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
├── Letta (Agent orchestration + Web UI)

Layer 3: APIs
├── Onyx API Server (Memory management)
└── LiteLLM (Model gateway)

Layer 2: Memory
└── Git Sync (GitHub synchronization)

Layer 1: Infrastructure
├── PostgreSQL (Database - Coolify-managed)
├── Redis (Cache)
└── Qdrant (Vector database)
```

### Data Flow

```
User → Letta → LiteLLM → OpenAI/Anthropic APIs
           ↓
         Onyx ← PostgreSQL (metadata)
           ↓     Qdrant (vectors)
           ↓     Redis (cache)
     Markdown files
           ↓
       Git Sync → GitHub (backup)
```

## Deployment Order

Services **must** be deployed in this order:

1. **PostgreSQL** (Coolify-managed database)
2. **Redis** (Cache layer)
3. **Qdrant** (Vector database)
4. **Git Sync** (Memory synchronization)
5. **Onyx API Server** (Memory management)
6. **LiteLLM** (Model gateway)
7. **Letta** (Agent orchestration)

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
   - **Database Name**: `onyx`
4. Click **Deploy**
5. Wait for status: **Healthy** ✅

#### 1.2. Save Connection Details

```bash
# Note these for Onyx deployment:
POSTGRES_HOST=<coolify-generated-host-name>
POSTGRES_PORT=5432
POSTGRES_USER=postgres
POSTGRES_PASSWORD=<your-generated-password>
POSTGRES_DB=onyx
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

**Critical**: This volume will be shared with Onyx!

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

### Step 5: Deploy Onyx API Server

See [`services/onyx/COOLIFY_DEPLOY.md`](services/onyx/COOLIFY_DEPLOY.md) for detailed instructions.

#### 5.1. Create Service

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
   - **Service Name**: `bears-onyx`
   - **Image**: `onyxdotapp/onyx-backend:latest`
   - **Port**: 8080

#### 5.2. Configure Environment Variables

```bash
# PostgreSQL (from Step 1)
POSTGRES_HOST=<your-coolify-postgres-host>
POSTGRES_PORT=5432
POSTGRES_USER=postgres
POSTGRES_PASSWORD=<from-step-1>
POSTGRES_DB=onyx

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

Click **Deploy** and watch logs for:

```
Onyx backend started
Connected to PostgreSQL
Connected to Qdrant
```

**Note**: First deployment doesn't need migrations (empty database).

**Verify**:
- Test: `curl http://bears-onyx:8080/health` → `{"status": "ok"}`
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
ONYX_URL=http://bears-onyx:8080
LLM_API_URL=http://bears-litellm:4000/v1

# Model Configuration
MODEL_NAME=gpt-4

# Letta Server
LETTA_SERVER_PORT=8283
LETTA_SERVER_PASS=<generate: openssl rand -base64 32>

# OpenAI (for embeddings)
OPENAI_API_KEY=sk-your-openai-key
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

## Post-Deployment

### Access the Web UI

1. Navigate to your configured Coolify domain or `http://<server-ip>:8283`
2. Login with `LETTA_SERVER_PASS`
3. Create your first agent
4. Start chatting!

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
- [ ] Onyx API - **Healthy** ✅
- [ ] LiteLLM - **Healthy** ✅
- [ ] Letta - **Healthy** ✅

### Connectivity Tests

```bash
# From any service terminal in Coolify:

# Test Redis
redis-cli -h bears-redis ping

# Test Qdrant
curl http://bears-qdrant:6333/

# Test Onyx
curl http://bears-onyx:8080/health

# Test LiteLLM
curl http://bears-litellm:4000/health/liveliness

# Test Letta
curl http://bears-letta:8283/v1/health
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

**Problem**: Onyx can't read memory files

**Solutions**:
- Verify `bears-memory` volume is shared between Git Sync and Onyx
- Check Git Sync cloned successfully: `ls /data/` in git-sync terminal
- Check Onyx can see files: `ls /app/memory/` in onyx terminal
- Review mount paths in both services

### Agents Not Creating Memories

**Problem**: Letta agents don't persist memories

**Solutions**:
- Verify Letta → Onyx connection: `curl $ONYX_URL/health` from Letta terminal
- Check Onyx logs for errors
- Test Onyx write permissions: Check `/app/memory/` is writable
- Review Git Sync logs for commit errors

### Resource Exhaustion

**Problem**: Services OOMKilled or slow performance

**Solutions**:
- Check resource usage in Coolify
- Increase memory limits (especially Qdrant, Onyx)
- Scale vertically or horizontally
- Monitor disk space

## Next Steps

### Production Hardening

- [ ] Enable authentication on Onyx (`AUTH_TYPE=basic`)
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

Start building your agentic assistants! 🐻
