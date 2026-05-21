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

install_bears_acp_adapter_binary() {
  local url="$1"
  local install_dir="$2"
  local expected_sha256="${3:-}"
  local expected_size="${4:-}"
  local tmp actual_sha256 actual_size

  tmp="$(mktemp)"
  echo "bears-acp-adapter: downloading ${url}"
  if ! curl -fsSL "${url}" -o "${tmp}"; then
    rm -f "${tmp}"
    return 1
  fi

  if [ -n "${expected_size}" ]; then
    actual_size="$(wc -c < "${tmp}" | tr -d '[:space:]')"
    if [ "${actual_size}" != "${expected_size}" ]; then
      rm -f "${tmp}"
      echo "bears-acp-adapter: size mismatch for ${url}: expected ${expected_size}, got ${actual_size}" >&2
      return 1
    fi
  fi

  if [ -n "${expected_sha256}" ]; then
    actual_sha256="$(sha256sum "${tmp}" | awk '{print $1}')"
    if [ "${actual_sha256}" != "${expected_sha256}" ]; then
      rm -f "${tmp}"
      echo "bears-acp-adapter: SHA-256 mismatch for ${url}: expected ${expected_sha256}, got ${actual_sha256}" >&2
      return 1
    fi
  fi

  mkdir -p "${install_dir}"
  install -m 0755 "${tmp}" "${install_dir}/bears-acp-adapter"
  rm -f "${tmp}"
  "${install_dir}/bears-acp-adapter" --help >/dev/null
  echo "bears-acp-adapter: installed to ${install_dir}/bears-acp-adapter"
}

install_bears_acp_adapter_from_source() {
  local install_dir="$1"
  if [ -f /workspace/tools/bears-acp-adapter/Cargo.toml ]; then
    echo "bears-acp-adapter: falling back to local source build" >&2
    if ! cargo build --release --locked --manifest-path /workspace/tools/bears-acp-adapter/Cargo.toml; then
      echo "bears-acp-adapter: locked source build failed; retrying without --locked so Cargo.lock can be refreshed for this checkout" >&2
      cargo build --release --manifest-path /workspace/tools/bears-acp-adapter/Cargo.toml
    fi
    ln -sf /workspace/tools/bears-acp-adapter/target/release/bears-acp-adapter "${install_dir}/bears-acp-adapter"
  else
    echo "bears-acp-adapter: set BEARS_ACP_ADAPTER_MANIFEST_URL or BEARS_ACP_ADAPTER_VERSION, or install manually" >&2
  fi
}

install_bears_acp_adapter() {
  local version="${BEARS_ACP_ADAPTER_VERSION:-}"
  local channel="${BEARS_ACP_ADAPTER_CHANNEL:-stable}"
  local install_dir="${BEARS_ACP_ADAPTER_INSTALL_DIR:-/usr/local/bin}"
  local arch triple asset manifest_url manifest_tmp url sha256 size parsed_version cargo_version

  arch="$(uname -m)"
  case "${arch}" in
    x86_64|amd64) triple="x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) triple="aarch64-unknown-linux-gnu" ;;
    *) echo "bears-acp-adapter: unsupported Linux architecture: ${arch}" >&2; return 0 ;;
  esac

  asset="bears-acp-adapter-${triple}"
  cargo_version=""
  if [ -f /workspace/tools/bears-acp-adapter/Cargo.toml ]; then
    cargo_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' /workspace/tools/bears-acp-adapter/Cargo.toml | head -n 1)"
  fi

  if [ -z "${version}" ]; then
    manifest_url="${BEARS_ACP_ADAPTER_MANIFEST_URL:-https://theartificial.github.io/BEARS/bears-acp-adapter/${channel}/${triple}.json}"
    manifest_tmp="$(mktemp)"
    echo "bears-acp-adapter: checking ${channel} manifest ${manifest_url}"
    if curl -fsSL "${manifest_url}" -o "${manifest_tmp}"; then
      if mapfile -t manifest_values < <(python3 - "${manifest_tmp}" "${triple}" <<'PY'
import json
import sys

path, target = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as f:
    manifest = json.load(f)
platform = manifest.get("platforms", {}).get(target)
if not platform:
    raise SystemExit(f"manifest does not contain target {target}")
url = platform.get("binary_url")
if not url:
    raise SystemExit("manifest target does not contain binary_url")
print(manifest.get("version", ""))
print(url)
print(platform.get("sha256", ""))
print(platform.get("size", ""))
PY
      ); then
        parsed_version="${manifest_values[0]:-}"
        url="${manifest_values[1]:-}"
        sha256="${manifest_values[2]:-}"
        size="${manifest_values[3]:-}"
        echo "bears-acp-adapter: installing ${asset} version ${parsed_version:-unknown} from update manifest"
        if install_bears_acp_adapter_binary "${url}" "${install_dir}" "${sha256}" "${size}"; then
          rm -f "${manifest_tmp}"
          return 0
        fi
      else
        echo "bears-acp-adapter: could not parse update manifest ${manifest_url}" >&2
      fi
    else
      echo "bears-acp-adapter: update manifest download failed for ${manifest_url}" >&2
    fi
    rm -f "${manifest_tmp}"
    version="${cargo_version:-0.1.0}"
  fi

  url="https://github.com/TheArtificial/BEARS/releases/download/bears-acp-adapter%2Fv${version}/${asset}"
  echo "bears-acp-adapter: installing ${asset} from release fallback ${url}"
  if ! install_bears_acp_adapter_binary "${url}" "${install_dir}"; then
    echo "bears-acp-adapter: release download failed for ${url}" >&2
    install_bears_acp_adapter_from_source "${install_dir}"
  fi
}

install_bears_acp_adapter
