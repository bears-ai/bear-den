#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
Usage: notarize.sh --pkg <path>

Notarize and staple a signed macOS package with Apple's notary service.

Required environment:
  APP_STORE_CONNECT_API_KEY_ID
  APP_STORE_CONNECT_API_ISSUER_ID
  APP_STORE_CONNECT_API_KEY_PATH

The API key path should point to the .p8 private key file.
USAGE
}

pkg=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --pkg)
      pkg="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "notarize.sh: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "$pkg" ]; then
  echo "notarize.sh: --pkg is required" >&2
  exit 2
fi

if [ ! -f "$pkg" ]; then
  echo "notarize.sh: package not found: $pkg" >&2
  exit 2
fi

: "${APP_STORE_CONNECT_API_KEY_ID:?APP_STORE_CONNECT_API_KEY_ID is required}"
: "${APP_STORE_CONNECT_API_ISSUER_ID:?APP_STORE_CONNECT_API_ISSUER_ID is required}"
: "${APP_STORE_CONNECT_API_KEY_PATH:?APP_STORE_CONNECT_API_KEY_PATH is required}"

xcrun notarytool submit "$pkg" \
  --key "$APP_STORE_CONNECT_API_KEY_PATH" \
  --key-id "$APP_STORE_CONNECT_API_KEY_ID" \
  --issuer "$APP_STORE_CONNECT_API_ISSUER_ID" \
  --wait

xcrun stapler staple "$pkg"
xcrun stapler validate "$pkg"

spctl --assess --type install --verbose=4 "$pkg"

echo "notarize.sh: notarized and stapled $pkg"
