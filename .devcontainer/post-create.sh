#!/usr/bin/env bash
set -Eeuo pipefail

# BEARS devcontainer post-create helper:
# validates toolchains needed for both Den (Rust) and Codepool (Node/TS)

failures=0
warnings=0

log()  { printf '\n[%s] %s\n' "$1" "$2"; }
ok()   { printf '  ✅ %s\n' "$1"; }
warn() { printf '  ⚠️  %s\n' "$1"; warnings=$((warnings + 1)); }
err()  { printf '  ❌ %s\n' "$1"; failures=$((failures + 1)); }

require_cmd() {
  local cmd="$1"
  local label="$2"
  if command -v "$cmd" >/dev/null 2>&1; then
    ok "$label: $(command -v "$cmd")"
  else
    err "$label is missing (expected command: $cmd)"
  fi
}

section_core_tools() {
  log "CHECK" "Core toolchains"
  require_cmd rustc "Rust compiler"
  require_cmd cargo "Cargo"
  require_cmd sqlx "SQLx CLI"
  require_cmd node "Node.js"
  require_cmd npm "npm"
  require_cmd psql "PostgreSQL client"
}

section_versions() {
  log "CHECK" "Tool versions"
  if command -v rustc >/dev/null 2>&1; then ok "rustc $(rustc --version)"; fi
  if command -v cargo >/dev/null 2>&1; then ok "cargo $(cargo --version)"; fi
  if command -v sqlx >/dev/null 2>&1; then ok "sqlx $(sqlx --version)"; fi
  if command -v node >/dev/null 2>&1; then ok "node $(node --version)"; fi
  if command -v npm >/dev/null 2>&1; then ok "npm $(npm --version)"; fi
  if command -v psql >/dev/null 2>&1; then ok "psql $(psql --version | head -n1)"; fi
}

section_node_version_gate() {
  log "CHECK" "Codepool Node.js requirement"
  if command -v node >/dev/null 2>&1; then
    local major
    major="$(node -p 'parseInt(process.versions.node.split(".")[0], 10)')"
    if [ "$major" -ge 20 ]; then
      ok "Node major version is $major (>=20 required)"
    else
      err "Node major version is $major, but Codepool requires >=20"
    fi
  fi
}

section_repo_layout() {
  log "CHECK" "Workspace layout"
  if [ -d /workspaces/den ]; then
    ok "Found /workspaces/den"
    if [ -f /workspaces/den/Cargo.toml ]; then
      ok "Found Den Cargo manifest"
    else
      err "Missing /workspaces/den/Cargo.toml"
    fi
  else
    err "Missing /workspaces/den"
  fi

  if [ -d /workspaces/codepool ]; then
    ok "Found /workspaces/codepool"
    if [ -f /workspaces/codepool/package.json ]; then
      ok "Found Codepool package.json"
    else
      err "Missing /workspaces/codepool/package.json"
    fi
  else
    err "Missing /workspaces/codepool"
  fi
}

section_optional_codepool_ts() {
  log "CHECK" "Codepool TypeScript availability (optional)"
  if [ -x /workspaces/codepool/node_modules/.bin/tsc ]; then
    ok "Found local TypeScript compiler"
    ok "$(/workspaces/codepool/node_modules/.bin/tsc --version)"
  else
    warn "TypeScript compiler not installed yet (run: cd /workspaces/codepool && npm install)"
  fi
}

summary() {
  log "SUMMARY" "Validation complete"
  if [ "$failures" -eq 0 ]; then
    ok "All required checks passed"
    if [ "$warnings" -gt 0 ]; then
      warn "$warnings warning(s) found"
    fi
    cat <<'EOF'

Next steps you can run in the container:
  cd /workspaces/den && cargo build
  cd /workspaces/codepool && npm install && npm run typecheck && npm run build
EOF
    return 0
  fi

  err "$failures required check(s) failed"
  if [ "$warnings" -gt 0 ]; then
    warn "$warnings warning(s) also found"
  fi
  return 1
}

main() {
  section_core_tools
  section_versions
  section_node_version_gate
  section_repo_layout
  section_optional_codepool_ts
  summary
}

main "$@"
