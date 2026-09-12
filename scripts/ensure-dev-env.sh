#!/bin/bash
set -euo pipefail

ROOT="${BEARS_ROOT:-/workspace}"
ENV_FILE="${ROOT}/.env"
EXAMPLE="${ROOT}/.env.example"

if [ -f "${ENV_FILE}" ]; then
  exit 0
fi

if [ ! -f "${EXAMPLE}" ]; then
  echo "ensure-dev-env: missing ${EXAMPLE}; cannot bootstrap .env" >&2
  exit 1
fi

cp "${EXAMPLE}" "${ENV_FILE}"

{
  echo ""
  echo "# --- devcontainer defaults (scripts/ensure-dev-env.sh) ---"
  echo "JWT_SECRET=dev-placeholder"
  echo "WEB_SERVER_URL=http://localhost:3000"
  echo "SESSION_COOKIE_SECURE=false"
  echo "DATABASE_URL=postgres://bears:bears@bears-postgres:5432/den?sslmode=disable"
  echo "LLM_API_URL=http://bears-bifrost:8080/v1"
  echo "AGENT_RUNTIME=native"
  echo "COMPOSE_PROFILES=bundled"
  echo "BIFROST_IMAGE=bears-bifrost-dev:latest"
  echo "DEN_IMAGE=bears-den-dev:latest"
  echo "RUN_API=true"
  echo "ACP_GATEWAY_ENABLED=true"
  echo "DEFAULT_LLM_MODEL=openai/gpt-5-mini"
  echo "BEAR_SQLITE_DATA_DIR=./data/bear-sqlite"
} >> "${ENV_FILE}"

echo "Created ${ENV_FILE} from .env.example with devcontainer defaults."
echo "Set OPENAI_API_KEY in ${ENV_FILE} before running native ACP smoke tests."
