# LiteLLM - Coolify Deployment Guide

## Overview

LiteLLM provides a unified gateway to multiple LLM providers (OpenAI, Anthropic, etc.) with a consistent API interface. It handles model routing, fallbacks, and load balancing.

## Prerequisites

- Coolify instance running
- API keys for LLM providers (OpenAI, Anthropic, etc.)
- Deploy **before** Letta

## Deployment Steps

### 1. Prepare Configuration File

LiteLLM requires a configuration file. You have two options:

#### Option A: Build Custom Image (Recommended for this stack)

This repository includes a Dockerfile and config file in `services/litellm/`.

#### Option B: Use Official Image + Config Mount

Use `ghcr.io/berriai/litellm:main-latest` and mount config as volume.

### 2. Deploy in Coolify (Custom Build)

1. **Add New Resource** → **Docker Image**

2. **Basic Configuration**:
   - **Service Name**: `bears-litellm`
   - **Deployment Type**: Build from Git Repository

3. **Build Configuration**:
   - **Git Repository**: `https://github.com/TheArtificial/bears-depoy`
   - **Branch**: `main`
   - **Dockerfile Location**: `services/litellm/docker/litellm/Dockerfile`
   - **Build Context**: `services/litellm/docker/litellm`

4. **Port Configuration**:
   - **Internal Port**: `4000`
   - **External Port**: `4000` (optional)

5. **Environment Variables**:

   ```bash
   # LLM Provider API Keys
   OPENAI_API_KEY=sk-...
   ANTHROPIC_API_KEY=sk-ant-...

   # LiteLLM Configuration
   LITELLM_MASTER_KEY=sk-litellm-...
   PORT=4000

   # Optional: Logging
   LITELLM_LOG=INFO
   ```

6. **Configuration File Mount**:

   Mount the config file from this repo:

   - **Source**: `services/litellm/litellm-config.yaml` (from repo)
   - **Target**: `/app/config.yaml`
   - **Read Only**: Yes

   **Or** create a custom config and mount it as a volume.

7. **Command Override**:

   ```bash
   --config /app/config.yaml --port 4000
   ```

8. **Health Check**:

   ```bash
   Command: wget --no-verbose --tries=1 --spider http://localhost:4000/health/liveliness || exit 1
   Interval: 30s
   Timeout: 10s
   Retries: 3
   Start Period: 40s
   ```

9. **Restart Policy**: `unless-stopped`

10. **Deploy** the service

### 3. Verify Deployment

Test the API:

```bash
# Health check
curl http://bears-litellm:4000/health/liveliness

# List models
curl http://bears-litellm:4000/v1/models

# Test completion (with master key)
curl http://bears-litellm:4000/v1/chat/completions \
  -H "Authorization: Bearer sk-litellm-..." \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration Reference

### litellm-config.yaml

Located at `services/litellm/litellm-config.yaml`:

```yaml
general_settings:
  default_model: openai/gpt-4
  telemetry: true

model_routing:
  openai/gpt-4:
    model_name: gpt-4
    provider: openai

  anthropic/claude-3-5-sonnet:
    model_name: claude-3-5-sonnet-20241022
    provider: anthropic
```

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `OPENAI_API_KEY` | ✅ Yes | OpenAI API key |
| `ANTHROPIC_API_KEY` | ✅ Yes | Anthropic API key |
| `LITELLM_MASTER_KEY` | ✅ Yes | Master key for LiteLLM API authentication |
| `PORT` | No | API port (default: 4000) |
| `LITELLM_LOG` | No | Log level (INFO/DEBUG) |

### Adding More Models

Edit `litellm-config.yaml`:

```yaml
model_routing:
  openai/gpt-4:
    model_name: gpt-4
    provider: openai

  openai/gpt-4-turbo:
    model_name: gpt-4-turbo-preview
    provider: openai

  anthropic/claude-3-5-sonnet:
    model_name: claude-3-5-sonnet-20241022
    provider: anthropic

  anthropic/claude-3-5-haiku:
    model_name: claude-3-5-haiku-20241022
    provider: anthropic
```

Redeploy in Coolify after config changes.

## Service Connectivity

### Coolify Internal URL

Letta will connect to LiteLLM:

```bash
http://bears-litellm:4000/v1
```

### OpenAI-Compatible API

LiteLLM exposes an OpenAI-compatible API at `/v1`:

- `POST /v1/chat/completions`
- `POST /v1/completions`
- `POST /v1/embeddings`
- `GET /v1/models`

## Monitoring

### Health Endpoints

```bash
# Liveness check
curl http://bears-litellm:4000/health/liveliness

# Readiness check
curl http://bears-litellm:4000/health/readiness
```

### Usage Statistics

```bash
# View metrics (if enabled)
curl http://bears-litellm:4000/metrics
```

### Logs

View in Coolify dashboard:

```bash
# Look for:
# - Model initialization
# - API requests/responses
# - Error messages
```

## Troubleshooting

### Service Won't Start

**Solutions**:
- Verify Dockerfile builds successfully
- Check config file is mounted at `/app/config.yaml`
- Ensure environment variables are set
- Review build logs in Coolify

### Invalid API Key Errors

**Solutions**:
- Verify `OPENAI_API_KEY` starts with `sk-`
- Check `ANTHROPIC_API_KEY` starts with `sk-ant-`
- Test keys work: `curl https://api.openai.com/v1/models -H "Authorization: Bearer $OPENAI_API_KEY"`
- Ensure API keys have sufficient credits

### Connection Refused from Letta

**Solutions**:
- Verify `bears-litellm` service is healthy
- Check port 4000 is exposed internally
- Test: `curl http://bears-litellm:4000/health/liveliness`
- Ensure both services in same Coolify network

### Rate Limiting

**Solutions**:
- Check LLM provider quotas
- Add fallback models in config
- Implement request queuing
- Upgrade API plan with provider

## Advanced Configuration

### Load Balancing

Route to multiple API keys for same model:

```yaml
model_routing:
  openai/gpt-4:
    - model_name: gpt-4
      provider: openai
      api_key: sk-key-1...
    - model_name: gpt-4
      provider: openai
      api_key: sk-key-2...
```

### Fallback Models

Automatically fallback if primary fails:

```yaml
model_routing:
  gpt-4:
    - model_name: gpt-4
      provider: openai
    - model_name: claude-3-5-sonnet-20241022
      provider: anthropic  # Falls back to Claude if GPT-4 fails
```

### Caching

Enable caching for repeated requests:

```yaml
general_settings:
  cache: true
  cache_type: redis
  redis_host: bears-redis
  redis_port: 6379
```

## Security

### Master Key

Generate a secure master key:

```bash
openssl rand -hex 32
# Use output for LITELLM_MASTER_KEY
```

### API Key Rotation

When rotating provider API keys:

1. Update environment variable in Coolify
2. Restart LiteLLM service
3. Test with new key

### Network Security

- ✅ Keep internal unless needed externally
- ✅ Use Coolify proxy for HTTPS if exposed
- ✅ Require master key authentication
- ❌ Don't expose raw provider API keys

## Next Steps

After LiteLLM is running:

1. ✅ Verify health: `curl http://bears-litellm:4000/health/liveliness`
2. ✅ List models: `curl http://bears-litellm:4000/v1/models`
3. ✅ Test completion with master key
4. ➡️ Deploy **Letta** (final service)

## Coolify Service Name Reference

When deploying Letta:

```bash
LLM_API_URL=http://bears-litellm:4000/v1
```
