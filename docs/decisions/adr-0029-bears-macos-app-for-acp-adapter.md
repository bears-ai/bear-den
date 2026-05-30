# ADR: Bears macOS app for ACP adapter distribution and updates

## Status
Proposed

## Context
We need a simple way for users to install and keep the Bears ACP adapter up to date on macOS without requiring Terminal use or administrator credentials.

The first release is macOS-only, but we want architectural choices that do not block a future iOS app. The adapter itself is macOS-only and will not ship on iOS.

The ACP adapter is a CLI invoked transactionally by ACP clients. It speaks ACP to clients and BearWire to Den. In later phases, the Bears app may also call Den APIs directly for bear administration and related app features, but that is out of scope for phase 0.

We want direct distribution rather than Mac App Store distribution, and we want automatic app updates via Sparkle.

## Decision
We will build a native **windowed macOS Bears app in SwiftUI** that distributes and updates the ACP adapter as part of the app.

### Packaging and runtime
- The app will **bundle the ACP adapter binary**.
- On first launch, and whenever the bundled version changes, the app will **install or update** the adapter into a user-writable location under:
  - `~/Library/Application Support/Bears/`
- The adapter will be executed from that managed per-user location, not directly from the app bundle.
- This avoids Terminal use, avoids admin privileges, and keeps runtime-managed artifacts out of the signed app bundle.

### Invocation model
- The adapter remains a **transactional CLI tool**.
- ACP clients will invoke the adapter directly when needed.
- The Bears app will **not** run the adapter as a continuously running background service in phase 0.

### Client configuration
- In phase 0, ACP clients will be **configured manually** to invoke the installed adapter CLI.
- A later phase may add app-driven client configuration flows.

### Updates
- The macOS app will use **Sparkle** for self-updates.
- The adapter version will be **coupled to the app version**.
- App updates will deliver the corresponding adapter version, and the app will refresh the managed adapter install as needed.

### Platform and architecture
- The app will be implemented in **SwiftUI**.
- Shared app/domain logic should be structured so that a future **iOS app** can reuse non-adapter-specific layers.
- macOS-specific adapter installation and runtime management will remain isolated behind platform-specific services.

### Logging
- The app should support viewing **ACP usage logs** in-app.
- ACP logs should be segmented and filterable by:
  - client
  - bear
  - session ID
- General app logging may follow normal macOS conventions.

### Distribution
- The app will use **direct distribution** outside the Mac App Store.
- This aligns with Sparkle-based updates and avoids App Store constraints that do not fit the bundled local adapter model.

## Consequences

### Positive
- Simple installation story for users.
- No Terminal or admin requirements.
- Clear and understandable update model: app version and adapter version move together.
- Per-user managed install location supports runtime control, logging, and future repair flows.
- SwiftUI and layered architecture preserve a path toward a future iOS app.

### Negative
- Clients still require manual configuration in phase 0.
- The app introduces an additional wrapper layer around a tool that remains CLI-invoked.
- Direct distribution requires our own signing, notarization, and update infrastructure.

### Neutral / follow-up
- We still need to define the exact managed filesystem layout for binaries, configs, and logs.
- We still need to define how the app validates, repairs, and replaces the managed adapter binary.
- We still need to design the ACP log schema and retention policy.
- Later Den API integration for bear administration remains a separate phase.
