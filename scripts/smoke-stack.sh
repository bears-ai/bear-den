#!/bin/bash
set -euo pipefail

ROOT="/workspace"
INFRA_ONLY=0

usage() {
  cat <<'EOF'
Usage: ./scripts/smoke-stack.sh [--infra]

Build and run the local smoke stack, seed it, and run smoke tests.

  --infra  Start and verify bundled Postgres only. This skips image builds,
           application startup, seeding, and smoke tests; use it before
           `cargo sqlx prepare`.
EOF
}

case "${1:-}" in
  "") ;;
  --infra) INFRA_ONLY=1 ;;
  -h|--help) usage; exit 0 ;;
  *)
    usage >&2
    exit 2
    ;;
esac

# shellcheck source=/workspace/scripts/load-env.sh
. "${ROOT}/scripts/load-env.sh"

export JWT_SECRET="${JWT_SECRET:-dev-placeholder}"
export OPENAI_API_KEY="${OPENAI_API_KEY:-dev-placeholder}"
export WEB_SERVER_URL="${WEB_SERVER_URL:-http://localhost:3000}"
export SESSION_COOKIE_SECURE="${SESSION_COOKIE_SECURE:-false}"
export DATABASE_URL="${DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}"
export BIFROST_BASE_URL="${BIFROST_BASE_URL:-http://bears-bifrost:8080}"
export LLM_API_URL="${LLM_API_URL:-http://bears-bifrost:8080/v1}"
export AGENT_RUNTIME="${AGENT_RUNTIME:-native}"

export BIFROST_IMAGE="${BIFROST_IMAGE:-bears-bifrost-dev:latest}"
export DEN_IMAGE="${DEN_IMAGE:-bears-den-dev:latest}"

# Opt-in derived-recall (Qdrant) service: SMOKE_RECALL=1 starts bears-qdrant, but
# the smoke suite does not exercise retrieval. Use the opt-in den-service live-Qdrant test.
export SMOKE_RECALL="${SMOKE_RECALL:-0}"

recall_enabled() {
  case "${SMOKE_RECALL:-0}" in 1 | true | yes | on) return 0 ;; *) return 1 ;; esac
}

if [ "${AGENT_RUNTIME}" != "native" ]; then
  printf 'smoke-stack.sh requires AGENT_RUNTIME=native\n' >&2
  exit 1
fi

export COMPOSE_ENV_FILES=(
  "JWT_SECRET=${JWT_SECRET}"
  "OPENAI_API_KEY=${OPENAI_API_KEY}"
  "WEB_SERVER_URL=${WEB_SERVER_URL}"
  "SESSION_COOKIE_SECURE=${SESSION_COOKIE_SECURE}"
  "DATABASE_URL=${DATABASE_URL}"
  "BIFROST_BASE_URL=${BIFROST_BASE_URL}"
  "LLM_API_URL=${LLM_API_URL}"
  "AGENT_RUNTIME=${AGENT_RUNTIME}"
  "BIFROST_IMAGE=${BIFROST_IMAGE}"
  "DEN_IMAGE=${DEN_IMAGE}"
)

COMPOSE_PROFILE_ARGS=(--profile bundled)

if recall_enabled; then
  export QDRANT_URL="${QDRANT_URL:-http://bears-qdrant:6333}"
  COMPOSE_ENV_FILES+=("QDRANT_URL=${QDRANT_URL}")
  COMPOSE_PROFILE_ARGS+=(--profile recall)
fi

compose_with_env() {
  env -i PATH="$PATH" HOME="$HOME" DOCKER_CONFIG="${DOCKER_CONFIG:-$HOME/.docker}" \
    "${COMPOSE_ENV_FILES[@]}" docker compose -f "${ROOT}/docker-compose.yaml" -f "${ROOT}/docker-compose.dev.yaml" "${COMPOSE_PROFILE_ARGS[@]}" "$@"
}

