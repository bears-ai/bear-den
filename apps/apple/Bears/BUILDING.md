# Building the Bears macOS app scaffold

This is the current lightweight build path for local testing.

## Current approach

The repo now includes a Swift Package manifest at:

- `apps/apple/Bears/Package.swift`

It builds a minimal macOS executable app target named `BearsApp`.

## Prerequisites

- Xcode 15+ or a compatible Swift 5.10 toolchain
- macOS 13+
- a local `bears-acp-adapter` executable available to copy into package resources

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

1. use an existing built adapter at `target/debug/bears-acp-adapter` if present;
2. otherwise use an existing built adapter at `tools/bears-acp-adapter/target/debug/bears-acp-adapter` if present;
3. otherwise fall back to `cargo build` if `cargo` is available.

To prepare a release adapter artifact instead:

```bash
cd apps/apple/Bears
PROFILE=release bash Scripts/prepare_adapter.sh
```

You can also point at an explicit prebuilt adapter binary:

```bash
cd apps/apple/Bears
ADAPTER_BINARY=/path/to/bears-acp-adapter bash Scripts/prepare_adapter.sh
```

The script places the adapter at:

- `apps/apple/Bears/BearsApp/Resources/Adapter/bears-acp-adapter`

### Remote download fallback

If no bundled adapter is present, the app will try to download a macOS adapter artifact.

## Important: publishing a rebuilt package requires a version bump

The GitHub release/update-site flow is version-driven. If you rebuild the adapter package and want GitHub to publish a new package artifact at the release/update URL, you must:

1. bump `version` in `tools/bears-acp-adapter/Cargo.toml`
2. update `tools/bears-acp-adapter/Cargo.lock`
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

## Run

```bash
cd apps/apple/Bears
swift run Bears
```

## Package layout note

The Swift sources needed by the executable target have now been consolidated under:

- `apps/apple/Bears/BearsApp/`

That keeps the initial Swift Package setup simple and gives the first local build a better chance of succeeding.

## Current limitations

This is an intentionally lightweight package-based scaffold for early testing.

Not yet complete:

- proper `.app` packaging
- codesigning/notarization flow
- Sparkle integration
- automated bundling of the Rust adapter artifact
- Xcode project/workspace configuration for shipping builds

The current goal is just to make the SwiftUI shell and install flow testable as quickly as possible.
