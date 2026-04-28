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

export JWT_SECRET="${JWT_SECRET:-dev-placeholder}"
export LETTA_SERVER_PASS="${LETTA_SERVER_PASS:-dev-placeholder}"
export OPENAI_API_KEY="${OPENAI_API_KEY:-dev-placeholder}"
export WEB_SERVER_URL="${WEB_SERVER_URL:-http://localhost:3000}"
export DATABASE_URL="${DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}"
export LETTA_PG_URI="${LETTA_PG_URI:-postgresql://bears:bears@bears-postgres:5432/den?sslmode=disable}"

set_status "starting"
log "Starting BEARS devcontainer stack"

if ! run_logged docker compose --profile bundled up -d bears-postgres; then
  set_status "postgres_start_failed"
  log "Postgres startup failed; devcontainer remains usable. See ${LOG_FILE}."
  exit 0
fi

log "Waiting for bundled Postgres readiness"
postgres_ready=0
for _ in $(seq 1 30); do
  if docker compose --profile bundled exec -T bears-postgres pg_isready -U bears -d den >>"${LOG_FILE}" 2>&1; then
    postgres_ready=1
    break
  fi
  sleep 2
done

if [ "${postgres_ready}" != "1" ]; then
  set_status "postgres_unready"
  log "Postgres did not become ready before seeding; devcontainer remains usable. See ${LOG_FILE}."
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
if ! run_logged docker compose --profile bundled up -d --no-recreate bears-memfs-manager bears-den bears-codepool; then
  stack_ok=0
  log "Full stack startup failed; devcontainer remains usable. See ${LOG_FILE}."
else
  stack_ok=1
fi

if [ "${seed_ok}" = "1" ] && [ "${stack_ok}" = "1" ]; then
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
