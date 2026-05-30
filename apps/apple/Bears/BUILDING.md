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

Before building, place the adapter executable at:

- `apps/apple/Bears/Resources/Adapter/bears-acp-adapter`

For example, after building the Rust adapter separately, copy it into that path.

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
