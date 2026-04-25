#!/bin/bash
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
export RUST_VERSION="${RUST_VERSION:-1.92.0}"
export RUSTUP_HOME=/usr/local/rustup
export CARGO_HOME=/usr/local/cargo
export PATH="${CARGO_HOME}/bin:${PATH}"

needs_apt=0
for cmd in bash curl git docker python3; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    needs_apt=1
  fi
done

if ! python3 - <<'PY' >/dev/null 2>&1
import pytest
import requests
PY
then
  needs_apt=1
fi

if [ "$needs_apt" = "1" ]; then
  apt-get update
  apt-get install -y \
    curl git bash ca-certificates build-essential pkg-config libssl-dev \
    python3 python3-pytest python3-requests \
    docker.io
  rm -rf /var/lib/apt/lists/*
fi

if ! docker compose version >/dev/null 2>&1; then
  mkdir -p /usr/local/lib/docker/cli-plugins
  arch="$(uname -m)"
  case "$arch" in
    x86_64) compose_arch="x86_64" ;;
    aarch64|arm64) compose_arch="aarch64" ;;
    *) echo "Unsupported architecture: $arch" >&2; exit 1 ;;
  esac
  curl -fsSL "https://github.com/docker/compose/releases/download/v2.29.7/docker-compose-linux-${compose_arch}" \
    -o /usr/local/lib/docker/cli-plugins/docker-compose
  chmod +x /usr/local/lib/docker/cli-plugins/docker-compose
fi

if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | bash -s -- -y --no-modify-path --profile minimal --default-toolchain "$RUST_VERSION"
  rustup component add clippy rustfmt
fi

toolchain_dir="$(find "${RUSTUP_HOME}/toolchains" -maxdepth 1 -mindepth 1 -type d | head -n 1)"
for name in cargo cargo-clippy clippy-driver rustc rustdoc rustfmt; do
  bin="${toolchain_dir}/bin/${name}"
  [ -x "$bin" ] || continue
  ln -sf "$bin" "${CARGO_HOME}/bin/${name}"
  ln -sf "$bin" "/usr/local/bin/${name}"
done

printf 'export PATH="%s/bin:$PATH"\n' "${CARGO_HOME}" > /etc/profile.d/cargo.sh
chmod -R a+w "${RUSTUP_HOME}" "${CARGO_HOME}"
