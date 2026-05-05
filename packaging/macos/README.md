# macOS installer packaging

This directory contains the first-pass macOS `.pkg` installer pipeline for `bears-acp-adapter`.

The package installs the adapter at:

```text
/usr/local/bin/bears-acp-adapter
```

That system-wide path is intentionally stable so non-technical users can paste the same command into ACP clients such as aizen, and so future client-specific configuration helpers can target one location.

## Local package build

Build the adapter first:

```bash
cargo build --release --target aarch64-apple-darwin --manifest-path tools/bears-acp-adapter/Cargo.toml
```

Then build an unsigned package:

```bash
./packaging/macos/build-pkg.sh \
  --binary tools/bears-acp-adapter/target/aarch64-apple-darwin/release/bears-acp-adapter \
  --output dist/macos/bears-acp-adapter-test.pkg
```

For local Intel builds, use the `x86_64-apple-darwin` target path. For release builds, GitHub Actions combines arm64 and x86_64 artifacts into one universal binary before packaging.

## Signing and notarization

The GitHub workflow supports Developer ID signing and notarization when these secrets are present:

| Secret | Description |
| --- | --- |
| `MACOS_CERTIFICATES_P12_BASE64` | Base64-encoded `.p12` containing Developer ID Application and Developer ID Installer certs, or enough certs for the identities below. |
| `MACOS_CERTIFICATES_PASSWORD` | Password for the `.p12` file. |
| `MACOS_KEYCHAIN_PASSWORD` | Temporary CI keychain password. Any strong random value is fine. |
| `MACOS_APPLICATION_CERT_IDENTITY` | Codesign identity, usually `Developer ID Application: ... (TEAMID)`. |
| `MACOS_INSTALLER_CERT_IDENTITY` | Installer identity, usually `Developer ID Installer: ... (TEAMID)`. |
| `APP_STORE_CONNECT_API_KEY_ID` | App Store Connect API key ID for notarization. |
| `APP_STORE_CONNECT_API_ISSUER_ID` | App Store Connect issuer ID for notarization. |
| `APP_STORE_CONNECT_API_KEY_BASE64` | Base64-encoded App Store Connect `.p8` private key. |

If signing secrets are absent, the workflow still builds an unsigned `.pkg` artifact for internal CI validation. If notarization secrets are absent, it skips notarization.

## Manual signed build

After importing your Developer ID certificates into your local keychain:

```bash
./packaging/macos/build-pkg.sh \
  --binary dist/macos/bears-acp-adapter \
  --output dist/macos/bears-acp-adapter-universal.pkg \
  --application-identity "Developer ID Application: Your Org (TEAMID)" \
  --installer-identity "Developer ID Installer: Your Org (TEAMID)"
```

Then notarize and staple:

```bash
APP_STORE_CONNECT_API_KEY_ID="..." \
APP_STORE_CONNECT_API_ISSUER_ID="..." \
APP_STORE_CONNECT_API_KEY_PATH="/path/to/AuthKey_XXXX.p8" \
./packaging/macos/notarize.sh --pkg dist/macos/bears-acp-adapter-universal.pkg
```

## Installing and validating

Install the package by double-clicking it, or with:

```bash
sudo installer -pkg dist/macos/bears-acp-adapter-universal.pkg -target /
```

Validate the installed adapter:

```bash
/usr/local/bin/bears-acp-adapter --version
/usr/local/bin/bears-acp-adapter doctor
```

`doctor` needs `BEARS_DEN_API_URL`, `BEARS_BEAR_SLUG`, and either `BEARS_DEN_TOKEN` or `BEARS_DEN_TOKEN_ENV` set in the same environment used by the ACP client for a complete pass. Without those values, it prints the missing setup items and exits non-zero.
