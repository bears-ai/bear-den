#!/bin/bash
set -euo pipefail

ROOT="/workspace"
DATABASE_URL="${SQLX_DATABASE_URL:-postgres://bears:bears@bears-postgres:5432/den?sslmode=disable}"
DEN="${ROOT}/services/den"

# Deletions tolerated by `prepare-all` before it stops and asks for review.
# Stale-entry pruning is legitimate but should be a deliberate, reviewed commit.
PRUNE_THRESHOLD="${PRUNE_THRESHOLD:-10}"

usage() {
  cat <<'EOF'
Usage: ./scripts/sqlx.sh <sqlx-command> [arguments...]
       ./scripts/sqlx.sh prepare-all [--allow-prune]

Start bundled Postgres if needed, then run `cargo sqlx` from the Den workspace
with a database URL reachable from this workspace container.
Set SQLX_DATABASE_URL for an isolated validation database; the default is
bundled Den Postgres. prepare-all uses the same override for every pass.

Examples:
  ./scripts/sqlx.sh migrate run
  ./scripts/sqlx.sh prepare-all
  ./scripts/sqlx.sh prepare --check --workspace -- --all-targets

`prepare-all` is the ONLY sanctioned way to refresh `.sqlx`.

Do not run bare `prepare --workspace -- --all-targets`: it never expands
member-crate *test* targets, and because prepare replaces `.sqlx` wholesale it
silently deletes every previously captured member-test entry (measured: 644 ->
499 entries, independent of cargo cache state). `prepare --check` only *warns*
about extra entries, so it cannot catch the loss; the real completeness gate is
an offline build, which `prepare-all` runs for you.
EOF
}

# Refresh `.sqlx` completely: workspace pass, then a per-crate pass whose output
# is merged additively (cp -n), then assert an offline build finds every query.
prepare_all() {
  local allow_prune="${1:-}"
  cd "${DEN}"

  local before after deleted
  before="$( { find .sqlx -name 'query-*.json' 2>/dev/null || true; } | wc -l | tr -d ' ')"

  echo "==> workspace pass"
  env DATABASE_URL="${DATABASE_URL}" cargo sqlx prepare --workspace -- --all-targets

  echo "==> per-crate passes (recovers member-crate test-target queries)"
  local crate name crate_count merged
  for crate in crates/*/; do
    name="$(basename "${crate}")"
    # Some crates have no queries at all, and some fail to build standalone;
    # neither is fatal for the merged result.
    if ! (cd "${crate}" && env DATABASE_URL="${DATABASE_URL}" \
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
  if ! (cd "${ROOT}" && git rev-parse --git-dir >/dev/null 2>&1); then
    # Without git we cannot tell regenerated-away entries from stale ones, so
    # the additive safety net below cannot run. Say so rather than implying it did.
    echo "WARNING: ${ROOT} is not a usable git checkout; skipping the additive" >&2
    echo "         restore. Verify 'git status -- services/den/.sqlx' yourself." >&2
    deleted=0
  else
    deleted="$( { cd "${ROOT}" && git status --porcelain -- services/den/.sqlx; } \
                | grep -c '^ *D' || true)"
  fi
  echo "==> .sqlx entries: ${before} -> ${after} (tracked deletions: ${deleted})"

  if [ "${deleted}" -gt 0 ]; then
    if [ "${allow_prune}" = "--allow-prune" ]; then
      echo "==> keeping ${deleted} deletion(s) (--allow-prune)"
    else
      # Default is additive: restore checked-in entries this pass did not
      # regenerate, so adding queries can never lose existing ones. Newly
      # generated entries are untracked and survive the checkout.
      (cd "${ROOT}" && git checkout -- services/den/.sqlx)
      after="$( { find .sqlx -name 'query-*.json' || true; } | wc -l | tr -d ' ')"
      echo "==> restored ${deleted} checked-in entry(ies); .sqlx now ${after} (additive)"
      if [ "${deleted}" -gt "${PRUNE_THRESHOLD}" ]; then
        cat >&2 <<EOF
NOTE: ${deleted} checked-in entries were not regenerated and may be stale.
Prune them deliberately in their own commit:  ./scripts/sqlx.sh prepare-all --allow-prune
EOF
      fi
    fi
  fi

  echo "==> completeness gate (offline build must resolve every query)"
  if SQLX_OFFLINE=true cargo clippy --workspace --all-targets 2>&1 \
     | grep -q 'no cached data'; then
    echo "ERROR: .sqlx is INCOMPLETE — do not commit." >&2
    echo "Missing entries:" >&2
    SQLX_OFFLINE=true cargo clippy --workspace --all-targets 2>&1 \
      | grep -A3 'no cached data' | grep -oE '[a-z/-]+\.rs:[0-9]+' | sort -u >&2
    exit 1
  fi
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
  prepare-all)
    shift
    "${ROOT}/scripts/smoke-stack.sh" --infra
    prepare_all "${1:-}"
    exit 0
    ;;
esac

"${ROOT}/scripts/smoke-stack.sh" --infra
cd "${DEN}"
exec env DATABASE_URL="${DATABASE_URL}" cargo sqlx "$@"
