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

ensure_path() {
  case ":${PATH}:" in
    *:/usr/local/cargo/bin:*) ;;
    *) export PATH="/usr/local/cargo/bin:${PATH}" ;;
  esac
}

link_rust_tools() {
  local tool
  for tool in rustc cargo rustup rustfmt cargo-clippy cargo-fmt clippy-driver sqlx; do
    if [ -x "/usr/local/cargo/bin/${tool}" ]; then
      ln -sf "/usr/local/cargo/bin/${tool}" "/usr/local/bin/${tool}"
    fi
  done
}

install_node20() {
  if ! command -v apt-get >/dev/null 2>&1; then
    err "apt-get is unavailable; cannot install Node.js 20"
    return
  fi

  if ! apt-get update >/dev/null 2>&1; then
    err "apt-get update failed while preparing Node.js install"
    return
  fi

  if ! apt-get install -y --no-install-recommends ca-certificates curl gnupg >/dev/null 2>&1; then
    err "Failed installing Node.js prerequisites"
    return
  fi

  mkdir -p /etc/apt/keyrings
  if ! curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg; then
    err "Failed adding NodeSource signing key"
    return
  fi

  echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_20.x nodistro main" > /etc/apt/sources.list.d/nodesource.list

  if ! apt-get update >/dev/null 2>&1; then
    err "apt-get update failed after adding NodeSource"
    return
  fi

  if ! apt-get install -y --no-install-recommends nodejs >/dev/null 2>&1; then
    err "Failed installing Node.js 20"
    return
  fi

  ok "Installed Node.js: $(node --version)"
}

install_psql() {
  if ! command -v apt-get >/dev/null 2>&1; then
    err "apt-get is unavailable; cannot install PostgreSQL client"
    return
  fi

  if ! apt-get update >/dev/null 2>&1; then
    err "apt-get update failed while preparing PostgreSQL client install"
    return
  fi

  if ! apt-get install -y --no-install-recommends postgresql-client >/dev/null 2>&1; then
    err "Failed installing PostgreSQL client"
    return
  fi

  ok "Installed PostgreSQL client: $(psql --version | head -n1)"
}

install_sqlx() {
  ensure_path

  if ! command -v cargo >/dev/null 2>&1; then
    err "Cargo is unavailable; cannot install SQLx CLI"
    return
  fi

  if ! cargo install --target-dir /usr/local/cargo/target sqlx-cli --no-default-features --features rustls,postgres >/dev/null 2>&1; then
    err "Failed installing SQLx CLI"
    return
  fi

  ok "Installed SQLx CLI: $(sqlx --version)"
}

section_bootstrap_tools() {
  log "FIXUP" "Bootstrap missing toolchains when needed"

  ensure_path
  link_rust_tools

  if command -v rustc >/dev/null 2>&1; then
    ok "Rust compiler available after PATH fix"
  elif [ -x /usr/local/cargo/bin/rustc ]; then
    ensure_path
    ok "Rust compiler found under /usr/local/cargo/bin"
  else
    err "Rust compiler is missing from image"
  fi

  if command -v cargo >/dev/null 2>&1; then
    if rustup component add rustfmt clippy >/dev/null 2>&1; then
      ok "Ensured rustfmt and clippy components"
    else
      warn "Could not add rustfmt/clippy components"
    fi
  fi

  if command -v node >/dev/null 2>&1; then
    major="$(node -p 'parseInt(process.versions.node.split(".")[0], 10)' 2>/dev/null || printf '0')"
    if [ "$major" -ge 20 ]; then
      ok "Node.js already satisfies >=20"
    else
      warn "Node.js is older than 20; upgrading"
      install_node20
    fi
  else
    warn "Node.js not found; installing Node.js 20"
    install_node20
  fi

  if ! command -v psql >/dev/null 2>&1; then
    warn "PostgreSQL client not found; installing"
    install_psql
  fi

  if ! command -v sqlx >/dev/null 2>&1; then
    warn "SQLx CLI not found; installing"
    install_sqlx
  fi
}

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
  section_bootstrap_tools
  section_core_tools
  section_versions
  section_node_version_gate
  section_repo_layout
  section_optional_codepool_ts
  summary
}

main "$@"
