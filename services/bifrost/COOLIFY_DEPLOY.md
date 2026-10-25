# Bifrost — Coolify deployment guide

**Stack order:** Deploy before Den when Den will use this Bifrost instance for model calls.

## Overview

[Bifrost](https://github.com/maximhq/bifrost) is the BEARS **model gateway**: OpenAI-compatible `/v1` API, multi-provider routing. Den calls Bifrost using `LLM_API_URL`.

This repository uses **file-based (GitOps) provider configuration**: `services/bifrost/config.json` is baked into the configured Bifrost image and placed at `/app/data/config.json` at startup. The SQLite config store remains enabled because Bifrost governance persists Den-provisioned Bear virtual keys there; the checked-in file remains the declared source for provider configuration.

Model availability starts with the **live provider catalog** and is filtered by the OpenAI provider key's explicit `models` allowlist in `services/bifrost/config.json`. Adding a model as a Bear default in Den does not grant it at the gateway: operators must also add it to this Bifrost allowlist and redeploy Bifrost. There is **no BEARS metadata sidecar** — Den reads the resulting availability and capability metadata from `/v1/models` directly.

The root compose stack also runs `bears-bifrost-valkey` (`valkey/valkey-bundle`) for Bifrost Redis/Valkey-compatible storage. Bifrost uses it as the `vector_store` for direct hash caching (`semantic_cache` with `dimension: 1`), and Den sends `x-bf-session-id`, `x-bf-cache-key`, and `x-bf-cache-type: direct` on LLM requests.

## Personal-context retention review

**2026-10-23, repository and pinned-upstream source review only; not live deployment verification or a provider-retention guarantee.** The [personal-memory plan](../../docs/roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#approved-v1-policy-amendment-2026-10-23) keeps remembered-name provider integration disabled. This review changes no config, environment, Compose, stored data or service. It does not prevent personal details supplied in ordinary conversation text from going through ordinary inference today.

The question is not whether the model can see a preferred name needed for a response; it must see the input used for inference. The question is **which additional copies are retained, who controls them, and what a later Den deletion can actually remove**. Hats are unrelated to these retention boundaries.

### Checked-in behavior and pinned defaults

| Layer | Evidence and consequence |
| --- | --- |
| Bifrost version/routing | [Dockerfile](Dockerfile) uses `maximhq/bifrost:v1.6.4`; [config](config.json) declares OpenAI as its provider. Root Compose permits an image override. This is not verification of a running image, route, provider account or agreement. |
| Gateway response cache | Config enables `semantic_cache` in direct-only mode (`dimension: 1`, no embedding provider), TTL **10 minutes**, namespace `BearsDirectCache`, `exclude_system_prompt: false`. Den sends conversation-qualified `x-bf-cache-key` and `x-bf-cache-type: direct` in [the LLM client](../den/crates/den-llm/src/client.rs). Inputs participate in hash matching, but cached responses/stream chunks can contain names. **A hashed cache key does not anonymize the stored answer.** |
| Gateway content logs | Config does not explicitly disable content logging or select a log-retention interval. Pinned Bifrost defaults enable logging and leave content logging enabled; an omitted `logs_store` uses stored configuration if present, otherwise SQLite `logs.db` under the config directory. No finite log lifetime is established by this repository. Stored/live settings and existing logs were **not inspected**. |
| Gateway disk/backups | [Root Compose](../../docker-compose.yaml) mounts named volumes for Bifrost `/app/data` and Valkey `/data`. Mounts make persistence possible, not proof of actual Valkey snapshots/AOF, existing retained content, or backup expiry. |
| Provider response storage | Den's `to_responses_body` does **not** set `store: false`. OpenAI documents default Responses application-state storage; setting `store: false` is separate from gateway logging/caching and provider abuse-monitoring retention. No account-level override is established here. |

Upstream evidence is pinned to the source associated with the v1.6.4 release, commit `c4cd51af26e0e870d4d16d006d1257c08822fd13`: [client/log-store defaults and configuration loading](https://github.com/maximhq/bifrost/blob/c4cd51af26e0e870d4d16d006d1257c08822fd13/transports/bifrost-http/lib/config.go), [response-cache storage](https://github.com/maximhq/bifrost/blob/c4cd51af26e0e870d4d16d006d1257c08822fd13/plugins/semanticcache/utils.go), [stream-cache storage](https://github.com/maximhq/bifrost/blob/c4cd51af26e0e870d4d16d006d1257c08822fd13/plugins/semanticcache/stream.go), and [conditional log cleanup](https://github.com/maximhq/bifrost/blob/c4cd51af26e0e870d4d16d006d1257c08822fd13/transports/bifrost-http/server/server.go).

**Implementation caution:** upstream [request cache controls](https://github.com/maximhq/bifrost/blob/c4cd51af26e0e870d4d16d006d1257c08822fd13/docs/features/semantic-caching.mdx) distinguish reads from writes: `x-bf-cache-no-store: true` skips new writes but can still serve existing hits. Simply setting that header, omitting it on fallback, or marking headers sensitive in Debug is not a tested read/write bypass. Any future policy must hold across every retry/fallback send and include existing-copy cleanup where promised.

### Provider controls are separate

Official OpenAI documentation, checked 2026-10-23, describes:

- **Training:** API content is not used to train/improve models unless the customer explicitly opts in. That does **not** mean no storage.
- **Abuse monitoring:** default logs can contain prompts/responses and are retained for up to **30 days**, with stated legal/safety exceptions. Approved **Modified Abuse Monitoring / Zero Data Retention** arrangements change this, subject to account, endpoint and feature limitations.
- **Responses application state:** default retention is **30 days** (the documentation says stored responses are kept for at least 30 days in the applicable cases). `store: false` addresses this application-state storage, not all other retention.
- **Provider prompt caching:** reuse of model-prefix computation/KV state, **not replay of a Bifrost cached answer**. Controls/retention differ by model and organization; current docs describe GPU-local cached state with a maximum 24-hour application-state retention limit and no manual cache-clear operation. Minimum cache lifetimes are not deletion deadlines. Verify the selected model/account rather than promise a universal “no cache” switch or that ZDR makes every feature storage-free.

Sources: [OpenAI data controls](https://developers.openai.com/api/docs/guides/your-data) and [prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching). These are dated public-policy descriptions, not verification of our account, contractual advice or guarantees for every future provider/model.

### Proportionate v1 baseline and reference options

After this review, the user clarified that we trust users not to divulge extremely sensitive information. The [memory plan's proportionality clarification](../../docs/roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#proportionate-protection-for-v1-2026-10-23) treats preferred-name v1 as ordinary non-secret personal context using the normal configured model/gateway boundary. Special zero-retention enrollment, blanket cache bypass or payload-log prohibition is **not a v1 prerequisite**. This is not a claim that providers retain nothing, approval of every possible future data category, or permission to change Compose/provider configuration.

The options below remain reference material for future operational hardening or stronger use cases, not mandatory work to unblock preferred-name v1. Ordinary Bear/user audience checks, correct cache association and the approved cleanup/non-resurrection obligations for in-scope Den-controlled copies still apply.

1. **Cloud inference with minimized extra copies:** for protected requests, propose explicit `store: false`, a verified gateway cache read/write bypass, and content-free gateway operational logs. Then explicitly accept or reject the selected provider's abuse-monitoring and model-prefix-cache retention. This simplifies local deletion but loses gateway replay savings and payload-level troubleshooting; it need not disable caching of unrelated requests or all provider prefix computation.
2. **Retain scoped gateway caching/logging:** possible only with demonstrated Bear/user audience isolation, bounded retention, dependency-aware invalidation/deletion and a defined backup policy. This preserves some performance/debugging benefits but creates additional body-bearing stores that Den must manage. It is not currently an implemented protected-data contract.
3. **Require stronger external retention guarantees:** select an eligible provider/account arrangement, or an appropriately operated local model route, whose actual terms meet the chosen requirements. Local deployment still needs its own logging/cache/backup policy and may involve capability, cost or throughput tradeoffs.

The review itself does not activate any option or change existing settings. Stronger retention arrangements can be evaluated separately when needed; they are not another approval gate for ordinary preferred-name processing. Preserve truthful deployment disclosures and test request/cache association and in-scope deletion behavior. Den deletion must remove the in-scope Den-controlled bodies/copies and prevent reuse, but cannot retroactively recall already-transmitted input or independently downloaded backups. Personal-memory integration remains disabled because the functional identity/audience and removal safeguards are not yet wired, not because a special provider-retention decision is outstanding.

## Prerequisites

- Coolify v4+
- Provider API keys for every `env.*` reference in `services/bifrost/config.json` (the default file requires `**OPENAI_API_KEY`**)
- `bears-bifrost-valkey` reachable on the compose network at `bears-bifrost-valkey:6379` for direct cache storage and session stickiness support
- A **GitHub** (or other Git) remote for this repo—only required for the **Git + Docker Compose** path below

---

## Option A (recommended): `config.json` from Git — Docker Compose build pack

Coolify can **clone your repository on each deploy** and bind-mount files from that checkout into the container. Then `config.json` is always whatever is on the branch you deploy—no pasting JSON into the Coolify UI.

Official pattern: [Docker Compose build pack](https://coolify.io/docs/builds/packs/docker-compose) with a compose file in this repo (`[docker-compose.yaml](docker-compose.yaml)`).

### 1. Create the resource

1. Open your Coolify **project** → **Add New Resource**.
2. Choose **Public Repository** or **Private Repository** (GitHub App / deploy key) and select **this** monorepo.
3. When asked for the **build pack**, pick **Docker Compose** (not Nixpacks, not “Docker Image” alone).

### 2. Point Coolify at the compose file

In the Docker Compose build-pack settings:


| Field                       | Value                                |
| --------------------------- | ------------------------------------ |
| **Branch**                  | e.g. `main` (or your release branch) |
| **Base Directory**          | `services/bifrost`                   |
| **Docker Compose Location** | `docker-compose.yaml`                |


Coolify combines **Base Directory** + **Docker Compose Location** to find the file.

### 3. Preserve the Git checkout (required for the bind mount)

Enable **Preserve Repository During Deployment** (wording may vary slightly by Coolify version).

Without this, bind mounts like `./config.json:/app/data/config.json` often fail because the compose working tree is not kept on the server. This is the usual fix when mounting **files from the repo** into containers on Coolify; see community notes in the [Coolify Docker Compose + Git guide](https://dev.to/mandrasch/simple-coolify-example-with-docker-compose-github-deployments-53m).

### 4. Pin the image in Git (recommended)

Edit `[docker-compose.yaml](docker-compose.yaml)` and replace `maximhq/bifrost:latest` with a **pinned tag** (e.g. `maximhq/bifrost:v1.4.9`). Check [Docker Hub tags](https://hub.docker.com/r/maximhq/bifrost/tags). Redeploy after edits.

### 5. Environment variables (still in Coolify — secrets only)

`config.json` references keys via `env.*` (for example `env.OPENAI_API_KEY`). **Secrets stay in Coolify**, not in Git:

1. Open the resource → **Environment Variables** (or **Production Variables**).
2. Add at least `**OPENAI_API_KEY`** (and any other provider keys your `config.json` references).
3. Optional: override defaults with `**APP_PORT**`, `**LOG_LEVEL**`, `**LOG_STYLE**` if you do not want the values from `docker-compose.yaml`.

### 6. Ports, health check, restart

- **Ports:** `docker-compose.yaml` maps **`8080:8080`**. Adjust in the compose file if Coolify should publish a different host port, or remove the `ports:` section if you only need internal access and will reach the service by stack DNS (advanced—see Coolify networking docs).
- **Health check:** `GET /health` — Bifrost may **503** if internal store pings fail. The repo’s **[`config.json`](config.json)** sets **`disable_db_pings_in_health`** so file-based GitOps stacks stay **200** without SQLite/log/vector probes. Compose uses **`CMD-SHELL`** + **`wget`** (same as the upstream image) with a **long `start_period`** for slow ARM hosts.
- **Restart:** `restart: unless-stopped` is already set on the service.

### 7. Deploy

**Deploy** / **Redeploy**. On success, other services on the **same Coolify network** should resolve `**http://bears-bifrost:${BIFROST_APP_PORT:-8080}`** by default. If multiple BEARS projects share a predefined Docker network, use distinct `BIFROST_APP_PORT` values per environment and let Den derive `BIFROST_ORIGIN` from that port.

### 8. Connecting Den across stacks

If Den is a **separate** Coolify resource, it must share a **Docker network** with Bifrost (Coolify “Connect to Predefined Network” / shared network; see [Coolify compose networking](https://coolify.io/docs/knowledge-base/docker/compose)). If both services live in the same compose stack, they already share a network.

---

## Option B: Public image only — manual `config.json` in Coolify

Use this when you **cannot** use the Git-backed compose flow (for example, no Git integration on that host). You pull `**maximhq/bifrost`** as a plain **Docker Image** resource and supply `config.json` via Coolify **Storages** (bind mount, pasted file, or host path)—as in the previous revision of this guide.

### 1. Create the service

1. **Add New Resource** → **Docker Image**.
2. **Save** to open the resource editor.

### 2. General / image


| Coolify field | Suggested value                                                                                                   |
| ------------- | ----------------------------------------------------------------------------------------------------------------- |
| **Name**      | `bears-bifrost`                                                                                                  |
| **Image**     | `maximhq/bifrost` — **pin a tag** in production; see [Docker Hub](https://hub.docker.com/r/maximhq/bifrost/tags). |


### 3. Ports

Expose **8080** internally (and publicly only if required). Match `**APP_PORT`** in environment.

### 4. Environment variables

Same secret set as **Option A §5** (`OPENAI_API_KEY`, …). See `[.env.example](.env.example)`.

### 5. Mount `config.json`

Under **Storages** / **Volumes**, mount **`config.json`** at **`/app/data/config.json`** (read-only): bind host path, Coolify file storage, or pasted content—**not** loaded automatically from Git in this mode.

### 6. Health check

The upstream Bifrost image includes BusyBox **`wget`** (not `curl`). Use the same probe as in [`docker-compose.yaml`](docker-compose.yaml):

```bash
wget --no-verbose --tries=1 -O /dev/null http://127.0.0.1:8080/health || exit 1
```

Use **`-O /dev/null`**, not GNU **`--spider`** — BusyBox `wget` in the image often does not support `--spider`, so the health check would fail every time.

- **Interval:** `30s` · **Timeout:** `10s` · **Retries:** `5` · **Start period:** `60s`

### 7. Restart policy

**`unless-stopped`**, then **Deploy**.

---

## Option C: Bake `config.json` into a tiny image (Git build, no bind mount)

If your Coolify plan prefers **one Dockerfile per service** instead of compose:

1. Add a `Dockerfile` next to `config.json` with:
  ```dockerfile
   FROM maximhq/bifrost:v1.4.9
   COPY config.json /app/data/config.json
  ```
2. In Coolify, create a **Dockerfile** deployment from Git: **Base Directory** `services/bifrost`, Dockerfile path `Dockerfile`.
3. Set provider secrets only in Coolify **Environment Variables**.

Every config change requires an **image rebuild**—good for immutability, less convenient for rapid `config.json` iteration than Option A.

---

## Verify (after deploy)

From a container on the **same Docker network** as `bears-bifrost` (or Coolify **Terminal** on that service):

```bash
curl -sS http://bears-bifrost:8080/health
curl -sS http://bears-bifrost:8080/v1/models
# Optional from the Valkey container:
valkey-cli ping
```

Optional smoke test:

```bash
curl -sS http://bears-bifrost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-bf-session-id: smoke-session" \
  -H "x-bf-cache-key: smoke-session" \
  -H "x-bf-cache-type: direct" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"ping"}]}'
```

## Den (next service)

Set `LLM_API_URL=http://bears-bifrost:8080/v1` on Den (adjust host if you renamed the compose service).

## Observability

- **Liveness:** `GET /health`
- **Prometheus / OTLP:** [Bifrost observability](https://docs.getbifrost.ai/features/observability/default)
- **Den:** optional `**BIFROST_BASE_URL`** — [PLAN.md](../../docs/planning/PLAN.md)

## Troubleshooting


| Symptom                                                   | What to check                                                                                                                                |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Compose deploy: “cannot mount … config.json” / empty file | **Preserve Repository During Deployment** enabled; **Base Directory** is `services/bifrost` so `./config.json` exists in the compose context (or use repo root + root `[docker-compose.yaml](../../docker-compose.yaml)` so `./services/bifrost/config.json` exists) |
| Logs: `read /app/data/config.json: is a directory` | Host path for the bind mount was missing, so Docker created a **directory** named `config.json`. Remove that bad path on the server, ensure the real file exists in the checkout, and match **Base Directory** to the compose path (see above). Repo compose sets **`create_host_path: false`** so this fails fast instead of creating a directory. |
| Container exits on start                                  | **Logs** — invalid JSON, missing `env.`* variables in Coolify                                                                                |
| **`bears-bifrost` unhealthy** (health check never passes) | **`GET /health`** returns **503** when config/log/vector store pings fail — set **`disable_db_pings_in_health`** in `client` (see repo `config.json`); ensure **`OPENAI_API_KEY`** is set if providers use `env.*`; allow a long **`start_period`** on ARM |
| Den cannot resolve `bears-bifrost`                     | Same **Docker network** (Option A §8), or use Coolify-generated hostname for the stack                                                       |


## Reference

- Compose stack for Coolify: `[docker-compose.yaml](docker-compose.yaml)`
- Example env keys: `[.env.example](.env.example)`
- Bifrost provider config: [Provider configuration](https://docs.getbifrost.ai/quickstart/gateway/provider-configuration)
