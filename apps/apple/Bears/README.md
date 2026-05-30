# Bears Apple app scaffold

This directory holds the native Apple app work for Bears.

## Intended structure

- `BearsApp/` — macOS app target sources and entry points
- `Shared/AppCore/` — shared models, view models, use cases, and service protocols
- `Shared/UI/` — reusable SwiftUI views/components where practical
- `macOS/AdapterInstall/` — adapter install/update/repair logic for macOS
- `macOS/Platform/` — macOS-specific platform helpers
- `macOS/Diagnostics/` — diagnostics and support helpers

## Phase-0 focus

The first execution slice is limited to:

- creating the SwiftUI macOS app shell;
- bundling and installing the ACP adapter into user Application Support;
- exposing adapter path and version state in the UI;
- supporting a basic repair/reinstall flow.

Sparkle, ACP usage log viewing, client auto-configuration, and Den-admin features belong to later slices.
