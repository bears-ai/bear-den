#!/bin/bash
set -u -o pipefail

ROOT="/workspace"
LOG_DIR="${ROOT}/.devcontainer/logs"
LOG_FILE="${LOG_DIR}/startup.log"
STATUS_FILE="${LOG_DIR}/startup.status"

mkdir -p "${LOG_DIR}"
: >"${LOG_FILE}"

log() {
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" | tee -a "${LOG_FILE}"
}

set_status() {
  printf '%s\n' "$1" >"${STATUS_FILE}"
}

run_logged() {
  log "+ $*"
  "$@" >>"${LOG_FILE}" 2>&1
}

ensure_devcontainer_network() {
  local network="${BEARS_DEVCONTAINER_NETWORK:-bears-stack_default}"
  local container_id
  container_id="$(hostname)"

  if ! command -v docker >/dev/null 2>&1; then
    log "Docker CLI is unavailable; skipping devcontainer network check"
    return 0
  fi

  if ! docker info >/dev/null 2>&1; then
    log "Docker daemon is unavailable; skipping devcontainer network check"
    return 0
  fi

  if ! docker network inspect "${network}" >/dev/null 2>&1; then
    log "Docker network ${network} is not present yet; creating it so service DNS can be attached"
    if ! run_logged docker network create "${network}"; then
      log "Could not create Docker network ${network}; tests may not resolve bears-postgres"
      return 0
    fi
  fi

  if docker inspect "${container_id}" --format "{{json .NetworkSettings.Networks}}" 2>/dev/null | grep -q "\"${network}\""; then
    log "Devcontainer is already attached to ${network}"
    return 0
  fi

  log "Attaching devcontainer ${container_id} to Docker network ${network}"
  if run_logged docker network connect "${network}" "${container_id}"; then
    log "Devcontainer attached to ${network}; bears-postgres/bears-letta-postgres DNS is available"
  else
    log "Could not attach devcontainer to ${network}; tests may not resolve bears-postgres"
  fi
}

ensure_devcontainer_network

export JWT_SECRET="${JWT_SECRET:-dev-placeholder}"
export LETTA_SERVER_PASS="${LETTA_SERVER_PASS:-dev-placeholder}"
export OPENAI_API_KEY="${OPENAI_API_KEY:-dev-placeholder}"
export WEB_SERVER_URL="${WEB_SERVER_URL:-http://localhost:3000}"
export SESSION_COOKIE_SECURE="${SESSION_COOKIE_SECURE:-false}"
export DATABASE_URL="${DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}"
export LETTA_PG_URI="${LETTA_PG_URI:-postgresql://bears:bears@bears-letta-postgres:5432/letta}"
export BIFROST_IMAGE="${BIFROST_IMAGE:-bears-bifrost-dev:latest}"
export DEN_IMAGE="${DEN_IMAGE:-bears-den-dev:latest}"
export DEN_PULL_POLICY="${DEN_PULL_POLICY:-never}"
export CODEPOOL_IMAGE="${CODEPOOL_IMAGE:-bears-codepool-dev:latest}"
export CODEPOOL_PULL_POLICY="${CODEPOOL_PULL_POLICY:-never}"

wait_postgres_service() {
  service="$1"
  user="$2"
  db="$3"
  label="$4"
  log "Waiting for ${label} readiness"
  for _ in $(seq 1 30); do
    if docker compose --profile bundled exec -T "${service}" pg_isready -U "${user}" -d "${db}" >>"${LOG_FILE}" 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

set_status "starting"
log "Starting BEARS devcontainer stack"

build_ok=1
if ! run_logged docker build -t "${BIFROST_IMAGE}" "${ROOT}/services/bifrost"; then
  build_ok=0
  log "Bifrost image build failed"
fi
if ! run_logged docker build --build-arg SQLX_OFFLINE=true -t "${DEN_IMAGE}" "${ROOT}/services/den"; then
  build_ok=0
  log "Den image build failed"
fi
if ! run_logged docker build -t "${CODEPOOL_IMAGE}" "${ROOT}/services/codepool"; then
  build_ok=0
  log "Codepool image build failed"
fi

if ! run_logged docker compose --profile bundled up -d bears-postgres bears-letta-postgres; then
  set_status "postgres_start_failed"
  log "Bundled Postgres startup failed; devcontainer remains usable. See ${LOG_FILE}."
  exit 0
fi

postgres_ready=1
if ! wait_postgres_service bears-postgres bears den "Den Postgres"; then
  postgres_ready=0
fi

letta_postgres_ready=1
if ! wait_postgres_service bears-letta-postgres bears letta "Letta PGVector Postgres"; then
  letta_postgres_ready=0
fi

if [ "${postgres_ready}" != "1" ]; then
  set_status "postgres_unready"
  log "Den Postgres did not become ready before seeding; devcontainer remains usable. See ${LOG_FILE}."
  exit 0
fi

log "Running dev/smoke seed profile"
seed_ok=0
if ! run_logged "${ROOT}/scripts/seed-dev.sh" smoke; then
  seed_ok=0
  log "Seed failed; devcontainer remains usable. Rerun with: /workspace/scripts/seed-dev.sh smoke"
  log "Detailed seed output is in ${LOG_FILE}."
else
  seed_ok=1
fi

log "Starting remaining BEARS services"
stack_ok=0
if [ "${build_ok}" != "1" ]; then
  log "Skipping full stack startup because one or more local source image builds failed"
elif [ "${letta_postgres_ready}" != "1" ]; then
  log "Skipping full stack startup because Letta PGVector Postgres is not ready"
elif ! run_logged docker compose --profile bundled up -d --force-recreate bears-memfs-manager bears-den bears-codepool; then
  stack_ok=0
  log "Full stack startup failed; devcontainer remains usable. See ${LOG_FILE}."
else
  stack_ok=1
fi

if [ "${build_ok}" != "1" ]; then
  set_status "local_image_build_failed"
  log "Local Den/Codepool/Bifrost image build failed; full stack was not started"
elif [ "${seed_ok}" = "1" ] && [ "${stack_ok}" = "1" ]; then
  set_status "ok"
  log "Devcontainer stack started and seed profile applied successfully"
elif [ "${seed_ok}" = "1" ]; then
  set_status "stack_failed_after_seed"
  log "Seed profile applied, but full stack startup failed"
elif [ "${stack_ok}" = "1" ]; then
  set_status "seed_failed"
  log "Full stack started, but seed profile failed"
else
  set_status "seed_and_stack_failed"
  log "Both seed profile and full stack startup failed"
fi
exit 0
