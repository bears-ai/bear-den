#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "Running smoke tests..."

RUNNER_SERVICE="bears-memfs-manager"
RUNNER_DIR="/tmp/bears-smoke"

docker compose exec -T "$RUNNER_SERVICE" sh -lc "rm -rf '$RUNNER_DIR' && mkdir -p '$RUNNER_DIR/tests'"
docker compose cp tests/smoke "$RUNNER_SERVICE:$RUNNER_DIR/tests"
API_URL=""
if docker compose exec -T bears-den sh -lc 'case "${RUN_API:-false}" in true|1|yes|on) exit 0 ;; *) exit 1 ;; esac' >/dev/null 2>&1; then
    API_URL="BEARS_API_URL=http://bears-den:3001"
fi

docker compose exec -T "$RUNNER_SERVICE" sh -lc "python -m pip install --quiet pytest requests && cd '$RUNNER_DIR' && BEARS_DEN_URL=http://bears-den:3000 BEARS_CODEPOOL_URL=http://bears-codepool:3030 BEARS_MEMFS_MANAGER_URL=http://bears-memfs-manager:8285 $API_URL python -m pytest tests/smoke/ -v"
