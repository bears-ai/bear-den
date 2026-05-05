#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
Usage: build-pkg.sh --binary <path> [options]

Build a macOS installer package for the BEARS ACP adapter.

Options:
  --binary <path>                 Path to the compiled bears-acp-adapter binary (required)
  --version <version>             Package version (default: read from Cargo.toml)
  --identifier <id>               Package identifier (default: ai.bears.acp-adapter)
  --install-location <path>       Install prefix (default: /usr/local/bin)
  --output <path>                 Output package path (default: dist/macos/bears-acp-adapter-<version>.pkg)
  --application-identity <name>   Developer ID Application identity for codesign
  --installer-identity <name>     Developer ID Installer identity for productbuild signing
  --scripts-dir <path>            Package scripts directory (default: packaging/macos/scripts)
  -h, --help                      Show this help

Environment fallbacks:
  MACOS_APPLICATION_CERT_IDENTITY
  MACOS_INSTALLER_CERT_IDENTITY

The script creates an unsigned package if no installer identity is supplied.
USAGE
}

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
version=""
identifier="ai.bears.acp-adapter"
install_location="/usr/local/bin"
output=""
binary=""
application_identity="${MACOS_APPLICATION_CERT_IDENTITY:-}"
installer_identity="${MACOS_INSTALLER_CERT_IDENTITY:-}"
scripts_dir="$repo_root/packaging/macos/scripts"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --binary)
      binary="${2:-}"
      shift 2
      ;;
    --version)
      version="${2:-}"
      shift 2
      ;;
    --identifier)
      identifier="${2:-}"
      shift 2
      ;;
    --install-location)
      install_location="${2:-}"
      shift 2
      ;;
    --output)
      output="${2:-}"
      shift 2
      ;;
    --application-identity)
      application_identity="${2:-}"
      shift 2
      ;;
    --installer-identity)
      installer_identity="${2:-}"
      shift 2
      ;;
    --scripts-dir)
      scripts_dir="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "build-pkg.sh: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "$binary" ]; then
  echo "build-pkg.sh: --binary is required" >&2
  exit 2
fi

if [ ! -f "$binary" ]; then
  echo "build-pkg.sh: binary not found: $binary" >&2
  exit 2
fi

if [ -z "$version" ]; then
  version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo_root/tools/bears-acp-adapter/Cargo.toml" | head -n 1)"
fi

if [ -z "$version" ]; then
  echo "build-pkg.sh: could not determine version; pass --version" >&2
  exit 2
fi

if [ -z "$output" ]; then
  output="$repo_root/dist/macos/bears-acp-adapter-$version.pkg"
fi

work_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT INT TERM

pkg_root="$work_dir/root"
component_pkg="$work_dir/bears-acp-adapter-component.pkg"
install_dir="$pkg_root$install_location"

mkdir -p "$install_dir" "$(dirname -- "$output")"
cp "$binary" "$install_dir/bears-acp-adapter"
chmod 755 "$install_dir/bears-acp-adapter"

if [ -n "$application_identity" ]; then
  echo "build-pkg.sh: signing binary with $application_identity"
  codesign --force --timestamp --options runtime --sign "$application_identity" "$install_dir/bears-acp-adapter"
  codesign --verify --strict --verbose=2 "$install_dir/bears-acp-adapter"
else
  echo "build-pkg.sh: no application signing identity supplied; leaving binary unsigned" >&2
fi

if [ -d "$scripts_dir" ]; then
  pkgbuild \
    --root "$pkg_root" \
    --identifier "$identifier" \
    --version "$version" \
    --install-location / \
    --scripts "$scripts_dir" \
    "$component_pkg"
else
  pkgbuild \
    --root "$pkg_root" \
    --identifier "$identifier" \
    --version "$version" \
    --install-location / \
    "$component_pkg"
fi

if [ -n "$installer_identity" ]; then
  echo "build-pkg.sh: signing package with $installer_identity"
  productbuild --sign "$installer_identity" --timestamp --package "$component_pkg" "$output"
else
  echo "build-pkg.sh: no installer signing identity supplied; creating unsigned package" >&2
  productbuild --package "$component_pkg" "$output"
fi

pkgutil --check-signature "$output" || true

echo "build-pkg.sh: wrote $output"
