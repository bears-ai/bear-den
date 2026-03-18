# LibreChat - Coolify Deployment Guide

Complete guide for deploying LibreChat as an additional chat UI for the BEARS Stack.

## Overview

LibreChat provides a modern, feature-rich chat interface that integrates with the BEARS Stack's Letta agent orchestration framework. It offers multi-user authentication, conversation management, and advanced features like code execution and file uploads. This deployment uses the cpfiffer/letta-libre fork, which enables LibreChat to serve as the primary UI for interacting with Letta-hosted agents.

## Prerequisites

- ✅ LiteLLM and Letta deployed and healthy  
- ✅ MongoDB (Coolify-managed or external)
- ✅ Domain configured in Coolify for LibreChat

## Architecture Integration

LibreChat integrates with BEARS services as follows:

- **Agent Interaction**: Connects to Letta (`http://bears-letta:8283`) for all agent interactions and model access
- **Memory/Context**: Letta native memory; shared knowledge via **Cabinet (Outline)** when **Den** is deployed ([PLAN.md](../../PLAN.md))
- **Authentication**: Built-in multi-user authentication with MongoDB backend
- **Search**: Uses MeiliSearch for conversation search functionality

## Deployment Steps

### Step 1: Deploy MongoDB

LibreChat requires MongoDB for user data, conversations, and configuration.

#### 1.1. Create MongoDB Service in Coolify

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
   - **Service Name**: `bears-mongodb`
   - **Image**: `mongo:7-jammy`
   - **Port**: 27017 (internal only)

#### 1.2. Add Persistent Storage

- **Volume Name**: `bears-mongodb-data`
- **Mount Path**: `/data/db`

#### 1.3. Configure Health Check

```bash
Command: mongosh --eval "db.adminCommand('ping')"
Interval: 30s
Timeout: 10s
Start Period: 30s
```

#### 1.4. Deploy

Click **Deploy** and wait for **Healthy** status.

### Step 2: Deploy MeiliSearch (Optional but Recommended)

For conversation search functionality.

#### 2.1. Create MeiliSearch Service

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
   - **Service Name**: `bears-meilisearch`
   - **Image**: `getmeili/meilisearch:v1.12.3`
   - **Port**: 7700 (internal)

#### 2.2. Environment Variables

```bash
MEILI_NO_ANALYTICS=true
MEILI_MASTER_KEY=DrhYf7zENyR6AlUCKmnz0eYASOQdl6zxH7s7MKFSfFCt
```

#### 2.3. Add Persistent Storage

- **Volume Name**: `bears-meilisearch-data`
- **Mount Path**: `/meili_data`

#### 2.4. Deploy

Click **Deploy** and wait for **Healthy** status.

### Step 3: Deploy LibreChat

#### 3.1. Create LibreChat Service

1. Coolify → **Add Resource** → **Docker Image**
2. Configure:
    - **Service Name**: `bears-librechat`
    - **Image**: `ghcr.io/cpfiffer/letta-libre:latest`
    - **Port**: 3080 (expose externally via Coolify proxy)

#### 3.2. Environment Variables

Copy the configuration from `services/librechat/.env.example` and customize:

```bash
# Core Configuration
HOST=0.0.0.0
PORT=3080
MONGO_URI=mongodb://bears-mongodb:27017/LibreChat
MEILI_HOST=http://bears-meilisearch:7700
MEILI_MASTER_KEY=DrhYf7zENyR6AlUCKmnz0eYASOQdl6zxH7s7MKFSfFCt

# Domain (update with your Coolify domain)
DOMAIN_CLIENT=https://librechat.yourdomain.com
DOMAIN_SERVER=https://librechat.yourdomain.com

# Letta Integration (primary model configuration)
LETTA_URL=http://bears-letta:8283
LETTA_SERVER_PASS=your-letta-admin-password-here

# Authentication
ALLOW_REGISTRATION=true
JWT_SECRET=your-secure-jwt-secret-here
JWT_REFRESH_SECRET=your-secure-refresh-secret-here

# File permissions
UID=1000
GID=1000
```

**Important**: Generate secure secrets for JWT:
```bash
JWT_SECRET=$(openssl rand -base64 32)
JWT_REFRESH_SECRET=$(openssl rand -base64 32)
```

**Note**: The `LETTA_SERVER_PASS` should match the password set in your Letta service configuration.

#### 3.3. Add Persistent Storage

- **Volume Name**: `bears-librechat-data`
- **Mount Path**: `/app/client/public/images`

