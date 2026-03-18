# Qdrant - Coolify Deployment Guide

> **Legacy.** Qdrant backs the **old Git+Qdrant knowledgebase**. **Cabinet (Outline)** is the target shared knowledgebase and does not require Qdrant in BEARS. See [PLAN.md](../../PLAN.md).

## Overview

For legacy deployments only: Qdrant stores embeddings for the standalone knowledgebase service.

## Prerequisites

- Coolify instance running  
- Deploy **before** the legacy memory/knowledgebase service (if used)

## Deployment Steps

### 1. Deploy in Coolify

1. **Add New Resource** → **Docker Image**

2. **Basic Configuration**:
   - **Service Name**: `bears-qdrant`
   - **Image**: `qdrant/qdrant:latest`
   - **Deployment Type**: Public Docker Image

3. **Port Configuration**:
   - **Internal Port**: `6333` (HTTP API)
   - **Internal Port**: `6334` (gRPC - optional)
   - **External Port**: `6333` (optional, for debugging only)
   - **Note**: External access should be restricted or disabled in production

4. **Environment Variables**: None required (defaults work well)

5. **Persistent Storage**:

   Create a named volume for vector data:

   - **Volume Name**: `bears-qdrant-data`
   - **Mount Path**: `/qdrant/storage`

6. **Health Check**:

   Configure in Coolify:
   ```bash
   Command: wget --no-verbose --tries=1 --spider http://localhost:6333/readyz || exit 1
   Interval: 30s
   Timeout: 10s
   Retries: 3
   Start Period: 60s
   ```

7. **Resource Limits** (Recommended):
   - **Memory Limit**: 2 GB (minimum for production)
   - **Memory Reservation**: 1 GB
   - **CPU Limit**: 2 cores

8. **Restart Policy**: `unless-stopped`

9. **Deploy** the service

### 2. Verify Deployment

Check health in Coolify dashboard - should show **healthy** within 60 seconds.

Test API:

```bash
# In Coolify terminal or via HTTP
curl http://bears-qdrant:6333/
# Should return Qdrant version info

curl http://bears-qdrant:6333/collections
# Should return empty collections list initially
```

## Configuration Reference

### Service Details

| Setting | Value |
|---------|-------|
| **Image** | `qdrant/qdrant:latest` |
| **HTTP API Port** | 6333 |
| **gRPC Port** | 6334 (optional) |
| **Data Directory** | `/qdrant/storage` |
| **Config Location** | `/qdrant/config/` |

### Environment Variables

Qdrant works with defaults, but you can customize:

```bash
# Optional: Performance tuning
QDRANT__SERVICE__GRPC_PORT=6334
QDRANT__SERVICE__HTTP_PORT=6333

# Optional: Memory optimization
QDRANT__STORAGE__OPTIMIZERS__MAX_SEGMENT_SIZE=200000
QDRANT__STORAGE__OPTIMIZERS__MEMMAP_THRESHOLD=50000

# Optional: Logging
QDRANT__LOG_LEVEL=INFO
```

### Volume Configuration

```
Volume Name: bears-qdrant-data
Mount Path: /qdrant/storage
Purpose: Vector indexes, collections, and snapshots
Size: Plan for ~2-5x size of your memory content
```

### Service Connectivity

### Coolify Internal URL

The memory service (knowledgebase/adapter) will connect to Qdrant using Coolify's internal Docker networking:

```bash
# HTTP API (recommended)
http://bears-qdrant:6333

# gRPC (optional, for high-performance scenarios)
grpc://bears-qdrant:6334
```

**Important**: Use the service name you configured in Coolify.

## Performance Tuning

### Memory Requirements

Qdrant loads indexes into memory for fast search:

**Minimum**: 1 GB RAM
**Recommended**: 2-4 GB RAM for typical BEARS deployment
**Large deployments**: Scale based on collection size

### Disk Space

Plan for:
- 2-5x the size of your raw memory content
- Vectors are typically 1536 dimensions (OpenAI embeddings) = ~6KB per vector
- Indexes and metadata add overhead

**Example**: 10,000 memories → ~60 MB vectors + indexes → ~150-300 MB total

### Optimization Settings

For large collections, create custom config file:

`qdrant-config.yaml`:
```yaml
storage:
  storage_path: /qdrant/storage
  optimizers:
    deleted_threshold: 0.2
    vacuum_min_vector_number: 1000
    max_segment_size: 200000
    memmap_threshold: 50000
  on_disk_payload: false

service:
  host: 0.0.0.0
  http_port: 6333
  grpc_port: 6334
```

Mount in Coolify:
```
Source: /path/to/qdrant-config.yaml
Target: /qdrant/config/config.yaml
```

## Collections Created by the memory service

Memory backends/adapters typically create collections such as:

1. **`chunks`** - Text chunks from documents
2. **`search`** - Optimized for search queries
3. **`messages`** - Chat history embeddings (if enabled)

