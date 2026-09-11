#!/usr/bin/env bash
# Keep process-local Den lifecycle/controller state canonical. Production code
# must receive a clone of a DenState composed at a sanctioned root rather than
# constructing independent turn coordinators in request/tool paths.
set -euo pipefail

cd "$(dirname "$0")/.."

# Exact production files currently allowed to compose DenState.
allowlist=(
    # Process composition root shared by API, runtime tools, and workers.
    "services/den/src/lib.rs"
    # Test-only state construction in a #[cfg(test)] module.
    "services/den/crates/den-bearwire/src/events.rs"
)

is_allowed() {
    file="$1"
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

record_violation() {
    line="$1"
    file="${line%%:*}"
    if ! is_allowed "$file"; then
        violations="${violations}${line}"$'\n'
    fi
}

if [ "${1:-}" = "--self-test" ]; then
    violations=""
    record_violation "services/den/src/core/tools/session/mod.rs:1:den_service::DenState::new("
    if [ -z "$violations" ]; then
        echo "DenState topology guard self-test failed: forbidden session tool construction was accepted" >&2
        exit 1
    fi
    echo "DenState topology guard self-test passed"
fi

violations=""
while IFS= read -r line; do
    record_violation "$line"
done < <(git ls-files --cached --others --exclude-standard -- '*.rs' |
    xargs grep -Hn "DenState::new" -- 2>/dev/null || true)

if [ -n "$violations" ]; then
    {
        echo "DenState topology violation: DenState::new outside sanctioned composition roots/tests:"
        echo
        printf '%s' "$violations"
        cat <<'EOF'

DenState owns process-local lifecycle/controller state. Production request and
tool paths must receive a clone of the canonical state; constructing a new state
creates independent coordinators and can split lifecycle ownership.

If this is genuinely a composition root or test-only file, add the narrowest
possible allowlist entry to scripts/check-den-state-topology.sh with a comment
explaining why. Do not sanction request or tool execution paths.
EOF
    } >&2
    exit 1
fi

echo "DenState topology check passed"
