#!/usr/bin/env bash
# Enforce the ADR-0031 write-topology amendment (one owning process per Bear
# database, one `MemoryStoreManager` per process): production code must receive
# a clone of the instance built at process startup, never construct its own.
# See docs/roadmap/MEMORY_WRITE_TOPOLOGY_PLAN.md and
# docs/decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md
# (write-topology amendment, 2026-07-30).
#
# The check greps every tracked/untracked Rust file for `MemoryStoreManager::new`
# and fails if a hit falls outside the sanctioned sites below.
set -euo pipefail

cd "$(dirname "$0")/.."

# Sanctioned construction sites (exact repo-relative paths).
allowlist=(
    # Process startup: the single production construction site. Everything
    # else in the server receives a clone of this instance via DenState.
    "services/den/src/lib.rs"
    # Short-lived CLI subcommands: each runs as its own (sole) owning process.
    # `den reindex` reads SQLite only; writes go to Postgres/Qdrant.
    "services/den/src/reindex.rs"
    # `den import-legacy-memory` requires the Bear's runtime to be stopped
    # (the CLI warns when WAL/SHM sidecars suggest a live database).
    "services/den/src/import_legacy_memory.rs"
    # `den seed` is a bootstrap CLI run before/without a live server.
    "services/den/src/seeds.rs"
    # The constructor's own definition and rustdoc live here.
    "services/den/crates/den-memory/src/manager.rs"
    # Test scaffolding for den-memory unit tests.
    "services/den/crates/den-memory/src/test_support.rs"
    # Files below construct managers only inside `#[cfg(test)]` modules
    # (verified 2026-07-31; keep new constructions test-gated).
    "services/den/crates/den-bearwire/src/events.rs"
    "services/den/crates/den-memory/src/admin_inspect.rs"
    "services/den/crates/den-runtime/src/agent_loop/session_stream.rs"
    "services/den/crates/den-runtime/src/native_runtime/turn.rs"
    "services/den/crates/den-web/src/lib.rs"
)

is_allowed() {
    file="$1"
    # Test code may construct throwaway managers: anything in a `tests/`
    # directory (integration tests, `src/**/tests/` modules) or a
    # `test.rs` / `*tests.rs` sibling-file test module.
    case "$file" in
        */tests/* | */test.rs | *tests.rs) return 0 ;;
    esac
    for allowed in "${allowlist[@]}"; do
        if [ "$file" = "$allowed" ]; then
            return 0
        fi
    done
    return 1
}

violations=""
while IFS= read -r line; do
    file="${line%%:*}"
    if ! is_allowed "$file"; then
        violations="${violations}${line}"$'\n'
    fi
done < <(git ls-files --cached --others --exclude-standard -- '*.rs' |
    xargs grep -Hn "MemoryStoreManager::new" -- 2>/dev/null || true)

if [ -n "$violations" ]; then
    {
        echo "Memory write-topology violation: MemoryStoreManager::new outside sanctioned sites:"
        echo
        printf '%s' "$violations"
        cat <<'EOF'

Per the ADR-0031 write-topology amendment (one owning process per Bear
database, one MemoryStoreManager per process), production code must receive
a clone of the manager built at startup (services/den/src/lib.rs), threaded
through DenState / runtime context — never call MemoryStoreManager::new.

See docs/roadmap/MEMORY_WRITE_TOPOLOGY_PLAN.md and the write-topology
amendment in
docs/decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md.

If this is genuinely a new sanctioned site (a short-lived CLI that owns the
database alone, or a #[cfg(test)]-gated construction), add it to the
allowlist in scripts/check-memory-write-topology.sh with a comment saying why.
EOF
    } >&2
    exit 1
fi

echo "Memory write-topology check passed"
