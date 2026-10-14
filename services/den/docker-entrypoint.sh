#!/bin/sh
set -eu

# Cabinet shares Postgres only: do not touch native memory or sandbox volumes.
if [ "${1:-}" = "cabinet-serve" ]; then
    exec su-exec appuser /bin/server "$@"
fi

DATA_DIR="${BEAR_SQLITE_DATA_DIR:-/var/lib/den/bear-sqlite}"
mkdir -p "${DATA_DIR}"
chown -R appuser:appuser "${DATA_DIR}"

if [ "${RUN_SANDBOX:-false}" = "true" ]; then
    SANDBOX_DIR="${SANDBOX_WORKSPACES_DIR:-./data/sandbox-workspaces}"
    mkdir -p "${SANDBOX_DIR}"
    chown -R appuser:appuser "${SANDBOX_DIR}"
fi

exec su-exec appuser /bin/server "$@"