wait_postgres_service() {
  service="$1"
  user="$2"
  db="$3"
  for _ in $(seq 1 30); do
    if compose_with_env exec -T "${service}" pg_isready -U "${user}" -d "${db}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

attach_caller_to_postgres_network() {
  # `bears-postgres` is a Compose service alias, not a Docker container name.
  # Derive its actual project network rather than assuming a project prefix.
  local postgres_id network caller_id
  postgres_id="$(compose_with_env ps -q bears-postgres)"
  network="$(docker inspect -f '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{end}}' "${postgres_id}")"
  caller_id="$(hostname)"

  if docker inspect "${caller_id}" --format '{{json .NetworkSettings.Networks}}' | grep -Fq "\"${network}\""; then
    return 0
  fi

  echo "Attaching this workspace container to ${network} so bears-postgres resolves..."
  docker network connect "${network}" "${caller_id}"
}

wait_compose_service_ready() {
  service="$1"
  for _ in $(seq 1 60); do
    container_id="$(compose_with_env ps -q "${service}" 2>/dev/null || true)"
    if [ -n "${container_id}" ]; then
      status="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "${container_id}" 2>/dev/null || true)"
      case "${status}" in
        healthy|running)
          return 0
          ;;
      esac
    fi
    sleep 2
  done
  printf '%s did not become ready in time\n' "${service}" >&2
  compose_with_env ps "${service}" >&2 || true
  return 1
}

smoke_bear_initialized() {
  local bear_id memory_dir
  bear_id="$(compose_with_env exec -T bears-postgres psql -U bears -d den -tAc "
    SELECT id
    FROM bears
    WHERE slug = 'test-bear'
      AND runtime_plan IS NOT NULL;
  " 2>/dev/null || true)"
  [ -n "${bear_id}" ] || return 1
  # seed-dev.sh initializes the memory store in this workspace, using this same path.
  memory_dir="${BEAR_SQLITE_DATA_DIR:-${ROOT}/services/den/data/bear-sqlite}"
  [ -s "${memory_dir}/${bear_id}.sqlite" ]
}

apply_smoke_seed_until_initialized() {
  for attempt in $(seq 1 5); do
    echo "Applying smoke seed profile (attempt ${attempt})..."
    "${ROOT}/scripts/seed-dev.sh" smoke
    if smoke_bear_initialized; then
      return 0
    fi
    sleep 2
  done

  printf 'smoke seed did not initialize test-bear runtime plan and memory store\n' >&2
  compose_with_env exec -T bears-postgres psql -U bears -d den -c "
    SELECT id, slug, runtime_plan
    FROM bears
    WHERE slug = 'test-bear';
  " >&2 || true
  return 1
}

echo "Starting bundled Postgres..."
compose_with_env up -d bears-postgres
wait_postgres_service bears-postgres bears den

if [ "${INFRA_ONLY}" -eq 1 ]; then
  attach_caller_to_postgres_network
  echo "Bundled Postgres is ready."
  echo "SQLx connection: postgres://bears:bears@bears-postgres:5432/den?sslmode=disable"
  exit 0
fi

echo "Building local Bifrost image (${BIFROST_IMAGE})..."
docker build -t "${BIFROST_IMAGE}" "${ROOT}/services/bifrost"

echo "Building local Den image (${DEN_IMAGE})..."
docker build --build-arg SQLX_OFFLINE=true -t "${DEN_IMAGE}" "${ROOT}/services/den"

if recall_enabled; then
  echo "Starting Qdrant (recall profile)..."
  compose_with_env up -d bears-qdrant
  wait_compose_service_ready bears-qdrant
fi

echo "Starting native BEARS stack (bifrost + den)..."
compose_with_env up -d --force-recreate bears-bifrost bears-den
wait_compose_service_ready bears-bifrost
wait_compose_service_ready bears-den

apply_smoke_seed_until_initialized

"${ROOT}/scripts/smoke.sh"
