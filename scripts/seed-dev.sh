#!/bin/bash
set -euo pipefail

profile="${1:-smoke}"

case "${profile}" in
  smoke|minimal) ;;
  *)
    printf 'unknown seed profile %s; expected smoke or minimal\n' "${profile}" >&2
    exit 2
    ;;
esac

export JWT_SECRET="${JWT_SECRET:-dev-placeholder}"
export LETTA_SERVER_PASS="${LETTA_SERVER_PASS:-dev-placeholder}"
export OPENAI_API_KEY="${OPENAI_API_KEY:-dev-placeholder}"
export WEB_SERVER_URL="${WEB_SERVER_URL:-http://localhost:3000}"
database_url="${DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}"
export LETTA_PG_URI="${LETTA_PG_URI:-postgresql://bears:bears@bears-letta-postgres:5432/letta}"

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
