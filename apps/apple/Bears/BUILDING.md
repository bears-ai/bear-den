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

Use the helper script to prepare the adapter resource automatically:

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

You can also point at an explicit prebuilt adapter binary, which is useful on hosts without Rust tooling or in future CI packaging flows:

```bash
cd apps/apple/Bears
ADAPTER_BINARY=/path/to/bears-acp-adapter bash Scripts/prepare_adapter.sh
```

The script places the adapter at:

- `apps/apple/Bears/BearsApp/Resources/Adapter/bears-acp-adapter`

This is intentionally compatible with a future GitHub Actions pipeline: CI can invoke the same script before building and packaging the app, while either reusing a previously built adapter artifact or providing an explicit adapter path.

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
