# Building the Bears macOS app scaffold

This is the current lightweight build path for local testing.

## Current approach

The repo now includes a Swift Package manifest at:

- `apps/apple/Bears/Package.swift`

It builds a minimal macOS executable app target named `BearsApp`.

## Prerequisites

- Xcode 15+ or a compatible Swift 5.10 toolchain
- macOS 13+
- a local `bear-armature` executable available to copy into package resources; the app still accepts the legacy bundled resource name `bears-acp-adapter`

## Prepare the bundled adapter resource

## Adapter source options

The app now supports two adapter-install sources:

1. a bundled adapter resource inside the app target, if present;
2. a downloaded macOS adapter artifact, if no bundled adapter is present.

This preview app is currently intended only for Apple Silicon Macs.

### Optional local bundled adapter for development

Use the helper script to prepare the bundled adapter resource automatically:

```bash
cd apps/apple/Bears
bash Scripts/prepare_adapter.sh
```

By default the script will:

1. use an existing built adapter at `target/debug/bear-armature` if present;
2. otherwise use an existing built adapter at `tools/bear-armature/target/debug/bear-armature` if present;
3. otherwise fall back to `cargo build --manifest-path tools/bear-armature/Cargo.toml` if `cargo` is available.

To prepare a release adapter artifact instead:

```bash
cd apps/apple/Bears
PROFILE=release bash Scripts/prepare_adapter.sh
```

You can also point at an explicit prebuilt adapter binary:

```bash
cd apps/apple/Bears
ADAPTER_BINARY=/path/to/bear-armature bash Scripts/prepare_adapter.sh
```

The script places the adapter at:

- `apps/apple/Bears/BearsApp/Resources/Adapter/bears-acp-adapter`

### Remote download fallback

If no bundled adapter is present, the app will try to download a macOS adapter artifact.

## Important: publishing a rebuilt package requires a version bump

The GitHub release/update-site flow is version-driven. If you rebuild the adapter package and want GitHub to publish a new package artifact at the release/update URL, you must:

1. bump `version` in `tools/bear-armature/Cargo.toml`
2. update the relevant Cargo lockfile if dependency resolution changes
3. rebuild and republish the release assets

If you only change packaging or installer scripts without bumping the adapter version, the existing published release/tag may be reused and the app can keep downloading an older `.pkg` built with stale packaging behavior.

When debugging install path or package-script changes, always treat this version bump as part of the publish step.

By default it uses the macOS update manifest:

- `https://bears-ai.github.io/bear-den/bears-acp-adapter/stable/macos.json`

The app reads `version` and `pkg_url` from that manifest, uses the version as the update reference, and downloads the package from `pkg_url`.

You can override that for development with either:

```bash
BEARS_ADAPTER_MANIFEST_URL=https://example.com/path/to/macos.json xcrun swift run Bears
```

or a direct package URL:

```bash
BEARS_ADAPTER_DOWNLOAD_URL=https://example.com/path/to/bears-acp-adapter-aarch64-apple-darwin.pkg xcrun swift run Bears
```

The app now supports either:

- a direct macOS Mach-O adapter binary, or
- a macOS installer package (`.pkg`)

When given a `.pkg`, the app invokes the system installer targeting `/`, and macOS may prompt for administrator credentials.

## Build

From the package root:

```bash
cd apps/apple/Bears
swift build
```

## Version metadata and regression tests

The app probes `bear-armature version --json` for installed/bundled binary metadata. Current source implements that configuration-free command: one JSON object on stdout with version/build facts, no Den authentication or browser/network probing. Human `--version` output remains on stderr. The app retains text fallback for older `bear-armature` and `bears-acp-adapter` binaries; it checks each stream independently and does not treat arbitrary diagnostic text as a version.

On macOS, run the app-side parser/probe regressions from this package root:

```bash
swift test --filter AdapterVersionReaderTests
```

From the repository root, run the binary CLI contract tests:

```bash
cargo test --manifest-path tools/bear-armature/Cargo.toml --locked --test version_cli
```

A version-probe failure does not itself prove the installation is corrupt. The app now reports the actual launch/probe/parse failure; inspect the installed executable path and full diagnostics for permissions, architecture, quarantine, missing-file or incompatible-version output. These are source changes, not evidence that an existing app/package contains the fix. Rebuild the app to receive parser/diagnostic improvements, and rebuild/release the armature package to supply JSON introspection to older apps. Publishing that package still requires the version bump described above.

## Run

```bash
cd apps/apple/Bears
swift run Bears
```

## Package layout note

The Swift sources needed by the executable target have now been consolidated under:

- `apps/apple/Bears/BearsApp/`

That keeps the initial Swift Package setup simple and gives the first local build a better chance of succeeding.

## Building a distributable DMG

A first-pass DMG packaging script now exists at:

- `packaging/macos/build-dmg.sh`

From the repo root:

```bash
./packaging/macos/build-dmg.sh --app-version 0.1.0
```

This produces:

- `dist/macos/Bears-0.1.0.dmg`

The script currently builds a release SwiftPM executable, wraps it into a minimal `Bears.app`, and creates a DMG containing that app plus an `/Applications` symlink.

## Current limitations

This is still an intentionally lightweight package-based scaffold for early testing and first-pass distribution.

Not yet complete:

- Developer ID signing for `Bears.app`
- DMG signing/notarization flow
- Sparkle integration
- automated app release publishing to gh-pages
- Xcode project/workspace configuration for shipping builds

The current goal is to make the SwiftUI shell, install flow, and downloadable DMG testable as quickly as possible.
