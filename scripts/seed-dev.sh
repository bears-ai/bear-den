#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=/workspace/scripts/load-env.sh
. "${ROOT}/scripts/load-env.sh"

profile="${1:-smoke}"

case "${profile}" in
  smoke|minimal) ;;
  *)
    printf 'unknown seed profile %s; expected smoke or minimal\n' "${profile}" >&2
    exit 2
    ;;
esac

export JWT_SECRET="${JWT_SECRET:-dev-placeholder}"
export OPENAI_API_KEY="${OPENAI_API_KEY:-dev-placeholder}"
export SQLX_OFFLINE="${SQLX_OFFLINE:-true}"
export WEB_SERVER_URL="${WEB_SERVER_URL:-http://localhost:3000}"
export BIFROST_BASE_URL="${BIFROST_BASE_URL:-http://bears-bifrost:8080}"
export BIFROST_ADMIN_USERNAME="${BIFROST_ADMIN_USERNAME:-admin}"
export AGENT_RUNTIME="${AGENT_RUNTIME:-native}"
export BEAR_SQLITE_DATA_DIR="${BEAR_SQLITE_DATA_DIR:-${ROOT}/services/den/data/bear-sqlite}"
database_url="${DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}"

if [[ "${database_url}" == *"@bears-postgres:"* ]] && ! getent hosts bears-postgres >/dev/null 2>&1; then
  postgres_container="$(docker compose --profile bundled ps -q bears-postgres)"
  if [ -z "${postgres_container}" ]; then
    printf 'bears-postgres container is not running; start the stack before seeding\n' >&2
    exit 1
  fi
  postgres_ip="$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "${postgres_container}")"
  if [ -z "${postgres_ip}" ]; then
    printf 'could not resolve bears-postgres container IP for seed command\n' >&2
    exit 1
  fi
  database_url="${database_url//@bears-postgres:/@${postgres_ip}:}"
fi

export DATABASE_URL="${database_url}"

cd /workspace/services/den
cargo run -- seed --profile "${profile}"
