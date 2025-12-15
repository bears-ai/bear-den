# Redis - Coolify Deployment Guide

## Overview

Redis serves as the cache layer for the memory service/knowledgebase, providing fast temporary storage for session data and query results.

## Prerequisites

- Coolify instance running
- Deploy **before** your memory service / knowledgebase (if applicable)

## Deployment Steps

### 1. Deploy in Coolify

1. **Add New Resource** → **Docker Image**

2. **Basic Configuration**:
   - **Service Name**: `bears-redis`
   - **Image**: `redis:7-alpine`
   - **Deployment Type**: Public Docker Image

3. **Port Configuration**:
   - **Internal Port**: `6379`
   - **External Port**: None (internal only)
   - **Note**: Redis doesn't need external access in Coolify

4. **Environment Variables**: None required

5. **Persistent Storage**:

   Create a named volume for Redis data:

   - **Volume Name**: `bears-redis-data`
   - **Mount Path**: `/data`

6. **Health Check**:

   Configure in Coolify:
   ```bash
   Command: redis-cli ping | grep PONG
   Interval: 10s
   Timeout: 5s
   Retries: 5
   Start Period: 10s
   ```

7. **Restart Policy**: `unless-stopped`

8. **Deploy** the service

### 2. Verify Deployment

Check health in Coolify dashboard - should show **healthy** within 10-20 seconds.

Test connectivity:

```bash
# In Coolify terminal for redis service
redis-cli ping
# Should return: PONG
```

## Configuration Reference

### Service Details

| Setting | Value |
|---------|-------|
| **Image** | `redis:7-alpine` |
| **Internal Port** | 6379 |
| **Data Directory** | `/data` |
| **Persistence** | AOF (Append-Only File) enabled by default |

### Environment Variables

Redis 7 works out-of-the-box with sensible defaults. No environment variables needed for basic deployment.

### Volume Configuration

```
Volume Name: bears-redis-data
Mount Path: /data
Purpose: Redis persistence (AOF and RDB files)
```

## Service Connectivity

### Coolify Internal URL

The memory service / knowledgebase will connect to Redis using Coolify's internal Docker networking:

```bash
# Format: <service-name>.<coolify-project>.internal
redis://bears-redis:6379
```

**Important**: Use the service name you configured in Coolify. If you named it differently, update your memory service configuration accordingly.

## Performance Tuning (Optional)

For production deployments, you can optimize Redis:

### Custom Redis Configuration

Create a custom `redis.conf`:

```conf
# Memory
maxmemory 256mb
maxmemory-policy allkeys-lru

# Persistence
save 900 1
save 300 10
save 60 10000
appendonly yes
appendfsync everysec

# Performance
tcp-backlog 511
timeout 300
tcp-keepalive 300
```

Mount as volume in Coolify:
```
Source: /path/to/redis.conf
Target: /usr/local/etc/redis/redis.conf
```

Add command override:
```bash
redis-server /usr/local/etc/redis/redis.conf
```

### Resource Limits

In Coolify, set resource limits:
- **Memory Limit**: 512 MB (adjust based on needs)
- **Memory Reservation**: 256 MB
- **CPU Limit**: 0.5 cores

## Monitoring

### Check Memory Usage

```bash
# In Coolify terminal
redis-cli INFO memory
```

### Check Connected Clients

```bash
# In Coolify terminal
redis-cli INFO clients
```

### Check Stats

```bash
# In Coolify terminal
redis-cli INFO stats
```

## Troubleshooting

### Service Won't Start

**Problem**: Container exits immediately

**Solutions**:
- Check Coolify logs for errors
- Verify volume is mounted correctly
- Ensure no port conflicts (if exposing externally)
- Check resource availability

### Connection Refused from the memory service

**Problem**: The memory service can't connect to Redis

**Solutions**:
- Verify both services are in same Coolify project
- Check service name matches your memory service configuration
- Confirm Redis is healthy in Coolify
- Test connection: `redis-cli -h bears-redis ping`

### Out of Memory

**Problem**: Redis evicting keys or refusing writes

**Solutions**:
- Increase memory limit in Coolify
- Set `maxmemory-policy` to appropriate strategy
- Review memory service cache usage patterns
- Consider scaling Redis (vertical or horizontal)

### Data Loss After Restart

**Problem**: Cache data doesn't persist

**Solutions**:
- Verify volume is configured and mounted
- Check `/data` directory contains `.rdb` or `.aof` files
- Enable persistence: `CONFIG SET save "900 1 300 10"`
- Restart Redis and check logs

## Security Considerations

### Authentication (Optional)

For added security, enable Redis authentication:

1. Add environment variable in Coolify:
   ```bash
   REDIS_PASSWORD=your-secure-password-here
   ```

2. Update your memory service configuration to use the password:
   ```bash
   REDIS_PASSWORD=your-secure-password-here
   ```

### Network Isolation

Redis should **NOT** be exposed publicly:
- ✅ Keep internal only (no external port mapping)
- ✅ Use Coolify internal networking
- ❌ Don't expose port 6379 to internet

## Next Steps

After Redis is running:

1. ✅ Verify health check passes
2. ✅ Test connectivity: `redis-cli ping`
3. ➡️ Deploy **Qdrant** (vector database)
4. ➡️ Deploy your memory service / knowledgebase (depends on Redis + Qdrant + Git Sync)

## Coolify Service Name Reference

When deploying your memory service/adapter, you'll need to reference this Redis service:

```bash
# If you named the service "bears-redis"
REDIS_HOST=bears-redis
REDIS_PORT=6379

# If you named it something else, update accordingly
REDIS_HOST=<your-service-name>
```
