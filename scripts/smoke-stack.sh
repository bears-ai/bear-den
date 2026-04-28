#!/bin/bash
set -euo pipefail

ROOT="/workspace"

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
  for _ in $(seq 1 30); do
    if docker compose --profile bundled exec -T "${service}" pg_isready -U "${user}" -d "${db}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

echo "Building local Bifrost image (${BIFROST_IMAGE})..."
docker build -t "${BIFROST_IMAGE}" "${ROOT}/services/bifrost"

echo "Building local Den image (${DEN_IMAGE})..."
docker build --build-arg SQLX_OFFLINE=true -t "${DEN_IMAGE}" "${ROOT}/services/den"

echo "Building local Codepool image (${CODEPOOL_IMAGE})..."
docker build -t "${CODEPOOL_IMAGE}" "${ROOT}/services/codepool"

echo "Starting bundled Postgres services..."
docker compose --profile bundled up -d bears-postgres bears-letta-postgres
wait_postgres_service bears-postgres bears den
wait_postgres_service bears-letta-postgres bears letta

echo "Applying smoke seed profile..."
"${ROOT}/scripts/seed-dev.sh" smoke

echo "Starting source-aware BEARS stack..."
docker compose --profile bundled up -d --force-recreate bears-memfs-manager bears-den bears-codepool

"${ROOT}/scripts/smoke.sh"
