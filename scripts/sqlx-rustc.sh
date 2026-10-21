#!/bin/bash
set -euo pipefail

# Cargo also invokes wrappers for compiler/version probes without a package name.
if [ "${CARGO_PKG_NAME:-}" = "den-memory" ]; then
  export DATABASE_URL="${DEN_SQLX_MEMORY_DATABASE_URL:?run preparation through scripts/sqlx.sh}"
fi

# Cargo already chains RUSTC_WRAPPER outside RUSTC_WORKSPACE_WRAPPER. Keep that
# chain intact, and delegate to an existing workspace wrapper if one was supplied.
if [ -n "${DEN_SQLX_PREVIOUS_WORKSPACE_WRAPPER:-}" ]; then
  exec "${DEN_SQLX_PREVIOUS_WORKSPACE_WRAPPER}" "$@"
fi
exec "$@"