Additional volumes for uploads and logs:
- **Volume Name**: `bears-librechat-uploads`
- **Mount Path**: `/app/uploads`

- **Volume Name**: `bears-librechat-logs`
- **Mount Path**: `/app/logs`

#### 3.4. Configure Health Check

```bash
Command: curl -f http://localhost:3080/api/health || exit 1
Interval: 30s
Timeout: 10s
Start Period: 60s
```

#### 3.5. Resource Limits

- **Memory**: 1 GB
- **CPU**: 1 core

#### 3.6. Deploy

Click **Deploy** and wait for **Healthy** status.

### Step 4: Configure Domain and SSL

1. In Coolify, configure custom domain for LibreChat service
2. Enable SSL/TLS certificate
3. Access LibreChat at `https://librechat.yourdomain.com`

## Configuration Details

### Letta Integration

LibreChat connects to Letta using the agent API configuration:

```bash
LETTA_URL=http://bears-letta:8283
LETTA_SERVER_PASS=<your-letta-admin-password>
```

This allows LibreChat to interact with Letta-hosted agents, which in turn access all models configured in LiteLLM. Letta handles agent orchestration, memory management, and model routing internally.

### Multi-User Authentication

LibreChat provides built-in user management:

- User registration (if enabled)
- Login/logout functionality
- User-specific conversation history
- Admin panel for user management

## Post-Deployment Configuration

### Step 5: Initial Setup

1. Access LibreChat at your configured domain
2. Create an admin account
3. Configure available models in LibreChat settings
4. Test model connectivity

### Step 6: Model Configuration

In LibreChat's admin panel:

1. Go to **Settings** → **Models**
2. Configure model endpoints (they should auto-detect from LiteLLM)
3. Set default models for conversations

### Step 7: User Management

1. Enable/disable user registration as needed
2. Configure user roles and permissions
3. Set up user groups if using team features

## Verification

### Health Checks

```bash
# From any service terminal in Coolify:

# Test MongoDB
mongosh mongodb://bears-mongodb:27017/LibreChat --eval "db.stats()"

# Test MeiliSearch
curl http://bears-meilisearch:7700/health

# Test LibreChat
curl https://librechat.yourdomain.com/api/health
```

### Functional Testing

1. **Login/Registration**: Create a user account
2. **Model Access**: Start a conversation and verify model selection works
3. **Conversation Persistence**: Send messages and verify they save
4. **Search**: Use the search functionality (if MeiliSearch enabled)

## Troubleshooting

### Common Issues

**LibreChat can't connect to LiteLLM:**
- Verify `OPENAI_REVERSE_PROXY` URL is correct
- Check LiteLLM service is healthy
- Review LibreChat logs for connection errors

**MongoDB connection failed:**
- Ensure MongoDB service is deployed and healthy
- Verify `MONGO_URI` format
- Check network connectivity between services

**File upload issues:**
- Verify volume mounts are correct
- Check file permissions (UID/GID settings)
- Ensure sufficient disk space

**Authentication problems:**
- Verify JWT secrets are set and secure
- Check MongoDB connectivity for user data
- Review browser console for client-side errors

### Logs and Debugging

- **LibreChat logs**: View in Coolify dashboard
- **MongoDB logs**: Check MongoDB service logs
- **Network issues**: Test connectivity between services using `curl`

## Security Considerations

- ✅ Use strong JWT secrets
- ✅ Enable HTTPS via Coolify
- ✅ Configure proper user permissions
- ✅ Regularly update LibreChat image
- ✅ Monitor for security updates

## Integration Benefits

Adding LibreChat to BEARS provides:

- **Primary UI**: Modern, feature-rich chat interface for interacting with Letta agents
- **Multi-user support**: Team collaboration capabilities
- **Advanced features**: Code execution, file uploads, conversation branching
- **Agent Orchestration**: Leverages Letta's agent management and memory systems
- **Scalability**: Handles multiple concurrent users through Letta's backend

## Primary Deployment

This deployment configures LibreChat as the primary user interface for the BEARS Stack:

1. Deploy LibreChat following the steps above (using the cpfiffer/letta-libre fork)
2. Ensure Letta is deployed with LiteLLM ([PLAN.md](../../PLAN.md) for Den/Cabinet)
3. Users access LibreChat for all chat interactions, which are routed through Letta agents
4. Letta remains available for advanced agent management and administration

---

**Deployment Complete!** 🎉

LibreChat is now integrated with your BEARS Stack, providing a modern chat interface for your AI agents.