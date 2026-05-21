#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
Usage: generate-update-manifest.sh --pkg <path> --output <path> --base-url <url> [options]

Generate the public update manifest consumed by `bears-acp-adapter update-check` and
`bears-acp-adapter update`.

Options:
  --pkg <path>                  Path to the signed/notarized .pkg (required)
  --output <path>               Manifest output path (required)
  --base-url <url>              Public directory URL containing the .pkg (required)
  --channel <name>              Update channel (default: stable)
  --version <version>           Adapter/package version (default: read from Cargo.toml)
  --target <triple>             Platform target (default: aarch64-apple-darwin)
  --release-notes-url <url>     Release notes URL
  --min-macos <version>         Minimum macOS version (default: 13.0)
  --package-identifier <id>     Package identifier (default: ai.bears.acp-adapter)
  --mandatory <true|false>      Whether the update is mandatory (default: false)
  -h, --help                    Show this help
USAGE
}

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
pkg=""
output=""
base_url=""
channel="stable"
version=""
target="aarch64-apple-darwin"
release_notes_url=""
min_macos="13.0"
package_identifier="ai.bears.acp-adapter"
mandatory="false"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --pkg)
      pkg="${2:-}"
      shift 2
      ;;
    --output)
      output="${2:-}"
      shift 2
      ;;
    --base-url)
      base_url="${2:-}"
      shift 2
      ;;
    --channel)
      channel="${2:-}"
      shift 2
      ;;
    --version)
      version="${2:-}"
      shift 2
      ;;
    --target)
      target="${2:-}"
      shift 2
      ;;
    --release-notes-url)
      release_notes_url="${2:-}"
      shift 2
      ;;
    --min-macos)
      min_macos="${2:-}"
      shift 2
      ;;
    --package-identifier)
      package_identifier="${2:-}"
      shift 2
      ;;
    --mandatory)
      mandatory="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "generate-update-manifest.sh: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "$pkg" ] || [ ! -f "$pkg" ]; then
  echo "generate-update-manifest.sh: --pkg is required and must point to a file" >&2
  exit 2
fi

if [ -z "$output" ]; then
  echo "generate-update-manifest.sh: --output is required" >&2
  exit 2
fi

if [ -z "$base_url" ]; then
  echo "generate-update-manifest.sh: --base-url is required" >&2
  exit 2
fi

if [ -z "$version" ]; then
  version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo_root/tools/bears-acp-adapter/Cargo.toml" | head -n 1)"
fi

if [ -z "$version" ]; then
  echo "generate-update-manifest.sh: could not determine version; pass --version" >&2
  exit 2
fi

case "$mandatory" in
  true|false) ;;
  *)
    echo "generate-update-manifest.sh: --mandatory must be true or false" >&2
    exit 2
    ;;
esac

base_url="$(printf '%s' "$base_url" | sed 's:/*$::')"
pkg_name="$(basename -- "$pkg")"
sha256="$(shasum -a 256 "$pkg" | awk '{print $1}')"
size="$(wc -c < "$pkg" | tr -d '[:space:]')"
pkg_url="$base_url/$pkg_name"

mkdir -p "$(dirname -- "$output")"

cat > "$output" <<JSON
{
  "channel": "$channel",
  "version": "$version",
  "platforms": {
    "$target": {
      "pkg_url": "$pkg_url",
      "sha256": "$sha256",
      "min_macos": "$min_macos",
      "size": $size,
      "package_identifier": "$package_identifier"
    }
  },
  "release_notes_url": "$release_notes_url",
  "mandatory": $mandatory
}
JSON

echo "generate-update-manifest.sh: wrote $output"
