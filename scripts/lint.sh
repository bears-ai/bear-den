#!/usr/bin/env bash
# Run the Den clippy gate locally, matching CI (.github/workflows/den-clippy.yml).
#
# The v2 gate (docs/roadmap/DEN_CRATE_SPLIT_PLAN.md) denies the default lint set
# plus clippy `pedantic` and `nursery`, which are set to `deny` in
# `[workspace.lints.clippy]` with a curated allow-list. `-D warnings` turns any
# remaining (non-allow-listed) lint into a hard failure.
set -euo pipefail

# Production topology: constructors only at sanctioned composition roots/tests.
bash "$(dirname "$0")/check-memory-write-topology.sh"
bash "$(dirname "$0")/check-den-state-topology.sh"

cd "$(dirname "$0")/../services/den"

export SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

exec cargo clippy --workspace --all-targets -- -D warnings
