#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# shellcheck source=/workspace/scripts/load-env.sh
. "${ROOT}/scripts/load-env.sh"

echo "Running smoke tests..."

export JWT_SECRET="${JWT_SECRET:-dev-placeholder}"
export OPENAI_API_KEY="${OPENAI_API_KEY:-dev-placeholder}"
export AGENT_RUNTIME="${AGENT_RUNTIME:-native}"
export BEARS_LIVE_MODEL_SMOKE="${BEARS_LIVE_MODEL_SMOKE:-auto}"
# When set, the Qdrant recall assertion in tests/smoke is active.
export QDRANT_URL="${QDRANT_URL:-}"
export EMBEDDING_MODEL="${EMBEDDING_MODEL:-text-embedding-3-small}"
export EMBEDDING_DIMENSIONS="${EMBEDDING_DIMENSIONS:-1536}"

if [ "${AGENT_RUNTIME}" != "native" ]; then
  printf 'smoke.sh requires AGENT_RUNTIME=native; legacy service health checks are not in the default compose stack\n' >&2
  exit 1
fi

compose_with_env() {
  env -i PATH="$PATH" HOME="$HOME" DOCKER_CONFIG="${DOCKER_CONFIG:-$HOME/.docker}" \
    JWT_SECRET="${JWT_SECRET}" \
    OPENAI_API_KEY="${OPENAI_API_KEY}" \
    AGENT_RUNTIME="${AGENT_RUNTIME}" \
    WEB_SERVER_URL="${WEB_SERVER_URL:-http://localhost:3000}" \
    SESSION_COOKIE_SECURE="${SESSION_COOKIE_SECURE:-false}" \
    DATABASE_URL="${DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}" \
    BIFROST_BASE_URL="${BIFROST_BASE_URL:-http://bears-bifrost:8080}" \
    LLM_API_URL="${LLM_API_URL:-http://bears-bifrost:8080/v1}" \
    BIFROST_IMAGE="${BIFROST_IMAGE:-bears-bifrost-dev:latest}" \
    DEN_IMAGE="${DEN_IMAGE:-bears-den-dev:latest}" \
    docker compose -f "${ROOT}/docker-compose.yaml" -f "${ROOT}/docker-compose.dev.yaml" --profile bundled "$@"
}

API_URL=""
if compose_with_env exec -T bears-den sh -lc 'case "${RUN_API:-false}" in true|1|yes|on) exit 0 ;; *) exit 1 ;; esac' >/dev/null 2>&1; then
  API_URL="http://bears-den:3001"
fi

DEN_URL="http://bears-den:3000" \
BEARS_API_URL="${API_URL}" \
AGENT_RUNTIME="${AGENT_RUNTIME}" \
BEARS_LIVE_MODEL_SMOKE="${BEARS_LIVE_MODEL_SMOKE}" \
QDRANT_URL="${QDRANT_URL}" \
OPENAI_API_KEY="${OPENAI_API_KEY}" \
EMBEDDING_MODEL="${EMBEDDING_MODEL}" \
EMBEDDING_DIMENSIONS="${EMBEDDING_DIMENSIONS}" \
python3 -m pytest tests/smoke/ -v
