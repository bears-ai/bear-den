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

Release builds currently package the arm64 macOS binary only to keep CI fast. Add an `x86_64-apple-darwin` build and a `lipo` combine step later if Intel Mac support becomes necessary.

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
  --output dist/macos/bears-acp-adapter-aarch64-apple-darwin.pkg \
  --application-identity "Developer ID Application: Your Org (TEAMID)" \
  --installer-identity "Developer ID Installer: Your Org (TEAMID)"
```

Then notarize and staple:

```bash
APP_STORE_CONNECT_API_KEY_ID="..." \
APP_STORE_CONNECT_API_ISSUER_ID="..." \
APP_STORE_CONNECT_API_KEY_PATH="/path/to/AuthKey_XXXX.p8" \
./packaging/macos/notarize.sh --pkg dist/macos/bears-acp-adapter-aarch64-apple-darwin.pkg
```

## Installing and validating

Install the package by double-clicking it, or with:

```bash
sudo installer -pkg dist/macos/bears-acp-adapter-aarch64-apple-darwin.pkg -target /
```

Validate the installed adapter:

```bash
/usr/local/bin/bears-acp-adapter --version
/usr/local/bin/bears-acp-adapter doctor
```

`doctor` needs `BEARS_DEN_API_URL`, `BEARS_BEAR_SLUG`, and either `BEARS_DEN_TOKEN` or `BEARS_DEN_TOKEN_ENV` set in the same environment used by the ACP client for a complete pass. Without those values, it prints the missing setup items and exits non-zero.

## Public update manifests

The adapter self-update command reads a small JSON manifest from a stable public URL and installs a newer signed/notarized `.pkg`. The default stable arm64 macOS manifest URL compiled into the adapter is:

```text
https://theartificial.github.io/BEARS/bears-acp-adapter/stable/aarch64-apple-darwin.json
```

GitHub Releases generate a Pages payload under:

```text
dist/update-site/bears-acp-adapter/<stable-or-beta>/
```

The payload contains:

- `bears-acp-adapter-aarch64-apple-darwin.pkg`
- `bears-acp-adapter-x86_64-unknown-linux-gnu`
- `bears-acp-adapter-aarch64-unknown-linux-gnu`
- `aarch64-apple-darwin.json`
- `x86_64-unknown-linux-gnu.json`
- `aarch64-unknown-linux-gnu.json`

The macOS manifest uses `pkg_url`. Linux manifests use `binary_url`; `.devcontainer/install-workspace-tools.sh` consumes those manifests to install the current adapter in Linux devcontainers.

Generate a manifest manually with:

```bash
./packaging/macos/generate-update-manifest.sh \
  --pkg dist/macos/bears-acp-adapter-aarch64-apple-darwin.pkg \
  --output dist/update-site/bears-acp-adapter/stable/aarch64-apple-darwin.json \
  --base-url https://theartificial.github.io/BEARS/bears-acp-adapter/stable \
  --channel stable \
  --target aarch64-apple-darwin \
  --release-notes-url https://github.com/TheArtificial/BEARS/releases/latest

./packaging/macos/generate-update-manifest.sh \
  --binary dist/update-site/bears-acp-adapter/stable/bears-acp-adapter-x86_64-unknown-linux-gnu \
  --output dist/update-site/bears-acp-adapter/stable/x86_64-unknown-linux-gnu.json \
  --base-url https://theartificial.github.io/BEARS/bears-acp-adapter/stable \
  --channel stable \
  --target x86_64-unknown-linux-gnu \
  --release-notes-url https://github.com/TheArtificial/BEARS/releases/latest
```

On release events, `.github/workflows/acp-adapter.yml` uploads that payload to the `gh-pages` branch while preserving other channels. Non-prerelease releases publish to `stable`; prereleases publish to `beta`. Enable repository Pages from the `gh-pages` branch before relying on the default URL.

Tag pushes matching `v*` or `bears-acp-adapter/v*` also build and publish the update site. The tag suffix becomes the adapter/package/manifest version, so `bears-acp-adapter/v0.1.1` publishes version `0.1.1`. Tags with a prerelease suffix such as `v0.1.1-beta.1` publish to `beta`; other tag pushes publish to `stable`.

Manual workflow dispatches do not publish the public update site unless `publish_update_site` is checked. For a manual publish, choose the `stable` or `beta` `update_channel` input and optionally provide a `release_notes_url`.

Linux devcontainer installs can be controlled with:

| Variable | Description |
| --- | --- |
| `BEARS_ACP_ADAPTER_CHANNEL` | Update channel for manifest-based installs, default `stable`. |
| `BEARS_ACP_ADAPTER_MANIFEST_URL` | Override the exact Linux manifest URL. |
| `BEARS_ACP_ADAPTER_VERSION` | Bypass the manifest and install a specific legacy GitHub Release asset version. |
| `BEARS_ACP_ADAPTER_INSTALL_DIR` | Install directory, default `/usr/local/bin`. |

For stricter updater signer verification, set repository variables used at build time:

| Variable | Description |
| --- | --- |
| `MACOS_INSTALLER_TEAM_ID` | Apple Team ID expected in `pkgutil --check-signature` output. |
| `MACOS_INSTALLER_CERT_IDENTITY_PUBLIC` | Public Developer ID Installer identity string expected in `pkgutil --check-signature` output. |

These are not secrets. The updater can also enforce the same checks at runtime with `BEARS_ACP_UPDATE_INSTALLER_TEAM_ID` or `BEARS_ACP_UPDATE_INSTALLER_IDENTITY`.