These are created on first use by the memory service or adapter - no manual setup needed.

## Monitoring

### Check Collections

```bash
curl http://bears-qdrant:6333/collections
```

### Check Collection Info

```bash
curl http://bears-qdrant:6333/collections/{collection_name}
```

### Check Cluster Status

```bash
curl http://bears-qdrant:6333/cluster
```

### Metrics (Prometheus Format)

```bash
curl http://bears-qdrant:6333/metrics
```

## Backup and Restore

### Create Snapshot

```bash
curl -X POST http://bears-qdrant:6333/collections/{collection_name}/snapshots
```

### List Snapshots

```bash
curl http://bears-qdrant:6333/collections/{collection_name}/snapshots
```

### Download Snapshot

```bash
curl http://bears-qdrant:6333/collections/{collection_name}/snapshots/{snapshot_name} \
  --output snapshot.zip
```

### Restore from Snapshot

```bash
curl -X POST http://bears-qdrant:6333/collections/{collection_name}/snapshots/upload \
  -F 'snapshot=@snapshot.zip'
```

**Note**: Snapshots are also saved in the persistent volume under `/qdrant/storage/snapshots/`

## Troubleshooting

### Service Won't Start

**Problem**: Container exits or restarts continuously

**Solutions**:
- Check Coolify logs for errors
- Verify volume is mounted at `/qdrant/storage`
- Ensure sufficient memory (minimum 512 MB, recommended 1 GB+)
- Check disk space availability

### Connection Refused from the memory service

**Problem**: The memory service can't reach Qdrant

**Solutions**:
- Verify both services are in the same Coolify project
- Check service name matches the memory service config: `QDRANT_HOST=bears-qdrant`
- Confirm Qdrant health check passes
- Test: `curl http://bears-qdrant:6333/`

### Slow Search Performance

**Problem**: Vector searches taking >1 second

**Solutions**:
- Increase memory allocation (Qdrant needs RAM for indexes)
- Check collection settings: `curl http://bears-qdrant:6333/collections/{name}`
- Optimize index: Use HNSW with appropriate `m` and `ef_construct` values
- Consider enabling on-disk payload storage for large collections

### Out of Memory

**Problem**: Qdrant OOMKilled or crashes

**Solutions**:
- Increase memory limit in Coolify (2 GB+ for production)
- Enable on-disk payload: `QDRANT__STORAGE__ON_DISK_PAYLOAD=true`
- Reduce `memmap_threshold` to offload more data to disk
- Archive old collections

### Data Loss After Restart

**Problem**: Collections disappear after restart

**Solutions**:
- Verify persistent volume is configured
- Check `/qdrant/storage` contains collection data
- Ensure volume mount path is exactly `/qdrant/storage`
- Review Coolify volume configuration

## Security Considerations

### API Access Control

Qdrant has an open API by default. For production:

1. **Option A**: Keep internal only (recommended for Coolify)
   - Don't expose external port
   - Only accessible within Coolify network

2. **Option B**: Enable API key authentication
   ```bash
   QDRANT__SERVICE__API_KEY=your-secure-api-key-here
   ```

  Then configure your memory service / knowledgebase to use the same API key, for example:
  ```bash
  QDRANT_API_KEY=your-secure-api-key-here
  ```

3. **Option C**: Use Coolify network policies to restrict access

### Network Isolation

- ✅ Keep internal only (no public exposure)
- ✅ Use Coolify internal networking
- ❌ Don't expose port 6333 to internet
- ❌ Don't allow unauthenticated public access

## Advanced Features

### Distributed Deployment (Optional)

For high availability, Qdrant supports clustering:

1. Deploy multiple Qdrant instances
2. Configure cluster mode with environment variables
3. Use consistent snapshot backups

See Qdrant documentation for cluster setup.

### Quantization (Memory Optimization)

Enable scalar or binary quantization to reduce memory:

```bash
curl -X PATCH http://bears-qdrant:6333/collections/{name} \
  -H 'Content-Type: application/json' \
  -d '{
    "quantization_config": {
      "scalar": {
        "type": "int8",
        "quantile": 0.99,
        "always_ram": true
      }
    }
  }'
```

## Next Steps

After Qdrant is running:

1. ✅ Verify health check passes
2. ✅ Test API: `curl http://bears-qdrant:6333/`
3. ✅ Check collections: `curl http://bears-qdrant:6333/collections`
4. ➡️ Deploy your memory service / knowledgebase (depends on Qdrant + Redis + Git Sync + Postgres)

## Coolify Service Name Reference

When deploying your memory service/adapter, you'll need to reference this Qdrant service:

```bash
# If you named the service "bears-qdrant"
QDRANT_HOST=bears-qdrant
QDRANT_PORT=6333

# If you named it something else, update accordingly
QDRANT_HOST=<your-service-name>
```
