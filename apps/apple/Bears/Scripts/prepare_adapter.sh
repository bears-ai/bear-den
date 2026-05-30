#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKSPACE_ROOT="$(cd "${APP_ROOT}/../../.." && pwd)"
ADAPTER_CRATE_DIR="${WORKSPACE_ROOT}/tools/bears-acp-adapter"
ADAPTER_RESOURCE_DIR="${APP_ROOT}/Resources/Adapter"
ADAPTER_RESOURCE_PATH="${ADAPTER_RESOURCE_DIR}/bears-acp-adapter"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${WORKSPACE_ROOT}/target}"
PROFILE="${PROFILE:-debug}"
ADAPTER_BINARY="${ADAPTER_BINARY:-}"

case "${PROFILE}" in
  debug)
    CARGO_ARGS=()
    RELATIVE_BINARY_PATH="debug/bears-acp-adapter"
    ;;
  release)
    CARGO_ARGS=(--release)
    RELATIVE_BINARY_PATH="release/bears-acp-adapter"
    ;;
  *)
    echo "error: unsupported PROFILE='${PROFILE}'. Use 'debug' or 'release'." >&2
    exit 2
    ;;
esac

DEFAULT_ADAPTER_BINARY_PATH="${CARGO_TARGET_DIR}/${RELATIVE_BINARY_PATH}"
CRATE_LOCAL_ADAPTER_BINARY_PATH="${ADAPTER_CRATE_DIR}/target/${RELATIVE_BINARY_PATH}"

mkdir -p "${ADAPTER_RESOURCE_DIR}"

if [[ -n "${ADAPTER_BINARY}" ]]; then
  ADAPTER_BINARY_PATH="${ADAPTER_BINARY}"
  echo "==> Using adapter from ADAPTER_BINARY override"
elif [[ -f "${DEFAULT_ADAPTER_BINARY_PATH}" ]]; then
  ADAPTER_BINARY_PATH="${DEFAULT_ADAPTER_BINARY_PATH}"
  echo "==> Using existing built adapter from workspace target (${PROFILE})"
elif [[ -f "${CRATE_LOCAL_ADAPTER_BINARY_PATH}" ]]; then
  ADAPTER_BINARY_PATH="${CRATE_LOCAL_ADAPTER_BINARY_PATH}"
  echo "==> Using existing built adapter from crate-local target (${PROFILE})"
else
  if ! command -v cargo >/dev/null 2>&1; then
    echo "error: no adapter binary found in expected locations, and cargo is not installed." >&2
    echo "checked:" >&2
    echo "  - ${DEFAULT_ADAPTER_BINARY_PATH}" >&2
    echo "  - ${CRATE_LOCAL_ADAPTER_BINARY_PATH}" >&2
    echo "hint: provide ADAPTER_BINARY=/path/to/bears-acp-adapter or build the adapter in an environment with Rust first." >&2
    exit 1
  fi

  echo "==> Building bears-acp-adapter (${PROFILE})"
  if [[ ${#CARGO_ARGS[@]} -eq 0 ]]; then
    cargo build --manifest-path "${ADAPTER_CRATE_DIR}/Cargo.toml"
  else
    cargo build --manifest-path "${ADAPTER_CRATE_DIR}/Cargo.toml" "${CARGO_ARGS[@]}"
  fi

  if [[ -f "${DEFAULT_ADAPTER_BINARY_PATH}" ]]; then
    ADAPTER_BINARY_PATH="${DEFAULT_ADAPTER_BINARY_PATH}"
  elif [[ -f "${CRATE_LOCAL_ADAPTER_BINARY_PATH}" ]]; then
    ADAPTER_BINARY_PATH="${CRATE_LOCAL_ADAPTER_BINARY_PATH}"
  else
    echo "error: adapter build completed, but no binary was found in expected locations." >&2
    echo "checked:" >&2
    echo "  - ${DEFAULT_ADAPTER_BINARY_PATH}" >&2
    echo "  - ${CRATE_LOCAL_ADAPTER_BINARY_PATH}" >&2
    exit 1
  fi
fi

if [[ ! -f "${ADAPTER_BINARY_PATH}" ]]; then
  echo "error: expected adapter binary at '${ADAPTER_BINARY_PATH}', but it was not found." >&2
  exit 1
fi

echo "==> Copying adapter into app resources"
cp "${ADAPTER_BINARY_PATH}" "${ADAPTER_RESOURCE_PATH}"
chmod +x "${ADAPTER_RESOURCE_PATH}"

echo "==> Prepared adapter resource"
echo "    source: ${ADAPTER_BINARY_PATH}"
echo "    target: ${ADAPTER_RESOURCE_PATH}"
