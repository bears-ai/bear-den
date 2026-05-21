#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
Usage: generate-update-manifest.sh (--pkg <path> | --binary <path>) --output <path> --base-url <url> [options]

Generate a public update manifest for `bears-acp-adapter`.

macOS manifests use `pkg_url` and are consumed by `bears-acp-adapter update-check` and
`bears-acp-adapter update`. Linux manifests use `binary_url` and are consumed by
`.devcontainer/install-workspace-tools.sh`.

Options:
  --pkg <path>                  Path to the signed/notarized .pkg
  --binary <path>               Path to a Linux adapter binary
  --output <path>               Manifest output path (required)
  --base-url <url>              Public directory URL containing the asset (required)
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
binary=""
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
    --binary)
      binary="${2:-}"
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

if [ -n "$pkg" ] && [ -n "$binary" ]; then
  echo "generate-update-manifest.sh: pass either --pkg or --binary, not both" >&2
  exit 2
fi

if [ -n "$pkg" ]; then
  asset="$pkg"
  url_field="pkg_url"
elif [ -n "$binary" ]; then
  asset="$binary"
  url_field="binary_url"
else
  echo "generate-update-manifest.sh: either --pkg or --binary is required" >&2
  exit 2
fi

if [ ! -f "$asset" ]; then
  echo "generate-update-manifest.sh: asset not found: $asset" >&2
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
asset_name="$(basename -- "$asset")"
sha256="$(shasum -a 256 "$asset" | awk '{print $1}')"
size="$(wc -c < "$asset" | tr -d '[:space:]')"
asset_url="$base_url/$asset_name"

mkdir -p "$(dirname -- "$output")"

if [ "$url_field" = "pkg_url" ]; then
  cat > "$output" <<JSON
{
  "channel": "$channel",
  "version": "$version",
  "platforms": {
    "$target": {
      "pkg_url": "$asset_url",
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
else
  cat > "$output" <<JSON
{
  "channel": "$channel",
  "version": "$version",
  "platforms": {
    "$target": {
      "binary_url": "$asset_url",
      "sha256": "$sha256",
      "size": $size
    }
  },
  "release_notes_url": "$release_notes_url",
  "mandatory": $mandatory
}
JSON
fi

echo "generate-update-manifest.sh: wrote $output"
