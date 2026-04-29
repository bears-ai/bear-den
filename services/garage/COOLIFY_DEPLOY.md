# Garage — Coolify deployment guide

**Stack order:** Deploy **before Den** (Den needs S3 credentials). Independent of Bifrost and Letta.

## Overview

[Garage](https://garagehq.deuxfleurs.fr/) is the BEARS **object store**: S3-compatible, self-hosted, written in Rust, maintained by the [Deuxfleurs](https://deuxfleurs.fr/) non-profit. **Den** uses it for **artifacts** (agent outputs, user uploads, routine files — **not** stored in Letta) and, in a **separate bucket**, **Cabinet / Outline** attachments. See [artifacts-garage.md](../../docs/architecture/adr/artifacts-garage.md).

Garage is lightweight (≈1 GB RAM), includes built-in deduplication and compression, and is designed for small-to-medium self-hosted deployments.

Den talks to Garage via the standard S3 API (presigned URLs for upload/download). End users never hit Garage directly in production — Den issues short-lived presigned URLs and the browser uses those.

## Prerequisites

- Coolify v4+
- Persistent volumes for metadata and data

---

## Option A (recommended): Docker Compose from Git

### 1. Create the resource

1. Open your Coolify **project** → **Add New Resource**.
2. Choose **Public Repository** or **Private Repository** and select this monorepo.
3. Build pack: **Docker Compose**.

### 2. Point Coolify at the compose file

| Field                       | Value                 |
| --------------------------- | --------------------- |
| **Branch**                  | `main`                |
| **Base Directory**          | `services/garage`     |
| **Docker Compose Location** | `docker-compose.yaml` |

Enable **Preserve Repository During Deployment** so `garage.toml` is available via bind mount.

### 3. Generate secrets for `garage.toml`

Before deploying, edit `garage.toml` (or override via Coolify file storage) with real secrets:

```bash
# RPC secret (hex, 32 bytes):
openssl rand -hex 32

# Admin / metrics tokens (base64):
openssl rand -base64 32
```

Replace the `REPLACE_ME_*` placeholder values in [`garage.toml`](garage.toml).

### 4. Ports

| Port | Purpose                                  |
| ---- | ---------------------------------------- |
| 3900 | S3 API (internal to stack)               |
| 3902 | Web / static site hosting (optional)     |
| 3903 | Admin API (metrics, management)          |

For production: keep **3900 internal-only** (Den reaches it on the Docker network). Expose **3903** behind Coolify proxy with HTTPS if you want Prometheus scraping — or leave it internal.

### 5. Persistent storage

The compose file defines two named volumes:

| Volume         | Mount path               | Purpose                |
| -------------- | ------------------------ | ---------------------- |
| `garage-meta`  | `/var/lib/garage/meta`   | Metadata (LMDB)        |
| `garage-data`  | `/var/lib/garage/data`   | Object data            |

**Back up both volumes** — they hold all uploaded media. Metadata is small but critical; data is the bulk storage.

### 6. Deploy

**Deploy** / **Redeploy**. On success, services on the same Coolify network resolve `http://bears-garage:3900` for the S3 API.

---

## Option B: Docker Image (no Git)

1. **Add New Resource** → **Docker Image**.
2. Image: `dxflrs/garage:v2.2.0`
3. Provide `garage.toml` via Coolify **Storages** at `/etc/garage.toml`.
4. Volumes: bind or named at `/var/lib/garage/meta` and `/var/lib/garage/data`.
5. Ports: `3900` (S3), `3903` (admin).
6. Health check: `garage stats -a` (or `curl -sf http://localhost:3903/health`).

---

## Post-deploy: cluster layout, buckets, and keys

After first deploy, set up the cluster layout and create **two buckets** (artifacts vs Cabinet). From the Garage container (Coolify **Terminal** or `docker exec`):

```bash
# 1. Check node status and get the node ID
garage status

# 2. Assign a layout (single-node; use the node ID prefix from step 1)
garage layout assign -z dc1 -c 10G <node_id_prefix>
garage layout apply --version 1

# 3. Artifacts bucket — ephemeral agent outputs, human uploads, routine files (GC by Den policy)
garage bucket create bears-artifacts

# 4. Cabinet bucket — Outline / Cabinet attachments only (no Den artifact GC; lifecycle per Outline)
garage bucket create bears-cabinet

# 5. Create a service key for Den (artifacts + presigned URLs for uploads)
garage key create den-service-key

# 6. Grant Den read+write on the artifacts bucket
garage bucket allow --read --write bears-artifacts --key den-service-key

# 7. Optional Phase 2+: grant Den or Outline writer on bears-cabinet per your Cabinet adapter
# garage bucket allow --read --write bears-cabinet --key den-service-key

# 8. Show the key credentials (copy Key ID and Secret key for Den config)
garage key info den-service-key
```

The output of step 8 gives you the **Key ID** and **Secret key** to use as Den's `S3_ACCESS_KEY_ID` and `S3_SECRET_ACCESS_KEY`.

**Migration:** Older docs used **`bears-media`** as a single bucket name. New deployments should use **`bears-artifacts`** for the artifact model; rename or `aws s3 sync` between buckets if you already have data in `bears-media`.

---

## Den configuration

Set these on the **Den** service (see [`../../services/den/COOLIFY_DEPLOY.md`](../../services/den/COOLIFY_DEPLOY.md)):

```bash
S3_ENDPOINT=http://bears-garage:3900
# Ephemeral artifacts + uploads (see ../../docs/architecture/adr/artifacts-garage.md). Den currently uses S3_BUCKET.
S3_BUCKET=bears-artifacts
# Phase 2+ Cabinet / Outline: separate bucket in Garage; wire when Den Cabinet adapter needs direct S3
# S3_BUCKET_CABINET=bears-cabinet
S3_REGION=garage
S3_ACCESS_KEY_ID=<Key ID from garage key info>
S3_SECRET_ACCESS_KEY=<Secret key from garage key info>
# Public URL that browsers use to fetch objects via presigned URLs.
# In production this is the externally-reachable Garage S3 URL (via Coolify proxy / CDN).
S3_PUBLIC_URL=https://media.bears.artificial.design
# Required for Garage (and most self-hosted S3):
S3_FORCE_PATH_STYLE=true
```

## Backup

**Garage volumes are part of the three-input contract** (repo + database backups + object storage — see [`../../AGENTS.md`](../../AGENTS.md)). Back them up with your existing volume backup strategy, or use `rclone sync` / `aws s3 sync` to mirror to an off-site bucket.

## Verify

From a container on the same Docker network:

```bash
# Admin health
curl -sf http://bears-garage:3903/health && echo OK

# S3 endpoint (expects 403 without auth — that's fine, proves the port is up)
curl -sI http://bears-garage:3900
```

Or with `awscli`:

```bash
export AWS_ACCESS_KEY_ID=<key-id>
export AWS_SECRET_ACCESS_KEY=<secret>
export AWS_DEFAULT_REGION=garage
export AWS_ENDPOINT_URL=http://bears-garage:3900

aws s3 ls s3://bears-artifacts/
aws s3 ls s3://bears-cabinet/
```

## Troubleshooting

| Symptom                              | What to check                                                                 |
| ------------------------------------ | ----------------------------------------------------------------------------- |
| Container exits immediately          | Logs — usually `garage.toml` not found or invalid `rpc_secret` format         |
| `garage status` shows no nodes       | Layout not applied — run `garage layout assign` + `garage layout apply`       |
| Den can't connect                    | Same Docker network; `S3_ENDPOINT` uses the compose service name              |
| Presigned URLs 403 from browser      | `S3_PUBLIC_URL` must be the externally-reachable URL, not the internal one     |
| Bucket operations fail               | Key must have `--read --write` on the bucket; check with `garage bucket info` |

## Reference

- Compose stack: [`docker-compose.yaml`](docker-compose.yaml)
- Garage config: [`garage.toml`](garage.toml)
- Secrets checklist: [`.env.example`](.env.example)
- Garage docs: [garagehq.deuxfleurs.fr](https://garagehq.deuxfleurs.fr/documentation/)
- S3 compatibility: [Garage S3 compatibility table](https://garagehq.deuxfleurs.fr/documentation/reference-manual/s3-compatibility/)
- Den S3 config: [`../../services/den/.env.example`](../../services/den/.env.example)
