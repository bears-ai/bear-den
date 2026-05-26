#!/bin/bash
set -euo pipefail

ROOT="/workspace"

export JWT_SECRET="${JWT_SECRET:-dev-placeholder}"
export LETTA_SERVER_PASS="${LETTA_SERVER_PASS:-dev-placeholder}"
export LETTA_API_KEY="${LETTA_API_KEY:-${LETTA_SERVER_PASS}}"
export OPENAI_API_KEY="${OPENAI_API_KEY:-dev-placeholder}"
export WEB_SERVER_URL="${WEB_SERVER_URL:-http://localhost:3000}"
export SESSION_COOKIE_SECURE="${SESSION_COOKIE_SECURE:-false}"
export DATABASE_URL="${DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}"
export LETTA_PG_URI="${LETTA_PG_URI:-postgresql://bears:bears@bears-letta-postgres:5432/letta}"
export LETTA_BASE_URL="${LETTA_BASE_URL:-http://bears-letta:8283}"
export BIFROST_BASE_URL="${BIFROST_BASE_URL:-http://bears-bifrost:8080}"
export LETTA_MEMFS_SERVICE_URL="${LETTA_MEMFS_SERVICE_URL:-http://bears-memfs-manager:8285}"

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

wait_compose_service_ready() {
  service="$1"
  for _ in $(seq 1 60); do
    container_id="$(docker compose --profile bundled ps -q "${service}" 2>/dev/null || true)"
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
  docker compose --profile bundled ps "${service}" >&2 || true
  return 1
}

smoke_pair_binding_ready() {
  result="$(docker compose --profile bundled exec -T bears-postgres psql -U bears -d den -tAc "
    SELECT EXISTS (
      SELECT 1
      FROM bears b
      INNER JOIN bear_agents ba ON ba.bear_id = b.id
      WHERE b.slug = 'test-bear'
        AND ba.role = 'pair'
        AND btrim(COALESCE(ba.letta_agent_id, '')) <> ''
    );
  " 2>/dev/null || true)"
  [ "${result}" = "t" ]
}

apply_smoke_seed_until_pair_ready() {
  for attempt in $(seq 1 5); do
    echo "Applying smoke seed profile (attempt ${attempt})..."
    "${ROOT}/scripts/seed-dev.sh" smoke
    if smoke_pair_binding_ready; then
      return 0
    fi
    sleep 2
  done

  printf 'smoke seed did not provision the test-bear pair role binding\n' >&2
  docker compose --profile bundled exec -T bears-postgres psql -U bears -d den -c "
    SELECT b.slug, ba.role, ba.letta_agent_id, ba.provisioning_status, ba.last_provisioning_error
    FROM bears b
    LEFT JOIN bear_agents ba ON ba.bear_id = b.id
    WHERE b.slug = 'test-bear'
    ORDER BY ba.role;
  " >&2 || true
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

echo "Starting source-aware BEARS stack..."
docker compose --profile bundled up -d --force-recreate bears-memfs-manager bears-den bears-codepool
wait_compose_service_ready bears-memfs-manager
wait_compose_service_ready bears-bifrost
wait_compose_service_ready bears-letta
wait_compose_service_ready bears-codepool
wait_compose_service_ready bears-den

apply_smoke_seed_until_pair_ready

"${ROOT}/scripts/smoke.sh"
