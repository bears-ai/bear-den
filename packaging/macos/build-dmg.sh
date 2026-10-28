#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
Usage: build-dmg.sh [options]

Build a macOS DMG containing Bears.app.

Options:
  --app <path>                    Path to an existing Bears.app bundle
  --app-version <version>         App version string (default: timestamp-based dev version)
  --bundle-id <id>                App bundle identifier (default: ai.bears.app)
  --display-name <name>           App display name (default: Bears)
  --output <path>                 Output DMG path (default: dist/macos/Bears-<version>.dmg)
  --source-root <path>            Swift package root (default: apps/apple/Bears)
  --binary-name <name>            Executable name inside the app (default: Bears)
  --application-identity <name>   Developer ID Application identity for codesign
  --background <path>             Optional background image copied into the DMG staging dir
  -h, --help                      Show this help

If --app is not supplied, the script builds a release executable with SwiftPM,
wraps it into a minimal Bears.app bundle, and then creates a DMG containing
that app plus an /Applications symlink.
USAGE
}

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
source_root="$repo_root/apps/apple/Bears"
app_path=""
app_version=""
bundle_id="ai.bears.app"
display_name="Bears"
binary_name="Bears"
output=""
application_identity="${MACOS_APPLICATION_CERT_IDENTITY:-}"
background=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --app)
      app_path="${2:-}"
      shift 2
      ;;
    --app-version)
      app_version="${2:-}"
      shift 2
      ;;
    --bundle-id)
      bundle_id="${2:-}"
      shift 2
      ;;
    --display-name)
      display_name="${2:-}"
      shift 2
      ;;
    --output)
      output="${2:-}"
      shift 2
      ;;
    --source-root)
      source_root="${2:-}"
      shift 2
      ;;
    --binary-name)
      binary_name="${2:-}"
      shift 2
      ;;
    --application-identity)
      application_identity="${2:-}"
      shift 2
      ;;
    --background)
      background="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "build-dmg.sh: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ ! -d "$source_root" ] && [ -z "$app_path" ]; then
  echo "build-dmg.sh: source root not found: $source_root" >&2
  exit 2
fi

if [ -z "$app_version" ]; then
  app_version="$(date -u +%Y.%m.%d.%H%M%S)"
fi

if [ -z "$output" ]; then
  output="$repo_root/dist/macos/${display_name}-${app_version}.dmg"
fi

work_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT INT TERM

staging_dir="$work_dir/dmg-root"
mkdir -p "$staging_dir" "$(dirname -- "$output")"

build_app_bundle() {
  app_bundle="$work_dir/${display_name}.app"
  contents_dir="$app_bundle/Contents"
  macos_dir="$contents_dir/MacOS"
  resources_dir="$contents_dir/Resources"
  frameworks_dir="$contents_dir/Frameworks"
  app_binary="$source_root/.build/release/$binary_name"
  plist_path="$contents_dir/Info.plist"
  pkginfo_path="$contents_dir/PkgInfo"

  echo "build-dmg.sh: building release app from Swift package"
  swift build -c release --product "$binary_name" --package-path "$source_root"

  if [ ! -x "$app_binary" ]; then
    echo "build-dmg.sh: expected built executable not found: $app_binary" >&2
    exit 1
  fi

  mkdir -p "$macos_dir" "$resources_dir" "$frameworks_dir"
  cp "$app_binary" "$macos_dir/$display_name"
  chmod 755 "$macos_dir/$display_name"

  codesign --remove-signature "$macos_dir/$display_name" >/dev/null 2>&1 || true
  xattr -cr "$macos_dir/$display_name" >/dev/null 2>&1 || true

  if [ -d "$source_root/BearsApp/Resources" ]; then
    cp -R "$source_root/BearsApp/Resources/." "$resources_dir/"
  fi
  # Keep SwiftPM's resource bundle relocatable as well as the existing raw helper resources.
  for resource_bundle in "$source_root"/.build/release/*.bundle; do
    [ -d "$resource_bundle" ] || continue
    cp -R "$resource_bundle" "$resources_dir/"
  done

  printf 'APPL????' >"$pkginfo_path"

  cat >"$plist_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>$display_name</string>
  <key>CFBundleIdentifier</key>
  <string>$bundle_id</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$display_name</string>
  <key>CFBundleDisplayName</key>
  <string>$display_name</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$app_version</string>
  <key>CFBundleVersion</key>
  <string>$app_version</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.developer-tools</string>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key>
      <string>$bundle_id.oauth</string>
      <key>CFBundleTypeRole</key>
      <string>Viewer</string>
      <key>CFBundleURLSchemes</key>
      <array><string>bears</string></array>
    </dict>
  </array>
</dict>
</plist>
EOF

  if [ -n "$application_identity" ]; then
    echo "build-dmg.sh: signing app bundle with $application_identity"
    codesign --force --deep --timestamp --options runtime --sign "$application_identity" "$app_bundle"
    codesign --verify --deep --strict --verbose=2 "$app_bundle"
  else
    echo "build-dmg.sh: no application signing identity supplied; leaving app unsigned" >&2
  fi

  app_path="$app_bundle"
}

if [ -z "$app_path" ]; then
  build_app_bundle
fi

if [ ! -d "$app_path" ]; then
  echo "build-dmg.sh: app bundle not found: $app_path" >&2
  exit 2
fi

cp -R "$app_path" "$staging_dir/"
ln -s /Applications "$staging_dir/Applications"

if [ -n "$background" ]; then
  if [ ! -f "$background" ]; then
    echo "build-dmg.sh: background not found: $background" >&2
    exit 2
  fi
  mkdir -p "$staging_dir/.background"
  cp "$background" "$staging_dir/.background/$(basename -- "$background")"
fi

volume_name="$display_name"

hdiutil create \
  -volname "$volume_name" \
  -srcfolder "$staging_dir" \
  -ov \
  -format UDZO \
  "$output"

echo "build-dmg.sh: wrote $output"
