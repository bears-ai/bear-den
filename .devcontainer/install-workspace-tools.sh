#!/bin/bash
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
export RUST_VERSION="${RUST_VERSION:-1.92.0}"
export RUSTUP_HOME=/usr/local/rustup
export CARGO_HOME=/usr/local/cargo
export PATH="${CARGO_HOME}/bin:${PATH}"

needs_apt=0
for cmd in bash curl git gh docker python3 clang ld.lld node npm; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    needs_apt=1
  fi
done

if ! command -v node >/dev/null 2>&1 || ! node -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 22 ? 0 : 1)' >/dev/null 2>&1; then
  needs_apt=1
fi

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
    curl git bash ca-certificates build-essential clang lld pkg-config libssl-dev \
    python3 python3-pytest python3-requests \
    docker.io
  if ! command -v gh >/dev/null 2>&1; then
    mkdir -p -m 755 /etc/apt/keyrings
    curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg -o /etc/apt/keyrings/githubcli-archive-keyring.gpg
    chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" > /etc/apt/sources.list.d/github-cli.list
    apt-get update
    apt-get install -y gh
  fi
  if ! command -v node >/dev/null 2>&1 || ! node -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 22 ? 0 : 1)' >/dev/null 2>&1; then
    curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
    apt-get install -y nodejs
  fi
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

install_bears_acp_adapter() {
  local version="${BEARS_ACP_ADAPTER_VERSION:-0.1.0}"
  local install_dir="${BEARS_ACP_ADAPTER_INSTALL_DIR:-/usr/local/bin}"
  local arch triple asset url tmp

  arch="$(uname -m)"
  case "${arch}" in
    x86_64|amd64) triple="x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) triple="aarch64-unknown-linux-gnu" ;;
    *) echo "bears-acp-adapter: unsupported Linux architecture: ${arch}" >&2; return 0 ;;
  esac

  asset="bears-acp-adapter-${triple}"
  url="https://github.com/TheArtificial/BEARS/releases/download/bears-acp-adapter%2Fv${version}/${asset}"
  tmp="$(mktemp)"

  echo "bears-acp-adapter: installing ${asset} from ${url}"
  if curl -fsSL "${url}" -o "${tmp}"; then
    mkdir -p "${install_dir}"
    install -m 0755 "${tmp}" "${install_dir}/bears-acp-adapter"
    rm -f "${tmp}"
    "${install_dir}/bears-acp-adapter" --help >/dev/null
    echo "bears-acp-adapter: installed to ${install_dir}/bears-acp-adapter"
  else
    rm -f "${tmp}"
    echo "bears-acp-adapter: release download failed for ${url}" >&2
    if [ -f /workspace/tools/bears-acp-adapter/Cargo.toml ]; then
      echo "bears-acp-adapter: falling back to local source build" >&2
      cargo build --release --locked --manifest-path /workspace/tools/bears-acp-adapter/Cargo.toml
      ln -sf /workspace/tools/bears-acp-adapter/target/release/bears-acp-adapter "${install_dir}/bears-acp-adapter"
    else
      echo "bears-acp-adapter: set BEARS_ACP_ADAPTER_VERSION to an existing release, or install manually" >&2
    fi
  fi
}

install_bears_acp_adapter
