#!/bin/bash
set -euo pipefail

ROOT="/workspace"
DATABASE_URL="${SQLX_DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}"
DEN="${ROOT}/services/den"

# Stale-entry pruning is legitimate but should be a deliberate, reviewed commit.
PRUNE_THRESHOLD="${PRUNE_THRESHOLD:-10}"
PREPARE_TMP=""
CACHE_BACKUP=""
ALLOW_PRUNE=0

restore_cache() {
  mkdir -p "${DEN}/.sqlx"
  cp -Rn "${CACHE_BACKUP}/." "${DEN}/.sqlx/"
}

cleanup() {
  local status=$?
  trap - EXIT
  if [ -n "${CACHE_BACKUP}" ] && { [ "${status}" -ne 0 ] || [ "${ALLOW_PRUNE}" -eq 0 ]; }; then
    restore_cache || status=1
  fi
  if [ -n "${PREPARE_TMP}" ]; then
    rm -rf "${PREPARE_TMP}"
  fi
  exit "${status}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

usage() {
  cat <<'EOF'
Usage: ./scripts/sqlx.sh <sqlx-command> [arguments...]
       ./scripts/sqlx.sh prepare-all [--allow-prune]
       ./scripts/sqlx.sh prepare-memory

Start bundled Postgres if needed, then run `cargo sqlx` from the Den workspace
with a database URL reachable from this workspace container.
Set SQLX_DATABASE_URL for an isolated validation database; the default is
bundled Den Postgres. Preparation routes den-memory to temporary SQLite built
from its schema.sql; other workspace packages use Postgres. prepare-memory needs
only Python 3 and Cargo, never starts Postgres, and additively refreshes the
production/test query metadata for den-memory alone.

Examples:
  ./scripts/sqlx.sh migrate run
  ./scripts/sqlx.sh prepare-all
  ./scripts/sqlx.sh prepare-memory
  ./scripts/sqlx.sh prepare --check --workspace -- --all-targets

Use prepare-all for a full refresh, or prepare-memory for SQLite-only changes.

Do not run bare `prepare --workspace -- --all-targets`: it never expands
member-crate *test* targets, and because prepare replaces `.sqlx` wholesale it
silently deletes every previously captured member-test entry (measured: 644 ->
499 entries, independent of cargo cache state). `prepare --check` only *warns*
about extra entries, so it cannot catch the loss; the real completeness gate is
an offline build, which `prepare-all` runs for you.
EOF
}

init_preparation() {
  PREPARE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/den-sqlx.XXXXXX")"
  python3 - "${DEN}/crates/den-memory/src/schema.sql" "${PREPARE_TMP}/memory.sqlite" <<'PY'
import sqlite3
import sys
from pathlib import Path

with sqlite3.connect(sys.argv[2]) as db:
    db.executescript(Path(sys.argv[1]).read_text())
PY
  # A fresh wrapper path changes Cargo's workspace fingerprint, forcing macro
  # expansion even with a warm target directory, without touching source/cleaning.
  cp "${ROOT}/scripts/sqlx-rustc.sh" "${PREPARE_TMP}/rustc-wrapper.sh"
  chmod +x "${PREPARE_TMP}/rustc-wrapper.sh"
}

online_prepare() (
  export SQLX_OFFLINE=false
  export DATABASE_URL
  export DEN_SQLX_MEMORY_DATABASE_URL="sqlite://${PREPARE_TMP}/memory.sqlite"
  export DEN_SQLX_PREVIOUS_WORKSPACE_WRAPPER="${RUSTC_WORKSPACE_WRAPPER:-}"
  export RUSTC_WORKSPACE_WRAPPER="${PREPARE_TMP}/rustc-wrapper.sh"
  "$@"
)

backup_cache() {
  CACHE_BACKUP="${PREPARE_TMP}/cache-before"
  mkdir -p "${CACHE_BACKUP}"
  if [ -d "${DEN}/.sqlx" ]; then
    cp -pR "${DEN}/.sqlx/." "${CACHE_BACKUP}/"
  fi
}

prepare_memory() {
  cd "${DEN}"
  mkdir -p "${PREPARE_TMP}/queries" .sqlx
  echo "==> den-memory SQLite pass (all targets; no Postgres)"
  online_prepare env SQLX_OFFLINE_DIR="${PREPARE_TMP}/queries" \
    cargo check -p den-memory --all-targets
  local query count=0
  for query in "${PREPARE_TMP}/queries/"query-*.json; do
    [ -f "${query}" ] || continue
    # Refresh emitted queries, including schema-only changes, without pruning
    # unrelated Postgres/SQLite entries from the shared cache.
    cp "${query}" .sqlx/
    count=$((count + 1))
  done
  if [ "${count}" -eq 0 ]; then
    echo "ERROR: den-memory emitted no SQLx metadata; check its macros feature and query macros." >&2
    return 1
  fi
  echo "OK: ${count} den-memory query metadata file(s) merged additively into ${DEN}/.sqlx."
}

# Workspace pass, then additive per-crate passes to capture member tests, then
# assert an offline build finds every query. Preserve the actual pre-run cache,
# including untracked/locally edited entries, rather than restoring from Git.
prepare_all() {
  cd "${DEN}"
  backup_cache

  local before after deleted query
  before="$( { find .sqlx -name 'query-*.json' 2>/dev/null || true; } | wc -l | tr -d ' ')"

  echo "==> workspace pass"
  online_prepare cargo sqlx prepare --workspace -- --all-targets

  echo "==> per-crate passes (recovers member-crate test-target queries)"
  local crate name crate_count merged
  for crate in crates/*/; do
    name="$(basename "${crate}")"
    # Some crates have no queries at all, and some fail to build standalone;
    # neither is fatal for the merged result.
    if ! (cd "${crate}" && online_prepare \
          cargo sqlx prepare -- --all-targets >/dev/null 2>&1); then
      # A failed pass can still leave a partial crate-local .sqlx behind;
      # never leave those in the tree for someone to commit by accident.
      rm -rf "${crate}.sqlx"
      printf '    %-20s skipped (standalone prepare failed)\n' "${name}"
      continue
    fi
    crate_count=0
    merged=0
    if [ -d "${crate}.sqlx" ]; then
      crate_count="$( { find "${crate}.sqlx" -name 'query-*.json' || true; } | wc -l | tr -d ' ')"
      merged="$( { cp -nv "${crate}.sqlx/"query-*.json .sqlx/ 2>/dev/null || true; } | wc -l | tr -d ' ')"
      rm -rf "${crate}.sqlx"
    fi
    printf '    %-20s crate=%-4s merged=%s\n' "${name}" "${crate_count}" "${merged}"
  done

  after="$( { find .sqlx -name 'query-*.json' || true; } | wc -l | tr -d ' ')"
  deleted=0
  for query in "${CACHE_BACKUP}/"query-*.json; do
    [ -f "${query}" ] || continue
    if [ ! -f ".sqlx/$(basename "${query}")" ]; then
      deleted=$((deleted + 1))
    fi
  done
  echo "==> .sqlx entries: ${before} -> ${after} (not regenerated: ${deleted})"

  if [ "${deleted}" -gt 0 ]; then
    if [ "${ALLOW_PRUNE}" -eq 1 ]; then
      echo "==> keeping ${deleted} deletion(s) (--allow-prune)"
    else
      restore_cache
      after="$( { find .sqlx -name 'query-*.json' || true; } | wc -l | tr -d ' ')"
      echo "==> preserved ${deleted} pre-run entry(ies); .sqlx now ${after} (additive)"
      if [ "${deleted}" -gt "${PRUNE_THRESHOLD}" ]; then
        cat >&2 <<EOF
NOTE: ${deleted} pre-run entries were not regenerated and may be stale.
Prune them deliberately in their own commit:  ./scripts/sqlx.sh prepare-all --allow-prune
EOF
      fi
    fi
  fi

  echo "==> completeness gate (offline build must resolve every query)"
  SQLX_OFFLINE=true cargo clippy --workspace --all-targets
  echo "OK: .sqlx is complete (${after} entries)."
}

if [ "$#" -eq 0 ]; then
  usage >&2
  exit 2
fi

case "$1" in
  -h | --help | help)
    usage
    exit 0
    ;;
  migrate)
    # Creating migration files is local-only: no database, Docker, or Cargo build.
    if [ "${2:-}" = "add" ]; then
      cd "${DEN}"
      exec sqlx "$@"
    fi
    ;;
  prepare-memory)
    if [ "$#" -ne 1 ]; then usage >&2; exit 2; fi
    init_preparation
    prepare_memory
    exit 0
    ;;
  prepare-all)
    shift
    case "$*" in
      "") ;;
      --allow-prune) ALLOW_PRUNE=1 ;;
      *) usage >&2; exit 2 ;;
    esac
    "${ROOT}/scripts/smoke-stack.sh" --infra
    init_preparation
    prepare_all
    exit 0
    ;;
  prepare)
    "${ROOT}/scripts/smoke-stack.sh" --infra
    init_preparation
    cd "${DEN}"
    online_prepare cargo sqlx "$@"
    exit 0
    ;;
esac

"${ROOT}/scripts/smoke-stack.sh" --infra
cd "${DEN}"
exec env DATABASE_URL="${DATABASE_URL}" cargo sqlx "$@"
