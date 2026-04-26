#!/usr/bin/env bash
# List (or strictly fail on) obvious starter placeholders after you rename the app.
# Default: print matches and exit 0. Use `./scripts/verify-rename.sh --strict` after rename
# to fail CI or pre-commit if anything is left (tweak paths/excludes if intentional).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STRICT=0
if [[ "${1:-}" == "--strict" ]]; then
  STRICT=1
fi

collect_hits() {
  if command -v rg >/dev/null 2>&1; then
    rg -n 'newapp\.example|\bnewapp\b' \
      --glob '!target/**' \
      --glob '!.sqlx/**' \
      src tests docs migrations README.md AGENTS.md tasks.md Cargo.toml Cargo.lock \
      .devcontainer Dockerfile .github 2>/dev/null || true
  else
    grep -rIn --exclude-dir=target --exclude-dir=.sqlx --exclude-dir=.git \
      --include='*.rs' --include='*.md' --include='*.toml' --include='*.yml' --include='*.json' \
      -E 'newapp\.example|(^|[^A-Za-z0-9_])newapp([^A-Za-z0-9_]|$)' \
      src tests docs migrations README.md AGENTS.md tasks.md Cargo.toml Cargo.lock \
      .devcontainer Dockerfile .github 2>/dev/null || true
  fi
}

mapfile -t HITS < <(collect_hits)

if [[ ${#HITS[@]} -eq 0 ]]; then
  echo "No starter placeholder matches in checked paths."
  exit 0
fi

printf '%s\n' "${HITS[@]}"
if [[ "$STRICT" == 1 ]]; then
  echo >&2 "Rename verification failed (${#HITS[@]} line(s)). See docs/rename-from-starter.md"
  exit 1
fi
echo >&2 "Tip: after replacing placeholders, run: $0 --strict"
